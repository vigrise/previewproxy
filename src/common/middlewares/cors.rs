use std::time::Duration;

use axum::http::HeaderValue;
use tower_http::cors::{AllowHeaders, AllowOrigin, Any, CorsLayer};

fn matches_origin(pattern: &str, origin: &str) -> bool {
  // Extract host (and optional port) from origin URL, e.g. "https://sub.example.com:8080"
  let host = origin
    .strip_prefix("https://")
    .or_else(|| origin.strip_prefix("http://"))
    .unwrap_or(origin)
    .split('/')
    .next()
    .unwrap_or(origin);

  if let Some(suffix) = pattern.strip_prefix("*.") {
    if let Some(rest) = host.strip_suffix(suffix) {
      let label = rest.strip_suffix('.').unwrap_or(rest);
      return !label.is_empty() && !label.contains('.');
    }
    false
  } else {
    pattern == host || pattern == origin
  }
}

/// Layer that applies the Cors middleware which adds headers for CORS.
pub fn cors_layer(allow_origin: &[String], max_age_secs: u64) -> CorsLayer {
  let origin = if allow_origin.iter().any(|o| o == "*") {
    AllowOrigin::any()
  } else if allow_origin.iter().any(|o| o.contains("*.")) {
    let patterns: Vec<String> = allow_origin.to_vec();
    AllowOrigin::predicate(move |origin: &HeaderValue, _| {
      let s = origin.to_str().unwrap_or("");
      patterns.iter().any(|p| matches_origin(p, s))
    })
  } else {
    AllowOrigin::list(
      allow_origin
        .iter()
        .filter_map(|o| o.parse::<HeaderValue>().ok()),
    )
  };

  CorsLayer::new()
    .allow_origin(origin)
    .allow_methods(Any)
    .allow_headers(AllowHeaders::mirror_request())
    .max_age(Duration::from_secs(max_age_secs))
}
