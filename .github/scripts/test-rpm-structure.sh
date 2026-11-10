#!/usr/bin/env bash
# Verify RPM package metadata, contents, ADB removal, and required scriptlets.
# Expects env vars: VERSION, APPIMAGE_ARCH
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
APPIMAGE_ARCH="${APPIMAGE_ARCH:?APPIMAGE_ARCH is required}"
RPM_FILE="gemacast-pc-${VERSION}.${APPIMAGE_ARCH}.rpm"

if [ ! -f "$RPM_FILE" ]; then
  echo "::error::$RPM_FILE not found"
  exit 1
fi

echo "=== RPM metadata ==="
rpm -qip "$RPM_FILE"

echo ""
echo "=== RPM contents ==="
CONTENTS=$(rpm -qlp "$RPM_FILE")
printf '%s\n' "$CONTENTS"

ERRORS=0

if printf '%s\n' "$CONTENTS" | grep -qE '/gemacast-pc([[:space:]]|$)'; then
  echo "PASS: Gemacast executable found in RPM"
else
  echo "FAIL: Gemacast executable not found in RPM"
  ERRORS=$((ERRORS + 1))
fi

if printf '%s\n' "$CONTENTS" | grep -Eiq '/(adb(\.exe)?|AdbWinApi\.dll|AdbWinUsbApi\.dll)([[:space:]]|$)'; then
  echo "FAIL: ADB runtime binary found in RPM"
  ERRORS=$((ERRORS + 1))
else
  echo "PASS: no ADB runtime binaries are packaged"
fi

if printf '%s\n' "$CONTENTS" | grep -q '/firewalld/services/gemacast.xml$'; then
  echo "PASS: firewalld service found in RPM"
else
  echo "FAIL: firewalld service not found in RPM"
  ERRORS=$((ERRORS + 1))
fi

# alien --scripts must carry the maintainer scripts into RPM scriptlets.
SCRIPTLETS=$(rpm -qp --scripts "$RPM_FILE" 2>/dev/null || true)
if printf '%s\n' "$SCRIPTLETS" | grep -q 'add-service=gemacast'; then
  echo "PASS: firewall post-install scriptlet carried into RPM"
else
  echo "FAIL: firewall post-install scriptlet missing from RPM"
  ERRORS=$((ERRORS + 1))
fi

if printf '%s\n' "$SCRIPTLETS" | grep -q 'remove-service=gemacast'; then
  echo "PASS: firewall post-uninstall scriptlet carried into RPM"
else
  echo "FAIL: firewall post-uninstall scriptlet missing from RPM"
  ERRORS=$((ERRORS + 1))
fi

if [ "$ERRORS" -gt 0 ]; then
  echo "=== $ERRORS RPM structure check(s) failed ==="
  exit 1
fi

echo "All RPM structure checks passed"
