use crate::common::errors::ProxyError;
use crate::modules::cache::memory::CacheEntry;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

type InflightResult = Option<Result<CacheEntry, String>>;

struct InflightEntry {
  result: Mutex<InflightResult>,
  notify: Notify,
}

#[derive(Clone)]
/// Tracks in-progress fetches so concurrent requests for the same key wait for
/// the first caller to complete instead of issuing duplicate upstream requests.
pub struct InflightMap {
  map: Arc<DashMap<String, Arc<InflightEntry>>>,
}

/// RAII guard held by the first caller for a key. Must call `complete()` on
/// success or error; `Drop` handles the panic/cancellation path automatically.
pub struct InflightGuard {
  key: String,
  map: Arc<DashMap<String, Arc<InflightEntry>>>,
  entry: Arc<InflightEntry>,
  completed: bool,
}

impl Default for InflightMap {
  fn default() -> Self {
    Self::new()
  }
}

impl InflightMap {
  pub fn new() -> Self {
    Self {
      map: Arc::new(DashMap::new()),
    }
  }

  /// Insert a new inflight entry and return a guard. Caller is responsible for calling complete().
  pub fn start(&self, key: String) -> InflightGuard {
    let entry = Arc::new(InflightEntry {
      result: Mutex::new(None),
      notify: Notify::new(),
    });
    self.map.insert(key.clone(), entry.clone());
    InflightGuard {
      key,
      map: self.map.clone(),
      entry,
      completed: false,
    }
  }

  /// Check if a key is in-flight.
  pub fn is_inflight(&self, key: &str) -> bool {
    self.map.contains_key(key)
  }

  /// Wait for an in-flight entry. Returns None if not found (race at boundary).
  /// Returns Some(result) after the first caller completes.
  pub async fn wait(&self, key: &str) -> Option<Result<CacheEntry, ProxyError>> {
    let entry = self.map.get(key)?.clone();
    entry.notify.notified().await;
    let guard = entry.result.lock().await;
    guard
      .as_ref()
      .map(|r| r.clone().map_err(ProxyError::InternalError))
  }
}

impl InflightGuard {
  /// Complete the inflight entry with a result.
  /// Protocol: store result → remove from map → notify waiters.
  pub fn complete(mut self, result: Result<CacheEntry, ProxyError>) {
    self.completed = true;
    let serialized: Result<CacheEntry, String> = result.map_err(|e| e.to_string());
    let entry = self.entry.clone();
    let key = self.key.clone();
    let map = self.map.clone();
    tokio::spawn(async move {
      *entry.result.lock().await = Some(serialized);
      map.remove(&key);
      entry.notify.notify_waiters();
    });
  }
}

impl Drop for InflightGuard {
  fn drop(&mut self) {
    if !self.completed {
      // Panic path: store error, clean up, notify waiters
      let entry = self.entry.clone();
      let key = self.key.clone();
      let map = self.map.clone();
      tokio::spawn(async move {
        let mut r = entry.result.lock().await;
        if r.is_none() {
          *r = Some(Err("internal_error".to_string()));
        }
        drop(r);
        map.remove(&key);
        entry.notify.notify_waiters();
      });
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::modules::cache::memory::CacheEntry;

  #[tokio::test]
  async fn test_concurrent_coalesced() {
    let inflight = InflightMap::new();
    let entry = CacheEntry {
      bytes: vec![42],
      content_type: "image/png".to_string(),
    };

    // First caller starts inflight
    let guard = inflight.start("key1".to_string());

    // Second caller waits
    let inflight2 = inflight.clone();
    let waiter = tokio::spawn(async move { inflight2.wait("key1").await });

    // First completes
    let entry_clone = entry.clone();
    guard.complete(Ok(entry_clone));

    let result = waiter.await.unwrap();
    assert!(result.is_some());
    let inner = result.unwrap();
    assert!(inner.is_ok());
    assert_eq!(inner.unwrap().bytes, vec![42]);
  }

  #[tokio::test]
  async fn test_error_propagated() {
    let inflight = InflightMap::new();
    let guard = inflight.start("errkey".to_string());

    let inflight2 = inflight.clone();
    let waiter = tokio::spawn(async move { inflight2.wait("errkey").await });

    guard.complete(Err(crate::common::errors::ProxyError::UpstreamNotFound));
    let result = waiter.await.unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().is_err());
  }

  #[tokio::test]
  async fn test_wait_no_inflight_returns_none() {
    let inflight = InflightMap::new();
    // No one is in-flight for this key - wait should return None quickly
    // (in practice, caller checks is_inflight first; this tests the race-at-boundary case)
    let result = tokio::time::timeout(
      tokio::time::Duration::from_millis(50),
      inflight.wait("nobody"),
    )
    .await;
    // Should either return None quickly or timeout (both are acceptable)
    // If it hangs, test fails via timeout
    match result {
      Ok(None) => {} // key not found, returned immediately
      Err(_) => panic!("wait hung for non-existent key"),
      Ok(Some(_)) => {} // race at boundary is fine
    }
  }
}
