use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Signs `message` with HMAC-SHA256 and returns a URL-safe base64 (no-pad) digest.
/// The canonical message format is `"<params>:<image_url>"`.
pub fn sign(key: &str, message: &str) -> String {
  let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC can take key of any size");
  mac.update(message.as_bytes());
  URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Verifies `sig` against the expected HMAC of `message` using constant-time comparison.
pub fn verify(key: &str, message: &str, sig: &str) -> bool {
  let expected = sign(key, message);
  // Constant-time comparison to prevent timing attacks
  let a = expected.as_bytes();
  let b = sig.as_bytes();
  if a.len() != b.len() {
    return false;
  }
  a.iter()
    .zip(b.iter())
    .fold(0u8, |acc, (x, y)| acc | (x ^ y))
    == 0
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn test_sign_and_verify() {
    let sig = sign("secret", "blur=5&format=webp:https://example.com/photo.jpg");
    assert!(verify(
      "secret",
      "blur=5&format=webp:https://example.com/photo.jpg",
      &sig
    ));
  }
  #[test]
  fn test_wrong_key_fails() {
    let sig = sign("key1", "msg");
    assert!(!verify("key2", "msg", &sig));
  }
  #[test]
  fn test_tampered_sig_fails() {
    let sig = sign("key", "msg");
    let tampered = format!("{sig}x");
    assert!(!verify("key", "msg", &tampered));
  }
  #[test]
  fn test_empty_message() {
    let sig = sign("key", "");
    assert!(verify("key", "", &sig));
    assert!(!verify("key", "notempty", &sig));
  }
}
