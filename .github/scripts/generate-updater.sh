#!/usr/bin/env bash
# Generate updater.json with SHA-256 checksums for all release assets.
# Expects env vars: TAG, REPO
set -euo pipefail

TAG="${TAG:?TAG is required}"
REPO="${REPO:?REPO is required}"

VERSION="${TAG#v}"
PUB_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

# Download all release artifacts to compute SHA-256 checksums.
mkdir -p dl
declare -A FILES=(
  ["windows-x86_64"]="gemacast-pc-x86_64-pc-windows-msvc.msi"
  ["darwin-x86_64"]="gemacast-pc-x64-apple-darwin.dmg"
  ["darwin-aarch64"]="gemacast-pc-aarch64-apple-darwin.dmg"
  ["darwin-universal"]="gemacast-pc-universal-apple-darwin.dmg"
  ["linux-x86_64"]="gemacast-pc-${VERSION}-x86_64.AppImage"
  ["linux-aarch64"]="gemacast-pc-${VERSION}-aarch64.AppImage"
  # The updater has no Android architecture key, so it must remain compatible
  # with every installed ABI even though the default manual download is ARM64.
  ["android"]="gemacast-mobile-universal.apk"
)

# Detached GPG signatures remain attached to the GitHub Release for manual
# verification. The in-app updater currently verifies the SHA-256 digest from
# this HTTPS manifest; it does not parse OpenPGP signatures.

# Download each artifact, hash it, and add it to the platform map.
#
# A platform whose artifact cannot be fetched is OMITTED rather than published
# with an empty sha256. The digest is the only integrity check the client
# performs on a downloaded update, so a digest-less entry is refused there — and
# omitting the platform fails closed more gracefully than shipping one: the
# client reports "no entry for platform" and simply does not offer an update,
# instead of downloading an artifact and then rejecting it.
PLATFORMS_JSON="{}"
MISSING=()

for platform in "${!FILES[@]}"; do
  file="${FILES[$platform]}"
  if ! gh release download "$TAG" --pattern "$file" --dir dl --repo "$REPO" 2>/dev/null; then
    echo "WARNING: could not download $file — omitting platform '$platform'" >&2
    MISSING+=("$platform")
    continue
  fi

  hash=$(sha256sum "dl/$file" | cut -d' ' -f1)
  echo "SHA-256 for $platform ($file): $hash"

  PLATFORMS_JSON=$(jq -n \
    --argjson acc "$PLATFORMS_JSON" \
    --arg key "$platform" \
    --arg url "${BASE_URL}/${file}" \
    --arg hash "$hash" \
    '$acc + { ($key): { url: $url, sha256: $hash } }')
done

# A manifest with no platforms would make every client report "no entry for
# platform" — indistinguishable from a deliberate drop, and silent. Fail instead.
if [[ "$(jq -r 'length' <<<"$PLATFORMS_JSON")" -eq 0 ]]; then
  echo "ERROR: no release artifact could be downloaded and hashed; refusing to" >&2
  echo "generate an empty updater.json." >&2
  exit 1
fi

if [[ ${#MISSING[@]} -gt 0 ]]; then
  echo "NOTE: ${#MISSING[@]} platform(s) omitted from updater.json: ${MISSING[*]}" >&2
  echo "Users on those platforms will not be offered this release." >&2
fi

jq -n \
  --arg version "$VERSION" \
  --arg pub_date "$PUB_DATE" \
  --argjson platforms "$PLATFORMS_JSON" \
  '{ version: $version, pub_date: $pub_date, platforms: $platforms }' > updater.json

echo "Generated updater.json:"
cat updater.json
