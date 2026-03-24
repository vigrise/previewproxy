use crate::common::errors::ProxyError;
use crate::modules::cache::manager::{CacheHit, CacheManager};
use crate::modules::cache::memory::CacheEntry;
use crate::modules::proxy::{
  dto::{params::TransformParams, ProcessResult},
  fetchable::Fetchable,
};
use crate::modules::security::{allowlist::Allowlist, hmac};
use crate::modules::transform::pipeline::{self, resolve_content_type};
use crate::modules::AppState;
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::{mpsc, OwnedSemaphorePermit};
use url::Url;

/// Per-request service that orchestrates allowlist checks, HMAC verification,
/// cache lookup/storage, in-flight coalescing, fetching, and image transforms.
pub struct ProxyService {
  fetcher: Arc<dyn Fetchable>,
  http_fetcher: Arc<crate::modules::proxy::sources::http::HttpFetcher>,
  cache: Arc<CacheManager>,
  allowlist: Allowlist,
  hmac_key: Option<String>,
  ffmpeg_path: String,
  ffprobe_path: String,
  max_source_bytes: u64,
  input_disallow: std::collections::HashSet<crate::common::config::DisallowedInput>,
  output_disallow: std::collections::HashSet<crate::common::config::DisallowedOutput>,
  transform_disallow: std::collections::HashSet<crate::common::config::DisallowedTransform>,
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
      ffprobe_path: state.cfg.ffprobe_path.clone(),
      max_source_bytes: state.cfg.max_source_bytes,
      input_disallow: state.cfg.input_disallow.clone(),
      output_disallow: state.cfg.output_disallow.clone(),
      transform_disallow: state.cfg.transform_disallow.clone(),
    }
  }

  /// Full request pipeline:
  ///
  /// 1. Allowlist check on image URL and watermark URL hosts
  /// 2. HMAC signature verification (skipped when no key is configured)
  /// 3. Cache lookup (L1 then L2); returns immediately on hit
  /// 4. In-flight deduplication: wait on existing request for the same key
  /// 5. **Streaming path** (HTTP source + no transforms): tee the upstream
  ///    response body directly to the client while writing to cache in the
  ///    background; the concurrency permit is held inside the stream
  /// 6. **Buffered path**: fetch full source bytes, run the transform
  ///    pipeline, store the result in cache, return `ProcessResult::Cached`
  ///
  /// Video and PDF sources always take the buffered path regardless of whether
  /// transforms are requested.
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

      if let Err(e) = self.check_input_disallow(&content_type) {
        guard.complete(Err(e.clone()));
        return Err(e);
      }

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
                if client_tx.send(b.clone()).await.is_err() {
                  let _ = cache_tx
                    .send(Err(ProxyError::InternalError(
                      "client_disconnected".to_string(),
                    )))
                    .await;
                  return;
                }
                let _ = cache_tx.send(Ok(b)).await;
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
                let entry = CacheEntry {
                  bytes: buf,
                  content_type: content_type_bg.clone(),
                };
                let key = CacheManager::preliminary_key(&canonical_bg);
                cache.set(&key, entry.clone()).await;
                guard.complete(Ok(entry));
                return;
              }
            }
          }
        });

        let stream = futures::stream::unfold((client_rx, permit), |(mut rx, permit)| async move {
          rx.recv()
            .await
            .map(|b| (Ok::<Bytes, ProxyError>(b), (rx, permit)))
        });

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

    if is_video
      && self
        .input_disallow
        .contains(&crate::common::config::DisallowedInput::Video)
    {
      guard.complete(Err(ProxyError::TransformDisabled("video".to_string())));
      return Err(ProxyError::TransformDisabled("video".to_string()));
    }

    if is_video {
      use crate::modules::proxy::dto::params::SeekMode;
      use crate::modules::proxy::sources::video::{extract_frame, probe_duration};

      let t_secs = match &params.seek {
        None => 0.0,
        Some(SeekMode::Absolute(s)) => *s,
        Some(SeekMode::Relative(r)) => match probe_duration(&src_bytes, &self.ffprobe_path).await {
          Ok(dur) => dur * r,
          Err(_) => {
            tracing::warn!("ffprobe failed for relative seek, falling back to t=0.0");
            0.0
          }
        },
        Some(SeekMode::Auto) => match probe_duration(&src_bytes, &self.ffprobe_path).await {
          Ok(dur) => dur * 0.5,
          Err(_) => {
            tracing::warn!("ffprobe failed for auto seek, falling back to t=0.0");
            0.0
          }
        },
      };

      match extract_frame(&src_bytes, t_secs, &self.ffmpeg_path).await {
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

    // Input disallow check on buffered path.
    // Note: if the fetcher returns no content-type (src_ct is None), this check is skipped.
    // Magic-byte-only sources (e.g. S3 objects without a content-type header) bypass
    // INPUT_DISALLOW_LIST. This is a known limitation.
    if let Some(ref ct) = src_ct {
      if let Err(e) = self.check_input_disallow(ct) {
        guard.complete(Err(e.clone()));
        return Err(e);
      }
    }

    // 9. Force pipeline for PDF to rasterize first page even without transform flags.
    let is_pdf =
      src_ct.as_deref() == Some("application/pdf") || (!is_video && src_bytes.starts_with(b"%PDF"));

    // 10. If has_transforms or is_pdf: run_pipeline(); else resolve_content_type()
    let pipeline_result = if params.has_transforms() || is_pdf {
      pipeline::run_pipeline(
        params,
        src_bytes,
        src_ct,
        self.fetcher.clone(),
        &self.output_disallow,
        &self.transform_disallow,
      )
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

    // 9. Write to cache
    self.cache.set(&prelim_key, entry.clone()).await;

    // 10. Call guard.complete(Ok(entry.clone()))
    guard.complete(Ok(entry.clone()));

    // 11. Return Ok(ProcessResult::Cached(entry, CacheHit::Miss))
    Ok(ProcessResult::Cached(entry, CacheHit::Miss))
  }

  /// Check if a content-type is in the input disallow list.
  /// Does NOT handle `video/*` MIME types - video is blocked via `DisallowedInput::Video`
  /// before the video extraction path, not through this function.
  /// Returns `Ok(())` for unknown or unrecognized content types.
  fn check_input_disallow(&self, content_type: &str) -> Result<(), ProxyError> {
    use crate::common::config::DisallowedInput;
    let token = match content_type {
      "image/jpeg" => Some(DisallowedInput::Jpeg),
      "image/png" => Some(DisallowedInput::Png),
      "image/gif" => Some(DisallowedInput::Gif),
      "image/webp" => Some(DisallowedInput::Webp),
      "image/avif" => Some(DisallowedInput::Avif),
      "image/jxl" => Some(DisallowedInput::Jxl),
      "image/bmp" => Some(DisallowedInput::Bmp),
      "image/tiff" => Some(DisallowedInput::Tiff),
      "application/pdf" => Some(DisallowedInput::Pdf),
      "image/vnd.adobe.photoshop" => Some(DisallowedInput::Psd),
      _ => None,
    };
    if let Some(t) = token {
      if self.input_disallow.contains(&t) {
        let name = format!("{t:?}").to_lowercase();
        return Err(ProxyError::TransformDisabled(name));
      }
    }
    Ok(())
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
      ffprobe_path: "ffprobe".to_string(),
      cors_allow_origin: vec!["*".to_string()],
      cors_max_age_secs: 600,
      max_concurrent_requests: 256,
      input_disallow: std::collections::HashSet::new(),
      output_disallow: std::collections::HashSet::new(),
      transform_disallow: std::collections::HashSet::new(),
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
          10,
          1_000_000,
          Arc::new(crate::modules::security::allowlist::Allowlist::new(vec![])),
        )
        .with_private_ip_check(false),
      ),
      cache,
      allowlist: Allowlist::new(allowed_hosts),
      hmac_key: None,
      ffmpeg_path: "ffmpeg".to_string(),
      ffprobe_path: "ffprobe".to_string(),
      max_source_bytes: 1_000_000,
      input_disallow: std::collections::HashSet::new(),
      output_disallow: std::collections::HashSet::new(),
      transform_disallow: std::collections::HashSet::new(),
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
        Arc::new(tokio::sync::Semaphore::new(1))
          .try_acquire_owned()
          .unwrap(),
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
        Arc::new(tokio::sync::Semaphore::new(1))
          .try_acquire_owned()
          .unwrap(),
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
          10,
          1_000_000,
          Arc::new(crate::modules::security::allowlist::Allowlist::new(vec![])),
        )
        .with_private_ip_check(false),
      ),
      cache,
      allowlist: Allowlist::new(vec![]),
      hmac_key: None,
      ffmpeg_path: "ffmpeg".to_string(),
      ffprobe_path: "ffprobe".to_string(),
      max_source_bytes: 1_000_000,
      input_disallow: std::collections::HashSet::new(),
      output_disallow: std::collections::HashSet::new(),
      transform_disallow: std::collections::HashSet::new(),
    };

    let params = TransformParams::default();
    let result = svc
      .process(
        params,
        "s3:/v.mp4".to_string(),
        Arc::new(tokio::sync::Semaphore::new(1))
          .try_acquire_owned()
          .unwrap(),
      )
      .await;
    assert!(
      matches!(result, Err(ProxyError::VideoDecodeError)),
      "expected VideoDecodeError for invalid video content"
    );
  }

  #[tokio::test]
  async fn test_video_disallowed_returns_transform_disabled() {
    use crate::common::config::DisallowedInput;

    struct VideoMockFetcher2;
    #[async_trait::async_trait]
    impl Fetchable for VideoMockFetcher2 {
      async fn fetch(&self, _url: &str) -> Result<(Vec<u8>, Option<String>), ProxyError> {
        let mut bytes = vec![0x00, 0x00, 0x00, 0x20];
        bytes.extend_from_slice(b"ftyp");
        bytes.extend(vec![0u8; 100]);
        Ok((bytes, Some("video/mp4".to_string())))
      }
    }

    let cfg = make_test_configuration();
    let fetcher: Arc<dyn Fetchable> = Arc::new(VideoMockFetcher2);
    let cache = CacheManager::new(&cfg);
    let mut input_disallow = std::collections::HashSet::new();
    input_disallow.insert(DisallowedInput::Video);
    let svc = ProxyService {
      fetcher,
      http_fetcher: Arc::new(
        crate::modules::proxy::sources::http::HttpFetcher::new(
          10,
          1_000_000,
          Arc::new(crate::modules::security::allowlist::Allowlist::new(vec![])),
        )
        .with_private_ip_check(false),
      ),
      cache,
      allowlist: Allowlist::new(vec![]),
      hmac_key: None,
      ffmpeg_path: "ffmpeg".to_string(),
      ffprobe_path: "ffprobe".to_string(),
      max_source_bytes: 1_000_000,
      input_disallow,
      output_disallow: std::collections::HashSet::new(),
      transform_disallow: std::collections::HashSet::new(),
    };
    let params = TransformParams::default();
    let result = svc
      .process(
        params,
        "s3:/v.mp4".to_string(),
        Arc::new(tokio::sync::Semaphore::new(1))
          .try_acquire_owned()
          .unwrap(),
      )
      .await;
    assert!(
      matches!(result, Err(ProxyError::TransformDisabled(_))),
      "expected TransformDisabled for disallowed video input, got: {result:?}"
    );
  }
}

