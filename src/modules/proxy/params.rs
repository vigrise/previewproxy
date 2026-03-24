use crate::common::errors::ProxyError;

#[derive(Debug, Clone, PartialEq)]
pub enum SeekMode {
  /// Absolute seconds from start (clamped >= 0). e.g. `t=5.0`
  Absolute(f32),
  /// Ratio of total duration 0.0-1.0 (clamped). e.g. `t=0.5r`
  Relative(f32),
  /// Middle of video (50% of duration via ffprobe). e.g. `t=auto`
  Auto,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GifAnimRange {
  All,
  From(usize),
  Range(usize, usize),
  Last(usize),
}

#[derive(Debug, Clone, Default)]
pub struct TransformParams {
  /// Output width in pixels (max 8192).
  pub w: Option<u32>,
  /// Output height in pixels (max 8192).
  pub h: Option<u32>,
  /// Resize mode: `contain`, `cover`, or `crop`.
  pub fit: Option<String>,
  /// Output format: `webp`, `jpeg`, `png`, `avif`, `gif`, `bmp`, `tiff`, `ico`, `jxl`.
  pub format: Option<String>,
  /// JPEG/WebP quality 1-100.
  pub q: Option<u32>,
  /// Clockwise rotation in degrees (e.g. 90, 180, 270).
  pub rotate: Option<u32>,
  /// Flip axis: `h` (horizontal) or `v` (vertical).
  pub flip: Option<String>,
  /// Gaussian blur radius 0-100.
  pub blur: Option<f32>,
  /// Convert to grayscale.
  pub grayscale: Option<bool>,
  /// Seek mode for video thumbnail extraction.
  pub seek: Option<SeekMode>,
  /// Brightness adjustment (-255 to 255).
  pub bright: Option<i32>,
  /// Contrast adjustment (-255 to 255).
  pub contrast: Option<i32>,
  /// Watermark image URL (http/https/s3/local).
  pub wm: Option<String>,
  /// HMAC signature for request validation (excluded from canonical string).
  pub sig: Option<String>,
  /// Animated GIF frame range selection.
  pub gif_anim: Option<GifAnimRange>,
  /// Return all GIF frames but only transform frames in gif_anim range.
  pub gif_af: Option<bool>,
}

impl TransformParams {
  /// Parse from path wildcard string (everything after the leading slash in /*path).
  /// Splits at the last /https:// or /http:// to find image URL.
  pub fn from_path(path: &str) -> Result<(Self, String), ProxyError> {
    // Find last occurrence of each delimiter, in priority order
    let https_pos = path.rfind("/https://");
    let http_pos = path.rfind("/http://");
    let s3_pos = path.rfind("/s3:/");
    let local_pos = path.rfind("/local:/");
    // Also handle percent-encoded slash in local:/ delimiter
    let local_pct_pos = path
      .rfind("/local:%2F")
      .or_else(|| path.rfind("/local:%2f"));

    // Pick the rightmost match across all delimiters
    let split_pos = [https_pos, http_pos, s3_pos, local_pos, local_pct_pos]
      .into_iter()
      .flatten()
      .max();

    let (opts_str, url) = if let Some(pos) = split_pos {
      (&path[..pos], &path[pos + 1..])
    } else if path.starts_with("https://")
      || path.starts_with("http://")
      || path.starts_with("s3:/")
      || path.starts_with("local:/")
      || path.starts_with("local:%2F")
      || path.starts_with("local:%2f")
    {
      ("", path)
    } else {
      return Err(ProxyError::InvalidParams(
        "No image URL found in path".to_string(),
      ));
    };

    let url = urlencoding::decode(url)
      .unwrap_or_else(|_| url.into())
      .to_string();
    let params = parse_options(opts_str.trim_matches('/'))?;
    Ok((params, url))
  }

