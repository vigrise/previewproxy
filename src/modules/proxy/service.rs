use crate::common::errors::ProxyError;
use crate::modules::cache::manager::{CacheHit, CacheManager};
use crate::modules::cache::memory::CacheEntry;
use crate::modules::proxy::{fetchable::Fetchable, params::TransformParams};
use crate::modules::security::{allowlist::Allowlist, hmac};
use crate::modules::transform::pipeline::{self, resolve_content_type};
use crate::modules::AppState;
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{mpsc, OwnedSemaphorePermit};
use url::Url;

pub enum ProcessResult {
  Cached(CacheEntry, CacheHit),
  Stream {
    /// Client-facing stream. Holds permit; drops permit when stream ends.
    body: futures::stream::BoxStream<'static, Result<Bytes, ProxyError>>,
    content_type: String,
  },
}

impl std::fmt::Debug for ProcessResult {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ProcessResult::Cached(_, _) => write!(f, "ProcessResult::Cached"),
      ProcessResult::Stream { content_type, .. } => {
        write!(f, "ProcessResult::Stream {{ content_type: {:?} }}", content_type)
      }
    }
  }
}

pub struct ProxyService {
  fetcher: Arc<dyn Fetchable>,
  http_fetcher: Arc<crate::modules::proxy::sources::http::HttpFetcher>,
  cache: Arc<CacheManager>,
  allowlist: Allowlist,
  hmac_key: Option<String>,
  ffmpeg_path: String,
  max_source_bytes: u64,
}

impl ProxyService {
  pub fn new(state: &AppState) -> Self {
    let allowlist = Allowlist::new(state.cfg.allowed_hosts.clone());
    Self {
      fetcher: state.fetcher.clone(),
      http_fetcher: state.http_fetcher.clone(),
      cache: state.cache.clone(),
      allowlist,
      hmac_key: state.cfg.hmac_key.clone(),
      ffmpeg_path: state.cfg.ffmpeg_path.clone(),
      max_source_bytes: state.cfg.max_source_bytes,
    }
  }

  pub async fn process(
    &self,
    params: TransformParams,
    image_url: String,
    permit: OwnedSemaphorePermit,
  ) -> Result<ProcessResult, ProxyError> {
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
      return Ok(ProcessResult::Cached(entry, hit));
    }

    // 6. Singleflight: check if already inflight, or start one
    if self.cache.inflight().is_inflight(&prelim_key) {
      if let Some(result) = self.cache.inflight().wait(&prelim_key).await {
        return result.map(|entry| ProcessResult::Cached(entry, CacheHit::Miss));
      }
    }
    let guard = self.cache.inflight().start(prelim_key.clone());

