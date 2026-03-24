pub mod cache;
pub mod cli;
pub mod health;
pub mod proxy;
pub mod security;
pub mod transform;

use crate::common::config::Config;
use crate::modules::cache::manager::CacheManager;
use crate::modules::proxy::fetchable::Fetchable;
use axum::Router;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct AppState {
  pub cfg: Config,
  pub cache: Arc<CacheManager>,
  pub fetcher: Arc<dyn Fetchable>,
  pub http_fetcher: Arc<crate::modules::proxy::sources::http::HttpFetcher>,
  pub concurrency: Arc<Semaphore>,
}

pub fn router(state: AppState) -> Router {
  Router::new()
    .merge(health::router())
    .merge(proxy::controller::router())
    .with_state(state)
}
