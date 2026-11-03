#!/usr/bin/env bash
# Builds MuskFlow.app (a menu-bar app: hold Fn anywhere to dictate) and installs it in /Applications.
#
#   scripts/make-app.sh [tier]      # lite | standard | max (default: standard)
#
# The app uses the models in this checkout's models/ folder (install them with
# `muskflow models install <tier>`). It is signed ad hoc, so macOS treats every
# rebuild as a new app: after rebuilding, turn MuskFlow off and on again under
# Input Monitoring and Accessibility.
set -euo pipefail
cd "$(dirname "$0")/.."

TIER="${1:-standard}"
ID="io.github.jatinbishla.muskflow"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
APP="dist/MuskFlow.app"

for dir in "$HOME/.cargo/bin" /opt/homebrew/opt/rustup/bin /usr/local/opt/rustup/bin; do
  [ -x "$dir/cargo" ] && PATH="$dir:$PATH"
done
cargo build --release --quiet

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/muskflow "$APP/Contents/MacOS/MuskFlow"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleName</key><string>MuskFlow</string>
  <key>CFBundleDisplayName</key><string>MuskFlow</string>
  <key>CFBundleExecutable</key><string>MuskFlow</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <!-- Menu-bar only: no Dock icon. -->
  <key>LSUIElement</key><true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>MuskFlow listens only while you hold Fn, and turns your speech into text on this Mac.</string>
  <key>LSEnvironment</key>
  <dict>
    <key>MUSKFLOW_MODELS</key><string>$PWD/models</string>
    <key>MUSKFLOW_TIER</key><string>$TIER</string>
  </dict>
</dict>
</plist>
EOF

codesign --force --sign - --identifier "$ID" "$APP" >/dev/null

DEST=/Applications/MuskFlow.app
pkill -x MuskFlow 2>/dev/null || true
rm -rf "$DEST"
if ditto "$APP" "$DEST" 2>/dev/null; then
  # Make LaunchServices pick up the new Info.plist (LSEnvironment).
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST"
  echo "Installed $DEST ($TIER tier). Open it from Applications or Spotlight; it lives in the menu bar."
else
  echo "Built $APP ($TIER tier). Copy it to /Applications to install."
fi
echo "Log: ~/Library/Logs/MuskFlow.log · Dictionary: ~/Library/Application Support/MuskFlow/dictionary.txt"
