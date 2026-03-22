use std::sync::Mutex;
use tower::ServiceExt;

// Serialize tests that mutate env vars to avoid race conditions
static ENV_MUTEX: Mutex<()> = Mutex::new(());

fn tiny_png() -> Vec<u8> {
  // 1x1 pixel PNG - properly encoded, decodeable by the image crate
  use base64::{engine::general_purpose::STANDARD, Engine};
  STANDARD
    .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC")
    .unwrap()
}

async fn build_test_app() -> axum::Router {
  unsafe {
    std::env::set_var("PORT", "8081");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("CACHE_DIR", "/tmp/previewproxy-test");
    std::env::set_var("CACHE_MEMORY_MAX_MB", "10");
    std::env::remove_var("HMAC_KEY");
    std::env::remove_var("ALLOWED_HOSTS");
    std::env::remove_var("LOCAL_ENABLED");
    std::env::remove_var("LOCAL_BASE_DIR");
  }
  let cfg = previewproxy::common::config::Configuration::new();
  let cache = previewproxy::modules::cache::manager::CacheManager::new(&cfg);
  previewproxy::app::router(cfg, cache).await
}

#[tokio::test]
async fn test_health_returns_ok() {
  let _guard = ENV_MUTEX.lock().unwrap();
  let app = build_test_app().await;
  let resp = app
    .oneshot(
      axum::http::Request::builder()
        .uri("/health")
        .body(axum::body::Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();
  assert_eq!(resp.status(), 200);
  use http_body_util::BodyExt;
  let body = resp.into_body().collect().await.unwrap().to_bytes();
  let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
  assert_eq!(json["status"], "ok");
  assert!(json["cache_memory_items"].is_number());
}

#[tokio::test]
async fn test_proxy_query_style_cache_miss() {
  use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
  let mock_server = MockServer::start().await;
  Mock::given(method("GET"))
    .respond_with(
      ResponseTemplate::new(200)
        .set_body_bytes(tiny_png())
        .insert_header("content-type", "image/png"),
    )
    .mount(&mock_server)
    .await;

  let _guard = ENV_MUTEX.lock().unwrap();
  let app = build_test_app().await;
  let url = format!("/proxy?url={}", urlencoding::encode(&mock_server.uri()));
  let resp = app
    .oneshot(
      axum::http::Request::builder()
        .uri(&url)
        .body(axum::body::Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();
  assert_eq!(resp.status(), 200);
  assert_eq!(resp.headers()["x-cache"], "MISS");
  let ct = resp.headers()["content-type"].to_str().unwrap();
  assert!(ct.starts_with("image/png"));
}

#[tokio::test]
async fn test_blocked_host_returns_403() {
  let _guard = ENV_MUTEX.lock().unwrap();
  unsafe {
    std::env::set_var("PORT", "8081");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("CACHE_DIR", "/tmp/previewproxy-test");
    std::env::set_var("CACHE_MEMORY_MAX_MB", "10");
    std::env::set_var("ALLOWED_HOSTS", "trusted.com");
    std::env::remove_var("HMAC_KEY");
    std::env::remove_var("LOCAL_ENABLED");
    std::env::remove_var("LOCAL_BASE_DIR");
  }
  let cfg = previewproxy::common::config::Configuration::new();
  let cache = previewproxy::modules::cache::manager::CacheManager::new(&cfg);
  let app = previewproxy::app::router(cfg, cache).await;

  let url = format!("/proxy?url={}", urlencoding::encode("https://http.cat/200"));
  let resp = app
    .oneshot(
      axum::http::Request::builder()
        .uri(&url)
        .body(axum::body::Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();
  assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_bad_hmac_returns_403() {
  let _guard = ENV_MUTEX.lock().unwrap();
  unsafe {
    std::env::set_var("PORT", "8081");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("CACHE_DIR", "/tmp/previewproxy-test");
    std::env::set_var("CACHE_MEMORY_MAX_MB", "10");
    std::env::set_var("HMAC_KEY", "secret");
    std::env::remove_var("ALLOWED_HOSTS");
    std::env::remove_var("LOCAL_ENABLED");
    std::env::remove_var("LOCAL_BASE_DIR");
  }
  let cfg = previewproxy::common::config::Configuration::new();
  let cache = previewproxy::modules::cache::manager::CacheManager::new(&cfg);
  let app = previewproxy::app::router(cfg, cache).await;

  let image_url = urlencoding::encode("https://http.cat/200");
  let url = format!("/proxy?url={}&sig=badsig", image_url);
  let resp = app
    .oneshot(
      axum::http::Request::builder()
        .uri(&url)
        .body(axum::body::Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();
  assert_eq!(resp.status(), 403);
}

/// Full proxy flow: local image file fetched with no transforms (passthrough)
#[tokio::test]
async fn test_local_source_passthrough() {
  let _guard = ENV_MUTEX.lock().unwrap();
  let tmp = tempfile::TempDir::new().unwrap();
  let img_path = tmp.path().join("test.png");
  std::fs::write(&img_path, tiny_png()).unwrap();

  unsafe {
    std::env::set_var("PORT", "8081");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("CACHE_DIR", "/tmp/previewproxy-test-local-passthrough");
    std::env::set_var("CACHE_MEMORY_MAX_MB", "10");
    std::env::remove_var("HMAC_KEY");
    std::env::remove_var("ALLOWED_HOSTS");
    std::env::set_var("LOCAL_ENABLED", "true");
    std::env::set_var("LOCAL_BASE_DIR", tmp.path().to_str().unwrap());
  }

  let cfg = previewproxy::common::config::Configuration::new();
  let cache = previewproxy::modules::cache::manager::CacheManager::new(&cfg);
  let app = previewproxy::app::router(cfg, cache).await;

  // local:/test.png - relative path joined to LOCAL_BASE_DIR
  let resp = app
    .oneshot(
      axum::http::Request::builder()
        .uri("/local:/test.png")
        .body(axum::body::Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), 200, "expected 200 for local passthrough");
}

/// Full proxy flow: local image file fetched, resized, returned correctly
#[tokio::test]
async fn test_local_source_with_resize() {
  let _guard = ENV_MUTEX.lock().unwrap();
  let tmp = tempfile::TempDir::new().unwrap();
  let img_path = tmp.path().join("test.png");
  std::fs::write(&img_path, tiny_png()).unwrap();

  unsafe {
    std::env::set_var("PORT", "8081");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("CACHE_DIR", "/tmp/previewproxy-test-local-resize");
    std::env::set_var("CACHE_MEMORY_MAX_MB", "10");
    std::env::remove_var("HMAC_KEY");
    std::env::remove_var("ALLOWED_HOSTS");
    std::env::set_var("LOCAL_ENABLED", "true");
    std::env::set_var("LOCAL_BASE_DIR", tmp.path().to_str().unwrap());
  }

  let cfg = previewproxy::common::config::Configuration::new();
  let cache = previewproxy::modules::cache::manager::CacheManager::new(&cfg);
  let app = previewproxy::app::router(cfg, cache).await;

  // 1x1,webp/local:/test.png - resize to 1x1 and convert to webp
  let resp = app
    .oneshot(
      axum::http::Request::builder()
        .uri("/1x1,webp/local:/test.png")
        .body(axum::body::Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), 200, "expected 200 for local with resize");
  let ct = resp.headers()["content-type"].to_str().unwrap();
  assert!(
    ct.starts_with("image/webp"),
    "expected image/webp content-type, got: {ct}"
  );
}
