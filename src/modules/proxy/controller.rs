use crate::common::config::Config;
use crate::common::errors::ProxyError;
use crate::modules::cache::manager::CacheHit;
use crate::modules::cache::memory::CacheEntry;
use crate::modules::proxy::{
  params::{from_query, TransformParams},
  service::ProxyService,
};
use crate::modules::AppState;
use axum::{
  extract::{Path, Query, State},
  http::{header, HeaderMap, StatusCode},
  response::{IntoResponse, Response},
  routing::get,
  Router,
};
use std::collections::HashMap;
use tokio::sync::OwnedSemaphorePermit;

pub fn router() -> Router<AppState> {
  Router::new()
    .route("/proxy", get(handle_query))
    .route("/{*path}", get(handle_path))
}

async fn handle_query(
  State(state): State<AppState>,
  Query(query): Query<HashMap<String, String>>,
) -> Response {
  let permit = match state.concurrency.clone().try_acquire_owned() {
    Ok(p) => p,
    Err(_) => {
      return (
        StatusCode::SERVICE_UNAVAILABLE,
        [(axum::http::header::HeaderName::from_static("retry-after"), "1")],
        axum::body::Body::empty(),
      )
        .into_response();
    }
  };
  handle_query_inner(state, query, permit)
    .await
    .unwrap_or_else(|e| e.into_response())
}

async fn handle_path(
  State(state): State<AppState>,
  Path(path): Path<String>,
  Query(query): Query<HashMap<String, String>>,
) -> Response {
  let permit = match state.concurrency.clone().try_acquire_owned() {
    Ok(p) => p,
    Err(_) => {
      return (
        StatusCode::SERVICE_UNAVAILABLE,
        [(axum::http::header::HeaderName::from_static("retry-after"), "1")],
        axum::body::Body::empty(),
      )
        .into_response();
    }
  };
  handle_path_inner(state, path, query, permit)
    .await
    .unwrap_or_else(|e| e.into_response())
}

async fn handle_query_inner(
  state: AppState,
  query: HashMap<String, String>,
  permit: OwnedSemaphorePermit,
) -> Result<Response, ProxyError> {
  let url = query
    .get("url")
    .cloned()
    .ok_or_else(|| ProxyError::InvalidParams("missing `url` query param".to_string()))?;
  let params = from_query(&query)?;
  let service = ProxyService::new(&state);
  let (entry, hit) = service.process(params, url, permit).await?;
  Ok(build_response(entry, hit, &state.cfg))
}

async fn handle_path_inner(
  state: AppState,
  path: String,
  query: HashMap<String, String>,
  permit: OwnedSemaphorePermit,
) -> Result<Response, ProxyError> {
  let (mut params, url) = TransformParams::from_path(&path)?;
  if !query.is_empty() {
    let query_params = from_query(&query)?;
    params.merge_from(query_params);
  }
  let svc = ProxyService::new(&state);
  let (entry, hit) = svc.process(params, url, permit).await?;
  Ok(build_response(entry, hit, &state.cfg))
}

fn build_response(entry: CacheEntry, hit: CacheHit, cfg: &Config) -> Response {
  let x_cache = match hit {
    CacheHit::L1 => "HIT-L1",
    CacheHit::L2 => "HIT-L2",
    CacheHit::Miss => "MISS",
  };
  let content_length = entry.bytes.len();
  let cache_control = format!("public, max-age={}", cfg.cache_disk_ttl_secs);

  let mut headers = HeaderMap::new();
  let ct_value = entry
    .content_type
    .parse()
    .unwrap_or_else(|_| "application/octet-stream".parse().unwrap());
  headers.insert(header::CONTENT_TYPE, ct_value);
  headers.insert(header::CONTENT_LENGTH, content_length.into());
  headers.insert(header::CACHE_CONTROL, cache_control.parse().unwrap());
  headers.insert("x-cache", x_cache.parse().unwrap());

  (headers, entry.bytes).into_response()
}

#[cfg(test)]
mod concurrency_tests {
  use crate::common::config::Configuration;
  use crate::modules::cache::manager::CacheManager;
  use crate::modules::proxy::sources::http::HttpFetcher;
  use crate::modules::security::allowlist::Allowlist;
  use crate::modules::AppState;
  use axum::http::StatusCode;
  use std::net::{Ipv4Addr, SocketAddr};
  use std::sync::Arc;
  use tokio::sync::Semaphore;
  use tower::ServiceExt;

  fn make_state(permits: usize) -> AppState {
    let cfg = Arc::new(Configuration {
      env: crate::common::config::Environment::Development,
      listen_address: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8080)),
      app_port: 8080,
      hmac_key: None,
      allowed_hosts: vec![],
      fetch_timeout_secs: 10,
      max_source_bytes: 1_000_000,
      cache_memory_max_mb: 16,
      cache_memory_ttl_secs: 60,
      cache_dir: "/tmp/previewproxy-ctrl-test".to_string(),
      cache_disk_ttl_secs: 60,
      cache_disk_max_mb: None,
      cache_cleanup_interval_secs: 600,
      s3_enabled: false,
      s3_bucket: None,
      s3_region: "us-east-1".to_string(),
      s3_access_key_id: None,
      s3_secret_access_key: None,
      s3_endpoint: None,
      local_enabled: false,
      local_base_dir: None,
      ffmpeg_path: "ffmpeg".to_string(),
      cors_allow_origin: vec!["*".to_string()],
      cors_max_age_secs: 600,
      max_concurrent_requests: permits,
    });
    let http = Arc::new(
      HttpFetcher::new(10, 1_000_000, Arc::new(Allowlist::new(vec![])))
        .with_private_ip_check(false),
    );
    AppState {
      cache: CacheManager::new(&cfg),
      fetcher: http.clone(),
      http_fetcher: http,
      concurrency: Arc::new(Semaphore::new(permits)),
      cfg,
    }
  }

  #[tokio::test]
  async fn test_503_when_semaphore_exhausted() {
    let state = AppState {
      concurrency: Arc::new(Semaphore::new(0)), // 0 permits
      ..make_state(1)
    };
    let app = crate::modules::router(state);
    let req = axum::http::Request::builder()
      .uri("/proxy?url=https://example.com/img.jpg")
      .body(axum::body::Body::empty())
      .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
      response.headers().get("retry-after").and_then(|v| v.to_str().ok()),
      Some("1")
    );
  }

  #[tokio::test]
  async fn test_permit_restored_after_buffered_request() {
    let sem = Arc::new(Semaphore::new(1));
    let state = AppState {
      concurrency: sem.clone(),
      ..make_state(1)
    };
    assert_eq!(sem.available_permits(), 1);
    let app = crate::modules::router(state);
    let req = axum::http::Request::builder()
      .uri("/proxy?url=https://0.0.0.0/img.jpg") // will fail fast (HostNotAllowed or connect error)
      .body(axum::body::Body::empty())
      .unwrap();
    let _ = app.oneshot(req).await.unwrap();
    assert_eq!(sem.available_permits(), 1);
  }
}
