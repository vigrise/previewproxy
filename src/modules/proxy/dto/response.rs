use crate::common::errors::ProxyError;
use crate::modules::cache::manager::CacheHit;
use crate::modules::cache::memory::CacheEntry;
use bytes::Bytes;

pub enum ProcessResult {
  Cached(CacheEntry, CacheHit),
  Stream {
    body: futures::stream::BoxStream<'static, Result<Bytes, ProxyError>>,
    content_type: String,
  },
}

impl std::fmt::Debug for ProcessResult {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ProcessResult::Cached(_, _) => write!(f, "ProcessResult::Cached"),
      ProcessResult::Stream { content_type, .. } => {
        write!(
          f,
          "ProcessResult::Stream {{ content_type: {:?} }}",
          content_type
        )
      }
    }
  }
}
