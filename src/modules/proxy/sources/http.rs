use crate::common::errors::ProxyError;
use crate::modules::proxy::fetchable::Fetchable;
use crate::modules::security::allowlist::{is_private_ip, Allowlist};
use futures::StreamExt;
use reqwest::Client;
use std::{sync::Arc, time::Duration};

pub struct HttpFetcher {
  client: Client,
  max_bytes: u64,
  check_private_ips: bool,
}

#[async_trait::async_trait]
impl Fetchable for HttpFetcher {
  async fn fetch(&self, url: &str) -> Result<(Vec<u8>, Option<String>), ProxyError> {
    self.fetch_url(url).await
  }
}

impl HttpFetcher {
  pub fn with_private_ip_check(mut self, check: bool) -> Self {
    self.check_private_ips = check;
    self
  }

  pub fn new(timeout_secs: u64, max_bytes: u64, allowlist: Arc<Allowlist>) -> Self {
    let al = allowlist.clone();
    let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
      if attempt.previous().len() >= 5 {
        return attempt.error("too_many_redirects");
      }
      let url = attempt.url();
      let host = url.host_str().unwrap_or("");
      if !al.is_allowed(host) {
        return attempt.error("host_not_allowed");
      }
      if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&(host, 80u16)) {
        for addr in addrs {
          if is_private_ip(addr.ip()) {
            return attempt.error("host_not_allowed");
          }
        }
      }
      attempt.follow()
    });

    let client = Client::builder()
      .timeout(Duration::from_secs(timeout_secs))
      .redirect(redirect_policy)
      .user_agent("ViGrise-PreviewProxy/1.0")
      .build()
      .expect("Failed to build reqwest client");

    Self {
      client,
      max_bytes,
      check_private_ips: true,
    }
  }

  async fn fetch_url(&self, url: &str) -> Result<(Vec<u8>, Option<String>), ProxyError> {
    if self.check_private_ips {
      let parsed =
        url::Url::parse(url).map_err(|_| ProxyError::InvalidParams("invalid URL".to_string()))?;
      let host = parsed.host_str().unwrap_or("");
      if let Ok(addrs) = tokio::net::lookup_host(format!("{}:80", host)).await {
        for addr in addrs {
          if is_private_ip(addr.ip()) {
            return Err(ProxyError::HostNotAllowed);
          }
        }
      }
    }

    let resp = self.client.get(url).send().await.map_err(|e| {
      if e.is_timeout() {
        ProxyError::UpstreamTimeout
      } else if e.is_redirect() {
        ProxyError::TooManyRedirects
      } else {
        let msg = e.to_string();
        if msg.contains("too_many_redirects") {
          ProxyError::TooManyRedirects
        } else if msg.contains("host_not_allowed") {
          ProxyError::HostNotAllowed
        } else {
          ProxyError::InternalError(msg)
        }
      }
    })?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
      return Err(ProxyError::UpstreamNotFound);
    }
    if !resp.status().is_success() {
      return Err(ProxyError::InternalError(format!(
        "Upstream returned {}",
        resp.status()
      )));
    }

    let content_type = resp
      .headers()
      .get(reqwest::header::CONTENT_TYPE)
      .and_then(|v| v.to_str().ok())
      .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());

    let bytes = self.read_body_limited(resp).await?;
    Ok((bytes, content_type))
  }

  async fn read_body_limited(&self, resp: reqwest::Response) -> Result<Vec<u8>, ProxyError> {
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
      let chunk = chunk.map_err(|e| ProxyError::InternalError(e.to_string()))?;
      buf.extend_from_slice(&chunk);
      if buf.len() as u64 > self.max_bytes {
        return Err(ProxyError::SourceTooLarge);
      }
    }
    Ok(buf)
  }

  /// Returns the reqwest::Response after header checks, without reading the body.
  /// Caller is responsible for reading or streaming the body.
  pub async fn fetch_streaming(&self, url: &str) -> Result<reqwest::Response, ProxyError> {
    if self.check_private_ips {
      let parsed =
        url::Url::parse(url).map_err(|_| ProxyError::InvalidParams("invalid URL".to_string()))?;
      let host = parsed.host_str().unwrap_or("");
      if let Ok(addrs) = tokio::net::lookup_host(format!("{}:80", host)).await {
        for addr in addrs {
          if is_private_ip(addr.ip()) {
            return Err(ProxyError::HostNotAllowed);
          }
        }
      }
    }

    let resp = self.client.get(url).send().await.map_err(|e| {
      if e.is_timeout() {
        ProxyError::UpstreamTimeout
      } else if e.is_redirect() {
        ProxyError::TooManyRedirects
      } else {
        let msg = e.to_string();
        if msg.contains("too_many_redirects") {
          ProxyError::TooManyRedirects
        } else if msg.contains("host_not_allowed") {
          ProxyError::HostNotAllowed
        } else {
          ProxyError::InternalError(msg)
        }
      }
    })?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
      return Err(ProxyError::UpstreamNotFound);
    }
    if !resp.status().is_success() {
      return Err(ProxyError::InternalError(format!(
        "Upstream returned {}",
        resp.status()
      )));
    }

    Ok(resp)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::modules::security::allowlist::Allowlist;
  use std::sync::Arc;
  use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

  fn open_fetcher() -> HttpFetcher {
    HttpFetcher::new(10, 1_000_000, Arc::new(Allowlist::new(vec![]))).with_private_ip_check(false)
  }

  #[tokio::test]
  async fn test_fetch_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(vec![0u8; 10])
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;
    let fetcher = open_fetcher();
    let (bytes, ct) = fetcher.fetch(&server.uri()).await.unwrap();
    assert_eq!(bytes.len(), 10);
    assert_eq!(ct, Some("image/png".to_string()));
  }

  #[tokio::test]
  async fn test_fetch_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(ResponseTemplate::new(404))
      .mount(&server)
      .await;
    let result = open_fetcher().fetch(&server.uri()).await;
    assert!(matches!(result, Err(ProxyError::UpstreamNotFound)));
  }

  #[tokio::test]
  async fn test_fetch_too_large() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(vec![0u8; 200])
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;
    let fetcher =
      HttpFetcher::new(10, 50, Arc::new(Allowlist::new(vec![]))).with_private_ip_check(false);
    let result = fetcher.fetch(&server.uri()).await;
    assert!(matches!(result, Err(ProxyError::SourceTooLarge)));
  }

  #[tokio::test]
  async fn test_no_content_type_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 5]))
      .mount(&server)
      .await;
    let (bytes, ct) = open_fetcher().fetch(&server.uri()).await.unwrap();
    assert_eq!(bytes.len(), 5);
    assert!(ct.is_none());
  }

  #[tokio::test]
  async fn test_fetch_streaming_returns_200_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(
        ResponseTemplate::new(200)
          .set_body_bytes(vec![0u8; 20])
          .insert_header("content-type", "image/png"),
      )
      .mount(&server)
      .await;
    let fetcher = open_fetcher();
    let resp = fetcher.fetch_streaming(&server.uri()).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let ct = resp
      .headers()
      .get("content-type")
      .and_then(|v| v.to_str().ok())
      .unwrap_or("");
    assert!(ct.contains("image/png"));
  }

  #[tokio::test]
  async fn test_fetch_streaming_404_is_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
      .respond_with(ResponseTemplate::new(404))
      .mount(&server)
      .await;
    let result = open_fetcher().fetch_streaming(&server.uri()).await;
    assert!(matches!(result, Err(ProxyError::UpstreamNotFound)));
  }
}
