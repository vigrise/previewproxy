use crate::common::errors::ProxyError;
use crate::modules::proxy::fetchable::Fetchable;
use crate::modules::proxy::sources::{HttpFetcher, LocalSource, S3Source};
use std::sync::Arc;

/// Routes fetch requests to the correct source based on URL scheme:
/// - `http://` / `https://` -> `HttpFetcher`
/// - `s3:/` -> `S3Source` (must be enabled in config)
/// - `local:/` -> `LocalSource` (must be enabled in config)
pub struct SourceRouter {
  http: Arc<HttpFetcher>,
  s3: Option<Arc<S3Source>>,
  local: Option<Arc<LocalSource>>,
}

impl SourceRouter {
  pub fn new(
    http: Arc<HttpFetcher>,
    s3: Option<Arc<S3Source>>,
    local: Option<Arc<LocalSource>>,
  ) -> Self {
    Self { http, s3, local }
  }
}

#[async_trait::async_trait]
impl Fetchable for SourceRouter {
  async fn fetch(&self, url: &str) -> Result<(Vec<u8>, Option<String>), ProxyError> {
    if url.starts_with("http://") || url.starts_with("https://") {
      return self.http.fetch(url).await;
    }
    if url.starts_with("s3:/") {
      return match &self.s3 {
        Some(s3) => s3.fetch(url).await,
        None => Err(ProxyError::InvalidParams(
          "S3 source is not enabled".to_string(),
        )),
      };
    }
    if url.starts_with("local:/") {
      return match &self.local {
        Some(local) => local.fetch(url).await,
        None => Err(ProxyError::InvalidParams(
          "local source is not enabled".to_string(),
        )),
      };
    }
    if url.contains("://") {
      return Err(ProxyError::InvalidParams("unsupported scheme".to_string()));
    }
    Err(ProxyError::InvalidParams(
      "unrecognized URL format".to_string(),
    ))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::modules::security::allowlist::Allowlist;
  use wiremock::matchers::{method, path};
  use wiremock::{Mock, MockServer, ResponseTemplate};

  fn make_http_fetcher() -> Arc<HttpFetcher> {
    Arc::new(
      HttpFetcher::new(10, 1_000_000, Arc::new(Allowlist::new(vec![])))
        .with_private_ip_check(false),
    )
  }

  fn make_router_http_only() -> SourceRouter {
    SourceRouter::new(make_http_fetcher(), None, None)
  }

  #[tokio::test]
  async fn test_http_url_routes_to_http_fetcher() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(b"hello".to_vec())
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;

    let router = make_router_http_only();
    let (bytes, ct) = router.fetch(&server.uri()).await.unwrap();
    assert_eq!(bytes, b"hello");
    assert_eq!(ct, Some("image/png".to_string()));
  }

  #[tokio::test]
  async fn test_s3_disabled_returns_error() {
    let router = make_router_http_only();
    let result = router.fetch("s3:/some/key.jpg").await;
    assert!(
      matches!(result, Err(ProxyError::InvalidParams(ref msg)) if msg == "S3 source is not enabled"),
      "unexpected result: {:?}",
      result
    );
  }

  #[tokio::test]
  async fn test_local_disabled_returns_error() {
    let router = make_router_http_only();
    let result = router.fetch("local:/some/path.jpg").await;
    assert!(
      matches!(result, Err(ProxyError::InvalidParams(ref msg)) if msg == "local source is not enabled"),
      "unexpected result: {:?}",
      result
    );
  }

  #[tokio::test]
  async fn test_ftp_scheme_unsupported() {
    let router = make_router_http_only();
    let result = router.fetch("ftp://example.com/file").await;
    assert!(
      matches!(result, Err(ProxyError::InvalidParams(ref msg)) if msg == "unsupported scheme"),
      "unexpected result: {:?}",
      result
    );
  }

  #[tokio::test]
  async fn test_file_scheme_unsupported() {
    let router = make_router_http_only();
    let result = router.fetch("file:///etc/passwd").await;
    assert!(
      matches!(result, Err(ProxyError::InvalidParams(ref msg)) if msg == "unsupported scheme"),
      "unexpected result: {:?}",
      result
    );
  }

  #[tokio::test]
  async fn test_bare_string_unrecognized() {
    let router = make_router_http_only();
    let result = router.fetch("not-a-url").await;
    assert!(
      matches!(result, Err(ProxyError::InvalidParams(ref msg)) if msg == "unrecognized URL format"),
      "unexpected result: {:?}",
      result
    );
  }

  #[tokio::test]
  async fn test_local_enabled_routes_to_local_source() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.jpg");
    let content = b"local image bytes";
    std::fs::write(&file_path, content).unwrap();

    let local_source = LocalSource::new(dir.path().to_str().unwrap(), 1_000_000)
      .await
      .unwrap();

    let router = SourceRouter::new(make_http_fetcher(), None, Some(Arc::new(local_source)));

    let (bytes, _ct) = router.fetch("local:/test.jpg").await.unwrap();
    assert_eq!(bytes, content);
  }

  #[tokio::test]
  async fn test_s3_enabled_routes_to_s3_source() {
    let server = MockServer::start().await;
    let body = b"s3imagedata".to_vec();

    Mock::given(method("GET"))
      .and(path("/test-bucket/test-key"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(body.clone())
          .insert_header("Content-Type", "image/jpeg"),
      )
      .mount(&server)
      .await;

    let s3_source = Arc::new(S3Source::new(
      "test-bucket".to_string(),
      "us-east-1".to_string(),
      "access".to_string(),
      "secret".to_string(),
      Some(server.uri()),
      1_000_000,
    ));

    let router = SourceRouter::new(make_http_fetcher(), Some(s3_source), None);
    let (bytes, ct) = router.fetch("s3:/test-key").await.unwrap();
    assert_eq!(bytes, body);
    assert_eq!(ct, Some("image/jpeg".to_string()));
  }
}