  /// Merge other's Some values into self (other takes precedence).
  pub fn merge_from(&mut self, other: TransformParams) {
    if other.w.is_some() {
      self.w = other.w;
    }
    if other.h.is_some() {
      self.h = other.h;
    }
    if other.fit.is_some() {
      self.fit = other.fit;
    }
    if other.format.is_some() {
      self.format = other.format;
    }
    if other.q.is_some() {
      self.q = other.q;
    }
    if other.rotate.is_some() {
      self.rotate = other.rotate;
    }
    if other.flip.is_some() {
      self.flip = other.flip;
    }
    if other.blur.is_some() {
      self.blur = other.blur;
    }
    if other.grayscale.is_some() {
      self.grayscale = other.grayscale;
    }
    if other.seek.is_some() {
      self.seek = other.seek;
    }
    if other.bright.is_some() {
      self.bright = other.bright;
    }
    if other.contrast.is_some() {
      self.contrast = other.contrast;
    }
    if other.wm.is_some() {
      self.wm = other.wm;
    }
    if other.sig.is_some() {
      self.sig = other.sig;
    }
    if other.gif_anim.is_some() {
      self.gif_anim = other.gif_anim;
    }
    if other.gif_af.is_some() {
      self.gif_af = other.gif_af;
    }
  }

  /// Canonical string for HMAC and cache key (excludes sig).
  /// Alphabetically sorted by query-style key name.
  pub fn canonical_string(&self, url: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = &self.blur {
      parts.push(format!("blur={v}"));
    }
    if let Some(v) = &self.bright {
      parts.push(format!("bright={v}"));
    }
    if let Some(v) = &self.contrast {
      parts.push(format!("contrast={v}"));
    }
    if let Some(v) = &self.fit {
      parts.push(format!("fit={v}"));
    }
    if let Some(v) = &self.flip {
      parts.push(format!("flip={v}"));
    }
    if let Some(v) = &self.format {
      parts.push(format!("format={v}"));
    }
    if self.gif_af == Some(true) {
      parts.push("gif_af=1".to_string());
    }
    if let Some(r) = &self.gif_anim {
      let s = match r {
        GifAnimRange::All => "gif_anim=all".to_string(),
        GifAnimRange::From(x) => format!("gif_anim={x}"),
        GifAnimRange::Range(x, y) => format!("gif_anim={x}-{y}"),
        GifAnimRange::Last(n) => format!("gif_anim=-{n}"),
      };
      parts.push(s);
    }
    if let Some(v) = &self.grayscale {
      parts.push(format!("grayscale={}", if *v { 1 } else { 0 }));
    }
    if let Some(v) = &self.h {
      parts.push(format!("h={v}"));
    }
    if let Some(v) = &self.q {
      parts.push(format!("q={v}"));
    }
    if let Some(v) = &self.rotate {
      parts.push(format!("rotate={v}"));
    }
    match &self.seek {
      Some(SeekMode::Auto) => parts.push("seek=auto".to_string()),
      Some(SeekMode::Relative(r)) => parts.push(format!("seek={r}r")),
      Some(SeekMode::Absolute(s)) => parts.push(format!("seek={s}")),
      None => {}
    }
    if let Some(v) = &self.w {
      parts.push(format!("w={v}"));
    }
    if let Some(v) = &self.wm {
      parts.push(format!("wm={v}"));
    }
    format!("{}:{}", parts.join("&"), url)
  }

