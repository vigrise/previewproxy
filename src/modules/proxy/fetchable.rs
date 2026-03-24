use crate::common::errors::ProxyError;

/// Abstraction over all image/media sources (HTTP, S3, local filesystem).
///
/// Returns `(bytes, content_type)` where `content_type` is `None` if the
/// source did not provide one (caller should sniff the format from bytes).
#[async_trait::async_trait]
pub trait Fetchable: Send + Sync {
  async fn fetch(&self, url: &str) -> Result<(Vec<u8>, Option<String>), ProxyError>;
}
