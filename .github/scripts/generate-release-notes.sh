#!/usr/bin/env bash
# Generate formatted release notes with download matrix.
# Expects env vars: TAG, REPO, SIGNING_KEY_FINGERPRINT
set -euo pipefail

TAG="${TAG:?TAG is required}"
REPO="${REPO:?REPO is required}"
FPR="${SIGNING_KEY_FINGERPRINT:?SIGNING_KEY_FINGERPRINT is required}"

VERSION="${TAG#v}"
BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

# Extract the selected version's entry from the manually maintained root changelog.
CHANGELOG_PATH="CHANGELOG.md"
: > current_changelog.md
if [ -f "$CHANGELOG_PATH" ]; then
  awk "/^## \[${VERSION}\]/ {flag=1; next} /^## \[/ {if(flag) exit} flag" "$CHANGELOG_PATH" > current_changelog.md
fi

# An empty extraction is a silent failure that still produces a publishable release,
# which is exactly how the wrong path survived three releases. Make it loud in the
# Actions log and visible in the notes themselves, rather than shipping a blank
# section again.
#
# Test for non-whitespace content, not `-s`: the awk emits the blank lines that
# follow the version heading even when it matches nothing useful, and a one-byte
# newline is "non-empty" to `-s` while rendering as an empty section on GitHub.
if ! grep -q '[^[:space:]]' current_changelog.md; then
  echo "::error title=Empty changelog::No '## [${VERSION}]' section with content found in ${CHANGELOG_PATH}. Falling back to a link."
  {
    echo ""
    echo "See [CHANGELOG.md](https://github.com/${REPO}/blob/${TAG}/CHANGELOG.md) for the full list of changes."
  } > current_changelog.md
fi

# Preserve the existing release body (contains cargo-dist checksum table)
EXISTING_BODY=$(gh release view "$TAG" --json body -q .body 2>/dev/null || echo "")

# Extract the cargo-dist checksum table if present
CHECKSUM_TABLE=""
if echo "$EXISTING_BODY" | grep -q "checksum"; then
  CHECKSUM_TABLE=$(echo "$EXISTING_BODY" | sed -n '/^|.*checksum/I,$ p')
fi

# Create the download matrix
{
  echo "## Gemacast ${VERSION} Downloads"
  echo ""
  echo "### Desktop Installers"
  echo "* **Windows**: [MSI Installer (x64)](${BASE_URL}/gemacast-pc-x86_64-pc-windows-msvc.msi)"
  echo "* **macOS (Universal)**: [DMG Installer](${BASE_URL}/gemacast-pc-universal-apple-darwin.dmg) | [App Bundle](${BASE_URL}/gemacast-pc-universal-apple-darwin.app.tar.gz)"
  echo "* **macOS (Apple Silicon)**: [DMG Installer](${BASE_URL}/gemacast-pc-aarch64-apple-darwin.dmg) | [App Bundle](${BASE_URL}/gemacast-pc-aarch64-apple-darwin.app.tar.gz)"
  echo "* **macOS (Intel)**: [DMG Installer](${BASE_URL}/gemacast-pc-x64-apple-darwin.dmg) | [App Bundle](${BASE_URL}/gemacast-pc-x64-apple-darwin.app.tar.gz)"
  echo "* **Linux (Debian/Ubuntu)**: [amd64 .deb](${BASE_URL}/gemacast-pc_${VERSION}_amd64.deb) | [arm64 .deb](${BASE_URL}/gemacast-pc_${VERSION}_arm64.deb)"
  echo "* **Linux (Fedora/RHEL)**: [x86_64 .rpm](${BASE_URL}/gemacast-pc-${VERSION}.x86_64.rpm) | [aarch64 .rpm](${BASE_URL}/gemacast-pc-${VERSION}.aarch64.rpm)"
  echo "* **Linux (Portable)**: [x86_64 AppImage](${BASE_URL}/gemacast-pc-${VERSION}-x86_64.AppImage) | [aarch64 AppImage](${BASE_URL}/gemacast-pc-${VERSION}-aarch64.AppImage)"
  echo ""
  echo "### Mobile"
  echo "* **Android (ARM64)**: [APK Installer](${BASE_URL}/gemacast-mobile.apk)"
  echo "* **Android (Universal)**: [APK Installer](${BASE_URL}/gemacast-mobile-universal.apk)"
  echo ""
  echo "### Security"
  echo "All binaries are signed with the Gemacast release key, fingerprint \`${FPR}\`."
  echo ""
  echo '```sh'
  echo "gpg --keyserver keys.openpgp.org --recv-keys ${FPR}"
  echo "gpg --verify gemacast-mobile.apk.sig gemacast-mobile.apk"
  echo '```'
  echo ""
  echo "\`pubkey.asc\` below is the same key, for offline import."
  echo ""
  echo "---"
  echo ""
  echo "### Changelog"
} > custom_header.md

# Combine: download matrix + changelog + cargo-dist checksums
cat custom_header.md current_changelog.md > new_notes.md
if [ -n "$CHECKSUM_TABLE" ]; then
  printf "\n\n---\n\n### Checksums\n\n%s\n" "$CHECKSUM_TABLE" >> new_notes.md
fi
gh release edit "$TAG" --notes-file new_notes.md --draft=false
