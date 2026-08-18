#!/usr/bin/env bash
# Downloads the standalone yt-dlp binary for macOS into dist/bin/yt-dlp.
# Idempotent: skips if already present and runnable.
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p dist/bin

if [ -x dist/bin/yt-dlp ] && dist/bin/yt-dlp --version >/dev/null 2>&1; then
  echo "yt-dlp already present: $(dist/bin/yt-dlp --version)"
  exit 0
fi

curl -fL --retry 3 -o dist/bin/yt-dlp \
  https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos
chmod +x dist/bin/yt-dlp
xattr -d com.apple.quarantine dist/bin/yt-dlp 2>/dev/null || true
dist/bin/yt-dlp --version
echo "fetched dist/bin/yt-dlp"
