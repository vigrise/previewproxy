use crate::common::errors::ProxyError;
use image::{imageops, DynamicImage};

pub fn apply_watermark_sync(
  base: DynamicImage,
  wm: DynamicImage,
) -> Result<DynamicImage, ProxyError> {
  let base_w = base.width();
  let base_h = base.height();

  // Resize watermark to 15% of base width
  let wm_target_w = ((base_w as f32) * 0.15).max(1.0) as u32;
  let wm_ratio = wm_target_w as f32 / wm.width() as f32;
  let wm_target_h = ((wm.height() as f32) * wm_ratio).max(1.0) as u32;
  let wm_resized = wm.resize(wm_target_w, wm_target_h, imageops::FilterType::Lanczos3);

  // Position: top-right with 10% margin
  let margin_x = (base_w as f32 * 0.10) as u32;
  let margin_y = (base_h as f32 * 0.10) as u32;
  let x = base_w
    .saturating_sub(wm_resized.width())
    .saturating_sub(margin_x);
  let y = margin_y;

  let mut base_rgba = base.to_rgba8();
  imageops::overlay(&mut base_rgba, &wm_resized.to_rgba8(), x as i64, y as i64);
  Ok(DynamicImage::ImageRgba8(base_rgba))
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
  fn test_watermark_does_not_change_base_dimensions() {
    let base = DynamicImage::new_rgba8(100, 100);
    let wm = load_tiny();
    let result = apply_watermark_sync(base, wm).unwrap();
    assert_eq!(result.width(), 100);
    assert_eq!(result.height(), 100);
  }
}
