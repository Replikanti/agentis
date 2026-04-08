#!/bin/sh
set -e

REPO="Replikanti/agentis"
INSTALL_DIR="${AGENTIS_INSTALL_DIR:-/usr/local/bin}"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)  PLATFORM="linux" ;;
  Darwin) PLATFORM="macos" ;;
  *)
    echo "Error: unsupported OS: $OS" >&2
    echo "Download manually from https://github.com/$REPO/releases" >&2
    exit 1
    ;;
esac

case "$ARCH" in
  x86_64|amd64)  ARCH_NAME="x86_64" ;;
  aarch64|arm64) ARCH_NAME="aarch64" ;;
  *)
    echo "Error: unsupported architecture: $ARCH" >&2
    echo "Download manually from https://github.com/$REPO/releases" >&2
    exit 1
    ;;
esac

BINARY="agentis-${PLATFORM}-${ARCH_NAME}"

# Helper: curl/wget
fetch() {
  url="$1"
  out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" ${out:+-o "$out"}
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "${out:--}" "$url"
  else
    echo "Error: curl or wget required" >&2
    exit 1
  fi
}

# Get latest release tag
RELEASE_JSON=$(fetch "https://api.github.com/repos/$REPO/releases/latest" "")

LATEST=$(echo "$RELEASE_JSON" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')

if [ -z "$LATEST" ]; then
  echo "Error: could not determine latest release" >&2
  exit 1
fi

URL="https://github.com/$REPO/releases/download/${LATEST}/${BINARY}"

echo "Installing agentis ${LATEST} (${PLATFORM}/${ARCH_NAME})..."

# Download
TMPFILE=$(mktemp "${TMPDIR:-/tmp}/agentis-install.XXXXXX")
fetch "$URL" "$TMPFILE"

# Verify download succeeded and file is non-empty
if [ ! -s "$TMPFILE" ]; then
  echo "Error: download failed or produced empty file" >&2
  echo "URL: $URL" >&2
  rm -f "$TMPFILE"
  exit 1
fi

chmod +x "$TMPFILE"

# Install (create directory if needed, use sudo if needed)
if [ -w "$INSTALL_DIR" ]; then
  mkdir -p "$INSTALL_DIR"
  mv "$TMPFILE" "$INSTALL_DIR/agentis"
elif [ -w "$(dirname "$INSTALL_DIR")" ]; then
  mkdir -p "$INSTALL_DIR"
  mv "$TMPFILE" "$INSTALL_DIR/agentis"
else
  echo "Installing to $INSTALL_DIR (requires sudo)..."
  sudo mkdir -p "$INSTALL_DIR"
  sudo mv "$TMPFILE" "$INSTALL_DIR/agentis"
fi

echo "Installed: $(agentis version 2>/dev/null || echo "$INSTALL_DIR/agentis")"
echo ""
echo "Get started:"
echo "  agentis init"
echo "  agentis go examples/fast-demo.ag"
