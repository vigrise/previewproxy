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

/// Extract a single frame at t_secs from video bytes using the ffmpeg CLI.
///
/// Spawns `<ffmpeg_bin> -ss <t_secs> -i <tmpfile> -vframes 1 -f image2 -vcodec png pipe:1`.
/// Uses a fast keyframe seek (-ss before -i); the extracted frame is the nearest
/// prior keyframe, not frame-accurate. Acceptable for preview thumbnail generation.
pub async fn extract_frame(
  bytes: &[u8],
  t_secs: f32,
  ffmpeg_bin: &str,
) -> Result<DynamicImage, ProxyError> {
  // Write bytes to a temp file. Hold the handle alive until ffmpeg exits.
  let tmp = tempfile::NamedTempFile::new().map_err(|e| ProxyError::InternalError(e.to_string()))?;
  std::fs::write(tmp.path(), bytes).map_err(|e| ProxyError::InternalError(e.to_string()))?;

  let output = tokio::process::Command::new(ffmpeg_bin)
    .args([
      "-ss",
      &t_secs.to_string(),
      "-i",
      tmp.path().to_str().unwrap_or(""),
      "-vframes",
      "1",
      "-f",
      "image2",
      "-vcodec",
      "png",
      "-loglevel",
      "error",
      "pipe:1",
    ])
    .output()
    .await
    .map_err(|e| {
      if e.kind() == std::io::ErrorKind::NotFound {
        tracing::warn!("ffmpeg binary not found at {ffmpeg_bin:?}: {e}");
      } else {
        tracing::warn!("ffmpeg spawn error: {e}");
      }
      ProxyError::VideoDecodeError
    })?;

  // Drop tempfile after subprocess exits (explicit for clarity).
  drop(tmp);

  if !output.status.success() || output.stdout.is_empty() {
    let stderr_snippet = String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)]);
    tracing::warn!(
      "ffmpeg failed (status={:?}): {}",
      output.status.code(),
      stderr_snippet
    );
    return Err(ProxyError::VideoDecodeError);
  }

  let img = image::ImageReader::new(Cursor::new(&output.stdout))
    .with_guessed_format()
    .map_err(|e| ProxyError::InternalError(e.to_string()))?
    .decode()
    .map_err(|_| ProxyError::VideoDecodeError)?;

  Ok(img)
}

/// Probe video duration in seconds using the ffprobe CLI.
///
/// Runs: `ffprobe -v error -show_entries format=duration -of csv=p=0 <tmpfile>`
/// Returns `ProxyError::VideoDecodeError` if ffprobe fails or output cannot be parsed.
pub async fn probe_duration(bytes: &[u8], ffprobe_bin: &str) -> Result<f32, ProxyError> {
  let tmp = tempfile::NamedTempFile::new().map_err(|e| ProxyError::InternalError(e.to_string()))?;
  std::fs::write(tmp.path(), bytes).map_err(|e| ProxyError::InternalError(e.to_string()))?;

  let output = tokio::process::Command::new(ffprobe_bin)
    .args([
      "-v",
      "error",
      "-show_entries",
      "format=duration",
      "-of",
      "csv=p=0",
      tmp.path().to_str().unwrap_or(""),
    ])
    .output()
    .await
    .map_err(|e| {
      if e.kind() == std::io::ErrorKind::NotFound {
        tracing::warn!("ffprobe binary not found at {ffprobe_bin:?}: {e}");
      } else {
        tracing::warn!("ffprobe spawn error: {e}");
      }
      ProxyError::VideoDecodeError
    })?;

  drop(tmp);

  if !output.status.success() || output.stdout.is_empty() {
    let snippet = String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(512)]);
    tracing::warn!(
      "ffprobe failed (status={:?}): {}",
      output.status.code(),
      snippet
    );
    return Err(ProxyError::VideoDecodeError);
  }

  let stdout = String::from_utf8_lossy(&output.stdout);
  stdout.trim().parse::<f32>().map_err(|_| {
    tracing::warn!("ffprobe returned unparseable duration: {stdout:?}");
    ProxyError::VideoDecodeError
  })
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

  #[tokio::test(flavor = "current_thread")]
  async fn test_extract_frame_ffmpeg_not_found() {
    let original_path = std::env::var("PATH").unwrap_or_default();
    // SAFETY: test-only, single-threaded context for env mutation.
    unsafe { std::env::set_var("PATH", "") };
    let bytes = vec![0u8; 32];
    let result = extract_frame(&bytes, 0.0, "ffmpeg").await;
    unsafe { std::env::set_var("PATH", original_path) };
    assert!(
      matches!(
        result,
        Err(crate::common::errors::ProxyError::VideoDecodeError)
      ),
      "expected VideoDecodeError when ffmpeg not in PATH"
    );
  }

  #[tokio::test]
  async fn test_extract_frame_invalid_video_returns_error() {
    let bytes = b"this is not a video file at all".to_vec();
    let result = extract_frame(&bytes, 0.0, "ffmpeg").await;
    assert!(
      matches!(
        result,
        Err(crate::common::errors::ProxyError::VideoDecodeError)
      ),
      "expected VideoDecodeError for non-video input"
    );
  }

  #[tokio::test]
  async fn test_extract_frame_empty_stdout_returns_error() {
    // Write valid tempfile bytes but pass /dev/null as ffmpeg input.
    // ffmpeg will exit 0 but produce no video output → empty stdout.
    // We simulate this by writing a valid PNG (not a video) and checking
    // that even if ffmpeg exits non-zero or produces empty stdout we get VideoDecodeError.
    // The easiest way: pass an empty byte slice - ffmpeg will fail producing empty stdout.
    let result = extract_frame(&[], 0.0, "ffmpeg").await;
    assert!(
      matches!(
        result,
        Err(crate::common::errors::ProxyError::VideoDecodeError)
      ),
      "expected VideoDecodeError for empty input"
    );
  }

  #[tokio::test]
  #[ignore = "requires ffmpeg binary in PATH and tests/fixtures/minimal.mp4"]
  async fn test_extract_frame() {
    let bytes = std::fs::read("tests/fixtures/minimal.mp4").unwrap();
    let img = extract_frame(&bytes, 0.0, "ffmpeg").await.unwrap();
    assert!(img.width() > 0 && img.height() > 0);
  }

  #[tokio::test]
  async fn test_probe_duration_ffprobe_not_found() {
    let original_path = std::env::var("PATH").unwrap_or_default();
    unsafe { std::env::set_var("PATH", "") };
    let result = probe_duration(&[], "ffprobe").await;
    unsafe { std::env::set_var("PATH", original_path) };
    assert!(matches!(
      result,
      Err(crate::common::errors::ProxyError::VideoDecodeError)
    ));
  }

  #[tokio::test]
  async fn test_probe_duration_invalid_input() {
    let result = probe_duration(b"not a video", "ffprobe").await;
    assert!(matches!(
      result,
      Err(crate::common::errors::ProxyError::VideoDecodeError)
    ));
  }

  #[tokio::test]
  #[ignore = "requires ffprobe binary in PATH and tests/fixtures/minimal.mp4"]
  async fn test_probe_duration_real_video() {
    let bytes = std::fs::read("tests/fixtures/minimal.mp4").unwrap();
    let dur = probe_duration(&bytes, "ffprobe").await.unwrap();
    assert!(dur > 0.0);
  }
}
