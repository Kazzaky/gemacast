#!/usr/bin/env bash
# Create a universal (fat) .app bundle via lipo from x64 and arm64 builds.
# Expects env vars: VERSION
# Expects directories: mac_x64/, mac_arm64/ with extracted binaries
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"

APP="Gemacast-universal.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

lipo -create -output "$APP/Contents/MacOS/gemacast-pc" \
  mac_x64/gemacast-pc mac_arm64/gemacast-pc

chmod +x "$APP/Contents/MacOS/gemacast-pc"

sed "s/VERSION_PLACEHOLDER/$VERSION/g" macos/Info.plist > "$APP/Contents/Info.plist"
cp gemacast-mobile/src-tauri/icons/icon.icns "$APP/Contents/Resources/AppIcon.icns"

echo "Universal binary architectures:"
lipo -info "$APP/Contents/MacOS/gemacast-pc"
