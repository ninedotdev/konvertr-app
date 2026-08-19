#!/usr/bin/env bash
# Assembles the (unsigned, untested) Windows zip on a windows runner's bash:
#   target/bundle/Konvertr-windows-x86_64.zip
# Bundled ffmpeg.exe / yt-dlp.exe are included when already present in
# dist/bin (the release workflow fetches Windows builds of both first).
set -euo pipefail

cd "$(dirname "$0")/.."

DIR=target/bundle/Konvertr-windows-x86_64
ZIP=target/bundle/Konvertr-windows-x86_64.zip

cargo build --release -p konvrt

rm -rf "$DIR"
mkdir -p "$DIR"
cp target/release/konvrt.exe "$DIR/konvrt.exe"

# Bundled helpers, when present — the app looks for them next to its binary.
for helper in ffmpeg.exe yt-dlp.exe; do
  if [ -f "dist/bin/$helper" ]; then
    cp "dist/bin/$helper" "$DIR/$helper"
  else
    echo "note: dist/bin/$helper missing — video/loader tools will need it on PATH"
  fi
done

cp dist/icon-1024.png "$DIR/konvertr.png"

cat > "$DIR/README.txt" <<'TXT'
Konvertr for Windows (x86_64)
=============================

HEADS UP: this build is UNSIGNED and has not been hand-tested on Windows
yet. It should work; if it doesn't, please open an issue.

Because it is unsigned, Windows SmartScreen will warn the first time you
run it. Click "More info", then "Run anyway".

Run konvrt.exe. Everything runs locally — no uploads, no accounts. The
bundled ffmpeg.exe and yt-dlp.exe sit next to the binary; keep them in
the same folder (or install your own on PATH).
TXT

rm -f "$ZIP"
if command -v 7z >/dev/null 2>&1; then
  (cd target/bundle && 7z a -tzip Konvertr-windows-x86_64.zip Konvertr-windows-x86_64 >/dev/null)
elif command -v pwsh >/dev/null 2>&1; then
  (cd target/bundle && pwsh -NoProfile -Command \
    "Compress-Archive -Path 'Konvertr-windows-x86_64' -DestinationPath 'Konvertr-windows-x86_64.zip' -Force")
else
  (cd target/bundle && powershell -NoProfile -Command \
    "Compress-Archive -Path 'Konvertr-windows-x86_64' -DestinationPath 'Konvertr-windows-x86_64.zip' -Force")
fi

echo "built $ZIP"
