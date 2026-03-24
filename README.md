<h1 align="center">PreviewProxy</h1>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/rust-stable-orange.svg?logo=rust" alt="Rust"></a>
  <a href="https://github.com/tokio-rs/axum"><img src="https://img.shields.io/badge/axum-0.8-blue.svg" alt="Axum"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://github.com/ViGrise/previewproxy/stargazers"><img src="https://img.shields.io/github/stars/ViGrise/previewproxy?style=social" alt="GitHub stars"></a>
  <a href="https://github.com/ViGrise/previewproxy/issues"><img src="https://img.shields.io/github/issues/ViGrise/previewproxy" alt="GitHub issues"></a>
</p>

A fast, self-hosted image proxy written in Rust. Fetch images from HTTP URLs, S3 buckets, or local storage - transform them on-the-fly and serve them with multi-tier caching.

## Features

- **On-the-fly transforms** - resize, crop, rotate, flip, grayscale, brightness/contrast, blur, watermark, format conversion (JPEG, PNG, WebP, AVIF, JXL)
- **Multiple sources** - remote HTTP URLs, S3-compatible buckets, and local filesystem
- **Two request styles** - path-style (`/300x200,webp/https://example.com/img.jpg`) and query-style (`/proxy?url=...&w=300&h=200`)
- **Animated GIF pipeline** - output all frames of an animated GIF, optionally applying transforms to a selected frame range
- **Video thumbnail extraction** - extract a frame from MP4, MKV, AVI and pass it through the normal transform pipeline
- **Multi-tier cache** - L1 in-memory (moka) + L2 disk with singleflight dedup
- **Security** - domain allowlist, optional HMAC request signing, configurable CORS origins, SSRF protection (private IP blocking, per-hop allowlist re-validation on redirects).

## Request Styles

### Path-style

```
GET /{transforms}/{image-url}
```

Transforms are comma-separated tokens before the image URL:

```bash
# Resize to 300x200
GET /300x200/https://example.com/photo.jpg
# Resize and convert to WebP
GET /300x200,webp/https://example.com/photo.jpg
# Grayscale only
GET /,grayscale/https://example.com/photo.jpg
# Grayscale and blur
GET /,grayscale,blur:0.8/https://example.com/photo.jpg
```

### Query-style

```
GET /proxy?url=https://example.com/photo.jpg&w=300&h=200&format=webp
```

Query params take precedence when both styles are combined.

## Transform Parameters

| Param       | Values                       | Description                          |
| ----------- | ---------------------------- | ------------------------------------ |
| `w`         | integer                      | Output width in pixels               |
| `h`         | integer                      | Output height in pixels              |
| `fit`       | `contain` (default), `cover` | Resize mode                          |
| `format`    | `jpeg`, `png`, `webp`        | Output format                        |
| `q`         | 1-100 (default: 85)          | Compression quality                  |
| `rotate`    | `90`, `180`, `270`           | Rotation degrees                     |
| `flip`      | `h`, `v`                     | Flip horizontal or vertical          |
| `grayscale` | `true`                       | Convert to grayscale                 |
| `bright`    | -100 to 100                  | Brightness adjustment                |
| `contrast`  | -100 to 100                  | Contrast adjustment                  |
| `blur`      | float (sigma)                        | Gaussian blur                                    |
| `wm`        | URL                                  | Watermark image URL                              |
| `seek`      | `5.0`, `0.5r`, `auto` (default: `0`) | Video seek: absolute seconds, relative ratio, or auto (middle frame) |
| `gif_anim`  | `all`, `N`, `N-M`, `-N`              | Animated GIF: output all frames, apply transforms starting at frame N, to frame range N-M, or to last N frames |
| `gif_af`    | `true`, `1`                          | GIF all-frames: apply style transforms (color, blur, etc.) to every frame, not just the `gif_anim` range |
| `sig`       | string                               | HMAC-SHA256 signature (if required)              |

## API Endpoints

| Method | Path      | Description                   |
| ------ | --------- | ----------------------------- |
| `GET`  | `/health` | Health check with cache stats |
| `GET`  | `/proxy`  | Query-style image proxy       |
| `GET`  | `/*path`  | Path-style image proxy        |

## Getting Started

### Linux / macOS

```shell
curl -o- https://raw.githubusercontent.com/ViGrise/previewproxy/main/install.sh | sudo bash
# or
wget -qO- https://raw.githubusercontent.com/ViGrise/previewproxy/main/install.sh | sudo bash
```

Installs to `/usr/local/bin`. Override with env vars:

| Var           | Default          | Description                |
| ------------- | ---------------- | -------------------------- |
| `INSTALL_DIR` | `/usr/local/bin` | Destination directory      |
| `VERSION`     | `latest`         | Release tag, e.g. `v1.0.0` |

### Windows

```powershell
irm https://raw.githubusercontent.com/ViGrise/previewproxy/main/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\previewproxy\bin` and adds it to your user `PATH`. Override with flags:

| Flag          | Default                           | Description                |
| ------------- | --------------------------------- | -------------------------- |
| `-InstallDir` | `%LOCALAPPDATA%\previewproxy\bin` | Destination directory      |
| `-Version`    | `latest`                          | Release tag, e.g. `v1.0.0` |

### Docker

```shell
docker run -d -p 8080:8080 \
  -e ALLOWED_HOSTS=img.example.com \
  -e HMAC_KEY=mysecret \
  ghcr.io/vigrise/previewproxy:latest
```

