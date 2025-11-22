use crate::common::errors::ProxyError;
use image::{imageops, DynamicImage};

pub fn to_grayscale(img: DynamicImage) -> Result<DynamicImage, ProxyError> {
  Ok(img.grayscale())
}

pub fn brightness_contrast(
  img: DynamicImage,
  bright: i32,
  contrast: i32,
) -> Result<DynamicImage, ProxyError> {
  let mut result = img;
  if bright != 0 {
    result = DynamicImage::ImageRgba8(imageops::brighten(&result, bright));
  }
  if contrast != 0 {
    result = DynamicImage::ImageRgba8(imageops::contrast(&result, contrast as f32));
  }
  Ok(result)
}

#[cfg(test)]
mod tests {
  use super::*;
  use image::DynamicImage;

  fn blank_rgb() -> DynamicImage {
    DynamicImage::new_rgb8(2, 2)
  }

  #[test]
  fn test_grayscale_returns_luma_image() {
    let img = blank_rgb();
    let result = to_grayscale(img).unwrap();
    assert!(matches!(
      result,
      DynamicImage::ImageLuma8(_) | DynamicImage::ImageLumaA8(_)
    ));
  }

  #[test]
  fn test_brightness_no_panic() {
    let img = blank_rgb();
    brightness_contrast(img, 10, 5).unwrap();
  }

  #[test]
  fn test_brightness_zero_unchanged() {
    let img = DynamicImage::new_rgb8(2, 2);
    let result = brightness_contrast(img, 0, 0).unwrap();
    assert_eq!(result.width(), 2);
  }
}
