#!/usr/bin/env bash
# Build a .rpm package from an existing .deb using alien.
# Expects env vars: VERSION, ARCH, APPIMAGE_ARCH
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
ARCH="${ARCH:?ARCH is required}"
APPIMAGE_ARCH="${APPIMAGE_ARCH:?APPIMAGE_ARCH is required}"

DEB_FILE="gemacast-pc_${VERSION}_${ARCH}.deb"

if [ ! -f "$DEB_FILE" ]; then
  echo "::error::$DEB_FILE not found — build .deb first"
  exit 1
fi

# Generate the RPM build directory and spec file with correct target architecture
sudo alien --generate --to-rpm --target="${APPIMAGE_ARCH}" --scripts "$DEB_FILE"

SPEC_FILE=$(find . -name '*.spec' | head -1)
if [ -z "$SPEC_FILE" ]; then
  echo "::error::alien did not produce a .spec file"
  exit 1
fi

# Build the RPM package from the modified spec file
BUILD_DIR=$(dirname "$SPEC_FILE")
RPMBUILD_OUT=$(sudo rpmbuild --define "_rpmdir $(pwd)" --buildroot "$(pwd)/$BUILD_DIR" --target "${APPIMAGE_ARCH}" -bb "$SPEC_FILE" 2>&1)
echo "$RPMBUILD_OUT"

# Parse the output path from rpmbuild's "Wrote:" line
RPM_FILE=$(echo "$RPMBUILD_OUT" | grep '^Wrote:' | awk '{print $2}' | head -1)
if [ -z "$RPM_FILE" ] || [ ! -f "$RPM_FILE" ]; then
  echo "::error::rpmbuild did not produce an RPM file (parsed: '$RPM_FILE')"
  exit 1
fi

mv "$RPM_FILE" "gemacast-pc-${VERSION}.${APPIMAGE_ARCH}.rpm"
echo "Built: gemacast-pc-${VERSION}.${APPIMAGE_ARCH}.rpm"
