#!/bin/sh
set -e

REPO="mauhiz/rustymount"
BIN="rustymount"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
      *)       echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64) TARGET="x86_64-apple-darwin" ;;
      arm64)  TARGET="aarch64-apple-darwin" ;;
      *)      echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "Unsupported OS: $OS" >&2
    exit 1
    ;;
esac

# Pick download tool
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1"; }
  resolve_tag() { curl -fsSL -o /dev/null -w '%{url_effective}' "$1" | sed 's|.*/||'; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO- "$1"; }
  resolve_tag() { wget -qO /dev/null --server-response "$1" 2>&1 | grep -i 'Location:' | tail -1 | sed 's|.*/||' | tr -d '\r'; }
else
  echo "curl or wget is required" >&2
  exit 1
fi

# Resolve latest tag unless TAG is already set
if [ -z "$TAG" ]; then
  TAG="$(resolve_tag "https://github.com/${REPO}/releases/latest")"
fi

if [ -z "$TAG" ]; then
  echo "Could not determine latest release tag" >&2
  exit 1
fi

ARCHIVE="${BIN}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${TAG}/${ARCHIVE}"

echo "Downloading $BIN $TAG for $TARGET..."

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fetch "$URL" > "$TMP/$ARCHIVE"
tar -xzf "$TMP/$ARCHIVE" -C "$TMP"

echo "Installing to $INSTALL_DIR/$BIN"
install -d "$INSTALL_DIR"
install -m 755 "$TMP/$BIN" "$INSTALL_DIR/$BIN"

echo "Done. Run: $BIN --help"

if [ "$OS" = "Darwin" ]; then
  echo ""
  echo "Prerequisite: macFUSE must be installed."
  echo "  brew install --cask macfuse"
fi
