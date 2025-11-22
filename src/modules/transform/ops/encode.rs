use crate::common::errors::ProxyError;
use image::{DynamicImage, ImageFormat};
use std::io::Cursor;

pub fn encode(
  img: DynamicImage,
  format: &str,
  quality: u32,
) -> Result<(Vec<u8>, String), ProxyError> {
  if format == "avif" {
    return Err(ProxyError::AvifNotSupported);
  }

  let (fmt, content_type) = match format {
    "webp" => (ImageFormat::WebP, "image/webp"),
    "png" => (ImageFormat::Png, "image/png"),
    _ => (ImageFormat::Jpeg, "image/jpeg"),
  };

  let mut buf = Cursor::new(Vec::new());

  if fmt == ImageFormat::Jpeg {
    let encoder =
      image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100) as u8);
    img
      .write_with_encoder(encoder)
      .map_err(|e| ProxyError::InternalError(e.to_string()))?;
  } else {
    img
      .write_to(&mut buf, fmt)
      .map_err(|e| ProxyError::InternalError(e.to_string()))?;
  }

  Ok((buf.into_inner(), content_type.to_string()))
}

#[cfg(test)]
mod tests {
  use super::*;
  use image::DynamicImage;

  #[test]
  fn test_encode_png() {
    let img = DynamicImage::new_rgb8(2, 2);
    let (bytes, ct) = encode(img, "png", 85).unwrap();
    assert_eq!(ct, "image/png");
    assert_eq!(&bytes[1..4], b"PNG");
  }

  #[test]
  fn test_encode_jpeg() {
    let img = DynamicImage::new_rgb8(2, 2);
    let (bytes, ct) = encode(img, "jpeg", 85).unwrap();
    assert_eq!(ct, "image/jpeg");
    assert_eq!(&bytes[0..2], &[0xFF, 0xD8]);
  }

  #[test]
  fn test_encode_avif_returns_error() {
    let img = DynamicImage::new_rgb8(2, 2);
    let result = encode(img, "avif", 85);
    assert!(matches!(
      result,
      Err(crate::common::errors::ProxyError::AvifNotSupported)
    ));
  }
}
