use crate::common::config::Config;
use crate::modules::cache::{
  disk::DiskCache,
  inflight::InflightMap,
  memory::{CacheEntry, MemoryCache},
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

/// Indicates which cache tier served the response.
pub enum CacheHit {
  /// Served from in-process memory cache (fastest).
  L1,
  /// Served from on-disk cache; entry was promoted to L1.
  L2,
  Miss,
}

/// Two-tier cache (memory L1 + disk L2) with in-flight request coalescing.
///
/// Keys are SHA-256 hex digests of the canonical request string. A "preliminary
/// key" (from params + URL before fetching) is used for cache lookup and
/// in-flight deduplication; the same key is used as the final storage key after
/// the result is computed.
pub struct CacheManager {
  l1: MemoryCache,
  pub l2: Arc<DiskCache>,
  inflight: InflightMap,
}

impl CacheManager {
  pub fn new(cfg: &Config) -> Arc<Self> {
    let l1 = MemoryCache::new(
      cfg.cache_memory_max_mb,
      Duration::from_secs(cfg.cache_memory_ttl_secs),
    );
    let l2 = Arc::new(DiskCache::new(
      cfg.cache_dir.clone(),
      cfg.cache_disk_ttl_secs,
      cfg.cache_disk_max_mb,
    ));
    Arc::new(Self {
      l1,
      l2,
      inflight: InflightMap::new(),
    })
  }

  pub fn preliminary_key(canonical: &str) -> String {
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
  }

  pub async fn get(&self, prelim_key: &str) -> (Option<CacheEntry>, CacheHit) {
    if let Some(e) = self.l1.get(prelim_key).await {
      return (Some(e), CacheHit::L1);
    }
    if let Ok(Some(e)) = self.l2.get(prelim_key).await {
      self.l1.set(prelim_key.to_string(), e.clone()).await;
      return (Some(e), CacheHit::L2);
    }
    (None, CacheHit::Miss)
  }

  pub async fn set(&self, final_key: &str, entry: CacheEntry) {
    let _ = self.l2.set(final_key, entry.clone()).await;
    self.l1.set(final_key.to_string(), entry).await;
  }

  pub fn inflight(&self) -> &InflightMap {
    &self.inflight
  }

  pub fn memory_item_count(&self) -> u64 {
    self.l1.item_count()
  }

  pub fn disk_total_bytes(&self) -> u64 {
    self
      .l2
      .total_bytes
      .load(std::sync::atomic::Ordering::Relaxed)
  }

  /// Unix timestamp (seconds) of the last disk-usage scan used for `disk_total_bytes`.
  pub fn disk_total_bytes_as_of(&self) -> u64 {
    self
      .l2
      .total_bytes_as_of
      .load(std::sync::atomic::Ordering::Relaxed)
  }

  pub async fn run_cleanup(&self) {
    let _ = self.l2.cleanup().await;
  }
}
