#!/usr/bin/env bash
# Create per-architecture .app bundles from extracted cargo-dist archives.
# Expects env vars: VERSION
# Expects directories: mac_x64/, mac_arm64/ with extracted binaries
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"

for ARCH in x64:mac_x64 aarch64:mac_arm64; do
  ARCH_NAME="${ARCH%%:*}"
  ARCH_DIR="${ARCH##*:}"

  APP="Gemacast-${ARCH_NAME}.app"
  rm -rf "$APP"
  mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

  # Main desktop executable only.
  cp "$ARCH_DIR/gemacast-pc" "$APP/Contents/MacOS/"
  chmod +x "$APP/Contents/MacOS/"*

  # Info.plist with version
  sed "s/VERSION_PLACEHOLDER/$VERSION/g" macos/Info.plist > "$APP/Contents/Info.plist"

  # Icon
  cp gemacast-mobile/src-tauri/icons/icon.icns "$APP/Contents/Resources/AppIcon.icns"

  echo "Created: $APP"
done
