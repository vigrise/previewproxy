use crate::common::config::Config;
use crate::modules::cache::{
  disk::DiskCache,
  inflight::InflightMap,
  memory::{CacheEntry, MemoryCache},
  origin::OriginCache,
};
use crate::modules::metrics::Metrics;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tracing::debug;

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
  origin_l2: OriginCache,
  inflight: InflightMap,
  metrics: Arc<Metrics>,
}

impl CacheManager {
  pub fn new(cfg: &Config, metrics: Arc<Metrics>) -> Arc<Self> {
    let l1 = MemoryCache::new(
      cfg.cache_memory_max_mb,
      Duration::from_secs(cfg.cache_memory_ttl_secs),
    );
    let l2 = Arc::new(DiskCache::new(
      cfg.cache_dir.clone(),
      cfg.cache_disk_ttl_secs,
      cfg.cache_disk_max_mb,
    ));
    let origin_l2 = OriginCache::new(&cfg.cache_dir, cfg.cache_disk_ttl_secs);
    Arc::new(Self {
      l1,
      l2,
      origin_l2,
      inflight: InflightMap::new(),
      metrics,
    })
  }

  pub fn preliminary_key(canonical: &str) -> String {
    let key = hex::encode(Sha256::digest(canonical.as_bytes()));
    debug!(key = %key, "computed preliminary cache key");
    key
  }

  #[tracing::instrument(skip(self), fields(key = %prelim_key))]
  pub async fn get(&self, prelim_key: &str) -> (Option<CacheEntry>, CacheHit) {
    if let Some(e) = self.l1.get(prelim_key).await {
      debug!(key = %prelim_key, bytes = e.bytes.len(), "L1 cache hit");
      self
        .metrics
        .cache_hits_total
        .with_label_values(&["memory"])
        .inc();
      self.record_cache_sizes();
      return (Some(e), CacheHit::L1);
    }
    if let Ok(Some(e)) = self.l2.get(prelim_key).await {
      debug!(key = %prelim_key, bytes = e.bytes.len(), "L2 cache hit - promoting to L1");
      self.l1.set(prelim_key.to_string(), e.clone()).await;
      self
        .metrics
        .cache_hits_total
        .with_label_values(&["disk"])
        .inc();
      self.record_cache_sizes();
      return (Some(e), CacheHit::L2);
    }
    debug!(key = %prelim_key, "cache miss");
    self
      .metrics
      .cache_misses_total
      .with_label_values(&["memory"])
      .inc();
    self
      .metrics
      .cache_misses_total
      .with_label_values(&["disk"])
      .inc();
    self.record_cache_sizes();
    (None, CacheHit::Miss)
  }

  #[tracing::instrument(skip(self, entry), fields(key = %final_key, bytes = entry.bytes.len()))]
  pub async fn set(&self, final_key: &str, entry: CacheEntry) {
    debug!(key = %final_key, bytes = entry.bytes.len(), "storing entry to L1 and L2");
    let _ = self.l2.set(final_key, entry.clone(), None).await;
    self.l1.set(final_key.to_string(), entry).await;
    self.record_cache_sizes();
  }

  fn record_cache_sizes(&self) {
    self
      .metrics
      .cache_memory_size_bytes
      .set(self.l1.size_bytes() as f64);
    self
      .metrics
      .cache_disk_size_bytes
      .set(self.disk_total_bytes() as f64);
    self
      .metrics
      .cache_entries
      .with_label_values(&["memory"])
      .set(self.l1.item_count() as i64);
    // disk entry count is not tracked; DiskCache does not expose it
  }

  pub async fn get_origin(&self, url: &str) -> Option<CacheEntry> {
    self.origin_l2.get(url).await
  }

  pub async fn set_origin(&self, url: &str, entry: CacheEntry, ttl_override: Option<u64>) {
    self.origin_l2.set(url, entry, ttl_override).await;
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
    let _ = self.origin_l2.cleanup().await;
  }
}
