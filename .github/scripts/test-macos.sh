#!/usr/bin/env bash
# Full macOS test: .app bundle structure, DMG mount/install, smoke test, archives.
# Expects env vars: VERSION (optional, defaults to TEST_VERSION or 0.0.0)
# Expects: Gemacast-{x64,aarch64,universal}.app directories
# Expects: gemacast-pc-*-apple-darwin.dmg files
# Expects: gemacast-pc-*-apple-darwin.app.tar.gz files
set -euo pipefail

VERSION="${VERSION:-${TEST_VERSION:-0.0.0}}"
ERRORS=0

# ── Verify .app bundle structure ─────────────────────────────────────────
echo "=== Verifying .app Bundles ==="
for VARIANT in x64 aarch64 universal; do
  APP="Gemacast-${VARIANT}.app"
  echo "--- Verifying $APP ---"

  # Check bundle structure
  if [ -f "$APP/Contents/MacOS/gemacast-pc" ]; then
    echo "PASS: Binary exists"

    # Swift-runtime LC_RPATH (from gemacast-pc/build.rs). Without it the app aborts
    # at launch. Every arch slice needs its own, so check them one by one.
    BIN="$APP/Contents/MacOS/gemacast-pc"
    SLICES=$(lipo -archs "$BIN")
    MISSING_RPATH=""
    for SLICE in $SLICES; do
      if ! otool -arch "$SLICE" -l "$BIN" | grep -A2 LC_RPATH | grep -q '/usr/lib/swift'; then
        MISSING_RPATH="$MISSING_RPATH $SLICE"
      fi
    done
    if [ -z "$MISSING_RPATH" ]; then
      echo "PASS: Swift runtime LC_RPATH present in all slices ($SLICES)"
    else
      echo "FAIL: Swift runtime LC_RPATH missing from slice(s):$MISSING_RPATH —" \
        "app will abort with 'Library not loaded: @rpath/libswift_Concurrency.dylib'"
      ERRORS=$((ERRORS+1))
    fi
  else
    echo "FAIL: Binary missing"; ERRORS=$((ERRORS+1))
  fi

  if [ -f "$APP/Contents/Info.plist" ]; then
    echo "PASS: Info.plist exists"
    # Verify bundle ID
    BUNDLE_ID=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$APP/Contents/Info.plist")
    if [ "$BUNDLE_ID" = "com.apir.gemacast" ]; then
      echo "PASS: Bundle ID is com.apir.gemacast"
    else
      echo "FAIL: Bundle ID is '$BUNDLE_ID', expected 'com.apir.gemacast'"; ERRORS=$((ERRORS+1))
    fi
    # Verify version
    BUNDLE_VERSION=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$APP/Contents/Info.plist")
    if [ "$BUNDLE_VERSION" = "$VERSION" ]; then
      echo "PASS: Version is $VERSION"
    else
      echo "FAIL: Version is '$BUNDLE_VERSION', expected '$VERSION'"; ERRORS=$((ERRORS+1))
    fi
  else
    echo "FAIL: Info.plist missing"; ERRORS=$((ERRORS+1))
  fi

  if [ -f "$APP/Contents/Resources/AppIcon.icns" ]; then
    echo "PASS: App icon exists"
  else
    echo "FAIL: App icon missing"; ERRORS=$((ERRORS+1))
  fi

  if [ -e "$APP/Contents/MacOS/adb" ]; then
    echo "FAIL: unexpected ADB binary bundled in $VARIANT"; ERRORS=$((ERRORS+1))
  else
    echo "PASS: no ADB runtime binary bundled"
  fi

  echo ""
done

# Verify universal binary contains both architectures
ARCHES=$(lipo -info "Gemacast-universal.app/Contents/MacOS/gemacast-pc" 2>&1)
if echo "$ARCHES" | grep -q "x86_64" && echo "$ARCHES" | grep -q "arm64"; then
  echo "PASS: Universal binary contains both x86_64 and arm64"
else
  echo "FAIL: Universal binary missing architectures: $ARCHES"; ERRORS=$((ERRORS+1))
fi

# ── Verify DMG (mount, install, smoke test, unmount) ─────────────────────
echo ""
echo "=== Testing DMG ==="
# Test with the universal DMG (covers lipo + packaging)
DMG="gemacast-pc-universal-apple-darwin.dmg"
echo "Testing $DMG..."

# Mount
MOUNT_OUTPUT=$(hdiutil attach "$DMG" -nobrowse -noverify 2>&1)
echo "$MOUNT_OUTPUT"
MOUNT_POINT=$(echo "$MOUNT_OUTPUT" | grep -o '/Volumes/[^ ]*' | tail -1)
if [ -z "$MOUNT_POINT" ]; then
  MOUNT_POINT=$(echo "$MOUNT_OUTPUT" | sed -n 's|.*\(/Volumes/.*\)|\1|p' | tail -1)
fi

echo "Mounted at: $MOUNT_POINT"

# Verify .app exists in DMG
APP_IN_DMG=$(find "$MOUNT_POINT" -name "*.app" -maxdepth 1 | head -1)
if [ -n "$APP_IN_DMG" ]; then
  echo "PASS: .app found in DMG: $APP_IN_DMG"
else
  echo "FAIL: No .app found in DMG"; ERRORS=$((ERRORS+1))
fi

# Simulate drag-to-install
if [ -n "$APP_IN_DMG" ]; then
  cp -R "$APP_IN_DMG" /tmp/Gemacast-test.app
  # Remove quarantine flag (unsigned app)
  xattr -dr com.apple.quarantine /tmp/Gemacast-test.app 2>/dev/null || true

  # Smoke test: start the binary
  echo "Smoke test: starting gemacast-pc from installed .app..."
  /tmp/Gemacast-test.app/Contents/MacOS/gemacast-pc &
  PID=$!
  sleep 5

  if kill -0 $PID 2>/dev/null; then
    echo "PASS: gemacast-pc is running from .app (PID $PID)"
    kill $PID 2>/dev/null || true
    wait $PID 2>/dev/null || true
  else
    echo "FAIL: gemacast-pc exited prematurely"; ERRORS=$((ERRORS+1))
  fi

  rm -rf /tmp/Gemacast-test.app
fi

# Unmount
hdiutil detach "$MOUNT_POINT" -quiet 2>/dev/null || hdiutil detach "$MOUNT_POINT" -force 2>/dev/null || true
echo "DMG unmounted"

# Verify all DMG files exist and are valid
for V in x64 aarch64 universal; do
  F="gemacast-pc-${V}-apple-darwin.dmg"
  if [ -f "$F" ] && [ -s "$F" ]; then
    echo "PASS: $F exists ($(du -h "$F" | cut -f1))"
  else
    echo "FAIL: $F missing or empty"; ERRORS=$((ERRORS+1))
  fi
done

# Verify .app.tar.gz archives can be extracted
for V in x64 aarch64 universal; do
  F="gemacast-pc-${V}-apple-darwin.app.tar.gz"
  if tar -tzf "$F" | grep -q "gemacast-pc"; then
    echo "PASS: $F contains gemacast-pc binary"
  else
    echo "FAIL: $F doesn't contain expected binary"; ERRORS=$((ERRORS+1))
  fi
done

if [ $ERRORS -gt 0 ]; then
  echo "=== $ERRORS check(s) failed ==="
  exit 1
fi
echo "All macOS installer checks passed"
