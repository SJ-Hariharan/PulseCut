#!/usr/bin/env bash
# Build PulseCut.app (and a .dmg when hdiutil is available).
# Usage: scripts/package-macos.sh [debug|release]
set -euo pipefail
PROFILE="${1:-release}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
[ -f env.sh ] && source env.sh
if [ "$PROFILE" = release ]; then cargo build --release -p pulsecut-app -p pulsecut-cli; BIN=target/release; else cargo build -p pulsecut-app -p pulsecut-cli; BIN=target/debug; fi
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP="target/dist/PulseCut.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/PulseCut" "$APP/Contents/MacOS/PulseCut"
cp "$BIN/pulsecut-cli" "$APP/Contents/MacOS/pulsecut-cli"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>PulseCut</string>
  <key>CFBundleDisplayName</key><string>PulseCut</string>
  <key>CFBundleIdentifier</key><string>app.pulsecut.PulseCut</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>PulseCut</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>PulseCut listens for excitement and your "Clip it!" voice command only when you enable it in Settings. Audio is analysed locally and never uploaded.</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
</dict></plist>
PLIST
# Ad-hoc signature so Gatekeeper/TCC can track permissions (replace with a Developer ID for distribution).
codesign --force --deep --sign - "$APP" 2>/dev/null || echo "codesign unavailable; app is unsigned"
echo "built $APP"
if command -v hdiutil >/dev/null && [ "$PROFILE" = release ]; then
  hdiutil create -volname PulseCut -srcfolder "$APP" -ov -format UDZO "target/dist/PulseCut-${VERSION}-macos-$(uname -m).dmg" >/dev/null && echo "built dmg"
fi
