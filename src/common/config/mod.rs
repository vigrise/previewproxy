pub mod shutdown;
pub mod telemetry;

use std::{
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
  // CORS
  pub cors_allow_origin: Vec<String>,
  pub cors_max_age_secs: u64,
  // Concurrency
  pub max_concurrent_requests: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Environment {
  Development,
  Production,
}

impl std::str::FromStr for Environment {
  type Err = String;
  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s {
      "development" => Ok(Environment::Development),
      "production" => Ok(Environment::Production),
      _ => Err(format!("Invalid environment: {s}")),
    }
  }
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
      cors_allow_origin: std::env::var("CORS_ALLOW_ORIGIN")
        .unwrap_or_else(|_| "*".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect(),
      cors_max_age_secs: env_var_u64("CORS_MAX_AGE_SECS", 600),
      max_concurrent_requests,
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
      .field("cors_allow_origin", &self.cors_allow_origin)
      .field("cors_max_age_secs", &self.cors_max_age_secs)
      .field("max_concurrent_requests", &self.max_concurrent_requests)
      .finish()
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn test_config_new() {
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
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("MAX_CONCURRENT_REQUESTS", "");
    std::env::remove_var("MAX_CONCURRENT_REQUESTS");
    let cfg = super::Configuration::new();
    assert_eq!(cfg.max_concurrent_requests, 256);
  }

  #[test]
  fn test_max_concurrent_requests_from_env() {
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("MAX_CONCURRENT_REQUESTS", "64");
    let cfg = super::Configuration::new();
    assert_eq!(cfg.max_concurrent_requests, 64);
  }

  #[test]
  fn test_max_concurrent_requests_zero_panics() {
    std::env::set_var("PORT", "8080");
    std::env::set_var("APP_ENV", "development");
    std::env::set_var("MAX_CONCURRENT_REQUESTS", "0");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
      super::Configuration::new();
    }));
    assert!(result.is_err(), "Expected Configuration::new() to panic");
  }
}
