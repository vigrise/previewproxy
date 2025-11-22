use crate::common::errors::ProxyError;
use image::{imageops::FilterType, DynamicImage};

pub fn resize(
  img: DynamicImage,
  w: Option<u32>,
  h: Option<u32>,
  fit: &str,
) -> Result<DynamicImage, ProxyError> {
  let orig_w = img.width();
  let orig_h = img.height();

  let (target_w, target_h) = match (w, h) {
    (None, None) | (Some(0), Some(0)) => return Ok(img),
    (Some(0), Some(h)) | (None, Some(h)) => {
      let ratio = h as f32 / orig_h as f32;
      ((orig_w as f32 * ratio) as u32, h)
    }
    (Some(w), Some(0)) | (Some(w), None) => {
      let ratio = w as f32 / orig_w as f32;
      (w, (orig_h as f32 * ratio) as u32)
    }
    (Some(w), Some(h)) => (w, h),
  };

  let result = match fit {
    "cover" | "crop" => img.resize_to_fill(target_w, target_h, FilterType::Lanczos3),
    _ => img.resize(target_w, target_h, FilterType::Lanczos3),
  };
  Ok(result)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::modules::transform::test_helpers::tiny_png_bytes;
  use image::ImageReader;
  use std::io::Cursor;

  fn load_tiny() -> image::DynamicImage {
    ImageReader::new(Cursor::new(tiny_png_bytes()))
      .with_guessed_format()
      .unwrap()
      .decode()
      .unwrap()
  }

  #[test]
  fn test_resize_both_dims() {
    let img = load_tiny();
    let result = resize(img, Some(10), Some(10), "contain").unwrap();
    assert_eq!(result.width(), 10);
    assert_eq!(result.height(), 10);
  }

  #[test]
  fn test_resize_no_dims_returns_unchanged() {
    let img = load_tiny();
    let (orig_w, orig_h) = (img.width(), img.height());
    let result = resize(img, None, None, "contain").unwrap();
    assert_eq!(result.width(), orig_w);
    assert_eq!(result.height(), orig_h);
  }
}
