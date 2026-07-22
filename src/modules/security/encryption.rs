use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use thiserror::Error;
use tracing::debug;

#[derive(Error, Debug, PartialEq)]
pub enum EncryptionError {
  #[error("invalid_key_length")]
  InvalidKeyLength,
  #[error("blob_too_short")]
  BlobTooShort,
  #[error("invalid_padding")]
  InvalidPadding,
  #[error("invalid_utf8")]
  InvalidUtf8,
  #[error("base64_decode_failed")]
  Base64,
}

fn validate_key(key: &[u8]) -> Result<(), EncryptionError> {
  match key.len() {
    16 | 24 | 32 => Ok(()),
    _ => Err(EncryptionError::InvalidKeyLength),
  }
}

fn derive_iv(key: &[u8], plaintext: &str) -> [u8; 16] {
  let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key size");
  mac.update(plaintext.as_bytes());
  let result = mac.finalize().into_bytes();
  let mut iv = [0u8; 16];
  iv.copy_from_slice(&result[..16]);
  iv
}

fn aes_encrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
  match key.len() {
    16 => cbc::Encryptor::<aes::Aes128>::new_from_slices(key, iv)
      .expect("key/iv lengths validated")
      .encrypt_padded_vec::<Pkcs7>(data),
    24 => cbc::Encryptor::<aes::Aes192>::new_from_slices(key, iv)
      .expect("key/iv lengths validated")
      .encrypt_padded_vec::<Pkcs7>(data),
    32 => cbc::Encryptor::<aes::Aes256>::new_from_slices(key, iv)
      .expect("key/iv lengths validated")
      .encrypt_padded_vec::<Pkcs7>(data),
    _ => unreachable!(),
  }
}

fn aes_decrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
  match key.len() {
    16 => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv)
      .expect("key/iv lengths validated")
      .decrypt_padded_vec::<Pkcs7>(data),
    24 => cbc::Decryptor::<aes::Aes192>::new_from_slices(key, iv)
      .expect("key/iv lengths validated")
      .decrypt_padded_vec::<Pkcs7>(data),
    32 => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
      .expect("key/iv lengths validated")
      .decrypt_padded_vec::<Pkcs7>(data),
    _ => unreachable!(),
  }
  .map_err(|_| EncryptionError::InvalidPadding)
}

/// Encrypts `plaintext` with AES-CBC using the given `key`.
///
/// The IV is derived deterministically as HMAC-SHA256(key, plaintext) truncated to 16 bytes.
/// This means the same URL always produces the same encrypted blob, which is intentional for
/// CDN cache-hit compatibility. The trade-off is that an observer can tell when two blobs
/// represent the same underlying URL. Acceptable for this use case; rotate keys if confidentiality
/// of URL equality is required.
#[tracing::instrument(skip(key, plaintext), fields(key_len = key.len()))]
pub fn encrypt(key: &[u8], plaintext: &str) -> Result<String, EncryptionError> {
  debug!("encrypt called");
  validate_key(key)?;
  let iv = derive_iv(key, plaintext);
  let ciphertext = aes_encrypt(key, &iv, plaintext.as_bytes());
  let mut combined = Vec::with_capacity(16 + ciphertext.len());
  combined.extend_from_slice(&iv);
  combined.extend_from_slice(&ciphertext);
  let result = Ok(URL_SAFE_NO_PAD.encode(combined));
  debug!(result = "ok", "encrypt completed");
  result
}

#[tracing::instrument(skip(key, blob), fields(key_len = key.len()))]
pub fn decrypt(key: &[u8], blob: &str) -> Result<String, EncryptionError> {
  debug!("decrypt called");
  validate_key(key)?;
  let raw = URL_SAFE_NO_PAD
    .decode(blob)
    .map_err(|_| EncryptionError::Base64)?;
  if raw.len() < 16 {
    debug!(result = "err", error = "blob_too_short", "decrypt failed");
    return Err(EncryptionError::BlobTooShort);
  }
  let (iv_bytes, ciphertext) = raw.split_at(16);
  let iv: [u8; 16] = iv_bytes
    .try_into()
    .expect("split_at(16) guarantees 16 bytes");
  let plaintext_bytes = aes_decrypt(key, &iv, ciphertext).map_err(|e| {
    debug!(result = "err", error = %e, "decrypt failed");
    e
  })?;
  let outcome = String::from_utf8(plaintext_bytes).map_err(|_| {
    debug!(result = "err", error = "invalid_utf8", "decrypt failed");
    EncryptionError::InvalidUtf8
  });
  if outcome.is_ok() {
    debug!(result = "ok", "decrypt completed");
  }
  outcome
}

#[cfg(test)]
mod tests {
  use super::*;

  const KEY_32: &[u8] = b"01234567890123456789012345678901"; // 32 bytes AES-256
  const KEY_16: &[u8] = b"0123456789012345"; // 16 bytes AES-128

  #[test]
  fn test_round_trip_aes256() {
    let blob = encrypt(KEY_32, "https://example.com/img.jpg").unwrap();
    let plain = decrypt(KEY_32, &blob).unwrap();
    assert_eq!(plain, "https://example.com/img.jpg");
  }

  #[test]
  fn test_round_trip_aes128() {
    let blob = encrypt(KEY_16, "https://cdn.example.com/photo.png").unwrap();
    let plain = decrypt(KEY_16, &blob).unwrap();
    assert_eq!(plain, "https://cdn.example.com/photo.png");
  }

  #[test]
  fn test_deterministic_iv() {
    let a = encrypt(KEY_32, "https://example.com/img.jpg").unwrap();
    let b = encrypt(KEY_32, "https://example.com/img.jpg").unwrap();
    assert_eq!(a, b, "same input must produce same blob for CDN caching");
  }

  #[test]
  fn test_different_urls_produce_different_blobs() {
    let a = encrypt(KEY_32, "https://example.com/a.jpg").unwrap();
    let b = encrypt(KEY_32, "https://example.com/b.jpg").unwrap();
    assert_ne!(a, b);
  }

  #[test]
  fn test_invalid_key_length() {
    assert_eq!(
      encrypt(b"short", "url").unwrap_err(),
      EncryptionError::InvalidKeyLength
    );
    assert_eq!(
      decrypt(b"short", "blob").unwrap_err(),
      EncryptionError::InvalidKeyLength
    );
  }

  #[test]
  fn test_blob_too_short() {
    let short = URL_SAFE_NO_PAD.encode([0u8; 15]);
    assert_eq!(
      decrypt(KEY_32, &short).unwrap_err(),
      EncryptionError::BlobTooShort
    );
  }

  #[test]
  fn test_bad_base64() {
    assert_eq!(
      decrypt(KEY_32, "not valid base64!!!").unwrap_err(),
      EncryptionError::Base64
    );
  }

  #[test]
  fn test_wrong_key_corrupts_padding() {
    let blob = encrypt(KEY_32, "https://example.com/img.jpg").unwrap();
    let wrong_key = b"99999999999999999999999999999999";
    let err = decrypt(wrong_key, &blob).unwrap_err();
    assert!(matches!(
      err,
      EncryptionError::InvalidPadding | EncryptionError::InvalidUtf8
    ));
  }
}
