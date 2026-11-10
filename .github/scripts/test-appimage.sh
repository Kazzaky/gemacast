#!/usr/bin/env bash
# Verify AppImage structure, absence of bundled ADB, and smoke test.
# Expects env vars: VERSION, ARCH
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
ARCH="${ARCH:?ARCH is required}"

APPIMAGE="gemacast-pc-${VERSION}-${ARCH}.AppImage"
ERRORS=0

if [ ! -f "$APPIMAGE" ]; then
  echo "::error::$APPIMAGE not found"
  exit 1
fi

# Downloaded files from GitHub releases don't have +x permission
chmod +x "$APPIMAGE"

# Verify file exists and is executable
if [ -x "$APPIMAGE" ]; then
  echo "PASS: AppImage is executable"
else
  echo "FAIL: AppImage is not executable"; ERRORS=$((ERRORS+1))
fi

# Verify it's a valid AppImage (has magic bytes)
if file "$APPIMAGE" | grep -qi "elf\|appimage\|executable"; then
  echo "PASS: AppImage is a valid ELF binary"
else
  echo "FAIL: AppImage doesn't appear to be a valid binary"; ERRORS=$((ERRORS+1))
fi

# Extract and verify contents
./"$APPIMAGE" --appimage-extract 2>/dev/null || true
if [ -f squashfs-root/usr/bin/gemacast-pc ]; then
  echo "PASS: gemacast-pc binary found inside AppImage"
else
  echo "FAIL: gemacast-pc binary not found inside AppImage"; ERRORS=$((ERRORS+1))
fi

if [ -e squashfs-root/usr/bin/adb ]; then
  echo "FAIL: unexpected ADB binary bundled inside AppImage"; ERRORS=$((ERRORS+1))
else
  echo "PASS: no ADB binary is bundled"
fi

# Smoke test: run under xvfb
echo ""
echo "Smoke test: running AppImage under xvfb..."
xvfb-run --auto-servernum ./"$APPIMAGE" --appimage-extract-and-run &
PID=$!
sleep 5

if kill -0 $PID 2>/dev/null; then
  echo "PASS: AppImage process is running (PID $PID)"
  kill $PID 2>/dev/null || true
  wait $PID 2>/dev/null || true
else
  echo "FAIL: AppImage process exited prematurely"; ERRORS=$((ERRORS+1))
fi

rm -rf squashfs-root

if [ $ERRORS -gt 0 ]; then
  echo "=== $ERRORS AppImage check(s) failed ==="
  exit 1
fi
echo "All AppImage checks passed"