Or with Docker Compose:

```shell
curl -O https://raw.githubusercontent.com/ViGrise/previewproxy/main/docker-compose.yml
curl -O https://raw.githubusercontent.com/ViGrise/previewproxy/main/.env.sample
cp .env.sample .env
# Edit .env as needed
docker-compose up -d
```

### CLI Reference

Configuration is read from environment variables (`.env` file) or CLI flags - CLI flags take precedence.

| Flag                            | Env var                       | Default             | Description                                                                                                       |
| ------------------------------- | ----------------------------- | ------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `--port`, `-p`                  | `PORT`                        | `8080`              | Server port                                                                                                       |
| `--env`, `-E`                   | `APP_ENV`                     | `development`       | `development` or `production`                                                                                     |
| `--hmac-key`, `-k`              | `HMAC_KEY`                    | -                   | HMAC signing key; omit to disable                                                                                 |
| `--allowed-hosts`, `-a`         | `ALLOWED_HOSTS`               | -                   | Comma-separated allowed domains; empty = allow all                                                                |
| `--fetch-timeout-secs`, `-t`    | `FETCH_TIMEOUT_SECS`          | `10`                | Upstream fetch timeout (seconds)                                                                                  |
| `--max-source-bytes`, `-s`      | `MAX_SOURCE_BYTES`            | `20971520`          | Max source image size (bytes)                                                                                     |
| `--cache-memory-max-mb`         | `CACHE_MEMORY_MAX_MB`         | `256`               | L1 in-memory cache size (MB)                                                                                      |
| `--cache-memory-ttl-secs`       | `CACHE_MEMORY_TTL_SECS`       | `3600`              | L1 cache TTL (seconds)                                                                                            |
| `--cache-dir`, `-D`             | `CACHE_DIR`                   | `/tmp/previewproxy` | L2 disk cache directory                                                                                           |
| `--cache-disk-ttl-secs`         | `CACHE_DISK_TTL_SECS`         | `86400`             | L2 cache TTL (seconds)                                                                                            |
| `--cache-disk-max-mb`           | `CACHE_DISK_MAX_MB`           | -                   | L2 disk cache size limit (MB); empty = unlimited                                                                  |
| `--cache-cleanup-interval-secs` | `CACHE_CLEANUP_INTERVAL_SECS` | `600`               | Background cleanup interval (seconds)                                                                             |
| `--ffmpeg-path`                 | `FFMPEG_PATH`                 | `ffmpeg`            | Path to the ffmpeg binary                                                                                         |
| `--ffprobe-path`                | `FFPROBE_PATH`                | (same dir as ffmpeg) | Path to the ffprobe binary; defaults to `ffprobe` in the same directory as ffmpeg                                |
| `--cors-allow-origin`           | `CORS_ALLOW_ORIGIN`           | `*`                 | Comma-separated allowed CORS origins; `*` = allow all; wildcards (`*.example.com`) match a single subdomain label |
| `--cors-max-age-secs`           | `CORS_MAX_AGE_SECS`           | `600`               | CORS preflight cache duration (seconds)                                                                           |
| -                               | `RUST_LOG`                    | `server=info,...`   | Log level filter                                                                                                  |

---

## Security

### Allowlist

Set `ALLOWED_HOSTS` to a comma-separated list of trusted upstream domains. Wildcards match a single label:

```ini
ALLOWED_HOSTS=img.example.com,*.cdn.example.com
```

Leave empty to allow any host (open mode - use only in trusted environments).

### HMAC Signing

Set `HMAC_KEY` to require signed requests. The signature is computed as:

```
HMAC-SHA256(key, canonical_string)
```

where `canonical_string` is alphabetically sorted `key=value` pairs (excluding `sig`) joined by `&`, followed by `:` and the decoded image URL. Encode the result as URL-safe base64 (no padding) and pass it as the `sig` parameter.

### CORS

Set `CORS_ALLOW_ORIGIN` to restrict which browser origins may access the proxy. Wildcards match a single subdomain label:

```ini
CORS_ALLOW_ORIGIN=https://app.example.com,*.cdn.example.com
```

Leave as `*` (default) to allow any origin.

### SSRF Protection

Private, loopback, link-local, and reserved IP ranges (RFC 1918, RFC 6598, IPv6 ULA) are always blocked. On redirects, each hop's resolved IP and host are re-validated before following, preventing bypass via open redirectors.

## Development

### Requirements

**Runtime**

- `ffmpeg` + `ffprobe` - video frame extraction and duration probing (`apt install ffmpeg` / `brew install ffmpeg`)

**Build-time native libs**

- `libheif` - HEIF/HEIC image support
- `libjxl` - JPEG XL support
- `libdav1d` - AV1 decoder (used by libheif/libjxl)
- `pkg-config`, `cmake`, `nasm` - build toolchain

```shell
# Ubuntu / Debian
sudo apt install ffmpeg pkg-config cmake nasm libheif-dev libjxl-dev libdav1d-dev

# macOS
brew install ffmpeg pkg-config cmake nasm libheif jpeg-xl dav1d
```

### Commands

```shell
# Start dev server
cargo run

# Or with watch mode
cargo watch -q -x run

# Run tests
cargo test

# Format
cargo fmt

# Lint
cargo clippy
```

## Contributing

Contributions are welcome. Please open an issue or pull request at [github.com/ViGrise/previewproxy](https://github.com/ViGrise/previewproxy).

## License

Apache 2.0 - see [LICENSE](LICENSE).
