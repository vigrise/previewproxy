use crate::modules::cache::{disk::DiskCache, memory::CacheEntry};
use reqwest::header::{CACHE_CONTROL, EXPIRES, HeaderMap};
use sha2::{Digest, Sha256};
use tracing::warn;

pub struct OriginCache {
  disk: DiskCache,
}

impl OriginCache {
  pub fn new(cache_dir: &str, default_ttl_secs: u64) -> Self {
    let origin_dir = format!("{}/origin", cache_dir);
    Self {
      disk: DiskCache::new(origin_dir, default_ttl_secs, None),
    }
  }

  fn url_key(url: &str) -> String {
    hex::encode(Sha256::digest(url.as_bytes()))
  }

  pub async fn get(&self, url: &str) -> Option<CacheEntry> {
    match self.disk.get(&Self::url_key(url)).await {
      Ok(Some(entry)) => Some(entry),
      Ok(None) => None,
      Err(e) => {
        warn!(url = url, error = %e, "origin cache read error - treating as miss");
        None
      }
    }
  }

  pub async fn set(&self, url: &str, entry: CacheEntry, ttl_override: Option<u64>) {
    // Skip writing if TTL override is explicitly 0 (e.g. Expires in the past)
    if ttl_override == Some(0) {
      return;
    }
    let key = Self::url_key(url);
    if let Err(e) = self.disk.set(&key, entry, ttl_override).await {
      warn!(url = url, error = %e, "origin cache write error - ignoring");
    }
  }

  pub fn extract_ttl(headers: &HeaderMap) -> Option<u64> {
    // 1. Cache-Control: max-age=N takes priority
    if let Some(val) = headers.get(CACHE_CONTROL)
      && let Ok(s) = val.to_str()
    {
      for directive in s.split(',') {
        let d = directive.trim();
        if let Some(rest) = d.strip_prefix("max-age=")
          && let Ok(n) = rest.trim().parse::<u64>()
        {
          return Some(n);
        }
      }
    }
    // 2. Expires header
    if let Some(val) = headers.get(EXPIRES)
      && let Ok(s) = val.to_str()
      && let Ok(expires) = httpdate::parse_http_date(s)
    {
      let now = std::time::SystemTime::now();
      return Some(match expires.duration_since(now) {
        Ok(d) => d.as_secs(),
        Err(_) => 0, // past = already expired
      });
    }
    None
  }

  pub async fn cleanup(&self) -> anyhow::Result<u64> {
    self.disk.cleanup().await
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use reqwest::header::HeaderValue;
  use tempfile::TempDir;

  fn make_entry(bytes: Vec<u8>) -> CacheEntry {
    CacheEntry {
      bytes,
      content_type: "image/jpeg".to_string(),
    }
  }

  #[tokio::test]
  async fn test_get_set() {
    let dir = TempDir::new().unwrap();
    let cache = OriginCache::new(dir.path().to_str().unwrap(), 86400);
    let entry = make_entry(vec![1, 2, 3]);
    cache
      .set("https://example.com/img.jpg", entry.clone(), None)
      .await;
    let result = cache.get("https://example.com/img.jpg").await;
    assert!(result.is_some());
    assert_eq!(result.unwrap().bytes, vec![1, 2, 3]);
  }

  #[tokio::test]
  async fn test_different_urls_different_keys() {
    let dir = TempDir::new().unwrap();
    let cache = OriginCache::new(dir.path().to_str().unwrap(), 86400);
    cache
      .set("https://example.com/a.jpg", make_entry(vec![1]), None)
      .await;
    cache
      .set("https://example.com/b.jpg", make_entry(vec![2]), None)
      .await;
    assert_eq!(
      cache.get("https://example.com/a.jpg").await.unwrap().bytes,
      vec![1]
    );
    assert_eq!(
      cache.get("https://example.com/b.jpg").await.unwrap().bytes,
      vec![2]
    );
  }

  #[tokio::test]
  async fn test_ttl_expired() {
    let dir = TempDir::new().unwrap();
    // TTL=0 = immediately expired
    let cache = OriginCache::new(dir.path().to_str().unwrap(), 0);
    cache
      .set("https://example.com/img.jpg", make_entry(vec![1]), None)
      .await;
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    assert!(cache.get("https://example.com/img.jpg").await.is_none());
  }

  #[tokio::test]
  async fn test_miss_returns_none() {
    let dir = TempDir::new().unwrap();
    let cache = OriginCache::new(dir.path().to_str().unwrap(), 86400);
    assert!(cache.get("https://example.com/missing.jpg").await.is_none());
  }

  #[tokio::test]
  async fn test_ttl_from_cache_control() {
    let mut headers = HeaderMap::new();
    headers.insert(
      CACHE_CONTROL,
      HeaderValue::from_static("public, max-age=300"),
    );
    assert_eq!(OriginCache::extract_ttl(&headers), Some(300));
  }

  #[tokio::test]
  async fn test_ttl_from_cache_control_no_store() {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    assert_eq!(OriginCache::extract_ttl(&headers), None);
  }

  #[tokio::test]
  async fn test_ttl_from_expires_future() {
    let mut headers = HeaderMap::new();
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(600);
    let expires_str = httpdate::fmt_http_date(future);
    headers.insert(EXPIRES, HeaderValue::from_str(&expires_str).unwrap());
    let ttl = OriginCache::extract_ttl(&headers);
    assert!(ttl.is_some());
    let t = ttl.unwrap();
    assert!(t > 590 && t <= 600, "expected ~600s, got {t}");
  }

  #[tokio::test]
  async fn test_ttl_from_expires_past() {
    let mut headers = HeaderMap::new();
    // Already expired
    headers.insert(
      EXPIRES,
      HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
    );
    assert_eq!(OriginCache::extract_ttl(&headers), Some(0));
  }

  #[tokio::test]
  async fn test_ttl_fallback_no_headers() {
    let headers = HeaderMap::new();
    assert_eq!(OriginCache::extract_ttl(&headers), None);
  }

  #[tokio::test]
  async fn test_skip_set_when_ttl_zero() {
    let dir = TempDir::new().unwrap();
    let cache = OriginCache::new(dir.path().to_str().unwrap(), 86400);
    // Explicit TTL override of 0 - should not write entry
    cache
      .set("https://example.com/img.jpg", make_entry(vec![1]), Some(0))
      .await;
    assert!(cache.get("https://example.com/img.jpg").await.is_none());
  }
}
