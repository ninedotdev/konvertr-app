#!/usr/bin/env bash
# Downloads a static ffmpeg build for the host OS/arch into dist/bin/
# (ffmpeg.exe on Windows). Idempotent: skips if already there and runnable.
# Runs under bash on macOS, Linux, and Windows (Git Bash on the CI runners).
set -euo pipefail

cd "$(dirname "$0")/.."

OS=$(uname -s)
ARCH=$(uname -m)
case "$OS" in
  MINGW* | MSYS* | CYGWIN*) DEST=dist/bin/ffmpeg.exe ;;
  *) DEST=dist/bin/ffmpeg ;;
esac

if [ -x "$DEST" ] && "$DEST" -version >/dev/null 2>&1; then
  echo "ffmpeg already present: $("$DEST" -version | head -1)"
  exit 0
fi

mkdir -p dist/bin
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

case "$OS" in
  Darwin)
    case "$ARCH" in
      arm64) URL="https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip" ;;
      x86_64) URL="https://evermeet.cx/ffmpeg/getrelease/zip" ;;
      *) echo "unsupported macOS arch: $ARCH" >&2; exit 1 ;;
    esac
    echo "downloading static ffmpeg (macOS $ARCH) from $URL"
    curl -fL --retry 3 -o "$TMP/ffmpeg.zip" "$URL"
    unzip -oq "$TMP/ffmpeg.zip" -d "$TMP"
    BIN=$(find "$TMP" -name ffmpeg -type f | head -1)
    ;;
  Linux)
    case "$ARCH" in
      x86_64) SLUG=amd64 ;;
      aarch64) SLUG=arm64 ;;
      *) echo "unsupported Linux arch: $ARCH" >&2; exit 1 ;;
    esac
    URL="https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-${SLUG}-static.tar.xz"
    echo "downloading static ffmpeg (Linux $SLUG) from $URL"
    curl -fL --retry 3 -o "$TMP/ffmpeg.tar.xz" "$URL"
    tar -xJf "$TMP/ffmpeg.tar.xz" -C "$TMP"
    BIN=$(find "$TMP" -name ffmpeg -type f | head -1)
    ;;
  MINGW* | MSYS* | CYGWIN*)
    URL="https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
    echo "downloading static ffmpeg (Windows) from $URL"
    curl -fL --retry 3 -o "$TMP/ffmpeg.zip" "$URL"
    unzip -oq "$TMP/ffmpeg.zip" -d "$TMP"
    BIN=$(find "$TMP" -name ffmpeg.exe -type f | head -1)
    ;;
  *)
    echo "unsupported OS: $OS" >&2
    exit 1
    ;;
esac

if [ -z "$BIN" ]; then
  echo "no ffmpeg binary found in archive" >&2
  exit 1
fi
mv "$BIN" "$DEST"
chmod +x "$DEST"

"$DEST" -version | head -1
echo "installed $DEST"
