use crate::common::errors::ProxyError;
use crate::modules::proxy::{dto::params::TransformParams, fetchable::Fetchable};
use crate::modules::transform::ops;
use image::{DynamicImage, ImageReader};
use std::{io::Cursor, sync::Arc};
use tokio::task::spawn_blocking;

/// Validate and resolve content-type. Returns the resolved MIME string or ProxyError.
pub fn resolve_content_type(header: Option<&str>, bytes: &[u8]) -> Result<String, ProxyError> {
  match header {
    Some(ct) if ct.starts_with("image/") => Ok(ct.to_string()),
    Some("application/pdf") => Ok("application/pdf".to_string()),
    Some(_) => Err(ProxyError::NotAnImage),
    None => {
      if let Some(kind) = infer::get(bytes)
        && (kind.mime_type().starts_with("image/") || kind.mime_type() == "application/pdf")
      {
        return Ok(kind.mime_type().to_string());
      }
      if bytes.starts_with(b"%PDF") {
        return Ok("application/pdf".to_string());
      }
      if bytes.starts_with(&[0xFF, 0x0A])
        || bytes.starts_with(&[0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20])
      {
        return Ok("image/jxl".to_string());
      }
      if bytes.starts_with(b"8BPS") {
        return Ok("image/vnd.adobe.photoshop".to_string());
      }
      Err(ProxyError::NotAnImage)
    }
  }
}

fn load_image(bytes: &[u8]) -> Result<DynamicImage, ProxyError> {
  ImageReader::new(Cursor::new(bytes))
    .with_guessed_format()
    .map_err(|e| ProxyError::InternalError(e.to_string()))?
    .decode()
    .map_err(|e| ProxyError::InternalError(e.to_string()))
}

