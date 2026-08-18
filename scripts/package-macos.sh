#!/usr/bin/env bash
# Builds Konvertr.app from the release binary + dist/icon-1024.png.
#
#   IDENTITY="Developer ID Application: … (TEAMID)"  real signing (default: ad-hoc)
#   NOTARIZE=1 with APPLE_ID / APPLE_TEAM_ID / APPLE_APP_PASSWORD  notarize + staple
#   DMG=1                                            also produce a .dmg
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=target/bundle/Konvertr.app

cargo build --release -p konvrt

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/konvrt "$APP/Contents/MacOS/konvrt"

# Bundle ffmpeg so the video converter works without brew
if [ -x dist/bin/ffmpeg ]; then
  cp dist/bin/ffmpeg "$APP/Contents/Resources/ffmpeg"
  chmod +x "$APP/Contents/Resources/ffmpeg"
else
  echo "note: dist/bin/ffmpeg missing — run scripts/fetch-ffmpeg.sh to bundle ffmpeg"
fi

# Bundle yt-dlp so yoinks works out of the box
if [ -x dist/bin/yt-dlp ]; then
  cp dist/bin/yt-dlp "$APP/Contents/Resources/yt-dlp"
  chmod +x "$APP/Contents/Resources/yt-dlp"
else
  echo "note: dist/bin/yt-dlp missing — run scripts/fetch-ytdlp.sh to bundle yt-dlp"
fi

# .icns from the 1024 master
ICONSET=$(mktemp -d)/icon.iconset
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z $size $size dist/icon-1024.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) dist/icon-1024.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>konvrt</string>
    <key>CFBundleIdentifier</key><string>com.konvertr.app</string>
    <key>CFBundleName</key><string>Konvertr</string>
    <key>CFBundleDisplayName</key><string>Konvertr</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>CFBundleIconFile</key><string>icon</string>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

IDENTITY="${IDENTITY:--}"
if [ "$IDENTITY" = "-" ]; then
  # Ad-hoc: no hardened runtime, fine for local runs.
  codesign --deep --force --sign - "$APP"
else
  # Nested binaries first, then the app — signatures nest inside-out.
  for helper in ffmpeg yt-dlp; do
    [ -f "$APP/Contents/Resources/$helper" ] || continue
    codesign --force --options runtime --timestamp \
      --entitlements dist/entitlements-helper.plist \
      --sign "$IDENTITY" "$APP/Contents/Resources/$helper"
  done
  codesign --force --options runtime --timestamp \
    --entitlements dist/entitlements-app.plist \
    --sign "$IDENTITY" "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
fi

if [ "${NOTARIZE:-0}" = "1" ]; then
  ZIP="target/bundle/Konvertr-notarize.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  xcrun notarytool submit "$ZIP" \
    --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" \
    --wait
  # Staple before packaging so the artifact carries its ticket offline.
  xcrun stapler staple "$APP"
  rm -f "$ZIP"
fi

if [ "${DMG:-0}" = "1" ]; then
  DMG_PATH="target/bundle/Konvertr-macos-$(uname -m | sed s/aarch64/arm64/).dmg"
  STAGE=$(mktemp -d)
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$DMG_PATH"
  hdiutil create -volname "Konvertr" -srcfolder "$STAGE" -ov -format ULFO "$DMG_PATH" >/dev/null
  rm -rf "$STAGE"
  [ "$IDENTITY" = "-" ] || codesign --force --timestamp --sign "$IDENTITY" "$DMG_PATH"
  [ "${NOTARIZE:-0}" = "1" ] && xcrun stapler staple "$DMG_PATH"
  echo "built $DMG_PATH"
fi

echo "built $APP"