#[cfg(test)]
mod streaming_tests {
  use super::*;
  use crate::modules::cache::manager::CacheManager;
  use crate::modules::proxy::dto::params::TransformParams;
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
      cache_disk_ttl_secs: 0,
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
      ffprobe_path: "ffprobe".to_string(),
      cors_allow_origin: vec!["*".to_string()],
      cors_max_age_secs: 600,
      max_concurrent_requests: 256,
      input_disallow: std::collections::HashSet::new(),
      output_disallow: std::collections::HashSet::new(),
      transform_disallow: std::collections::HashSet::new(),
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
      ffprobe_path: "ffprobe".to_string(),
      max_source_bytes: max_bytes,
      input_disallow: std::collections::HashSet::new(),
      output_disallow: std::collections::HashSet::new(),
      transform_disallow: std::collections::HashSet::new(),
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
    let prelim_key = CacheManager::preliminary_key(&canonical);
    let (entry, _) = cache.get(&prelim_key).await;
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
    let prelim_key = CacheManager::preliminary_key(&canonical);
    let (entry, _) = cache.get(&prelim_key).await;
    assert!(
      entry.is_some(),
      "cache entry should exist after clean stream"
    );
  }

  #[tokio::test]
  async fn test_streaming_input_disallowed_returns_transform_disabled() {
    use crate::common::config::DisallowedInput;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(vec![1u8; 50])
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;

    let (mut svc, _cache) = make_svc(1_000_000);
    svc.input_disallow.insert(DisallowedInput::Png);

    let result = svc
      .process(
        TransformParams::default(),
        format!("{}/img.png", server.uri()),
        permit(),
      )
      .await;

    assert!(
      matches!(result, Err(ProxyError::TransformDisabled(_))),
      "expected TransformDisabled for disallowed PNG in streaming path, got: {result:?}"
    );
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
    let prelim_key = CacheManager::preliminary_key(&canonical);
    let (entry, _) = cache.get(&prelim_key).await;
    assert!(
      entry.is_none(),
      "cache must not be written when size limit exceeded"
    );
  }
}