/// Applies the full image transform pipeline to `src_bytes`.
///
/// Steps (in order): content-type resolution, disallow checks, PDF/HEIC/PSD
/// decode, watermark fetch, image decode into `DynamicImage`, then sequentially:
/// resize, rotate, flip, grayscale, brightness, contrast, blur, watermark
/// composite, and finally encode to the requested output format.
///
/// CPU-bound ops (decode, transform, encode) are run on a blocking thread via
/// `spawn_blocking` to avoid stalling the async runtime.
pub async fn run_pipeline(
  params: TransformParams,
  src_bytes: Vec<u8>,
  src_content_type: Option<String>,
  fetcher: Arc<dyn Fetchable>,
  output_disallow: &std::collections::HashSet<crate::common::config::DisallowedOutput>,
  transform_disallow: &std::collections::HashSet<crate::common::config::DisallowedTransform>,
) -> Result<(Vec<u8>, String), ProxyError> {
  // 1. Validate content-type
  let resolved_ct = resolve_content_type(src_content_type.as_deref(), &src_bytes)?;
  let is_document = resolved_ct == "application/pdf";

  // Output disallow / format validation
  if let Some(fmt) = params.format.as_deref() {
    use crate::common::config::DisallowedOutput;
    let token: Option<DisallowedOutput> = match fmt {
      "jpeg" => Some(DisallowedOutput::Jpeg),
      "png" => Some(DisallowedOutput::Png),
      "gif" => Some(DisallowedOutput::Gif),
      "webp" => Some(DisallowedOutput::Webp),
      "avif" => Some(DisallowedOutput::Avif),
      "jxl" => Some(DisallowedOutput::Jxl),
      "bmp" => Some(DisallowedOutput::Bmp),
      "tiff" => Some(DisallowedOutput::Tiff),
      "ico" => Some(DisallowedOutput::Ico),
      _ => return Err(ProxyError::UnsupportedFormat(fmt.to_string())),
    };
    if let Some(t) = token
      && output_disallow.contains(&t)
    {
      return Err(ProxyError::TransformDisabled(fmt.to_string()));
    }
  }

  // Transform disallow check
  {
    use crate::common::config::DisallowedTransform;
    if (params.gif_anim.is_some() || params.gif_af.is_some())
      && transform_disallow.contains(&DisallowedTransform::GifAnim)
    {
      return Err(ProxyError::TransformDisabled("gif_anim".to_string()));
    }
    if (params.w.is_some() || params.h.is_some())
      && transform_disallow.contains(&DisallowedTransform::Resize)
    {
      return Err(ProxyError::TransformDisabled("resize".to_string()));
    }
    if params.rotate.is_some() && transform_disallow.contains(&DisallowedTransform::Rotate) {
      return Err(ProxyError::TransformDisabled("rotate".to_string()));
    }
    if params.flip.is_some() && transform_disallow.contains(&DisallowedTransform::Flip) {
      return Err(ProxyError::TransformDisabled("flip".to_string()));
    }
    if params.bright.is_some() && transform_disallow.contains(&DisallowedTransform::Brightness) {
      return Err(ProxyError::TransformDisabled("brightness".to_string()));
    }
    if params.contrast.is_some() && transform_disallow.contains(&DisallowedTransform::Contrast) {
      return Err(ProxyError::TransformDisabled("contrast".to_string()));
    }
    if params.grayscale == Some(true)
      && transform_disallow.contains(&DisallowedTransform::Grayscale)
    {
      return Err(ProxyError::TransformDisabled("grayscale".to_string()));
    }
    if params.blur.is_some() && transform_disallow.contains(&DisallowedTransform::Blur) {
      return Err(ProxyError::TransformDisabled("blur".to_string()));
    }
    if params.wm.is_some() && transform_disallow.contains(&DisallowedTransform::Watermark) {
      return Err(ProxyError::TransformDisabled("watermark".to_string()));
    }
  }

  // 2. Passthrough: no transforms → return as-is with resolved content-type
  if !params.has_transforms() && !is_document {
    return Ok((src_bytes, resolved_ct));
  }

  // 3. Fetch watermark bytes if needed (async, before spawn_blocking)
  let wm_bytes: Option<Vec<u8>> = if let Some(wm_url) = &params.wm {
    let (bytes, wm_ct) = fetcher
      .fetch(wm_url)
      .await
      .map_err(|_| ProxyError::WatermarkFetchFailed)?;
    let _ = resolve_content_type(wm_ct.as_deref(), &bytes)?;
    Some(bytes)
  } else {
    None
  };

  // 4. Run synchronous image ops in spawn_blocking
  let params_clone = params.clone();

  // 4a. Animated GIF path
  if params_clone.gif_anim.is_some() && resolved_ct == "image/gif" {
    let range = params_clone.gif_anim.clone().unwrap();
    let all_frames = params_clone.gif_af.unwrap_or(false);
    let result = spawn_blocking(move || {
      let wm_img = if let Some(wm_data) = wm_bytes {
        let wm = image::ImageReader::new(std::io::Cursor::new(wm_data))
          .with_guessed_format()
          .map_err(|e| ProxyError::InternalError(e.to_string()))?
          .decode()
          .map_err(|e| ProxyError::InternalError(e.to_string()))?;
        Some(wm)
      } else {
        None
      };
      crate::modules::transform::ops::gif_anim::run(
        &src_bytes,
        &range,
        all_frames,
        &params_clone,
        wm_img,
      )
    })
    .await
    .map_err(|e| ProxyError::InternalError(format!("spawn_blocking panic: {e}")))?;
    return result.map(|bytes| (bytes, "image/gif".to_string()));
  }

  let resolved_ct_clone = resolved_ct.clone();

  spawn_blocking(move || -> Result<(Vec<u8>, String), ProxyError> {
    let mut img = crate::modules::transform::ops::decode::dispatch(&resolved_ct_clone, &src_bytes)?;

    // Resize
    let fit = params_clone.fit.as_deref().unwrap_or("contain");
    img = ops::resize::resize(img, params_clone.w, params_clone.h, fit)?;

    // Rotate
    img = ops::rotate::rotate(img, params_clone.rotate)?;

    // Flip
    img = ops::rotate::flip(img, params_clone.flip.as_deref())?;

    // Brightness / contrast
    let bright = params_clone.bright.unwrap_or(0);
    let contrast = params_clone.contrast.unwrap_or(0);
    if bright != 0 || contrast != 0 {
      img = ops::color::brightness_contrast(img, bright, contrast)?;
    }

    // Grayscale
    if params_clone.grayscale == Some(true) {
      img = ops::color::to_grayscale(img)?;
    }

    // Blur
    if let Some(sigma) = params_clone.blur {
      img = ops::blur::gaussian_blur(img, sigma)?;
    }

    // Watermark
    if let Some(wm_data) = wm_bytes {
      let wm_img = load_image(&wm_data)?;
      img = ops::watermark::apply_watermark_sync(img, wm_img)?;
    }

    // Encode
    let fmt = params_clone.format.as_deref().unwrap_or("jpeg");
    let quality = params_clone.q.unwrap_or(85);
    ops::encode::encode(img, fmt, quality)
  })
  .await
  .map_err(|e| ProxyError::InternalError(format!("spawn_blocking panic: {e}")))?
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::modules::proxy::dto::params::TransformParams;
  use crate::modules::security::allowlist::Allowlist;
  use crate::modules::transform::test_helpers::tiny_png_bytes;
  use std::sync::Arc;

  fn test_fetcher() -> Arc<dyn Fetchable> {
    use crate::modules::proxy::sources::http::HttpFetcher;
    Arc::new(
      HttpFetcher::new(10, 1_000_000, Arc::new(Allowlist::new(vec![])))
        .with_private_ip_check(false),
    )
  }

  #[tokio::test]
  async fn test_passthrough_no_transforms() {
    let params = TransformParams::default();
    let bytes = tiny_png_bytes();
    let (out, ct) = run_pipeline(
      params,
      bytes,
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await
    .unwrap();
    assert_eq!(ct, "image/png");
    assert!(!out.is_empty());
  }

  #[tokio::test]
  async fn test_resize_and_encode_webp() {
    let params = TransformParams {
      w: Some(10),
      h: Some(10),
      format: Some("webp".to_string()),
      ..Default::default()
    };
    let bytes = tiny_png_bytes();
    let (out, ct) = run_pipeline(
      params,
      bytes,
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await
    .unwrap();
    assert_eq!(ct, "image/webp");
    assert!(!out.is_empty());
  }

  #[tokio::test]
  async fn test_non_image_content_type_rejected() {
    let params = TransformParams::default();
    let result = run_pipeline(
      params,
      b"not an image".to_vec(),
      Some("text/html".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await;
    assert!(matches!(
      result,
      Err(crate::common::errors::ProxyError::NotAnImage)
    ));
  }

  #[tokio::test]
  async fn test_absent_content_type_inferred() {
    let bytes = tiny_png_bytes();
    let params = TransformParams::default();
    let (_, ct) = run_pipeline(
      params,
      bytes,
      None,
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await
    .unwrap();
    assert!(ct.starts_with("image/"));
  }

  #[test]
  fn test_pdf_content_type_accepted_in_resolve() {
    let result = resolve_content_type(Some("application/pdf"), b"%PDF-1.4 fake");
    assert!(result.is_ok(), "application/pdf should be accepted");
  }

  #[test]
  fn test_jxl_magic_bytes_detected() {
    let jxl_magic = &[0xFF, 0x0A, 0x00, 0x00];
    let result = resolve_content_type(None, jxl_magic);
    assert!(result.is_ok(), "JXL magic bytes should be detected");
  }

  #[tokio::test]
  async fn test_gif_anim_all_frames_pipeline() {
    use crate::modules::proxy::dto::params::GifAnimRange;
    use crate::modules::transform::test_helpers::tiny_gif_anim_bytes;
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;
    use std::io::Cursor;

    let params = TransformParams {
      gif_anim: Some(GifAnimRange::All),
      w: Some(2),
      h: Some(2),
      ..Default::default()
    };
    let bytes = tiny_gif_anim_bytes();
    let (out, ct) = run_pipeline(
      params,
      bytes,
      Some("image/gif".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await
    .unwrap();
    assert_eq!(ct, "image/gif");
    let decoder = GifDecoder::new(Cursor::new(&out)).unwrap();
    let frames = decoder.into_frames().collect_frames().unwrap();
    assert_eq!(frames.len(), 3);
  }

  #[tokio::test]
  async fn test_gif_anim_passthrough_not_taken() {
    // gif_anim alone with no other transforms must still re-encode (not passthrough)
    use crate::modules::proxy::dto::params::GifAnimRange;
    use crate::modules::transform::test_helpers::tiny_gif_anim_bytes;
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;
    use std::io::Cursor;

    let params = TransformParams {
      gif_anim: Some(GifAnimRange::All),
      ..Default::default()
    };
    let bytes = tiny_gif_anim_bytes();
    let (out, ct) = run_pipeline(
      params,
      bytes,
      Some("image/gif".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await
    .unwrap();
    assert_eq!(ct, "image/gif");
    let decoder = GifDecoder::new(Cursor::new(&out)).unwrap();
    let frames = decoder.into_frames().collect_frames().unwrap();
    assert_eq!(frames.len(), 3);
  }

  #[tokio::test]
  async fn test_gif_anim_on_non_gif_uses_static_path() {
    use crate::modules::proxy::dto::params::GifAnimRange;
    use crate::modules::transform::test_helpers::tiny_png_bytes;

    let params = TransformParams {
      gif_anim: Some(GifAnimRange::All),
      ..Default::default()
    };
    let bytes = tiny_png_bytes();
    let (out, ct) = run_pipeline(
      params,
      bytes,
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await
    .unwrap();
    // Static path default format is jpeg
    assert_eq!(ct, "image/jpeg");
    assert!(!out.is_empty());
  }

  #[tokio::test]
  async fn test_static_path_unaffected_no_gif_anim() {
    // PNG without gif_anim must still take the existing static path
    let params = TransformParams {
      w: Some(2),
      h: Some(2),
      format: Some("png".to_string()),
      ..Default::default()
    };
    let bytes = tiny_png_bytes();
    let (out, ct) = run_pipeline(
      params,
      bytes,
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await
    .unwrap();
    assert_eq!(ct, "image/png");
    assert_eq!(&out[1..4], b"PNG");
  }

  #[tokio::test]
  async fn test_output_disallowed_avif_returns_error() {
    use crate::common::config::DisallowedOutput;
    let mut output_disallow = std::collections::HashSet::new();
    output_disallow.insert(DisallowedOutput::Avif);
    let params = TransformParams {
      format: Some("avif".to_string()),
      ..Default::default()
    };
    let result = run_pipeline(
      params,
      tiny_png_bytes(),
      Some("image/png".to_string()),
      test_fetcher(),
      &output_disallow,
      &std::collections::HashSet::new(),
    )
    .await;
    assert!(matches!(
      result,
      Err(crate::common::errors::ProxyError::TransformDisabled(_))
    ));
  }

  #[tokio::test]
  async fn test_transform_disallowed_blur_returns_error() {
    use crate::common::config::DisallowedTransform;
    let mut transform_disallow = std::collections::HashSet::new();
    transform_disallow.insert(DisallowedTransform::Blur);
    let params = TransformParams {
      blur: Some(2.0),
      ..Default::default()
    };
    let result = run_pipeline(
      params,
      tiny_png_bytes(),
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &transform_disallow,
    )
    .await;
    assert!(matches!(
      result,
      Err(crate::common::errors::ProxyError::TransformDisabled(_))
    ));
  }

  #[tokio::test]
  async fn test_gif_af_alone_disallowed_gif_anim_returns_error() {
    use crate::common::config::DisallowedTransform;
    let mut transform_disallow = std::collections::HashSet::new();
    transform_disallow.insert(DisallowedTransform::GifAnim);
    let params = TransformParams {
      gif_af: Some(true),
      ..Default::default()
    };
    let result = run_pipeline(
      params,
      tiny_png_bytes(),
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &transform_disallow,
    )
    .await;
    assert!(matches!(
      result,
      Err(crate::common::errors::ProxyError::TransformDisabled(_))
    ));
  }

  #[tokio::test]
  async fn test_allowed_ops_pass_through() {
    let params = TransformParams {
      w: Some(4),
      h: Some(4),
      ..Default::default()
    };
    let result = run_pipeline(
      params,
      tiny_png_bytes(),
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await;
    assert!(result.is_ok());
  }

  #[tokio::test]
  async fn test_input_disallow_avif_does_not_block_avif_output() {
    use crate::common::config::DisallowedOutput;
    // avif output allowed when output_disallow is empty
    let params = TransformParams {
      format: Some("avif".to_string()),
      ..Default::default()
    };
    let result = run_pipeline(
      params,
      tiny_png_bytes(),
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await;
    assert!(
      result.is_ok(),
      "avif output must be allowed when output_disallow is empty"
    );

    // avif output blocked when output_disallow contains Avif
    let params2 = TransformParams {
      format: Some("avif".to_string()),
      ..Default::default()
    };
    let mut output_disallow = std::collections::HashSet::new();
    output_disallow.insert(DisallowedOutput::Avif);
    let result2 = run_pipeline(
      params2,
      tiny_png_bytes(),
      Some("image/png".to_string()),
      test_fetcher(),
      &output_disallow,
      &std::collections::HashSet::new(),
    )
    .await;
    assert!(matches!(
      result2,
      Err(crate::common::errors::ProxyError::TransformDisabled(_))
    ));
  }

  #[tokio::test]
  async fn test_unknown_format_returns_unsupported_format_error() {
    let params = TransformParams {
      format: Some("heic".to_string()),
      ..Default::default()
    };
    let result = run_pipeline(
      params,
      tiny_png_bytes(),
      Some("image/png".to_string()),
      test_fetcher(),
      &std::collections::HashSet::new(),
      &std::collections::HashSet::new(),
    )
    .await;
    assert!(
      matches!(
        result,
        Err(crate::common::errors::ProxyError::UnsupportedFormat(_))
      ),
      "expected UnsupportedFormat for unknown format value, got: {result:?}"
    );
  }
}
