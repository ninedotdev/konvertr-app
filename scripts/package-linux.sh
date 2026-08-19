#!/usr/bin/env bash
# Assembles the (untested, unsigned) Linux tarball:
#   target/bundle/Konvertr-linux-x86_64.tar.gz
# Bundled ffmpeg / yt-dlp are included when already present in dist/bin
# (the release workflow fetches Linux builds of both before calling this).
set -euo pipefail

cd "$(dirname "$0")/.."

DIR=target/bundle/konvertr-linux-x86_64
TARBALL=target/bundle/Konvertr-linux-x86_64.tar.gz

cargo build --release -p konvrt

rm -rf "$DIR"
mkdir -p "$DIR"
cp target/release/konvrt "$DIR/konvertr"
chmod +x "$DIR/konvertr"

# Bundled helpers, when present — the app looks for them next to its binary.
for helper in ffmpeg yt-dlp; do
  if [ -x "dist/bin/$helper" ]; then
    cp "dist/bin/$helper" "$DIR/$helper"
    chmod +x "$DIR/$helper"
  else
    echo "note: dist/bin/$helper missing — video/loader tools will need it on PATH"
  fi
done

cp dist/icon-1024.png "$DIR/konvertr.png"

cat > "$DIR/konvertr.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Konvertr
Comment=Local converter tools: images, video, audio, PDF, and more
Exec=konvertr
Icon=konvertr
Terminal=false
Categories=Utility;Graphics;AudioVideo;
DESKTOP

cat > "$DIR/INSTALL.txt" <<'TXT'
Konvertr for Linux (x86_64)
===========================

HEADS UP: this build is produced by CI but has not been hand-tested on
Linux yet. It should work; if it doesn't, please open an issue.

Run it:

    ./konvertr

Everything runs locally — no uploads, no accounts. The bundled ffmpeg and
yt-dlp sit next to the binary; keep them in the same folder (or install
your own on PATH).

Optional desktop entry:

    1. Move this folder somewhere permanent, e.g. ~/.local/opt/konvertr
    2. Edit konvertr.desktop: set Exec= and Icon= to absolute paths, e.g.
         Exec=/home/you/.local/opt/konvertr/konvertr
         Icon=/home/you/.local/opt/konvertr/konvertr.png
    3. Copy it into place:
         cp konvertr.desktop ~/.local/share/applications/
TXT

rm -f "$TARBALL"
tar -czf "$TARBALL" -C target/bundle konvertr-linux-x86_64

echo "built $TARBALL"
