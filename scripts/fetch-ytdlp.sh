#!/usr/bin/env bash
# Downloads the standalone yt-dlp binary for the host OS into dist/bin/
# (yt-dlp.exe on Windows). Idempotent: skips if already present and runnable.
# Runs under bash on macOS, Linux, and Windows (Git Bash on the CI runners).
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p dist/bin

OS=$(uname -s)
ARCH=$(uname -m)
BASE=https://github.com/yt-dlp/yt-dlp/releases/latest/download
case "$OS" in
  Darwin)
    DEST=dist/bin/yt-dlp
    URL="$BASE/yt-dlp_macos"
    ;;
  Linux)
    DEST=dist/bin/yt-dlp
    case "$ARCH" in
      x86_64) URL="$BASE/yt-dlp_linux" ;;
      aarch64) URL="$BASE/yt-dlp_linux_aarch64" ;;
      *) echo "unsupported Linux arch: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  MINGW* | MSYS* | CYGWIN*)
    DEST=dist/bin/yt-dlp.exe
    URL="$BASE/yt-dlp.exe"
    ;;
  *)
    echo "unsupported OS: $OS" >&2
    exit 1
    ;;
esac

if [ -x "$DEST" ] && "$DEST" --version >/dev/null 2>&1; then
  echo "yt-dlp already present: $("$DEST" --version)"
  exit 0
fi

curl -fL --retry 3 -o "$DEST" "$URL"
chmod +x "$DEST"
if [ "$OS" = Darwin ]; then
  xattr -d com.apple.quarantine "$DEST" 2>/dev/null || true
fi
"$DEST" --version
echo "fetched $DEST"
