#!/usr/bin/env bash
# Build a .deb package from an extracted cargo-dist binary archive.
# Expects env vars: VERSION, ARCH, DIST_EXTRACTED (path to extracted binary dir)
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
ARCH="${ARCH:?ARCH is required}"
DIST_EXTRACTED="${DIST_EXTRACTED:-dist_extracted}"

PKG_NAME="gemacast-pc_${VERSION}_${ARCH}"

mkdir -p "${PKG_NAME}/DEBIAN"
mkdir -p "${PKG_NAME}/usr/bin"
mkdir -p "${PKG_NAME}/usr/lib/firewalld/services"
mkdir -p "${PKG_NAME}/usr/share/applications"
mkdir -p "${PKG_NAME}/usr/share/icons/hicolor/256x256/apps"

# Binary
cp "${DIST_EXTRACTED}/gemacast-pc" "${PKG_NAME}/usr/bin/"
chmod 755 "${PKG_NAME}/usr/bin/gemacast-pc"

# Desktop entry & icon
cp linux/gemacast-pc.desktop "${PKG_NAME}/usr/share/applications/"
cp linux/gemacast-pc.png "${PKG_NAME}/usr/share/icons/hicolor/256x256/apps/"

# firewalld service definition (opens UDP 23555/23556 + TCP 23559 for LAN).
# Fedora/RHEL ship firewalld blocking-by-default, so without this a fresh install
# cannot be discovered. Named plainly `gemacast.xml` so `--add-service=gemacast`
# in the maintainer scripts resolves it.
cp linux/gemacast.firewalld.xml "${PKG_NAME}/usr/lib/firewalld/services/gemacast.xml"

# Maintainer scripts: apply/remove the firewall rules on install/uninstall.
# Best-effort (they never fail the transaction) — see the scripts. alien's
# `--scripts` carries these into the .rpm, so Fedora gets them too.
cp linux/deb-postinst.sh "${PKG_NAME}/DEBIAN/postinst"
cp linux/deb-postrm.sh "${PKG_NAME}/DEBIAN/postrm"
chmod 755 "${PKG_NAME}/DEBIAN/postinst" "${PKG_NAME}/DEBIAN/postrm"

# Control file — written field-by-field to avoid heredoc whitespace issues
{
  echo "Package: gemacast-pc"
  echo "Version: ${VERSION}"
  echo "Section: sound"
  echo "Priority: optional"
  echo "Architecture: ${ARCH}"
  echo "Maintainer: Echa Apriliyanto <echa.apriliyanto.dev@gmail.com>"
  echo "Description: Low-latency real-time audio streaming from PC to Android"
  echo " Gemacast streams audio from your PC to Android devices over a local"
  echo " network with minimal latency using Opus codec and UDP transport."
  echo "Homepage: https://github.com/Kazzaky/gemacast"
} > "${PKG_NAME}/DEBIAN/control"

dpkg-deb --build --root-owner-group "${PKG_NAME}"
echo "Built: ${PKG_NAME}.deb"
