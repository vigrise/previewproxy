use crate::common::config::types::{
  DisallowedInput, DisallowedOutput, DisallowedTransform, Environment,
};
use std::{
  collections::{HashMap, HashSet},
  net::{Ipv4Addr, SocketAddr},
  sync::Arc,
};
use tracing::info;

pub type Config = Arc<Configuration>;

#[derive(Clone)]
pub struct Configuration {
  pub env: Environment,
  pub listen_address: SocketAddr,
  pub app_port: u16,
  // Security
  pub hmac_key: Option<String>,
  pub allowed_hosts: Vec<String>,
  // Fetching
  pub fetch_timeout_secs: u64,
  pub max_source_bytes: u64,
  // Cache
  pub cache_memory_max_mb: u64,
  pub cache_memory_ttl_secs: u64,
  pub cache_dir: String,
  pub cache_disk_ttl_secs: u64,
  pub cache_disk_max_mb: Option<u64>,
  pub cache_cleanup_interval_secs: u64,
  // S3 source
  pub s3_enabled: bool,
  pub s3_bucket: Option<String>,
  pub s3_region: String,
  pub s3_access_key_id: Option<String>,
  pub s3_secret_access_key: Option<String>,
  pub s3_endpoint: Option<String>,
  // Local filesystem source
  pub local_enabled: bool,
  pub local_base_dir: Option<String>,
  // Video
  pub ffmpeg_path: String,
  pub ffprobe_path: String,
  // CORS
  pub cors_allow_origin: Vec<String>,
  pub cors_max_age_secs: u64,
  // Concurrency
  pub max_concurrent_requests: usize,
  // Disallow lists
  pub input_disallow: HashSet<DisallowedInput>,
  pub output_disallow: HashSet<DisallowedOutput>,
  pub transform_disallow: HashSet<DisallowedTransform>,
  // URL aliases
  pub url_aliases: Option<HashMap<String, String>>,
}

fn env_var(name: &str) -> String {
  std::env::var(name).unwrap_or_else(|_| panic!("Missing env var: {name}"))
}

