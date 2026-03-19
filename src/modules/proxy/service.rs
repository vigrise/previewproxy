use crate::common::errors::ProxyError;
use crate::modules::cache::manager::{CacheHit, CacheManager};
use crate::modules::cache::memory::CacheEntry;
use crate::modules::proxy::{fetchable::Fetchable, params::TransformParams};
use crate::modules::security::{allowlist::Allowlist, hmac};
use crate::modules::transform::pipeline::{self, resolve_content_type};
use crate::modules::AppState;
use std::sync::Arc;
use url::Url;

pub struct ProxyService {
  fetcher: Arc<dyn Fetchable>,
  cache: Arc<CacheManager>,
  allowlist: Allowlist,
  hmac_key: Option<String>,
}

impl ProxyService {
  pub fn new(state: &AppState) -> Self {
    let allowlist = Allowlist::new(state.cfg.allowed_hosts.clone());
    Self {
      fetcher: state.fetcher.clone(),
      cache: state.cache.clone(),
      allowlist,
      hmac_key: state.cfg.hmac_key.clone(),
    }
  }

  pub async fn process(
    &self,
    params: TransformParams,
    image_url: String,
  ) -> Result<(CacheEntry, CacheHit), ProxyError> {
    // 1. Allowlist check for image URL host (HTTP/HTTPS only)
    if image_url.starts_with("http://") || image_url.starts_with("https://") {
      let image_host = Url::parse(&image_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
      if !self.allowlist.is_allowed(&image_host) {
        return Err(ProxyError::HostNotAllowed);
      }
    }

    // 2. Allowlist check for watermark URL host (HTTP/HTTPS only)
    if let Some(wm_url) = &params.wm {
      if wm_url.starts_with("http://") || wm_url.starts_with("https://") {
        let wm_host = Url::parse(wm_url)
          .ok()
          .and_then(|u| u.host_str().map(|h| h.to_string()))
          .unwrap_or_default();
        if !self.allowlist.is_allowed(&wm_host) {
          return Err(ProxyError::HostNotAllowed);
        }
      }
    }

    // 3. Compute canonical once (Issue 3 fix)
    let canonical = params.canonical_string(&image_url);

    // 4. HMAC check: if self.hmac_key is Some, verify params.sig against canonical_string
    if let Some(key) = &self.hmac_key {
      match &params.sig {
        None => return Err(ProxyError::InvalidSignature),
        Some(sig) if !hmac::verify(key, &canonical, sig) => {
          return Err(ProxyError::InvalidSignature)
        }
        _ => {}
      }
    }

    // 5. Cache lookup
    let prelim_key = CacheManager::preliminary_key(&canonical);

    let (cached, hit) = self.cache.get(&prelim_key).await;
    if let Some(entry) = cached {
      return Ok((entry, hit));
    }

    // 6. Singleflight: check if already inflight, or start one
    if self.cache.inflight().is_inflight(&prelim_key) {
      if let Some(result) = self.cache.inflight().wait(&prelim_key).await {
        return result.map(|entry| (entry, CacheHit::Miss));
      }
    }
    let guard = self.cache.inflight().start(prelim_key.clone());

    // 7. Fetch (Issue 4 fix: pass original error to guard)
    let fetch_result = self.fetcher.fetch(&image_url).await;
    let (mut src_bytes, mut src_ct) = match fetch_result {
      Ok(v) => v,
      Err(e) => {
        guard.complete(Err(e.clone()));
        return Err(e);
      }
    };

    // 8. Video interception (extract first/seeked frame and continue as PNG)
    let is_video = src_ct
      .as_deref()
      .map(|ct| ct.starts_with("video/"))
      .unwrap_or_else(|| crate::modules::proxy::sources::video::is_video_magic(&src_bytes));

    if is_video {
      match crate::modules::proxy::sources::video::extract_frame(&src_bytes, params.t.unwrap_or(0.0)) {
        Ok(frame) => match crate::modules::proxy::sources::video::frame_to_png_bytes(frame) {
          Ok(png_bytes) => {
            src_bytes = png_bytes;
            src_ct = Some("image/png".to_string());
          }
          Err(e) => {
            guard.complete(Err(e.clone()));
            return Err(e);
          }
        },
        Err(e) => {
          guard.complete(Err(e.clone()));
          return Err(e);
        }
      }
    }

    // 9. Force pipeline for PDF to rasterize first page even without transform flags.
    let is_pdf = src_ct.as_deref() == Some("application/pdf") || (!is_video && src_bytes.starts_with(b"%PDF"));

    // 10. If has_transforms or is_pdf: run_pipeline(); else resolve_content_type()
    let pipeline_result = if params.has_transforms() || is_pdf {
      pipeline::run_pipeline(params, src_bytes, src_ct, self.fetcher.clone())
        .await
        .map(|(bytes, ct)| CacheEntry {
          bytes,
          content_type: ct,
        })
    } else {
      resolve_content_type(src_ct.as_deref(), &src_bytes).map(|ct| CacheEntry {
        bytes: src_bytes,
        content_type: ct,
      })
    };

    let entry = match pipeline_result {
      Ok(e) => e,
      Err(e) => {
        guard.complete(Err(e.clone()));
        return Err(e);
      }
    };

    // 9. Write to cache with final_key = CacheManager::final_key(canonical, mime)
    let final_key = CacheManager::final_key(&canonical, &entry.content_type);
    self.cache.set(&final_key, entry.clone()).await;

    // 10. Call guard.complete(Ok(entry.clone()))
    guard.complete(Ok(entry.clone()));

    // 11. Return Ok((entry, CacheHit::Miss))
    Ok((entry, CacheHit::Miss))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::common::config::Configuration;
  use crate::modules::cache::manager::CacheManager;
  use crate::modules::proxy::fetchable::Fetchable;
  use std::net::{Ipv4Addr, SocketAddr};
  use std::sync::Arc;

  fn make_test_configuration() -> Arc<Configuration> {
    Arc::new(Configuration {
      env: crate::common::config::Environment::Development,
      listen_address: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8080)),
      app_port: 8080,
      hmac_key: None,
      allowed_hosts: vec![],
      fetch_timeout_secs: 10,
      max_source_bytes: 1_000_000,
      cache_memory_max_mb: 16,
      cache_memory_ttl_secs: 60,
      cache_dir: "/tmp/previewproxy-test".to_string(),
      cache_disk_ttl_secs: 60,
      cache_disk_max_mb: None,
      cache_cleanup_interval_secs: 600,
      s3_enabled: false,
      s3_bucket: None,
      s3_region: "us-east-1".to_string(),
      s3_access_key_id: None,
      s3_secret_access_key: None,
      s3_endpoint: None,
      local_enabled: false,
      local_base_dir: None,
    })
  }

