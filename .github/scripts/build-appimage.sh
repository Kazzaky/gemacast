#!/usr/bin/env bash
# Build an AppImage from an extracted cargo-dist binary archive.
# The AppImage contains only Gemacast; runtime transport does not use ADB.
# Expects env vars: VERSION, ARCH, DIST_EXTRACTED (path to extracted binary dir)
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
ARCH="${ARCH:?ARCH is required}"
DIST_EXTRACTED="${DIST_EXTRACTED:-dist_extracted}"

APPDIR="Gemacast.AppDir"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"

cp "${DIST_EXTRACTED}/gemacast-pc" "$APPDIR/usr/bin/"

# The desktop AppImage contains only Gemacast; device transport does not use ADB.
chmod +x "$APPDIR/usr/bin/"*

cp linux/gemacast-pc.desktop "$APPDIR/"
cp linux/gemacast-pc.desktop "$APPDIR/usr/share/applications/"
cp linux/gemacast-pc.png "$APPDIR/gemacast-pc.png"
cp linux/gemacast-pc.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/gemacast-pc.png"
cp linux/gemacast-pc.png "$APPDIR/.DirIcon"

# Create AppRun — written via printf to avoid heredoc/YAML conflicts
printf '#!/bin/bash\nSELF="$(readlink -f "$0")"\nHERE="${SELF%/*}"\nexport PATH="${HERE}/usr/bin:${PATH}"\nexec "${HERE}/usr/bin/gemacast-pc" "$@"\n' > "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

# Download appimagetool for the current architecture
wget -q "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-${ARCH}.AppImage" -O appimagetool
chmod +x appimagetool

./appimagetool --appimage-extract-and-run --no-appstream "$APPDIR" \
  "gemacast-pc-${VERSION}-${ARCH}.AppImage"

echo "Built: gemacast-pc-${VERSION}-${ARCH}.AppImage"
