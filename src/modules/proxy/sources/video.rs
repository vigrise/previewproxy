use crate::common::errors::ProxyError;
use image::DynamicImage;
use std::io::Cursor;

/// Detect video container by magic bytes.
pub fn is_video_magic(bytes: &[u8]) -> bool {
  if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
    return true;
  }
  if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
    return true;
  }
  if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"AVI " {
    return true;
  }
  false
}

/// Serialise DynamicImage to PNG bytes for pipeline re-entry.
pub fn frame_to_png_bytes(img: DynamicImage) -> Result<Vec<u8>, ProxyError> {
  let mut buf = Cursor::new(Vec::new());
  img
    .write_to(&mut buf, image::ImageFormat::Png)
    .map_err(|e| ProxyError::InternalError(format!("frame_to_png: {e}")))?;
  Ok(buf.into_inner())
}

/// Extract a single frame at t_secs from video bytes.
pub fn extract_frame(bytes: &[u8], t_secs: f32) -> Result<DynamicImage, ProxyError> {
  use ffmpeg_next as ffmpeg;

  ffmpeg::init().map_err(|_| ProxyError::VideoDecodeError)?;

  let tmp = tempfile::NamedTempFile::new().map_err(|e| ProxyError::InternalError(e.to_string()))?;
  std::fs::write(tmp.path(), bytes).map_err(|e| ProxyError::InternalError(e.to_string()))?;

  let mut ictx = ffmpeg::format::input(&tmp.path()).map_err(|_| ProxyError::VideoDecodeError)?;

  let video_stream_index = ictx
    .streams()
    .best(ffmpeg::media::Type::Video)
    .ok_or(ProxyError::VideoDecodeError)?
    .index();

  let input = ictx
    .stream(video_stream_index)
    .ok_or(ProxyError::VideoDecodeError)?;
  let context_decoder = ffmpeg::codec::context::Context::from_parameters(input.parameters())
    .map_err(|_| ProxyError::VideoDecodeError)?;
  let mut decoder = context_decoder
    .decoder()
    .video()
    .map_err(|_| ProxyError::VideoDecodeError)?;

  let time_base = input.time_base();
  let denom = time_base.denominator() as f32;
  let numer = time_base.numerator() as f32;
  let ts = if numer > 0.0 {
    (t_secs * (denom / numer)) as i64
  } else {
    0
  };
  let _ = ictx.seek(ts, ..ts);

  let mut scaler = ffmpeg::software::scaling::context::Context::get(
    decoder.format(),
    decoder.width(),
    decoder.height(),
    ffmpeg::format::Pixel::RGB24,
    decoder.width(),
    decoder.height(),
    ffmpeg::software::scaling::flag::Flags::BILINEAR,
  )
  .map_err(|_| ProxyError::VideoDecodeError)?;

  for (stream, packet) in ictx.packets() {
    if stream.index() != video_stream_index {
      continue;
    }

    if decoder.send_packet(&packet).is_err() {
      continue;
    }

    let mut frame = ffmpeg::frame::Video::empty();
    if decoder.receive_frame(&mut frame).is_ok() {
      let mut rgb_frame = ffmpeg::frame::Video::empty();
      scaler
        .run(&frame, &mut rgb_frame)
        .map_err(|_| ProxyError::VideoDecodeError)?;

      let w = rgb_frame.width();
      let h = rgb_frame.height();
      let stride = rgb_frame.stride(0);
      let src = rgb_frame.data(0);
      let row_bytes = (w * 3) as usize;
      let mut contiguous = vec![0_u8; row_bytes * h as usize];
      for y in 0..h as usize {
        let src_off = y * stride;
        let dst_off = y * row_bytes;
        contiguous[dst_off..dst_off + row_bytes].copy_from_slice(&src[src_off..src_off + row_bytes]);
      }

      let img = image::RgbImage::from_raw(w, h, contiguous).ok_or(ProxyError::VideoDecodeError)?;
      return Ok(DynamicImage::ImageRgb8(img));
    }
  }

  Err(ProxyError::VideoDecodeError)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_is_video_magic_mp4() {
    let mut bytes = vec![0x00, 0x00, 0x00, 0x20];
    bytes.extend_from_slice(b"ftyp");
    bytes.extend(vec![0u8; 20]);
    assert!(is_video_magic(&bytes));
  }

  #[test]
  fn test_is_video_magic_mkv() {
    let bytes = [0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x00];
    assert!(is_video_magic(&bytes));
  }

  #[test]
  fn test_is_video_magic_png_is_false() {
    let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert!(!is_video_magic(&bytes));
  }

  #[test]
  #[ignore = "requires ffmpeg system library and MP4 fixture"]
  fn test_extract_frame() {
    let bytes = std::fs::read("tests/fixtures/minimal.mp4").unwrap();
    let img = extract_frame(&bytes, 0.0).unwrap();
    assert!(img.width() > 0 && img.height() > 0);
  }
}
