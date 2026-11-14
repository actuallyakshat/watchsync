#!/usr/bin/env bash
# Build WatchSync.app (Apple Silicon + Intel) and zip it into dist/.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP=dist/WatchSync.app

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --target "$target"
done

rm -rf dist && mkdir -p "$APP/Contents/MacOS"
lipo -create \
  target/aarch64-apple-darwin/release/watchsync \
  target/x86_64-apple-darwin/release/watchsync \
  -output "$APP/Contents/MacOS/WatchSync"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>WatchSync</string>
  <key>CFBundleDisplayName</key><string>WatchSync</string>
  <key>CFBundleIdentifier</key><string>app.watchsync.WatchSync</string>
  <key>CFBundleExecutable</key><string>WatchSync</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature (required to run on Apple Silicon). Not notarized.
codesign --force --deep --sign - "$APP"

(cd dist && ditto -c -k --keepParent WatchSync.app "WatchSync-${VERSION}-macos.zip")
echo "Built dist/WatchSync-${VERSION}-macos.zip"