    // --- Streaming path: HTTP source, no transforms ---
    let is_http = image_url.starts_with("http://") || image_url.starts_with("https://");
    if is_http && !params.has_transforms() {
      let resp = match self.http_fetcher.fetch_streaming(&image_url).await {
        Ok(r) => r,
        Err(e) => {
          guard.complete(Err(e.clone()));
          return Err(e);
        }
      };

      let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
        .unwrap_or_default();

      if content_type.starts_with("video/") || content_type == "application/pdf" {
        drop(resp);
        // fall through to buffered path
      } else if content_type.starts_with("image/") {
        let (client_tx, client_rx) = mpsc::channel::<Bytes>(8);
        let (cache_tx, mut cache_rx) = mpsc::channel::<Result<Bytes, ProxyError>>(8);

        // Tee task: drives reqwest body -> both channels
        tokio::spawn(async move {
          use futures::StreamExt;
          let mut s = resp.bytes_stream();
          while let Some(chunk) = s.next().await {
            match chunk {
              Ok(b) => {
                let _ = cache_tx.send(Ok(b.clone())).await;
                if client_tx.send(b).await.is_err() {
                  let _ = cache_tx.send(Err(ProxyError::InternalError("client_disconnected".to_string()))).await;
                  return;
                }
              }
              Err(e) => {
                let pe = ProxyError::InternalError(e.to_string());
                let _ = cache_tx.send(Err(pe)).await;
                return;
              }
            }
          }
        });

        // Cache writer task: accumulates, writes on clean close, discards on error
        let cache = self.cache.clone();
        let max_bytes = self.max_source_bytes;
        let content_type_bg = content_type.clone();
        let canonical_bg = canonical.clone();
        tokio::spawn(async move {
          let mut buf: Vec<u8> = Vec::new();
          loop {
            match cache_rx.recv().await {
              Some(Ok(b)) => {
                buf.extend_from_slice(&b);
                if buf.len() as u64 > max_bytes {
                  guard.complete(Err(ProxyError::SourceTooLarge));
                  return;
                }
              }
              Some(Err(e)) => {
                guard.complete(Err(e));
                return;
              }
              None => {
                let entry = CacheEntry { bytes: buf, content_type: content_type_bg.clone() };
                let final_key = CacheManager::final_key(&canonical_bg, &content_type_bg);
                cache.set(&final_key, entry.clone()).await;
                guard.complete(Ok(entry));
                return;
              }
            }
          }
        });

        let stream = futures::stream::unfold(
          (client_rx, permit),
          |(mut rx, permit)| async move {
            rx.recv().await.map(|b| (Ok::<Bytes, ProxyError>(b), (rx, permit)))
          },
        );

        return Ok(ProcessResult::Stream {
          body: Box::pin(stream),
          content_type,
        });
      } else {
        guard.complete(Err(ProxyError::NotAnImage));
        return Err(ProxyError::NotAnImage);
      }
    }
    // --- End streaming path (video/PDF fell through to here) ---
    drop(permit);

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
      match crate::modules::proxy::sources::video::extract_frame(
        &src_bytes,
        params.t.unwrap_or(0.0),
        &self.ffmpeg_path,
      )
      .await
      {
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
    let is_pdf =
      src_ct.as_deref() == Some("application/pdf") || (!is_video && src_bytes.starts_with(b"%PDF"));

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

    // 11. Return Ok(ProcessResult::Cached(entry, CacheHit::Miss))
    Ok(ProcessResult::Cached(entry, CacheHit::Miss))
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
      ffmpeg_path: "ffmpeg".to_string(),
      cors_allow_origin: vec!["*".to_string()],
      cors_max_age_secs: 600,
      max_concurrent_requests: 256,
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
      http_fetcher: Arc::new(
        crate::modules::proxy::sources::http::HttpFetcher::new(
          10, 1_000_000,
          Arc::new(crate::modules::security::allowlist::Allowlist::new(vec![]))
        ).with_private_ip_check(false)
      ),
      cache,
      allowlist: Allowlist::new(allowed_hosts),
      hmac_key: None,
      ffmpeg_path: "ffmpeg".to_string(),
      max_source_bytes: 1_000_000,
    }
  }

  #[tokio::test]
  async fn test_s3_image_url_skips_allowlist() {
    // Allowlist only allows "example.com"; s3:/ URL should bypass allowlist entirely.
    let svc = make_service_with_allowlist(vec!["example.com".to_string()]);
    let params = TransformParams::default();
    let result = svc
      .process(
        params,
        "s3:/some/key.jpg".to_string(),
        Arc::new(tokio::sync::Semaphore::new(1)).try_acquire_owned().unwrap(),
      )
      .await;
    // Should NOT be HostNotAllowed - it should reach the fetcher and return the mock error.
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
    // Actually, the image URL is also not http - let's use a non-http image too to keep it simple.
    let params = TransformParams {
      wm: Some("local:/watermarks/logo.png".to_string()),
      ..TransformParams::default()
    };
    let result = svc
      .process(
        params,
        "s3:/images/photo.jpg".to_string(),
        Arc::new(tokio::sync::Semaphore::new(1)).try_acquire_owned().unwrap(),
      )
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
      http_fetcher: Arc::new(
        crate::modules::proxy::sources::http::HttpFetcher::new(
          10, 1_000_000,
          Arc::new(crate::modules::security::allowlist::Allowlist::new(vec![]))
        ).with_private_ip_check(false)
      ),
      cache,
      allowlist: Allowlist::new(vec![]),
      hmac_key: None,
      ffmpeg_path: "ffmpeg".to_string(),
      max_source_bytes: 1_000_000,
    };

    let params = TransformParams::default();
    let result = svc
      .process(
        params,
        "s3:/v.mp4".to_string(),
        Arc::new(tokio::sync::Semaphore::new(1)).try_acquire_owned().unwrap(),
      )
      .await;
    assert!(
      matches!(result, Err(ProxyError::VideoDecodeError)),
      "expected VideoDecodeError for invalid video content"
    );
  }
}

#[cfg(test)]
mod streaming_tests {
  use super::*;
  use crate::modules::cache::manager::CacheManager;
  use crate::modules::proxy::params::TransformParams;
  use crate::modules::proxy::sources::http::HttpFetcher;
  use crate::modules::security::allowlist::Allowlist;
  use futures::StreamExt;
  use std::sync::Arc;
  use tokio::sync::Semaphore;
  use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

  fn make_svc(max_bytes: u64) -> (ProxyService, Arc<CacheManager>) {
    let cfg = Arc::new(crate::common::config::Configuration {
      env: crate::common::config::Environment::Development,
      listen_address: "0.0.0.0:8080".parse().unwrap(),
      app_port: 8080,
      hmac_key: None,
      allowed_hosts: vec![],
      fetch_timeout_secs: 10,
      max_source_bytes: max_bytes,
      cache_memory_max_mb: 16,
      cache_memory_ttl_secs: 60,
      cache_dir: "/tmp/previewproxy-svc-stream-test".to_string(),
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
      ffmpeg_path: "ffmpeg".to_string(),
      cors_allow_origin: vec!["*".to_string()],
      cors_max_age_secs: 600,
      max_concurrent_requests: 256,
    });
    let http = Arc::new(
      HttpFetcher::new(10, max_bytes, Arc::new(Allowlist::new(vec![])))
        .with_private_ip_check(false),
    );
    let cache = CacheManager::new(&cfg);
    let svc = ProxyService {
      fetcher: http.clone(),
      http_fetcher: http,
      cache: cache.clone(),
      allowlist: Allowlist::new(vec![]),
      hmac_key: None,
      ffmpeg_path: "ffmpeg".to_string(),
      max_source_bytes: max_bytes,
    };
    (svc, cache)
  }

