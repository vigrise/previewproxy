use crate::common::errors::ProxyError;
use crate::modules::proxy::fetchable::Fetchable;
use aws_sdk_s3::Client;

pub struct S3Source {
  client: Client,
  bucket: String,
  max_bytes: u64,
}

impl S3Source {
  pub fn new(
    bucket: String,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    endpoint: Option<String>,
    max_bytes: u64,
  ) -> Self {
    use aws_sdk_s3::config::{BehaviorVersion, Builder, Credentials, Region};
    let creds = Credentials::new(access_key_id, secret_access_key, None, None, "previewproxy");
    let mut builder = Builder::new()
      .behavior_version(BehaviorVersion::latest())
      .region(Region::new(region))
      .credentials_provider(creds);
    if let Some(ep) = endpoint {
      builder = builder.endpoint_url(ep).force_path_style(true);
    }
    Self {
      client: Client::from_conf(builder.build()),
      bucket,
      max_bytes,
    }
  }
}

#[async_trait::async_trait]
impl Fetchable for S3Source {
  async fn fetch(&self, url: &str) -> Result<(Vec<u8>, Option<String>), ProxyError> {
    // Strip "s3:/" prefix to get S3 key
    let key = url.strip_prefix("s3:/").unwrap_or(url);

    let resp = self
      .client
      .get_object()
      .bucket(&self.bucket)
      .key(key)
      .send()
      .await
      .map_err(|e| {
        if let Some(service_err) = e.as_service_error()
          && service_err.is_no_such_key() {
            return ProxyError::UpstreamNotFound;
          }
        // Also check raw HTTP status for 404
        if let Some(raw) = e.raw_response()
          && raw.status().as_u16() == 404 {
            return ProxyError::UpstreamNotFound;
          }
        ProxyError::InternalError(e.to_string())
      })?;

    // Check content_length before downloading body
    if let Some(content_length) = resp.content_length()
      && content_length as u64 > self.max_bytes {
        return Err(ProxyError::SourceTooLarge);
      }

    let content_type = resp.content_type().map(|s| s.to_string());

    let collected = resp
      .body
      .collect()
      .await
      .map_err(|e| ProxyError::InternalError(e.to_string()))?;

    let bytes = collected.into_bytes();

    if bytes.len() as u64 > self.max_bytes {
      return Err(ProxyError::SourceTooLarge);
    }

    Ok((bytes.to_vec(), content_type))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use wiremock::matchers::{method, path};
  use wiremock::{Mock, MockServer, ResponseTemplate};

  fn make_source(server_uri: &str, max_bytes: u64) -> S3Source {
    S3Source::new(
      "test-bucket".to_string(),
      "us-east-1".to_string(),
      "access".to_string(),
      "secret".to_string(),
      Some(server_uri.to_string()),
      max_bytes,
    )
  }

  #[tokio::test]
  async fn test_happy_path() {
    let server = MockServer::start().await;
    let body = b"fakejpegdata".to_vec();

    Mock::given(method("GET"))
      .and(path("/test-bucket/images/photo.jpg"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(body.clone())
          .insert_header("Content-Type", "image/jpeg"),
      )
      .mount(&server)
      .await;

    let source = make_source(&server.uri(), 1024 * 1024);
    let result = source.fetch("s3:/images/photo.jpg").await;
    let (bytes, content_type) = result.expect("should succeed");
    assert_eq!(bytes, body);
    assert_eq!(content_type, Some("image/jpeg".to_string()));
  }

  #[tokio::test]
  async fn test_missing_key_404() {
    let server = MockServer::start().await;

    let xml_body = r#"<?xml version="1.0" encoding="UTF-8"?>
<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>"#;

    Mock::given(method("GET"))
      .and(path("/test-bucket/missing/key.jpg"))
      .respond_with(
        ResponseTemplate::new(404)
          .set_body_string(xml_body)
          .insert_header("Content-Type", "application/xml"),
      )
      .mount(&server)
      .await;

    let source = make_source(&server.uri(), 1024 * 1024);
    let result = source.fetch("s3:/missing/key.jpg").await;
    assert!(
      matches!(result, Err(ProxyError::UpstreamNotFound)),
      "expected UpstreamNotFound, got: {:?}",
      result
    );
  }

  #[tokio::test]
  async fn test_oversized_via_content_length() {
    let server = MockServer::start().await;

    // 1000 bytes content but max_bytes = 100
    let body = vec![0u8; 1000];

    Mock::given(method("GET"))
      .and(path("/test-bucket/big/image.jpg"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(body)
          .insert_header("Content-Length", "1000")
          .insert_header("Content-Type", "image/jpeg"),
      )
      .mount(&server)
      .await;

    let source = make_source(&server.uri(), 100);
    let result = source.fetch("s3:/big/image.jpg").await;
    assert!(
      matches!(result, Err(ProxyError::SourceTooLarge)),
      "expected SourceTooLarge, got: {:?}",
      result
    );
  }

  #[tokio::test]
  async fn test_oversized_via_streaming() {
    let server = MockServer::start().await;

    // Body > max_bytes but no Content-Length header
    let body = vec![0u8; 500];

    Mock::given(method("GET"))
      .and(path("/test-bucket/stream/image.jpg"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(body)
          .insert_header("Content-Type", "image/jpeg"),
        // No Content-Length header
      )
      .mount(&server)
      .await;

    let source = make_source(&server.uri(), 100);
    let result = source.fetch("s3:/stream/image.jpg").await;
    assert!(
      matches!(result, Err(ProxyError::SourceTooLarge)),
      "expected SourceTooLarge, got: {:?}",
      result
    );
  }
}
