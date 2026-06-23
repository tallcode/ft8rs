#!/usr/bin/env bash
#
# Build a macOS .app bundle for the ft8.rs GUI.
#
# macOS release packaging runs locally (hosted-runner queue times keep macOS
# disabled in CI). This assembles dist/ft8.rs.app from a release build of
# ft8rs-gui, generating the .icns from the PNG logo and writing an Info.plist
# that includes the microphone-usage string — without it a bundled app crashes
# the moment it opens the soundcard on modern macOS.
#
# Usage:  ./scripts/bundle-macos.sh
# Output: dist/ft8.rs.app  (double-click to run; drag to /Applications to install)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="ft8.rs"
BIN_NAME="ft8rs-gui"
BUNDLE_ID="io.github.tallcode.ft8rs"
PNG="crates/ft8rs-gui/assets/ft8rs.png"
ALLCALL="crates/ft8rs-core/ALLCALL7.TXT"

APP="dist/${APP_NAME}.app"
CONTENTS="$APP/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RES_DIR="$CONTENTS/Resources"

VERSION="$(git describe --tags --always --dirty 2>/dev/null || echo "0.0.0")"
# CFBundleShortVersionString must look like x.y.z; fall back when on an untagged
# commit (which yields a hash).
SHORT_VERSION="$(printf '%s' "$VERSION" | grep -Eo '^v?[0-9]+\.[0-9]+\.[0-9]+' | sed 's/^v//' || true)"
[ -n "$SHORT_VERSION" ] || SHORT_VERSION="0.0.0"

echo "==> Building release binary"
cargo build --release -p ft8rs-gui

echo "==> Generating .icns from $PNG"
ICONSET="$(mktemp -d)/${APP_NAME}.iconset"
mkdir -p "$ICONSET"
# Standard macOS icon sizes (1x + @2x). The 500px source upscales for the
# largest entries; replace with a 1024px export for crisp Retina icons.
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$PNG" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  dbl=$((size * 2))
  sips -z "$dbl" "$dbl" "$PNG" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$MACOS_DIR" "$RES_DIR"
cp "target/release/$BIN_NAME" "$MACOS_DIR/$BIN_NAME"
chmod +x "$MACOS_DIR/$BIN_NAME"
iconutil -c icns "$ICONSET" -o "$RES_DIR/${APP_NAME}.icns"
rm -rf "$(dirname "$ICONSET")"

# ALLCALL7.TXT next to the binary (searchcalls looks in the exe directory first)
# so the JTDX profile's callsign filtering works in the bundled app.
[ -f "$ALLCALL" ] && cp "$ALLCALL" "$MACOS_DIR/ALLCALL7.TXT"

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleExecutable</key>
    <string>${BIN_NAME}</string>
    <key>CFBundleIconFile</key>
    <string>${APP_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${SHORT_VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.15</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>ft8.rs captures audio from your selected input device to decode FT8 signals.</string>
</dict>
</plist>
PLIST

plutil -lint "$CONTENTS/Info.plist" >/dev/null

echo ""
echo "Built $APP  (version $VERSION)"
echo "  • Double-click to run, or drag into /Applications."
echo "  • Unsigned: first launch needs right-click → Open (Gatekeeper)."