fn env_var_opt(name: &str) -> Option<String> {
  std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn env_var_u64(name: &str, default: u64) -> u64 {
  std::env::var(name)
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(default)
}

fn env_var_bool(name: &str) -> bool {
  std::env::var(name)
    .map(|v| v == "true" || v == "1")
    .unwrap_or(false)
}

fn parse_input_disallow(s: &str) -> HashSet<DisallowedInput> {
  s.split(',')
    .map(|t| t.trim())
    .filter(|t| !t.is_empty())
    .filter_map(|t| match t {
      "jpeg" => Some(DisallowedInput::Jpeg),
      "png" => Some(DisallowedInput::Png),
      "gif" => Some(DisallowedInput::Gif),
      "webp" => Some(DisallowedInput::Webp),
      "avif" => Some(DisallowedInput::Avif),
      "jxl" => Some(DisallowedInput::Jxl),
      "bmp" => Some(DisallowedInput::Bmp),
      "tiff" => Some(DisallowedInput::Tiff),
      "pdf" => Some(DisallowedInput::Pdf),
      "psd" => Some(DisallowedInput::Psd),
      "video" => Some(DisallowedInput::Video),
      other => {
        tracing::warn!("unknown INPUT_DISALLOW_LIST token: {:?}, ignoring", other);
        None
      }
    })
    .collect()
}

fn parse_output_disallow(s: &str) -> HashSet<DisallowedOutput> {
  s.split(',')
    .map(|t| t.trim())
    .filter(|t| !t.is_empty())
    .filter_map(|t| match t {
      "jpeg" => Some(DisallowedOutput::Jpeg),
      "png" => Some(DisallowedOutput::Png),
      "gif" => Some(DisallowedOutput::Gif),
      "webp" => Some(DisallowedOutput::Webp),
      "avif" => Some(DisallowedOutput::Avif),
      "jxl" => Some(DisallowedOutput::Jxl),
      "bmp" => Some(DisallowedOutput::Bmp),
      "tiff" => Some(DisallowedOutput::Tiff),
      "ico" => Some(DisallowedOutput::Ico),
      other => {
        tracing::warn!("unknown OUTPUT_DISALLOW_LIST token: {:?}, ignoring", other);
        None
      }
    })
    .collect()
}

fn parse_transform_disallow(s: &str) -> HashSet<DisallowedTransform> {
  s.split(',')
    .map(|t| t.trim())
    .filter(|t| !t.is_empty())
    .filter_map(|t| match t {
      "resize" => Some(DisallowedTransform::Resize),
      "rotate" => Some(DisallowedTransform::Rotate),
      "flip" => Some(DisallowedTransform::Flip),
      "grayscale" => Some(DisallowedTransform::Grayscale),
      "brightness" => Some(DisallowedTransform::Brightness),
      "contrast" => Some(DisallowedTransform::Contrast),
      "blur" => Some(DisallowedTransform::Blur),
      "watermark" => Some(DisallowedTransform::Watermark),
      "gif_anim" => Some(DisallowedTransform::GifAnim),
      other => {
        tracing::warn!(
          "unknown TRANSFORM_DISALLOW_LIST token: {:?}, ignoring",
          other
        );
        None
      }
    })
    .collect()
}

fn parse_url_aliases(s: &str) -> Option<HashMap<String, String>> {
  let map: HashMap<String, String> = s
    .split(',')
    .map(|token| token.trim())
    .filter(|token| !token.is_empty())
    .filter_map(|token| {
      let (name, base) = match token.split_once('=') {
        Some(pair) => pair,
        None => {
          tracing::warn!("URL_ALIASES: skipping invalid entry (no '='): {:?}", token);
          return None;
        }
      };
      let name = name.trim();
      let base = base.trim();
      if name.is_empty() {
        tracing::warn!("URL_ALIASES: skipping entry with empty name");
        return None;
      }
      if base.is_empty() {
        tracing::warn!("URL_ALIASES: skipping entry {:?} with empty base URL", name);
        return None;
      }
      if !base.starts_with("http://") && !base.starts_with("https://") {
        tracing::warn!(
          "URL_ALIASES: skipping entry {:?} - base URL must be http:// or https://, got {:?}",
          name, base
        );
        return None;
      }
      if name == "s3" {
        tracing::warn!(
          "URL_ALIASES: scheme 's3' is shadowed by the built-in S3 source and will never be reachable"
        );
      }
      if name == "local" {
        tracing::warn!(
          "URL_ALIASES: scheme 'local' is shadowed by the built-in local source and will never be reachable"
        );
      }
      Some((name.to_string(), base.to_string()))
    })
    .collect();

  if map.is_empty() { None } else { Some(map) }
}

impl Configuration {
  pub fn new() -> Config {
    let env = env_var("APP_ENV")
      .parse::<Environment>()
      .expect("APP_ENV must be 'development' or 'production'");
    let app_port = env_var("PORT")
      .parse::<u16>()
      .expect("PORT must be a valid u16");
    let listen_address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, app_port));

    let allowed_hosts = std::env::var("ALLOWED_HOSTS")
      .unwrap_or_default()
      .split(',')
      .map(|s| s.trim().to_string())
      .filter(|s| !s.is_empty())
      .collect();

    let max_concurrent_requests = env_var_u64("MAX_CONCURRENT_REQUESTS", 256) as usize;
    if max_concurrent_requests == 0 {
      panic!("MAX_CONCURRENT_REQUESTS must be > 0");
    }

    let cfg = Arc::new(Configuration {
      env,
      listen_address,
      app_port,
      hmac_key: env_var_opt("HMAC_KEY"),
      allowed_hosts,
      fetch_timeout_secs: env_var_u64("FETCH_TIMEOUT_SECS", 10),
      max_source_bytes: env_var_u64("MAX_SOURCE_BYTES", 20_971_520),
      cache_memory_max_mb: env_var_u64("CACHE_MEMORY_MAX_MB", 256),
      cache_memory_ttl_secs: env_var_u64("CACHE_MEMORY_TTL_SECS", 3600),
      cache_dir: std::env::var("CACHE_DIR").unwrap_or_else(|_| "/tmp/previewproxy".to_string()),
      cache_disk_ttl_secs: env_var_u64("CACHE_DISK_TTL_SECS", 86400),
      cache_disk_max_mb: env_var_opt("CACHE_DISK_MAX_MB").and_then(|v| v.parse().ok()),
      cache_cleanup_interval_secs: env_var_u64("CACHE_CLEANUP_INTERVAL_SECS", 600),
      s3_enabled: env_var_bool("S3_ENABLED"),
      s3_bucket: env_var_opt("S3_BUCKET"),
      s3_region: std::env::var("S3_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
      s3_access_key_id: env_var_opt("S3_ACCESS_KEY_ID"),
      s3_secret_access_key: env_var_opt("S3_SECRET_ACCESS_KEY"),
      s3_endpoint: env_var_opt("S3_ENDPOINT"),
      local_enabled: env_var_bool("LOCAL_ENABLED"),
      local_base_dir: env_var_opt("LOCAL_BASE_DIR"),
      ffmpeg_path: std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string()),
      ffprobe_path: {
        let explicit = std::env::var("FFPROBE_PATH").unwrap_or_default();
        if !explicit.is_empty() {
          explicit
        } else {
          let ffmpeg = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string());
          let path = std::path::Path::new(&ffmpeg);
          match path.parent() {
            Some(dir) if dir != std::path::Path::new("") => {
              dir.join("ffprobe").to_string_lossy().to_string()
            }
            _ => "ffprobe".to_string(),
          }
        }
      },
      cors_allow_origin: std::env::var("CORS_ALLOW_ORIGIN")
        .unwrap_or_else(|_| "*".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect(),
      cors_max_age_secs: env_var_u64("CORS_MAX_AGE_SECS", 600),
      max_concurrent_requests,
      input_disallow: parse_input_disallow(
        &std::env::var("INPUT_DISALLOW_LIST").unwrap_or_default(),
      ),
      output_disallow: parse_output_disallow(
        &std::env::var("OUTPUT_DISALLOW_LIST").unwrap_or_default(),
      ),
      transform_disallow: parse_transform_disallow(
        &std::env::var("TRANSFORM_DISALLOW_LIST").unwrap_or_default(),
      ),
      url_aliases: parse_url_aliases(
        &std::env::var("URL_ALIASES").unwrap_or_default(),
      ),
    });
    if cfg.hmac_key.is_none() {
      tracing::warn!("HMAC_KEY is not set - all requests are unauthenticated");
    }
    if cfg.s3_enabled {
      if cfg.s3_bucket.is_none() {
        panic!("S3_ENABLED=true but S3_BUCKET is not set");
      }
      if cfg.s3_access_key_id.is_none() {
        panic!("S3_ENABLED=true but S3_ACCESS_KEY_ID is not set");
      }
      if cfg.s3_secret_access_key.is_none() {
        panic!("S3_ENABLED=true but S3_SECRET_ACCESS_KEY is not set");
      }
    }
    if cfg.local_enabled && cfg.local_base_dir.is_none() {
      panic!("LOCAL_ENABLED=true but LOCAL_BASE_DIR is not set");
    }
    if cfg.allowed_hosts.is_empty() {
      tracing::warn!("ALLOWED_HOSTS is not set - proxying requests to any host is allowed");
    }
    if cfg.env == Environment::Production {
      if cfg.hmac_key.is_none() {
        tracing::error!("Running in production without HMAC_KEY - this is a security risk");
      }
      if cfg.allowed_hosts.is_empty() {
        tracing::error!("Running in production without ALLOWED_HOSTS - this is a security risk");
      }
    }
    info!(?cfg, "Configuration loaded");
    cfg
  }
}

