use axum::{
  response::{IntoResponse, Response},
  Json,
};
use hyper::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::error;

#[derive(Error, Debug, Clone)]
pub enum ProxyError {
  #[error("upstream_not_found")]
  UpstreamNotFound,
  #[error("upstream_timeout")]
  UpstreamTimeout,
  #[error("too_many_redirects")]
  TooManyRedirects,
  #[error("not_an_image")]
  NotAnImage,
  #[error("source_too_large")]
  SourceTooLarge,
  #[error("host_not_allowed")]
  HostNotAllowed,
  #[error("invalid_signature")]
  InvalidSignature,
  #[error("invalid_params")]
  InvalidParams(String),
  #[error("watermark_fetch_failed")]
  WatermarkFetchFailed,
  #[error("avif_not_supported")]
  AvifNotSupported,
  #[error("internal_error")]
  InternalError(String),
}

impl From<anyhow::Error> for ProxyError {
  fn from(e: anyhow::Error) -> Self {
    ProxyError::InternalError(e.to_string())
  }
}

#[derive(Serialize, Deserialize)]
pub struct ErrorBody {
  pub error: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub detail: Option<String>,
}

impl IntoResponse for ProxyError {
  fn into_response(self) -> Response {
    let msg = self.to_string();
    error!("{}", msg);
    let status = match &self {
      ProxyError::UpstreamNotFound => StatusCode::NOT_FOUND,
      ProxyError::UpstreamTimeout
      | ProxyError::TooManyRedirects
      | ProxyError::WatermarkFetchFailed => StatusCode::BAD_GATEWAY,
      ProxyError::NotAnImage => StatusCode::UNPROCESSABLE_ENTITY,
      ProxyError::SourceTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
      ProxyError::HostNotAllowed | ProxyError::InvalidSignature => StatusCode::FORBIDDEN,
      ProxyError::InvalidParams(_) => StatusCode::BAD_REQUEST,
      ProxyError::AvifNotSupported => StatusCode::NOT_IMPLEMENTED,
      ProxyError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let detail = if let ProxyError::InvalidParams(d) = &self {
      Some(d.clone())
    } else {
      None
    };
    let body = ErrorBody { error: msg, detail };
    (status, Json(body)).into_response()
  }
}
