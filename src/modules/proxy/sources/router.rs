use crate::common::errors::ProxyError;
use crate::modules::proxy::fetchable::Fetchable;
use crate::modules::proxy::sources::{AliasSource, HttpFetcher, LocalSource, S3Source};
use std::sync::Arc;

/// Routes fetch requests to the correct source based on URL scheme:
/// - `http://` / `https://` -> `HttpFetcher`
/// - `s3:/` -> `S3Source` (must be enabled in config)
/// - `local:/` -> `LocalSource` (must be enabled in config)
/// - `<alias>:/` -> `AliasSource` (must be enabled in config)
pub struct SourceRouter {
  http: Arc<HttpFetcher>,
  s3: Option<Arc<S3Source>>,
  local: Option<Arc<LocalSource>>,
  alias: Option<Arc<AliasSource>>,
}

impl SourceRouter {
  pub fn new(
    http: Arc<HttpFetcher>,
    s3: Option<Arc<S3Source>>,
    local: Option<Arc<LocalSource>>,
    alias: Option<Arc<AliasSource>>,
  ) -> Self {
    Self { http, s3, local, alias }
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
    if url.contains(":/") {
      return match &self.alias {
        Some(alias) => alias.fetch(url).await,
        None => Err(ProxyError::InvalidParams("alias source is not enabled".to_string())),
      };
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
    SourceRouter::new(make_http_fetcher(), None, None, None)
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

    let router = SourceRouter::new(make_http_fetcher(), None, Some(Arc::new(local_source)), None);

    let (bytes, _ct) = router.fetch("local:/test.jpg").await.unwrap();
    assert_eq!(bytes, content);
  }

  fn make_alias_source_for_test(server_uri: &str) -> Arc<crate::modules::proxy::sources::AliasSource> {
    use crate::modules::proxy::sources::AliasSource;
    use crate::modules::security::allowlist::Allowlist;
    let http = Arc::new(
      HttpFetcher::new(10, 1_000_000, Arc::new(Allowlist::new(vec![]))).with_private_ip_check(false),
    );
    let mut map = std::collections::HashMap::new();
    map.insert("mycdn".to_string(), server_uri.to_string());
    Arc::new(AliasSource::new(map, http))
  }

  #[tokio::test]
  async fn test_alias_routes_to_alias_source() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .and(path("/img.jpg"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(b"aliasdata".to_vec())
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;

    let alias = make_alias_source_for_test(&server.uri());
    let router = SourceRouter::new(make_http_fetcher(), None, None, Some(alias));
    let (bytes, _) = router.fetch("mycdn:/img.jpg").await.unwrap();
    assert_eq!(bytes, b"aliasdata");
  }

  #[tokio::test]
  async fn test_alias_disabled_returns_error() {
    let router = SourceRouter::new(make_http_fetcher(), None, None, None);
    let result = router.fetch("mycdn:/img.jpg").await;
    assert!(
      matches!(result, Err(ProxyError::InvalidParams(ref msg)) if msg == "alias source is not enabled"),
      "unexpected: {:?}", result
    );
  }

  #[tokio::test]
  async fn test_double_slash_alias_scheme_is_unsupported() {
    let router = SourceRouter::new(make_http_fetcher(), None, None, None);
    let result = router.fetch("mycdn://img.jpg").await;
    assert!(
      matches!(result, Err(ProxyError::InvalidParams(ref msg)) if msg == "unsupported scheme"),
      "unexpected: {:?}", result
    );
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

    let router = SourceRouter::new(make_http_fetcher(), Some(s3_source), None, None);
    let (bytes, ct) = router.fetch("s3:/test-key").await.unwrap();
    assert_eq!(bytes, body);
    assert_eq!(ct, Some("image/jpeg".to_string()));
  }
}