impl std::fmt::Debug for Configuration {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Configuration")
      .field("env", &self.env)
      .field("listen_address", &self.listen_address)
      .field("app_port", &self.app_port)
      .field("hmac_key", &self.hmac_key.as_ref().map(|_| "[redacted]"))
      .field("allowed_hosts", &self.allowed_hosts)
      .field("fetch_timeout_secs", &self.fetch_timeout_secs)
      .field("max_source_bytes", &self.max_source_bytes)
      .field("cache_memory_max_mb", &self.cache_memory_max_mb)
      .field("cache_memory_ttl_secs", &self.cache_memory_ttl_secs)
      .field("cache_dir", &self.cache_dir)
      .field("cache_disk_ttl_secs", &self.cache_disk_ttl_secs)
      .field("cache_disk_max_mb", &self.cache_disk_max_mb)
      .field(
        "cache_cleanup_interval_secs",
        &self.cache_cleanup_interval_secs,
      )
      .field("s3_enabled", &self.s3_enabled)
      .field("s3_bucket", &self.s3_bucket)
      .field("s3_region", &self.s3_region)
      .field(
        "s3_access_key_id",
        &self.s3_access_key_id.as_ref().map(|_| "[redacted]"),
      )
      .field(
        "s3_secret_access_key",
        &self.s3_secret_access_key.as_ref().map(|_| "[redacted]"),
      )
      .field("s3_endpoint", &self.s3_endpoint)
      .field("local_enabled", &self.local_enabled)
      .field("local_base_dir", &self.local_base_dir)
      .field("ffmpeg_path", &self.ffmpeg_path)
      .field("ffprobe_path", &self.ffprobe_path)
      .field("cors_allow_origin", &self.cors_allow_origin)
      .field("cors_max_age_secs", &self.cors_max_age_secs)
      .field("max_concurrent_requests", &self.max_concurrent_requests)
      .field("input_disallow", &{
        let mut v: Vec<_> = self
          .input_disallow
          .iter()
          .map(|x| format!("{x:?}"))
          .collect();
        v.sort();
        v
      })
      .field("output_disallow", &{
        let mut v: Vec<_> = self
          .output_disallow
          .iter()
          .map(|x| format!("{x:?}"))
          .collect();
        v.sort();
        v
      })
      .field("transform_disallow", &{
        let mut v: Vec<_> = self
          .transform_disallow
          .iter()
          .map(|x| format!("{x:?}"))
          .collect();
        v.sort();
        v
      })
      .field("url_aliases", &self.url_aliases.as_ref().map(|m| {
        let mut keys: Vec<_> = m.keys().cloned().collect();
        keys.sort();
        keys
      }))
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Mutex;
  // Serialize all tests that mutate env vars to prevent cross-test pollution.
  static ENV_LOCK: Mutex<()> = Mutex::new(());