  struct MockFetcher {
    error: ProxyError,
  }

  #[async_trait::async_trait]
  impl Fetchable for MockFetcher {
    async fn fetch(&self, _url: &str) -> Result<(Vec<u8>, Option<String>), ProxyError> {
      Err(self.error.clone())
    }
  }

  fn make_service_with_allowlist(allowed_hosts: Vec<String>) -> ProxyService {
    let fetcher: Arc<dyn Fetchable> = Arc::new(MockFetcher {
      error: ProxyError::InvalidParams("source not configured".to_string()),
    });
    let mut cfg = (*make_test_configuration()).clone();
    cfg.allowed_hosts = allowed_hosts.clone();
    let cfg = Arc::new(cfg);
    let cache = CacheManager::new(&cfg);
    ProxyService {
      fetcher,
      cache,
      allowlist: Allowlist::new(allowed_hosts),
      hmac_key: None,
    }
  }

  #[tokio::test]
  async fn test_s3_image_url_skips_allowlist() {
    // Allowlist only allows "example.com"; s3:/ URL should bypass allowlist entirely.
    let svc = make_service_with_allowlist(vec!["example.com".to_string()]);
    let params = TransformParams::default();
    let result = svc.process(params, "s3:/some/key.jpg".to_string()).await;
    // Should NOT be HostNotAllowed — it should reach the fetcher and return the mock error.
    assert!(
      !matches!(result, Err(ProxyError::HostNotAllowed)),
      "expected s3 URL to bypass allowlist, but got HostNotAllowed"
    );
  }

  #[tokio::test]
  async fn test_local_watermark_url_skips_allowlist() {
    // Allowlist only allows "example.com"; local:/ watermark URL should bypass allowlist.
    let svc = make_service_with_allowlist(vec!["example.com".to_string()]);
    // Use a mock HTTP image URL that IS allowed, with a local:/ watermark.
    // But since the fetcher is a mock that always errors, we just verify not HostNotAllowed.
    // Actually, the image URL is also not http — let's use a non-http image too to keep it simple.
    let params = TransformParams {
      wm: Some("local:/watermarks/logo.png".to_string()),
      ..TransformParams::default()
    };
    let result = svc
      .process(params, "s3:/images/photo.jpg".to_string())
      .await;
    assert!(
      !matches!(result, Err(ProxyError::HostNotAllowed)),
      "expected local:/ watermark to bypass allowlist, but got HostNotAllowed"
    );
  }

  #[tokio::test]
  async fn test_video_content_type_triggers_video_decode_error() {
    struct VideoMockFetcher;

    #[async_trait::async_trait]
    impl Fetchable for VideoMockFetcher {
      async fn fetch(&self, _url: &str) -> Result<(Vec<u8>, Option<String>), ProxyError> {
        let mut bytes = vec![0x00, 0x00, 0x00, 0x20];
        bytes.extend_from_slice(b"ftyp");
        bytes.extend(vec![0u8; 100]);
        Ok((bytes, Some("video/mp4".to_string())))
      }
    }

    let cfg = make_test_configuration();
    let fetcher: Arc<dyn Fetchable> = Arc::new(VideoMockFetcher);
    let cache = CacheManager::new(&cfg);
    let svc = ProxyService {
      fetcher,
      cache,
      allowlist: Allowlist::new(vec![]),
      hmac_key: None,
    };

    let params = TransformParams::default();
    let result = svc.process(params, "https://example.com/v.mp4".to_string()).await;
    assert!(
      matches!(result, Err(ProxyError::VideoDecodeError)),
      "expected VideoDecodeError for invalid video content"
    );
  }
}