  /// Returns true if any transform option is set.
  pub fn has_transforms(&self) -> bool {
    self.w.is_some()
      || self.h.is_some()
      || self.fit.is_some()
      || self.format.is_some()
      || self.q.is_some()
      || self.rotate.is_some()
      || self.flip.is_some()
      || self.blur.is_some()
      || self.grayscale.is_some()
      || self.seek.is_some()
      || self.bright.is_some()
      || self.contrast.is_some()
      || self.wm.is_some()
      || self.gif_anim.is_some()
  }
}

fn parse_gif_anim_value(s: &str) -> Result<GifAnimRange, ProxyError> {
  if s.is_empty() || s == "all" {
    return Ok(GifAnimRange::All);
  }
  if let Some(rest) = s.strip_prefix('-') {
    let n = rest
      .parse::<usize>()
      .map_err(|_| ProxyError::InvalidParams("invalid gif_anim".to_string()))?;
    return Ok(GifAnimRange::Last(n));
  }
  // split_once takes only the first '-'; "1-2-3" correctly fails at y_str.parse
  if let Some((x_str, y_str)) = s.split_once('-') {
    let x = x_str
      .parse::<usize>()
      .map_err(|_| ProxyError::InvalidParams("invalid gif_anim".to_string()))?;
    let y = y_str
      .parse::<usize>()
      .map_err(|_| ProxyError::InvalidParams("invalid gif_anim".to_string()))?;
    return Ok(GifAnimRange::Range(x, y));
  }
  let x = s
    .parse::<usize>()
    .map_err(|_| ProxyError::InvalidParams("invalid gif_anim".to_string()))?;
  Ok(GifAnimRange::From(x))
}

const MAX_DIMENSION: u32 = 8192;
const MAX_BLUR: f32 = 100.0;

fn parse_options(opts: &str) -> Result<TransformParams, ProxyError> {
  let mut p = TransformParams::default();
  if opts.is_empty() {
    return Ok(p);
  }
  for token in opts.split(',') {
    let token = token.trim();
    if token.is_empty() {
      continue;
    }
    // WxH
    if let Some((w_str, h_str)) = token.split_once('x') {
      if let (Ok(w), Ok(h)) = (w_str.parse::<u32>(), h_str.parse::<u32>()) {
        p.w = Some(w);
        p.h = Some(h);
        continue;
      }
    }
    // q80
    if let Some(rest) = token.strip_prefix('q') {
      if let Ok(v) = rest.parse::<u32>() {
        p.q = Some(v);
        continue;
      }
    }
    // r90 r180 r270
    if let Some(rest) = token.strip_prefix('r') {
      if let Ok(v) = rest.parse::<u32>() {
        p.rotate = Some(v);
        continue;
      }
    }
    // blur:5
    if let Some(val) = token.strip_prefix("blur:") {
      if let Ok(v) = val.parse::<f32>() {
        p.blur = Some(v);
        continue;
      }
    }
    // seek:5.0 / seek:0.5r / seek:auto
    if let Some(val) = token.strip_prefix("seek:") {
      if val == "auto" {
        p.seek = Some(SeekMode::Auto);
        continue;
      }
      if let Some(rel) = val.strip_suffix('r') {
        if let Ok(v) = rel.parse::<f32>() {
          p.seek = Some(SeekMode::Relative(v.clamp(0.0, 1.0)));
          continue;
        }
      }
      if let Ok(v) = val.parse::<f32>() {
        p.seek = Some(SeekMode::Absolute(v.max(0.0)));
        continue;
      }
    }
    // bright:10
    if let Some(val) = token.strip_prefix("bright:") {
      if let Ok(v) = val.parse::<i32>() {
        p.bright = Some(v);
        continue;
      }
    }
    // contrast:5
    if let Some(val) = token.strip_prefix("contrast:") {
      if let Ok(v) = val.parse::<i32>() {
        p.contrast = Some(v);
        continue;
      }
    }
    // wm:https://...
    if let Some(val) = token.strip_prefix("wm:") {
      p.wm = Some(val.to_string());
      continue;
    }
    // sig:hash
    if let Some(val) = token.strip_prefix("sig:") {
      p.sig = Some(val.to_string());
      continue;
    }
    // gif_anim / gif_anim:X / gif_anim:X-Y / gif_anim:-N
    if token == "gif_anim" {
      p.gif_anim = Some(GifAnimRange::All);
      continue;
    }
    if let Some(val) = token.strip_prefix("gif_anim:") {
      p.gif_anim = Some(parse_gif_anim_value(val)?);
      continue;
    }
    // gif_af
    if token == "gif_af" {
      p.gif_af = Some(true);
      continue;
    }
    match token {
      "contain" | "cover" | "crop" => {
        p.fit = Some(token.to_string());
      }
      "webp" | "jpeg" | "png" | "avif" | "gif" | "bmp" | "tiff" | "ico" | "jxl" => {
        p.format = Some(token.to_string());
      }
      "fliph" => {
        p.flip = Some("h".to_string());
      }
      "flipv" => {
        p.flip = Some("v".to_string());
      }
      "grayscale" => {
        p.grayscale = Some(true);
      }
      _ => {
        return Err(ProxyError::InvalidParams(format!(
          "Unknown option: {token}"
        )));
      }
    }
  }
  p.w = p.w.map(|v| v.min(MAX_DIMENSION));
  p.h = p.h.map(|v| v.min(MAX_DIMENSION));
  p.blur = p.blur.map(|v| v.clamp(0.0, MAX_BLUR));
  Ok(p)
}

/// Parse from query string HashMap into TransformParams.
pub fn from_query(
  query: &std::collections::HashMap<String, String>,
) -> Result<TransformParams, ProxyError> {
  let mut p = TransformParams::default();
  macro_rules! parse_field {
    ($field:ident, $key:expr, $typ:ty) => {
      if let Some(v) = query.get($key) {
        p.$field = Some(
          v.parse::<$typ>()
            .map_err(|_| ProxyError::InvalidParams(format!("invalid {}", $key)))?,
        );
      }
    };
  }
  parse_field!(w, "w", u32);
  parse_field!(h, "h", u32);
  parse_field!(q, "q", u32);
  parse_field!(rotate, "rotate", u32);
  parse_field!(blur, "blur", f32);
  parse_field!(bright, "bright", i32);
  parse_field!(contrast, "contrast", i32);
  if let Some(fit) = query.get("fit") {
    match fit.as_str() {
      "contain" | "cover" | "crop" => p.fit = Some(fit.clone()),
      _ => return Err(ProxyError::InvalidParams(format!("invalid fit: {fit}"))),
    }
  }
  if let Some(format) = query.get("format") {
    match format.as_str() {
      "jpeg" | "png" | "webp" | "avif" | "gif" | "bmp" | "tiff" | "ico" | "jxl" => {
        p.format = Some(format.clone())
      }
      _ => {
        return Err(ProxyError::InvalidParams(format!(
          "invalid format: {format}"
        )))
      }
    }
  }
  if let Some(flip) = query.get("flip") {
    match flip.as_str() {
      "h" | "v" => p.flip = Some(flip.clone()),
      _ => return Err(ProxyError::InvalidParams(format!("invalid flip: {flip}"))),
    }
  }
  if let Some(v) = query.get("wm") {
    p.wm = Some(v.clone());
  }
  if let Some(v) = query.get("sig") {
    p.sig = Some(v.clone());
  }
  if let Some(v) = query.get("grayscale") {
    p.grayscale = Some(v == "1" || v.eq_ignore_ascii_case("true"));
  }
  if let Some(v) = query.get("seek") {
    if v == "auto" {
      p.seek = Some(SeekMode::Auto);
    } else if let Some(rel) = v.strip_suffix('r') {
      let ratio = rel
        .parse::<f32>()
        .map_err(|_| ProxyError::InvalidParams("invalid seek".to_string()))?;
      p.seek = Some(SeekMode::Relative(ratio.clamp(0.0, 1.0)));
    } else {
      let secs = v
        .parse::<f32>()
        .map_err(|_| ProxyError::InvalidParams("invalid seek".to_string()))?;
      p.seek = Some(SeekMode::Absolute(secs.max(0.0)));
    }
  }
  if let Some(v) = query.get("gif_anim") {
    p.gif_anim = Some(parse_gif_anim_value(v)?);
  }
  if let Some(v) = query.get("gif_af") {
    p.gif_af = Some(v == "1" || v.eq_ignore_ascii_case("true"));
  }
  p.w = p.w.map(|v| v.min(MAX_DIMENSION));
  p.h = p.h.map(|v| v.min(MAX_DIMENSION));
  p.blur = p.blur.map(|v| v.clamp(0.0, MAX_BLUR));
  Ok(p)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_path_parse_basic() {
    let (params, url) =
      TransformParams::from_path("300x200,webp,q80/https://example.com/img.jpg").unwrap();
    assert_eq!(params.w, Some(300));
    assert_eq!(params.h, Some(200));
    assert_eq!(params.format, Some("webp".to_string()));
    assert_eq!(params.q, Some(80));
    assert_eq!(url, "https://example.com/img.jpg");
  }

  #[test]
  fn test_path_no_options() {
    let (params, url) = TransformParams::from_path("https://example.com/img.jpg").unwrap();
    assert_eq!(params.w, None);
    assert_eq!(params.format, None);
    assert_eq!(url, "https://example.com/img.jpg");
  }

  #[test]
  fn test_watermark_url_split_uses_last_http() {
    // wm URL contains https:// - must split at the LAST /https://
    let (params, url) =
      TransformParams::from_path("wm:https://logo.png/https://example.com/img.jpg").unwrap();
    assert_eq!(params.wm, Some("https://logo.png".to_string()));
    assert_eq!(url, "https://example.com/img.jpg");
  }

  #[test]
  fn test_canonical_string_sorted() {
    let params = TransformParams {
      w: Some(300),
      format: Some("webp".to_string()),
      ..Default::default()
    };
    let s = params.canonical_string("https://example.com/img.jpg");
    assert_eq!(s, "format=webp&w=300:https://example.com/img.jpg");
  }

  #[test]
  fn test_canonical_string_excludes_sig() {
    let params = TransformParams {
      w: Some(100),
      sig: Some("abc123".to_string()),
      ..Default::default()
    };
    let s = params.canonical_string("https://example.com/img.jpg");
    assert_eq!(s, "w=100:https://example.com/img.jpg");
  }

  #[test]
  fn test_query_merge_overrides_path() {
    let mut path_params = TransformParams {
      w: Some(100),
      h: Some(200),
      ..Default::default()
    };
    let query_params = TransformParams {
      w: Some(300),
      ..Default::default()
    };
    path_params.merge_from(query_params);
    assert_eq!(path_params.w, Some(300)); // query wins
    assert_eq!(path_params.h, Some(200)); // path kept
  }

  #[test]
  fn test_grayscale_canonical() {
    let params = TransformParams {
      grayscale: Some(true),
      ..Default::default()
    };
    let s = params.canonical_string("https://example.com/img.jpg");
    assert_eq!(s, "grayscale=1:https://example.com/img.jpg");
  }

  #[test]
  fn test_has_transforms_true() {
    let params = TransformParams {
      w: Some(100),
      ..Default::default()
    };
    assert!(params.has_transforms());
  }

  #[test]
  fn test_has_transforms_false() {
    let params = TransformParams::default();
    assert!(!params.has_transforms());
  }

  #[test]
  fn test_fliph_parsed() {
    let (params, _) = TransformParams::from_path("fliph/https://example.com/img.jpg").unwrap();
    assert_eq!(params.flip, Some("h".to_string()));
  }

  #[test]
  fn test_t_in_canonical_string() {
    let params = TransformParams {
      seek: Some(SeekMode::Absolute(5.0)),
      ..Default::default()
    };
    let s = params.canonical_string("https://example.com/v.mp4");
    assert!(
      s.contains("seek=5"),
      "canonical string must include seek: {s}"
    );
  }

  #[test]
  fn test_has_transforms_t_only() {
    let params = TransformParams {
      seek: Some(SeekMode::Absolute(3.0)),
      ..Default::default()
    };
    assert!(params.has_transforms());
  }

  #[test]
  fn test_format_gif_accepted() {
    let mut map = std::collections::HashMap::new();
    map.insert("format".to_string(), "gif".to_string());
    let p = super::from_query(&map).unwrap();
    assert_eq!(p.format, Some("gif".to_string()));
  }

  #[test]
  fn test_format_avif_now_accepted() {
    let mut map = std::collections::HashMap::new();
    map.insert("format".to_string(), "avif".to_string());
    let p = super::from_query(&map).unwrap();
    assert_eq!(p.format, Some("avif".to_string()));
  }

  #[test]
  fn test_format_jxl_accepted() {
    let mut map = std::collections::HashMap::new();
    map.insert("format".to_string(), "jxl".to_string());
    let p = super::from_query(&map).unwrap();
    assert_eq!(p.format, Some("jxl".to_string()));
  }

  #[test]
  fn test_t_from_query() {
    let mut map = std::collections::HashMap::new();
    map.insert("seek".to_string(), "2.5".to_string());
    let p = super::from_query(&map).unwrap();
    assert_eq!(p.seek, Some(SeekMode::Absolute(2.5)));
  }

  #[test]
  fn test_t_clamped_negative() {
    let mut map = std::collections::HashMap::new();
    map.insert("seek".to_string(), "-1.0".to_string());
    let p = super::from_query(&map).unwrap();
    assert_eq!(p.seek, Some(SeekMode::Absolute(0.0)));
  }

  #[test]
  fn test_t_from_path() {
    let (params, _) = TransformParams::from_path("seek:5.0/https://example.com/v.mp4").unwrap();
    assert_eq!(params.seek, Some(SeekMode::Absolute(5.0)));
  }

  #[test]
  fn test_t_merge_from() {
    let mut base = TransformParams {
      seek: Some(SeekMode::Absolute(1.0)),
      ..Default::default()
    };
    let other = TransformParams {
      seek: Some(SeekMode::Absolute(9.0)),
      ..Default::default()
    };
    base.merge_from(other);
    assert_eq!(base.seek, Some(SeekMode::Absolute(9.0)));
  }

  #[test]
  fn test_blur_parsed() {
    let (params, _) = TransformParams::from_path("blur:3.5/https://example.com/img.jpg").unwrap();
    assert_eq!(params.blur, Some(3.5));
  }

  #[test]
  fn test_s3_no_options() {
    let (params, url) = TransformParams::from_path("s3:/images/photo.jpg").unwrap();
    assert_eq!(params.w, None);
    assert_eq!(url, "s3:/images/photo.jpg");
  }

  #[test]
  fn test_s3_with_options() {
    let (params, url) = TransformParams::from_path("300x200,webp/s3:/images/photo.jpg").unwrap();
    assert_eq!(params.w, Some(300));
    assert_eq!(params.h, Some(200));
    assert_eq!(params.format, Some("webp".to_string()));
    assert_eq!(url, "s3:/images/photo.jpg");
  }

  #[test]
  fn test_local_no_options() {
    let (params, url) = TransformParams::from_path("local:/srv/img.png").unwrap();
    assert_eq!(params.w, None);
    assert_eq!(url, "local:/srv/img.png");
  }

  #[test]
  fn test_local_percent_encoded_no_options() {
    let (params, url) = TransformParams::from_path("local:%2Fsrv%2Fimg.png").unwrap();
    assert_eq!(params.w, None);
    assert_eq!(url, "local:/srv/img.png");
  }

  #[test]
  fn test_local_percent_encoded_with_options() {
    let (params, url) = TransformParams::from_path("300x200/local:%2Fsrv%2Fimg.png").unwrap();
    assert_eq!(params.w, Some(300));
    assert_eq!(params.h, Some(200));
    assert_eq!(url, "local:/srv/img.png");
  }

  #[test]
  fn test_watermark_s3_image_https() {
    let (params, url) =
      TransformParams::from_path("wm:s3:/overlay.png/https://example.com/img.jpg").unwrap();
    assert_eq!(params.wm, Some("s3:/overlay.png".to_string()));
    assert_eq!(url, "https://example.com/img.jpg");
  }

  #[test]
  fn test_from_query() {
    let mut map = std::collections::HashMap::new();
    map.insert("w".to_string(), "400".to_string());
    map.insert("format".to_string(), "webp".to_string());
    map.insert("grayscale".to_string(), "1".to_string());
    let params = from_query(&map).unwrap();
    assert_eq!(params.w, Some(400));
    assert_eq!(params.format, Some("webp".to_string()));
    assert_eq!(params.grayscale, Some(true));
  }

  #[test]
  fn test_seek_mode_absolute_path() {
    let (p, _) = TransformParams::from_path("seek:5.0/https://example.com/v.mp4").unwrap();
    assert!(matches!(p.seek, Some(SeekMode::Absolute(v)) if (v - 5.0).abs() < f32::EPSILON));
  }

  #[test]
  fn test_seek_mode_relative_path() {
    let (p, _) = TransformParams::from_path("seek:0.5r/https://example.com/v.mp4").unwrap();
    assert!(matches!(p.seek, Some(SeekMode::Relative(v)) if (v - 0.5).abs() < f32::EPSILON));
  }

  #[test]
  fn test_seek_mode_auto_path() {
    let (p, _) = TransformParams::from_path("seek:auto/https://example.com/v.mp4").unwrap();
    assert!(matches!(p.seek, Some(SeekMode::Auto)));
  }

  #[test]
  fn test_seek_mode_relative_clamped_above_one() {
    let (p, _) = TransformParams::from_path("seek:1.5r/https://example.com/v.mp4").unwrap();
    assert!(matches!(p.seek, Some(SeekMode::Relative(v)) if (v - 1.0).abs() < f32::EPSILON));
  }

  #[test]
  fn test_seek_mode_absolute_negative_clamped() {
    let (p, _) = TransformParams::from_path("seek:-1.0/https://example.com/v.mp4").unwrap();
    assert!(matches!(p.seek, Some(SeekMode::Absolute(v)) if (v - 0.0).abs() < f32::EPSILON));
  }

  #[test]
  fn test_seek_mode_auto_query() {
    let mut map = std::collections::HashMap::new();
    map.insert("seek".to_string(), "auto".to_string());
    let p = super::from_query(&map).unwrap();
    assert!(matches!(p.seek, Some(SeekMode::Auto)));
  }

  #[test]
  fn test_seek_mode_relative_query() {
    let mut map = std::collections::HashMap::new();
    map.insert("seek".to_string(), "0.3r".to_string());
    let p = super::from_query(&map).unwrap();
    assert!(matches!(p.seek, Some(SeekMode::Relative(v)) if (v - 0.3).abs() < 1e-5));
  }

  #[test]
  fn test_seek_mode_canonical_auto() {
    let p = TransformParams {
      seek: Some(SeekMode::Auto),
      ..Default::default()
    };
    assert_eq!(p.canonical_string("u"), "seek=auto:u");
  }

  #[test]
  fn test_seek_mode_canonical_relative() {
    let p = TransformParams {
      seek: Some(SeekMode::Relative(0.5)),
      ..Default::default()
    };
    assert_eq!(p.canonical_string("u"), "seek=0.5r:u");
  }

  #[test]
  fn test_seek_mode_canonical_absolute() {
    let p = TransformParams {
      seek: Some(SeekMode::Absolute(5.0)),
      ..Default::default()
    };
    assert_eq!(p.canonical_string("u"), "seek=5:u");
  }

  #[test]
  fn test_has_transforms_seek_auto() {
    let p = TransformParams {
      seek: Some(SeekMode::Auto),
      ..Default::default()
    };
    assert!(p.has_transforms());
  }

  #[test]
  fn test_gif_anim_has_transforms() {
    use super::GifAnimRange;
    let p = TransformParams {
      gif_anim: Some(GifAnimRange::All),
      ..Default::default()
    };
    assert!(p.has_transforms());
  }

  #[test]
  fn test_gif_anim_has_transforms_from_variant() {
    use super::GifAnimRange;
    let p = TransformParams {
      gif_anim: Some(GifAnimRange::From(1)),
      ..Default::default()
    };
    assert!(p.has_transforms());
  }

  #[test]
  fn test_gif_af_alone_does_not_trigger_has_transforms() {
    let p = TransformParams {
      gif_af: Some(true),
      ..Default::default()
    };
    assert!(!p.has_transforms());
  }

  #[test]
  fn test_gif_anim_canonical_all() {
    use super::GifAnimRange;
    let p = TransformParams {
      gif_anim: Some(GifAnimRange::All),
      ..Default::default()
    };
    assert_eq!(p.canonical_string("u"), "gif_anim=all:u");
  }

  #[test]
  fn test_gif_anim_canonical_from() {
    use super::GifAnimRange;
    let p = TransformParams {
      gif_anim: Some(GifAnimRange::From(2)),
      ..Default::default()
    };
    assert_eq!(p.canonical_string("u"), "gif_anim=2:u");
  }

  #[test]
  fn test_gif_anim_canonical_range() {
    use super::GifAnimRange;
    let p = TransformParams {
      gif_anim: Some(GifAnimRange::Range(1, 5)),
      ..Default::default()
    };
    assert_eq!(p.canonical_string("u"), "gif_anim=1-5:u");
  }

  #[test]
  fn test_gif_anim_canonical_last() {
    use super::GifAnimRange;
    let p = TransformParams {
      gif_anim: Some(GifAnimRange::Last(3)),
      ..Default::default()
    };
    assert_eq!(p.canonical_string("u"), "gif_anim=-3:u");
  }

  #[test]
  fn test_gif_af_canonical_true_included() {
    let p = TransformParams {
      gif_af: Some(true),
      ..Default::default()
    };
    assert!(p.canonical_string("u").contains("gif_af=1"));
  }

  #[test]
  fn test_gif_af_canonical_false_excluded() {
    let p = TransformParams {
      gif_af: Some(false),
      ..Default::default()
    };
    assert!(!p.canonical_string("u").contains("gif_af"));
  }

  // --- Parse tests (Task 2) ---

  #[test]
  fn test_gif_anim_path_all() {
    use super::GifAnimRange;
    let (p, _) = TransformParams::from_path("gif_anim/https://x.com/a.gif").unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::All)));
  }

  #[test]
  fn test_gif_anim_path_from() {
    use super::GifAnimRange;
    let (p, _) = TransformParams::from_path("gif_anim:2/https://x.com/a.gif").unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::From(2))));
  }

  #[test]
  fn test_gif_anim_path_range() {
    use super::GifAnimRange;
    let (p, _) = TransformParams::from_path("gif_anim:1-5/https://x.com/a.gif").unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::Range(1, 5))));
  }

  #[test]
  fn test_gif_anim_path_last() {
    use super::GifAnimRange;
    let (p, _) = TransformParams::from_path("gif_anim:-3/https://x.com/a.gif").unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::Last(3))));
  }

  #[test]
  fn test_gif_af_path() {
    let (p, _) = TransformParams::from_path("gif_af/https://x.com/a.gif").unwrap();
    assert_eq!(p.gif_af, Some(true));
  }

  #[test]
  fn test_gif_anim_query_all_keyword() {
    use super::GifAnimRange;
    let mut map = std::collections::HashMap::new();
    map.insert("gif_anim".to_string(), "all".to_string());
    let p = super::from_query(&map).unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::All)));
  }

  #[test]
  fn test_gif_anim_query_all_empty() {
    use super::GifAnimRange;
    let mut map = std::collections::HashMap::new();
    map.insert("gif_anim".to_string(), "".to_string());
    let p = super::from_query(&map).unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::All)));
  }

  #[test]
  fn test_gif_anim_query_from() {
    use super::GifAnimRange;
    let mut map = std::collections::HashMap::new();
    map.insert("gif_anim".to_string(), "2".to_string());
    let p = super::from_query(&map).unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::From(2))));
  }

  #[test]
  fn test_gif_anim_query_range() {
    use super::GifAnimRange;
    let mut map = std::collections::HashMap::new();
    map.insert("gif_anim".to_string(), "1-5".to_string());
    let p = super::from_query(&map).unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::Range(1, 5))));
  }

  #[test]
  fn test_gif_anim_query_last() {
    use super::GifAnimRange;
    let mut map = std::collections::HashMap::new();
    map.insert("gif_anim".to_string(), "-3".to_string());
    let p = super::from_query(&map).unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::Last(3))));
  }

  #[test]
  fn test_gif_af_query() {
    let mut map = std::collections::HashMap::new();
    map.insert("gif_af".to_string(), "1".to_string());
    let p = super::from_query(&map).unwrap();
    assert_eq!(p.gif_af, Some(true));
  }

  #[test]
  fn test_gif_af_query_false() {
    let mut map = std::collections::HashMap::new();
    map.insert("gif_af".to_string(), "false".to_string());
    let p = super::from_query(&map).unwrap();
    assert_eq!(p.gif_af, Some(false));
  }

  #[test]
  fn test_gif_af_query_zero() {
    let mut map = std::collections::HashMap::new();
    map.insert("gif_af".to_string(), "0".to_string());
    let p = super::from_query(&map).unwrap();
    assert_eq!(p.gif_af, Some(false));
  }

  #[test]
  fn test_gif_anim_path_last_zero_parses_to_last_zero() {
    use super::GifAnimRange;
    // gif_anim:-0 must parse to Last(0) so the runtime error path is exercised
    let (p, _) = TransformParams::from_path("gif_anim:-0/https://x.com/a.gif").unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::Last(0))));
  }

  #[test]
  fn test_gif_anim_query_last_zero_parses_to_last_zero() {
    use super::GifAnimRange;
    let mut map = std::collections::HashMap::new();
    map.insert("gif_anim".to_string(), "-0".to_string());
    let p = super::from_query(&map).unwrap();
    assert!(matches!(p.gif_anim, Some(GifAnimRange::Last(0))));
  }

  #[test]
  fn test_gif_anim_merge_from() {
    use super::GifAnimRange;
    let mut base = TransformParams {
      gif_anim: Some(GifAnimRange::All),
      ..Default::default()
    };
    let other = TransformParams {
      gif_anim: Some(GifAnimRange::From(2)),
      gif_af: Some(true),
      ..Default::default()
    };
    base.merge_from(other);
    assert!(matches!(base.gif_anim, Some(GifAnimRange::From(2))));
    assert_eq!(base.gif_af, Some(true));
  }
}
