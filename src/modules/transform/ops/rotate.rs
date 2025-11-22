use crate::common::errors::ProxyError;
use image::DynamicImage;

pub fn rotate(img: DynamicImage, degrees: Option<u32>) -> Result<DynamicImage, ProxyError> {
  let result = match degrees {
    Some(90) => img.rotate90(),
    Some(180) => img.rotate180(),
    Some(270) => img.rotate270(),
    _ => img,
  };
  Ok(result)
}

pub fn flip(img: DynamicImage, direction: Option<&str>) -> Result<DynamicImage, ProxyError> {
  let result = match direction {
    Some("h") => img.fliph(),
    Some("v") => img.flipv(),
    _ => img,
  };
  Ok(result)
}

#[cfg(test)]
mod tests {
  use super::*;
  use image::DynamicImage;

  fn blank() -> DynamicImage {
    DynamicImage::new_rgb8(4, 2)
  }

  #[test]
  fn test_rotate_90_swaps_dims() {
    let img = blank(); // 4x2
    let result = rotate(img, Some(90)).unwrap();
    assert_eq!(result.width(), 2);
    assert_eq!(result.height(), 4);
  }

  #[test]
  fn test_rotate_none_unchanged() {
    let img = blank();
    let result = rotate(img, None).unwrap();
    assert_eq!(result.width(), 4);
  }

  #[test]
  fn test_flip_h() {
    let img = blank();
    let result = flip(img, Some("h")).unwrap();
    assert_eq!(result.width(), 4);
  }
}
