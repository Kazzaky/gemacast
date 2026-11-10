#!/usr/bin/env bash
# Verify RPM package metadata and contents.
# Expects env vars: VERSION, APPIMAGE_ARCH
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
APPIMAGE_ARCH="${APPIMAGE_ARCH:?APPIMAGE_ARCH is required}"

RPM_FILE="gemacast-pc-${VERSION}.${APPIMAGE_ARCH}.rpm"

if [ ! -f "$RPM_FILE" ]; then
  echo "::error::$RPM_FILE not found"
  exit 1
fi

# Verify RPM is a valid package
rpm -qip "$RPM_FILE"

# List contents and verify key files are present
CONTENTS=$(rpm -qlp "$RPM_FILE")
echo "$CONTENTS"

ERRORS=0
if echo "$CONTENTS" | grep -q "gemacast-pc"; then
  echo "PASS: Binary found in RPM"
else
  echo "FAIL: Binary not found in RPM"; ERRORS=$((ERRORS+1))
fi

if echo "$CONTENTS" | grep -Eiq '/(adb(\.exe)?|AdbWinApi\.dll|AdbWinUsbApi\.dll)

# firewalld service must survive alien's deb→rpm conversion
if echo "$CONTENTS" | grep -q "firewalld/services/gemacast.xml"; then
  echo "PASS: firewalld service found in RPM"
else
  echo "FAIL: firewalld service not found in RPM"; ERRORS=$((ERRORS+1))
fi

# alien --scripts must carry the postinst/postrm across; rpm stores them as
# scriptlets, visible via --scripts (not in the file list).
SCRIPTLETS=$(rpm -qp --scripts "$RPM_FILE" 2>/dev/null || true)
if echo "$SCRIPTLETS" | grep -q "add-service=gemacast"; then
  echo "PASS: postinst scriptlet carried into RPM"
else
  echo "FAIL: postinst scriptlet missing from RPM"; ERRORS=$((ERRORS+1))
fi

if [ $ERRORS -gt 0 ]; then exit 1; fi
echo "All .rpm structure checks passed"
; then
  echo "FAIL: ADB runtime binary found in RPM"; ERRORS=$((ERRORS+1))
else
  echo "PASS: no ADB runtime binaries are packaged"
fi

# firewalld service must survive alien's deb→rpm conversion
if echo "$CONTENTS" | grep -q "firewalld/services/gemacast.xml"; then
  echo "PASS: firewalld service found in RPM"
else
  echo "FAIL: firewalld service not found in RPM"; ERRORS=$((ERRORS+1))
fi

# alien --scripts must carry the postinst/postrm across; rpm stores them as
# scriptlets, visible via --scripts (not in the file list).
SCRIPTLETS=$(rpm -qp --scripts "$RPM_FILE" 2>/dev/null || true)
if echo "$SCRIPTLETS" | grep -q "add-service=gemacast"; then
  echo "PASS: postinst scriptlet carried into RPM"
else
  echo "FAIL: postinst scriptlet missing from RPM"; ERRORS=$((ERRORS+1))
fi

if [ $ERRORS -gt 0 ]; then exit 1; fi
echo "All .rpm structure checks passed"
