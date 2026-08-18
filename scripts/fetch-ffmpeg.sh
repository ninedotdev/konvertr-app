#!/usr/bin/env bash
# Downloads a static macOS ffmpeg build for the current arch into
# dist/bin/ffmpeg. Idempotent: skips if the binary is already there and runs.
set -euo pipefail

cd "$(dirname "$0")/.."

DEST=dist/bin/ffmpeg
if [ -x "$DEST" ] && "$DEST" -version >/dev/null 2>&1; then
  echo "ffmpeg already present: $("$DEST" -version | head -1)"
  exit 0
fi

ARCH=$(uname -m)
case "$ARCH" in
  arm64) URL="https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip" ;;
  x86_64) URL="https://evermeet.cx/ffmpeg/getrelease/zip" ;;
  *) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

mkdir -p dist/bin
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "downloading static ffmpeg ($ARCH) from $URL"
curl -fL --retry 3 -o "$TMP/ffmpeg.zip" "$URL"
unzip -oq "$TMP/ffmpeg.zip" -d "$TMP"
BIN=$(find "$TMP" -name ffmpeg -type f | head -1)
if [ -z "$BIN" ]; then
  echo "no ffmpeg binary found in archive" >&2
  exit 1
fi
mv "$BIN" "$DEST"
chmod +x "$DEST"

"$DEST" -version | head -1
echo "installed $DEST"
