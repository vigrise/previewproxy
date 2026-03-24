use crate::common::errors::ProxyError;
use crate::modules::cache::manager::CacheHit;
use crate::modules::cache::memory::CacheEntry;
use bytes::Bytes;

/// The outcome of a [`ProxyService::process`] call.
///
/// - `Cached` - result was served from L1 or L2 cache; the full bytes are
///   available immediately and will be written into a buffered HTTP response.
/// - `Stream` - a fresh HTTP fetch with no transforms; the upstream response
///   body is tee'd so it streams to the client while simultaneously being
///   written to the cache in a background task. The semaphore permit is held
///   inside the stream and dropped only when the body is fully consumed.
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
