use axum::{
  extract::{rejection::PathRejection, FromRequestParts, Path},
  http::request::Parts,
};
use serde::de::DeserializeOwned;

use crate::common::errors::ProxyError;

/// A custom Path extractor that returns `ProxyError` on rejection.
pub struct ValidatedPath<T>(pub T);

impl<S, T> FromRequestParts<S> for ValidatedPath<T>
where
  T: DeserializeOwned + Send,
  S: Send + Sync,
{
  type Rejection = ProxyError;

  async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
    match Path::<T>::from_request_parts(parts, state).await {
      Ok(Path(value)) => Ok(ValidatedPath(value)),
      Err(rejection) => Err(path_rejection_to_proxy_error(rejection)),
    }
  }
}

fn path_rejection_to_proxy_error(rejection: PathRejection) -> ProxyError {
  match rejection {
    PathRejection::FailedToDeserializePathParams(inner) => {
      ProxyError::InvalidParams(inner.body_text())
    }
    PathRejection::MissingPathParams(inner) => ProxyError::InvalidParams(inner.body_text()),
    _ => ProxyError::InvalidParams("Invalid path parameter".to_string()),
  }
}
