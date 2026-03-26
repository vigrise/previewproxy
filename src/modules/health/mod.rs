pub mod controller;
pub mod dto;
pub mod service;

use crate::modules::AppState;
use axum::{Router, routing::get};

pub fn router() -> Router<AppState> {
  Router::new().route("/health", get(controller::index))
}
