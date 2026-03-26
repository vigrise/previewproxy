use crate::modules::cache::memory::CacheEntry;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
  path::{Path, PathBuf},
  sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
  },
};
use tokio::fs;

#[derive(Serialize, Deserialize)]
struct Meta {
  content_type: String,
  created_at: u64,
}

/// Persistent on-disk cache stored as sharded flat files.
///
/// Each entry is two files under `<dir>/<key[..2]>/`:
/// - `<key>.bin` - raw image bytes
/// - `<key>.meta` - JSON with `content_type` and `created_at` (unix seconds)
///
/// Sharding by the first two hex characters of the SHA-256 key limits the
/// number of files per directory. `ttl_secs = 0` means every entry is
/// treated as immediately expired (effectively disabling disk caching).
/// When `max_bytes` is set, `cleanup` evicts the oldest entries first until
/// the total size falls below the limit.
pub struct DiskCache {
  dir: String,
  ttl_secs: u64,
  max_bytes: Option<u64>,
  /// Running total of live cache bytes, updated after each `cleanup` run.
  pub total_bytes: Arc<AtomicU64>,
  /// Unix timestamp of the `cleanup` scan that produced `total_bytes`.
  pub total_bytes_as_of: Arc<AtomicU64>,
}

impl DiskCache {
  pub fn new(dir: String, ttl_secs: u64, max_mb: Option<u64>) -> Self {
    Self {
      dir,
      ttl_secs,
      max_bytes: max_mb.map(|m| m * 1024 * 1024),
      total_bytes: Arc::new(AtomicU64::new(0)),
      total_bytes_as_of: Arc::new(AtomicU64::new(0)),
    }
  }

  fn shard_dir(&self, key: &str) -> PathBuf {
    let prefix = &key[..2.min(key.len())];
    Path::new(&self.dir).join(prefix)
  }

  fn bin_path(&self, key: &str) -> PathBuf {
    self.shard_dir(key).join(format!("{key}.bin"))
  }

  fn meta_path(&self, key: &str) -> PathBuf {
    self.shard_dir(key).join(format!("{key}.meta"))
  }

  pub async fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
    let meta_path = self.meta_path(key);
    if !meta_path.exists() {
      return Ok(None);
    }
    let meta_bytes = fs::read(&meta_path).await?;
    let meta: Meta = serde_json::from_slice(&meta_bytes)?;
    let now = now_unix();
    let age = now.saturating_sub(meta.created_at);
    let expired = if self.ttl_secs == 0 {
      true
    } else {
      age >= self.ttl_secs
    };
    if expired {
      let _ = fs::remove_file(self.bin_path(key)).await;
      let _ = fs::remove_file(&meta_path).await;
      return Ok(None);
    }
    let bytes = fs::read(self.bin_path(key)).await?;
    Ok(Some(CacheEntry {
      bytes,
      content_type: meta.content_type,
    }))
  }

  pub async fn set(&self, key: &str, entry: CacheEntry) -> Result<()> {
    let shard = self.shard_dir(key);
    fs::create_dir_all(&shard).await?;
    let meta = Meta {
      content_type: entry.content_type,
      created_at: now_unix(),
    };
    fs::write(self.meta_path(key), serde_json::to_vec(&meta)?).await?;
    fs::write(self.bin_path(key), &entry.bytes).await?;
    Ok(())
  }

  pub async fn cleanup(&self) -> Result<u64> {
    let now = now_unix();
    let mut total: u64 = 0;
    let dir_path = Path::new(&self.dir);
    if !dir_path.exists() {
      return Ok(0);
    }

    // Collect live entries: (created_at, bin_path, meta_path, bin_size)
    let mut live: Vec<(u64, PathBuf, PathBuf, u64)> = Vec::new();

    let mut dir = fs::read_dir(dir_path).await?;
    while let Some(shard) = dir.next_entry().await? {
      if !shard.file_type().await?.is_dir() {
        continue;
      }
      let mut shard_dir = fs::read_dir(shard.path()).await?;
      while let Some(f) = shard_dir.next_entry().await? {
        let name = f.file_name().into_string().unwrap_or_default();
        if !name.ends_with(".meta") {
          continue;
        }
        let key = name.trim_end_matches(".meta");
        let bin = shard.path().join(format!("{key}.bin"));
        let meta_bytes = match fs::read(f.path()).await {
          Ok(b) => b,
          Err(_) => continue,
        };
        if let Ok(meta) = serde_json::from_slice::<Meta>(&meta_bytes) {
          let age = now.saturating_sub(meta.created_at);
          let stale = if self.ttl_secs == 0 {
            true
          } else {
            age >= self.ttl_secs
          };
          if stale {
            let _ = fs::remove_file(&bin).await;
            let _ = fs::remove_file(f.path()).await;
          } else {
            let bin_size = fs::metadata(&bin).await.map(|m| m.len()).unwrap_or(0);
            total += bin_size;
            live.push((meta.created_at, bin, f.path().to_path_buf(), bin_size));
          }
        }
      }
    }

    // Enforce max_bytes: evict oldest entries first until under the limit
    if let Some(max) = self.max_bytes
      && total > max {
        // Sort oldest first
        live.sort_by_key(|(created_at, _, _, _)| *created_at);
        for (_, bin, meta, size) in live {
          if total <= max {
            break;
          }
          let _ = fs::remove_file(&bin).await;
          let _ = fs::remove_file(&meta).await;
          total = total.saturating_sub(size);
        }
      }

    self.total_bytes.store(total, Ordering::Relaxed);
    self.total_bytes_as_of.store(now, Ordering::Relaxed);
    Ok(total)
  }
}

fn now_unix() -> u64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::modules::cache::memory::CacheEntry;
  use tempfile::TempDir;

  #[tokio::test]
  async fn test_write_and_read() {
    let dir = TempDir::new().unwrap();
    let disk = DiskCache::new(dir.path().to_str().unwrap().to_string(), 86400, None);
    let entry = CacheEntry {
      bytes: vec![1, 2, 3],
      content_type: "image/png".to_string(),
    };
    disk.set("abc123def456", entry.clone()).await.unwrap();
    let result = disk.get("abc123def456").await.unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap().bytes, vec![1, 2, 3]);
  }

  #[tokio::test]
  async fn test_stale_returns_none() {
    let dir = TempDir::new().unwrap();
    let disk = DiskCache::new(dir.path().to_str().unwrap().to_string(), 0, None); // 0s TTL = immediately stale
    let entry = CacheEntry {
      bytes: vec![1],
      content_type: "image/png".to_string(),
    };
    disk.set("stalekey0011", entry).await.unwrap();
    // Even with 0s TTL, entry is stale immediately
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    let result = disk.get("stalekey0011").await.unwrap();
    assert!(result.is_none());
  }

  #[tokio::test]
  async fn test_miss() {
    let dir = TempDir::new().unwrap();
    let disk = DiskCache::new(dir.path().to_str().unwrap().to_string(), 86400, None);
    assert!(disk.get("nonexistent00").await.unwrap().is_none());
  }
}