  #[test]
  fn test_config_new() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::remove_var("MAX_CONCURRENT_REQUESTS");
    let cfg = super::Configuration::new();
    assert_eq!(cfg.app_port, 8080);
    assert_eq!(cfg.fetch_timeout_secs, 10);
    assert_eq!(cfg.cache_memory_max_mb, 256);
    assert!(cfg.hmac_key.is_none());
    assert!(cfg.allowed_hosts.is_empty());
    assert!(!cfg.s3_enabled);
    assert!(cfg.s3_bucket.is_none());
    assert_eq!(cfg.s3_region, "us-east-1");
    assert!(cfg.s3_access_key_id.is_none());
    assert!(cfg.s3_secret_access_key.is_none());
    assert!(cfg.s3_endpoint.is_none());
    assert!(!cfg.local_enabled);
    assert!(cfg.local_base_dir.is_none());
    assert_eq!(cfg.max_concurrent_requests, 256);
  }

  #[test]
  fn test_max_concurrent_requests_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::remove_var("MAX_CONCURRENT_REQUESTS");
    let cfg = super::Configuration::new();
    assert_eq!(cfg.max_concurrent_requests, 256);
  }

  #[test]
  fn test_max_concurrent_requests_from_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("MAX_CONCURRENT_REQUESTS", "64");
    let cfg = super::Configuration::new();
    std::env::remove_var("MAX_CONCURRENT_REQUESTS");
    assert_eq!(cfg.max_concurrent_requests, 64);
  }

  #[test]
  fn test_max_concurrent_requests_zero_panics() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("MAX_CONCURRENT_REQUESTS", "0");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      super::Configuration::new();
    }));
    std::env::remove_var("MAX_CONCURRENT_REQUESTS");
    assert!(result.is_err(), "Expected Configuration::new() to panic");
  }

  #[test]
  fn test_disallow_defaults_when_unset() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::remove_var("INPUT_DISALLOW_LIST");
    std::env::remove_var("OUTPUT_DISALLOW_LIST");
    std::env::remove_var("TRANSFORM_DISALLOW_LIST");
    let cfg = super::Configuration::new();
    assert!(cfg.input_disallow.is_empty());
    assert!(cfg.output_disallow.is_empty());
    assert!(cfg.transform_disallow.is_empty());
  }

  #[test]
  fn test_disallow_empty_string_means_all_allowed() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("INPUT_DISALLOW_LIST", "");
    std::env::set_var("OUTPUT_DISALLOW_LIST", "");
    std::env::set_var("TRANSFORM_DISALLOW_LIST", "");
    let cfg = super::Configuration::new();
    std::env::remove_var("INPUT_DISALLOW_LIST");
    std::env::remove_var("OUTPUT_DISALLOW_LIST");
    std::env::remove_var("TRANSFORM_DISALLOW_LIST");
    assert!(cfg.input_disallow.is_empty());
    assert!(cfg.output_disallow.is_empty());
    assert!(cfg.transform_disallow.is_empty());
  }

  #[test]
  fn test_disallow_unknown_token_does_not_panic() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("TRANSFORM_DISALLOW_LIST", "blur,not_a_real_op");
    let cfg = super::Configuration::new();
    std::env::remove_var("TRANSFORM_DISALLOW_LIST");
    assert!(cfg
      .transform_disallow
      .contains(&super::DisallowedTransform::Blur));
    assert_eq!(cfg.transform_disallow.len(), 1);
  }

  #[test]
  fn test_url_aliases_unset_is_none() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::remove_var("URL_ALIASES");
    let cfg = super::Configuration::new();
    assert!(cfg.url_aliases.is_none());
  }

  #[test]
  fn test_url_aliases_empty_is_none() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("URL_ALIASES", "");
    let cfg = super::Configuration::new();
    std::env::remove_var("URL_ALIASES");
    assert!(cfg.url_aliases.is_none());
  }

  #[test]
  fn test_url_aliases_valid_parses() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("URL_ALIASES", "mycdn=https://img.example.com,cdn2=https://other.com");
    let cfg = super::Configuration::new();
    std::env::remove_var("URL_ALIASES");
    let map = cfg.url_aliases.clone().unwrap();
    assert_eq!(map.get("mycdn").map(|s| s.as_str()), Some("https://img.example.com"));
    assert_eq!(map.get("cdn2").map(|s| s.as_str()), Some("https://other.com"));
  }

  #[test]
  fn test_url_aliases_skips_empty_name() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("URL_ALIASES", "=https://img.example.com,valid=https://ok.com");
    let cfg = super::Configuration::new();
    std::env::remove_var("URL_ALIASES");
    let map = cfg.url_aliases.clone().unwrap();
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("valid"));
  }

  #[test]
  fn test_url_aliases_skips_non_http_base() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("URL_ALIASES", "bad=file:///etc/passwd,ok=https://img.example.com");
    let cfg = super::Configuration::new();
    std::env::remove_var("URL_ALIASES");
    let map = cfg.url_aliases.clone().unwrap();
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("ok"));
  }

  #[test]
  fn test_url_aliases_all_invalid_is_none() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("URL_ALIASES", "bad=file:///etc/passwd");
    let cfg = super::Configuration::new();
    std::env::remove_var("URL_ALIASES");
    assert!(cfg.url_aliases.is_none());
  }

  #[test]
  fn test_input_disallow_parses_all_tokens() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var(
      "INPUT_DISALLOW_LIST",
      "jpeg,png,gif,webp,avif,jxl,bmp,tiff,pdf,psd,video",
    );
    let cfg = super::Configuration::new();
    std::env::remove_var("INPUT_DISALLOW_LIST");
    assert_eq!(cfg.input_disallow.len(), 11);
  }
}