  fn permit() -> tokio::sync::OwnedSemaphorePermit {
    Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap()
  }

  #[tokio::test]
  async fn test_streaming_passthrough_no_transforms() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(vec![1u8; 50])
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;
    let (svc, _) = make_svc(1_000_000);
    let result = svc
      .process(TransformParams::default(), server.uri(), permit())
      .await
      .unwrap();
    assert!(matches!(result, ProcessResult::Stream { .. }));
  }

  #[tokio::test]
  async fn test_streaming_falls_back_for_non_image() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(b"<html>".to_vec())
          .insert_header("content-type", "text/html"),
      )
      .mount(&server)
      .await;
    let (svc, _) = make_svc(1_000_000);
    let result = svc
      .process(TransformParams::default(), server.uri(), permit())
      .await;
    assert!(matches!(result, Err(ProxyError::NotAnImage)));
  }

  #[tokio::test]
  async fn test_streaming_falls_back_for_video() {
    let server = MockServer::start().await;
    let mut body = vec![0x00, 0x00, 0x00, 0x20];
    body.extend_from_slice(b"ftyp");
    body.extend(vec![0u8; 100]);
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(body)
          .insert_header("content-type", "video/mp4"),
      )
      .mount(&server)
      .await;
    let (svc, _) = make_svc(1_000_000);
    let result = svc
      .process(TransformParams::default(), server.uri(), permit())
      .await;
    assert!(
      matches!(result, Err(ProxyError::VideoDecodeError)),
      "expected VideoDecodeError proving video fell through to buffered path, got: {:?}",
      result
    );
  }

  #[tokio::test]
  async fn test_streaming_falls_back_for_pdf() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(b"%PDF-1.4 fake".to_vec())
          .insert_header("content-type", "application/pdf"),
      )
      .mount(&server)
      .await;
    let (svc, _) = make_svc(1_000_000);
    let result = svc
      .process(TransformParams::default(), server.uri(), permit())
      .await;
    assert!(matches!(result, Err(ProxyError::PdfRenderError)));
  }

  #[tokio::test]
  async fn test_cache_not_written_when_client_drops_stream_early() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(vec![1u8; 100])
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;
    let (svc, cache) = make_svc(1_000_000);
    let url = server.uri();
    let result = svc
      .process(TransformParams::default(), url.clone(), permit())
      .await
      .unwrap();
    if let ProcessResult::Stream { body, .. } = result {
      drop(body);
      tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let canonical = TransformParams::default().canonical_string(&url);
    let final_key = CacheManager::final_key(&canonical, "image/png");
    let (entry, _) = cache.get(&final_key).await;
    assert!(
      entry.is_none(),
      "cache must not be written when stream is dropped before exhaustion"
    );
  }

  #[tokio::test]
  async fn test_streaming_falls_back_for_s3() {
    let (svc, _) = make_svc(1_000_000);
    let result = svc
      .process(
        TransformParams::default(),
        "s3:/some/key.jpg".to_string(),
        permit(),
      )
      .await;
    assert!(!matches!(result, Ok(ProcessResult::Stream { .. })));
  }

  #[tokio::test]
  async fn test_cache_written_after_clean_stream() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(vec![1u8; 20])
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;
    let (svc, cache) = make_svc(1_000_000);
    let url = server.uri();
    let result = svc
      .process(TransformParams::default(), url.clone(), permit())
      .await
      .unwrap();
    if let ProcessResult::Stream { mut body, .. } = result {
      while body.next().await.is_some() {}
      drop(body);
      tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let canonical = TransformParams::default().canonical_string(&url);
    let final_key = CacheManager::final_key(&canonical, "image/png");
    let (entry, _) = cache.get(&final_key).await;
    assert!(entry.is_some(), "cache entry should exist after clean stream");
  }

  #[tokio::test]
  async fn test_cache_not_written_when_max_bytes_exceeded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(vec![1u8; 200])
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;
    let (svc, cache) = make_svc(50);
    let url = server.uri();
    let result = svc
      .process(TransformParams::default(), url.clone(), permit())
      .await
      .unwrap();
    if let ProcessResult::Stream { mut body, .. } = result {
      while body.next().await.is_some() {}
      drop(body);
      tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let canonical = TransformParams::default().canonical_string(&url);
    let final_key = CacheManager::final_key(&canonical, "image/png");
    let (entry, _) = cache.get(&final_key).await;
    assert!(entry.is_none(), "cache must not be written when size limit exceeded");
  }
}
