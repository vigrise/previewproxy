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
  http::{header, HeaderMap},
  response::{IntoResponse, Response},
  routing::get,
  Router,
};
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
  Router::new()
    .route("/proxy", get(handle_query))
    .route("/{*path}", get(handle_path))
}

async fn handle_query(
  State(state): State<AppState>,
  Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ProxyError> {
  let url = query
    .get("url")
    .cloned()
    .ok_or_else(|| ProxyError::InvalidParams("missing `url` query param".to_string()))?;
  let params = from_query(&query)?;
  let service = ProxyService::new(&state);
  let (entry, hit) = service.process(params, url).await?;
  Ok(build_response(entry, hit, &state.cfg))
}

async fn handle_path(
  State(state): State<AppState>,
  Path(path): Path<String>,
  Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ProxyError> {
  let (mut params, url) = TransformParams::from_path(&path)?;
  if !query.is_empty() {
    let query_params = from_query(&query)?;
    params.merge_from(query_params);
  }
  let svc = ProxyService::new(&state);
  let (entry, hit) = svc.process(params, url).await?;
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
