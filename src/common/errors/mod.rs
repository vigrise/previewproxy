use axum::{
  Json,
  response::{IntoResponse, Response},
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
  #[error("heic_decode_error")]
  HeicDecodeError,
  #[error("pdf_render_error")]
  PdfRenderError,
  #[error("video_decode_error")]
  VideoDecodeError,
  #[error("unsupported_format")]
  UnsupportedFormat(String),
  #[error("transform_disabled")]
  TransformDisabled(String),
  #[error("internal_error")]
  InternalError(String),
}

impl From<anyhow::Error> for ProxyError {
  fn from(e: anyhow::Error) -> Self {
    ProxyError::InternalError(format!("{:#}", e))
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
    match &self {
      ProxyError::InternalError(detail) => error!("internal_error: {}", detail),
      _ => error!("{}", msg),
    };
    let status = match &self {
      ProxyError::UpstreamNotFound => StatusCode::NOT_FOUND,
      ProxyError::UpstreamTimeout
      | ProxyError::TooManyRedirects
      | ProxyError::WatermarkFetchFailed => StatusCode::BAD_GATEWAY,
      ProxyError::NotAnImage => StatusCode::UNPROCESSABLE_ENTITY,
      ProxyError::SourceTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
      ProxyError::HostNotAllowed | ProxyError::InvalidSignature => StatusCode::FORBIDDEN,
      ProxyError::InvalidParams(_) => StatusCode::BAD_REQUEST,
      ProxyError::HeicDecodeError | ProxyError::PdfRenderError | ProxyError::VideoDecodeError => {
        StatusCode::UNPROCESSABLE_ENTITY
      }
      ProxyError::UnsupportedFormat(_) => StatusCode::BAD_REQUEST,
      ProxyError::TransformDisabled(_) => StatusCode::BAD_REQUEST,
      ProxyError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let detail = match &self {
      ProxyError::InvalidParams(d) => Some(d.clone()),
      ProxyError::UnsupportedFormat(d) => Some(d.clone()),
      ProxyError::TransformDisabled(d) => Some(d.clone()),
      _ => None,
    };
    let body = ErrorBody { error: msg, detail };
    (status, Json(body)).into_response()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use axum::response::IntoResponse;

  #[test]
  fn test_unsupported_format_is_400() {
    let err = ProxyError::UnsupportedFormat("avif".to_string());
    let resp = err.into_response();
    assert_eq!(resp.status(), hyper::StatusCode::BAD_REQUEST);
  }

  #[test]
  fn test_heic_decode_error_is_422() {
    let resp = ProxyError::HeicDecodeError.into_response();
    assert_eq!(resp.status(), hyper::StatusCode::UNPROCESSABLE_ENTITY);
  }

  #[test]
  fn test_video_decode_error_is_422() {
    let resp = ProxyError::VideoDecodeError.into_response();
    assert_eq!(resp.status(), hyper::StatusCode::UNPROCESSABLE_ENTITY);
  }

  #[test]
  fn test_pdf_render_error_is_422() {
    let resp = ProxyError::PdfRenderError.into_response();
    assert_eq!(resp.status(), hyper::StatusCode::UNPROCESSABLE_ENTITY);
  }

  #[test]
  fn test_transform_disabled_is_400() {
    let err = ProxyError::TransformDisabled("watermark".to_string());
    let resp = err.into_response();
    assert_eq!(resp.status(), hyper::StatusCode::BAD_REQUEST);
  }
}
