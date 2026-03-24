use crate::common::errors::ProxyError;
use crate::modules::proxy::params::{GifAnimRange, TransformParams};
use crate::modules::transform::ops;
use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::{AnimationDecoder, DynamicImage, Frame};
use std::io::Cursor;

pub fn run(
  src_bytes: &[u8],
  range: &GifAnimRange,
  all_frames: bool,
  params: &TransformParams,
  wm_img: Option<DynamicImage>,
) -> Result<Vec<u8>, ProxyError> {
  // Decode all frames
  let decoder = GifDecoder::new(Cursor::new(src_bytes))
    .map_err(|e| ProxyError::InternalError(e.to_string()))?;
  let frames: Vec<Frame> = decoder
    .into_frames()
    .collect_frames()
    .map_err(|e| ProxyError::InternalError(e.to_string()))?;
  let total = frames.len();

  // Resolve in-range bounds as (start, end) inclusive - all variants produce contiguous ranges
  let (range_start, range_end) = match range {
    GifAnimRange::All => (0, total.saturating_sub(1)),
    GifAnimRange::From(x) => {
      if *x >= total {
        return Err(ProxyError::InvalidParams(
          "gif_anim: start frame out of range".to_string(),
        ));
      }
      (*x, total - 1)
    }
    GifAnimRange::Range(x, y) => {
      if x > y {
        return Err(ProxyError::InvalidParams(
          "gif_anim: X must be <= Y".to_string(),
        ));
      }
      if *x >= total {
        return Err(ProxyError::InvalidParams(
          "gif_anim: start frame out of range".to_string(),
        ));
      }
      let y_clamped = (*y).min(total - 1);
      (*x, y_clamped)
    }
    GifAnimRange::Last(n) => {
      if *n == 0 {
        return Err(ProxyError::InvalidParams(
          "gif_anim: frame count must be >= 1".to_string(),
        ));
      }
      let n_clamped = (*n).min(total);
      (total - n_clamped, total - 1)
    }
  };

  // Build output frames
  let fit = params.fit.as_deref().unwrap_or("contain");
  let bright = params.bright.unwrap_or(0);
  let contrast = params.contrast.unwrap_or(0);
  let has_geometric =
    params.w.is_some() || params.h.is_some() || params.rotate.is_some() || params.flip.is_some();

  let mut out_frames: Vec<Frame> = Vec::new();
  for (idx, frame) in frames.into_iter().enumerate() {
    let delay = frame.delay();
    let left = frame.left();
    let top = frame.top();

    if idx >= range_start && idx <= range_end {
      let mut img = DynamicImage::ImageRgba8(frame.into_buffer());

      // Geometric transforms
      img = ops::resize::resize(img, params.w, params.h, fit)?;
      img = ops::rotate::rotate(img, params.rotate)?;
      img = ops::rotate::flip(img, params.flip.as_deref())?;
      // Style transforms
      if bright != 0 || contrast != 0 {
        img = ops::color::brightness_contrast(img, bright, contrast)?;
      }
      if params.grayscale == Some(true) {
        img = ops::color::to_grayscale(img)?;
      }
      if let Some(sigma) = params.blur {
        img = ops::blur::gaussian_blur(img, sigma)?;
      }
      if let Some(ref wm) = wm_img {
        img = ops::watermark::apply_watermark_sync(img, wm.clone())?;
      }

      let (out_left, out_top) = if has_geometric { (0, 0) } else { (left, top) };
      out_frames.push(Frame::from_parts(
        img.into_rgba8(),
        out_left,
        out_top,
        delay,
      ));
    } else if all_frames {
      // Out-of-range passthrough: apply geometric transforms only to keep dimensions consistent
      if has_geometric {
        let mut img = DynamicImage::ImageRgba8(frame.into_buffer());
        img = ops::resize::resize(img, params.w, params.h, fit)?;
        img = ops::rotate::rotate(img, params.rotate)?;
        img = ops::rotate::flip(img, params.flip.as_deref())?;
        out_frames.push(Frame::from_parts(img.into_rgba8(), 0, 0, delay));
      } else {
        out_frames.push(Frame::from_parts(frame.into_buffer(), left, top, delay));
      }
    }
  }

  // Encode
  let mut buf = Cursor::new(Vec::new());
  {
    let mut encoder = GifEncoder::new(&mut buf);
    encoder
      .set_repeat(Repeat::Infinite)
      .map_err(|e| ProxyError::InternalError(e.to_string()))?;
    for frame in out_frames {
      encoder
        .encode_frame(frame)
        .map_err(|e| ProxyError::InternalError(e.to_string()))?;
    }
  }

  Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::modules::proxy::params::TransformParams;
  use crate::modules::transform::test_helpers::tiny_gif_anim_bytes;

  fn frame_count(bytes: &[u8]) -> usize {
    let decoder = GifDecoder::new(Cursor::new(bytes)).unwrap();
    decoder.into_frames().collect_frames().unwrap().len()
  }

  #[test]
  fn test_all_frames_returned_and_transformed() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams {
      w: Some(2),
      h: Some(2),
      ..Default::default()
    };
    let out = run(&bytes, &GifAnimRange::All, false, &params, None).unwrap();
    assert_eq!(frame_count(&out), 3);
  }

  #[test]
  fn test_from_index_skips_earlier_frames() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    let out = run(&bytes, &GifAnimRange::From(1), false, &params, None).unwrap();
    assert_eq!(frame_count(&out), 2);
  }

  #[test]
  fn test_range_returns_correct_count() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    let out = run(&bytes, &GifAnimRange::Range(0, 1), false, &params, None).unwrap();
    assert_eq!(frame_count(&out), 2);
  }

  #[test]
  fn test_last_n_frames() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    let out = run(&bytes, &GifAnimRange::Last(1), false, &params, None).unwrap();
    assert_eq!(frame_count(&out), 1);
  }

  #[test]
  fn test_gif_af_passthrough_frames_resized() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams {
      w: Some(2),
      h: Some(2),
      ..Default::default()
    };
    // From(1) + gif_af: 3 frames total; all resized; frame 0 gets geometric only; frames 1-2 get all transforms
    let out = run(&bytes, &GifAnimRange::From(1), true, &params, None).unwrap();
    assert_eq!(frame_count(&out), 3);
    // Verify all frames are resized (2x2) - geometric transforms applied to all
    let decoder = GifDecoder::new(Cursor::new(&out)).unwrap();
    let frames = decoder.into_frames().collect_frames().unwrap();
    for frame in &frames {
      assert_eq!(frame.buffer().width(), 2);
      assert_eq!(frame.buffer().height(), 2);
    }
  }

  #[test]
  fn test_gif_af_passthrough_frames_not_style_transformed() {
    // Frame 0 is red (255,0,0,255) in the fixture.
    // With grayscale applied only to frames 1-2 (From(1) range),
    // frame 0 must remain non-grayscale (R != G or R != B).
    // If style transforms were wrongly applied to frame 0, it would become gray (R==G==B).
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams {
      grayscale: Some(true),
      ..Default::default()
    };
    let out = run(&bytes, &GifAnimRange::From(1), true, &params, None).unwrap();
    let decoder = GifDecoder::new(Cursor::new(&out)).unwrap();
    let frames = decoder.into_frames().collect_frames().unwrap();
    assert_eq!(frames.len(), 3);
    // Frame 0 (passthrough): pixel must NOT be grayscale - R channel should differ from G/B
    let px = frames[0].buffer().get_pixel(0, 0);
    assert_ne!(
      px[0], px[1],
      "frame 0 should not be grayscale: got r={} g={}",
      px[0], px[1]
    );
    // Frames 1-2 (in-range): pixel should be grayscale (R == G == B)
    let px1 = frames[1].buffer().get_pixel(0, 0);
    assert_eq!(px1[0], px1[1], "frame 1 should be grayscale");
    assert_eq!(px1[0], px1[2], "frame 1 should be grayscale");
  }

  #[test]
  fn test_inverted_range_returns_error() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    let result = run(&bytes, &GifAnimRange::Range(5, 2), false, &params, None);
    assert!(result.is_err());
  }

  #[test]
  fn test_from_out_of_range_returns_error() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    let result = run(&bytes, &GifAnimRange::From(99), false, &params, None);
    assert!(result.is_err());
  }

  #[test]
  fn test_last_zero_returns_error() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    let result = run(&bytes, &GifAnimRange::Last(0), false, &params, None);
    assert!(result.is_err());
  }

  #[test]
  fn test_last_n_larger_than_total_clamped() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    // Last(99) on 3-frame gif => all 3 frames
    let out = run(&bytes, &GifAnimRange::Last(99), false, &params, None).unwrap();
    assert_eq!(frame_count(&out), 3);
  }

  #[test]
  fn test_range_start_out_of_range_returns_error() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    // Range(99, 200) - start beyond total frame count
    let result = run(&bytes, &GifAnimRange::Range(99, 200), false, &params, None);
    assert!(result.is_err());
  }

  #[test]
  fn test_range_y_clamped_to_last_frame() {
    let bytes = tiny_gif_anim_bytes();
    let params = TransformParams::default();
    // Range(1, 99) on 3-frame gif => frames 1 and 2
    let out = run(&bytes, &GifAnimRange::Range(1, 99), false, &params, None).unwrap();
    assert_eq!(frame_count(&out), 2);
  }
}
