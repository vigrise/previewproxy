use crate::common::{config::telemetry, config::Config, config::Environment, middlewares};
use crate::modules::cache::manager::CacheManager;
use crate::modules::proxy::fetchable::Fetchable;
use crate::modules::proxy::sources::http::HttpFetcher;
use crate::modules::proxy::sources::{LocalSource, S3Source, SourceRouter};
use crate::modules::security::allowlist::Allowlist;
use crate::modules::AppState;
use axum::Router;
use std::sync::Arc;

pub async fn router(cfg: Config, cache: Arc<CacheManager>) -> Router {
  let allowlist = Arc::new(Allowlist::new(cfg.allowed_hosts.clone()));
  let check_private = cfg.env == Environment::Production;
  let http = Arc::new(
    HttpFetcher::new(cfg.fetch_timeout_secs, cfg.max_source_bytes, allowlist)
      .with_private_ip_check(check_private),
  );

  let s3 = if cfg.s3_enabled {
    Some(Arc::new(S3Source::new(
      cfg.s3_bucket.clone().unwrap(),
      cfg.s3_region.clone(),
      cfg.s3_access_key_id.clone().unwrap(),
      cfg.s3_secret_access_key.clone().unwrap(),
      cfg.s3_endpoint.clone(),
      cfg.max_source_bytes,
    )))
  } else {
    None
  };

  let local = if cfg.local_enabled {
    Some(Arc::new(
      LocalSource::new(cfg.local_base_dir.as_deref().unwrap(), cfg.max_source_bytes)
        .await
        .unwrap_or_else(|e| panic!("Failed to initialize LocalSource: {e}")),
    ))
  } else {
    None
  };

  let fetcher: Arc<dyn Fetchable> = Arc::new(SourceRouter::new(http, s3, local));

  let cors_layer = middlewares::cors_layer(&cfg.cors_allow_origin, cfg.cors_max_age_secs);

  let app_state = AppState {
    cfg,
    cache,
    fetcher,
  };

  let trace_layer = telemetry::trace_layer();
  let request_id_layer = middlewares::request_id_layer();
  let propagate_request_id_layer = middlewares::propagate_request_id_layer();
  let timeout_layer = middlewares::timeout_layer();
  let normalize_path_layer = middlewares::normalize_path_layer();

  let router = crate::modules::router(app_state.clone());

  Router::new()
    .merge(router)
    .layer(normalize_path_layer)
    .layer(cors_layer)
    .layer(timeout_layer)
    .layer(propagate_request_id_layer)
    .layer(trace_layer)
    .layer(request_id_layer)
}
