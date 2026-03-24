#!/usr/bin/env bash
set -euo pipefail

REPO="vigrise/previewproxy"
BIN_NAME="previewproxy"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
VERSION="${VERSION:-latest}"

# Detect OS
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$OS" in
  linux | darwin) ;;
  *) echo "Unsupported OS: $OS" >&2; exit 1 ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64)  ARCH="x86_64" ;;
  aarch64 | arm64) ARCH="arm64" ;;
  *) echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

ARTIFACT="${BIN_NAME}-${OS}-${ARCH}"
if [ "$VERSION" = "latest" ]; then
  URL="https://github.com/${REPO}/releases/latest/download/${ARTIFACT}"
else
  URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"
fi
DEST="${INSTALL_DIR}/${BIN_NAME}"

echo "Downloading ${ARTIFACT}..."
if command -v curl &>/dev/null; then
  curl -fL --progress-bar "$URL" -o "$DEST"
elif command -v wget &>/dev/null; then
  wget -qO "$DEST" --show-progress "$URL"
else
  echo "curl or wget is required" >&2
  exit 1
fi

chmod +x "$DEST"
echo "Installed to $DEST"
"$DEST" --version
