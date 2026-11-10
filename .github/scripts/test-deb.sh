#!/usr/bin/env bash
# Full .deb installer test: install → verify → smoke test → uninstall → verify cleanup.
# Expects env vars: VERSION, ARCH
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
ARCH="${ARCH:?ARCH is required}"

DEB_FILE="gemacast-pc_${VERSION}_${ARCH}.deb"

if [ ! -f "$DEB_FILE" ]; then
  echo "::error::$DEB_FILE not found"
  exit 1
fi

# ── Install ────────────────────────────────────────────────────────────────
echo "=== Installing .deb ==="
sudo dpkg -i "$DEB_FILE"

# ── Verify Installation ───────────────────────────────────────────────────
echo ""
echo "=== Verifying .deb Installation ==="
ERRORS=0

# Binary installed
if command -v gemacast-pc &>/dev/null; then
  echo "PASS: gemacast-pc is on PATH at $(which gemacast-pc)"
else
  echo "FAIL: gemacast-pc not found on PATH"; ERRORS=$((ERRORS+1))
fi

# The desktop package must not bundle the removed ADB runtime.
PACKAGE_CONTENTS=$(dpkg-deb --contents "$DEB_FILE")
if echo "$PACKAGE_CONTENTS" | grep -Eiq '/(adb(\.exe)?|AdbWinApi\.dll|AdbWinUsbApi\.dll)([[:space:]]|$)'; then
  echo "FAIL: ADB binary found in the .deb package"; ERRORS=$((ERRORS+1))
else
  echo "PASS: no ADB runtime binaries are packaged"
fi
if [ -e /usr/lib/gemacast/adb ]; then
  echo "FAIL: legacy ADB binary was installed"; ERRORS=$((ERRORS+1))
else
  echo "PASS: no ADB runtime binary installed"
fi

# Desktop entry
if [ -f /usr/share/applications/gemacast-pc.desktop ]; then
  echo "PASS: Desktop entry installed"
else
  echo "FAIL: Desktop entry not found"; ERRORS=$((ERRORS+1))
fi

# Icon
if [ -f /usr/share/icons/hicolor/256x256/apps/gemacast-pc.png ]; then
  echo "PASS: Application icon installed"
else
  echo "FAIL: Application icon not found"; ERRORS=$((ERRORS+1))
fi

# firewalld service definition
if [ -f /usr/lib/firewalld/services/gemacast.xml ]; then
  echo "PASS: firewalld service definition installed"
else
  echo "FAIL: firewalld service definition not found"; ERRORS=$((ERRORS+1))
fi

# The postinst's actual effect, not just its presence. ufw is on the runner but
# inactive; `ufw allow` still records the rule, and `show added` lists it.
# firewalld cannot be checked the same way - the daemon is not running here.
if command -v ufw &>/dev/null; then
  ADDED=$(sudo ufw show added 2>/dev/null || true)
  if echo "$ADDED" | grep -q "23555,23556/udp" && echo "$ADDED" | grep -q "23559/tcp"; then
    echo "PASS: postinst added the ufw rules"
  else
    echo "FAIL: ufw rules missing after install"; ERRORS=$((ERRORS+1))
  fi
else
  echo "SKIP: ufw not installed, cannot verify its rules"
fi

# Port reservation against ephemeral allocation.
if [ -f /etc/sysctl.d/99-gemacast.conf ]; then
  echo "PASS: sysctl drop-in installed"
else
  echo "FAIL: /etc/sysctl.d/99-gemacast.conf not created"; ERRORS=$((ERRORS+1))
fi

if sysctl -n net.ipv4.ip_local_reserved_ports 2>/dev/null | grep -q "23555-23559"; then
  echo "PASS: 23555-23559 reserved in the running kernel"
else
  echo "FAIL: 23555-23559 not in ip_local_reserved_ports"; ERRORS=$((ERRORS+1))
fi

# Maintainer scripts present in the package (they run during install, so they are
# not left on disk — inspect the .deb's control archive instead).
CTRL_FILES=$(dpkg-deb --ctrl-tarfile "$DEB_FILE" | tar -tf - 2>/dev/null || true)
for script in postinst postrm; do
  if echo "$CTRL_FILES" | grep -qE "(^|/)${script}\$"; then
    echo "PASS: DEBIAN/${script} present in package"
  else
    echo "FAIL: DEBIAN/${script} missing from package"; ERRORS=$((ERRORS+1))
  fi
done

# Smoke test: run with xvfb (provides virtual display for the tray app)
echo ""
echo "Smoke test: starting gemacast-pc under xvfb..."
xvfb-run --auto-servernum gemacast-pc &
PID=$!
sleep 5

if kill -0 $PID 2>/dev/null; then
  echo "PASS: gemacast-pc is running (PID $PID)"
  kill $PID 2>/dev/null || true
  wait $PID 2>/dev/null || true
else
  echo "FAIL: gemacast-pc exited prematurely"; ERRORS=$((ERRORS+1))
fi

if [ $ERRORS -gt 0 ]; then
  echo ""
  echo "=== $ERRORS installation check(s) failed ==="
  exit 1
fi
echo ""
echo "All .deb installation checks passed"

# ── Uninstall ──────────────────────────────────────────────────────────────
echo ""
echo "=== Uninstalling .deb ==="
sudo dpkg -r gemacast-pc

# ── Verify Uninstall ──────────────────────────────────────────────────────
echo ""
echo "=== Verifying .deb Uninstall ==="
ERRORS=0

if command -v gemacast-pc &>/dev/null; then
  echo "FAIL: gemacast-pc still on PATH after uninstall"; ERRORS=$((ERRORS+1))
else
  echo "PASS: gemacast-pc removed from PATH"
fi

if [ -e /usr/lib/gemacast/adb ]; then
  echo "FAIL: legacy ADB binary still exists after uninstall"; ERRORS=$((ERRORS+1))
else
  echo "PASS: no ADB runtime binary remains"
fi

if [ -f /usr/share/applications/gemacast-pc.desktop ]; then
  echo "FAIL: Desktop entry still exists"; ERRORS=$((ERRORS+1))
else
  echo "PASS: Desktop entry removed"
fi

if [ -f /usr/lib/firewalld/services/gemacast.xml ]; then
  echo "FAIL: firewalld service definition still exists"; ERRORS=$((ERRORS+1))
else
  echo "PASS: firewalld service definition removed"
fi

if command -v ufw &>/dev/null; then
  ADDED=$(sudo ufw show added 2>/dev/null || true)
  if echo "$ADDED" | grep -q "23555,23556/udp"; then
    echo "FAIL: ufw rules survived uninstall"; ERRORS=$((ERRORS+1))
  else
    echo "PASS: ufw rules removed"
  fi
fi

if [ -f /etc/sysctl.d/99-gemacast.conf ]; then
  echo "FAIL: sysctl drop-in survived uninstall"; ERRORS=$((ERRORS+1))
else
  echo "PASS: sysctl drop-in removed"
fi

if sysctl -n net.ipv4.ip_local_reserved_ports 2>/dev/null | grep -q "23555-23559"; then
  echo "FAIL: 23555-23559 still reserved in the running kernel"; ERRORS=$((ERRORS+1))
else
  echo "PASS: port reservation cleared"
fi

if [ $ERRORS -gt 0 ]; then
  echo "=== $ERRORS uninstall check(s) failed ==="
  exit 1
fi
echo "All .deb uninstall checks passed"
