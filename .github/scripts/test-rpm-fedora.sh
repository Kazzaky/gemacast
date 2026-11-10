#!/usr/bin/env bash
# Full RPM test in Fedora container: install → verify → smoke test → uninstall → verify.
# Expects env var: RPM_DIR (path containing the .rpm files)
set -euo pipefail

RPM_DIR="${RPM_DIR:-rpm_download}"

# Install necessary test tools, appindicator libs, pipewire, and android-tools
dnf update -y
dnf install -y --setopt=strict=0 which xorg-x11-server-Xvfb xdotool zenity file \
  libappindicator-gtk3 libayatana-appindicator pipewire pipewire-devel

# ── Install ────────────────────────────────────────────────────────────────
echo "=== Installing RPM ==="
dnf install -y "${RPM_DIR}"/*.rpm

# ── Verify ─────────────────────────────────────────────────────────────────
echo ""
echo "=== Verifying RPM Installation ==="
ERRORS=0

# Verify binary on PATH
if command -v gemacast-pc &>/dev/null; then
  echo "PASS: gemacast-pc is on PATH at $(which gemacast-pc)"
else
  echo "FAIL: gemacast-pc not found on PATH"; ERRORS=$((ERRORS+1))
fi

# Verify the RPM has no legacy ADB runtime dependency.
if [ -e /usr/lib/gemacast/adb ]; then
  echo "FAIL: legacy ADB runtime binary was installed"; ERRORS=$((ERRORS+1))
else
  echo "PASS: no ADB runtime binary installed"
fi

# Proof the scriptlet body actually ran, not just that alien carried its text.
# rpm invokes %post with an install count in $1, never dpkg's "configure", so a
# guard that only matches "configure" silently skips the whole firewall setup.
# The sysctl drop-in is the cheapest observable the body writes; /proc/sys is
# read-only in this container, so check the file rather than the live value.
if [ -f /etc/sysctl.d/99-gemacast.conf ]; then
  echo "PASS: postinst scriptlet ran"
else
  echo "FAIL: postinst scriptlet did not run - check its \$1 guard"; ERRORS=$((ERRORS+1))
fi

# Smoke test under xvfb
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
  echo "=== $ERRORS check(s) failed ==="
  exit 1
fi
echo ""
echo "All Fedora RPM installation checks passed"

# ── Uninstall ──────────────────────────────────────────────────────────────
echo ""
echo "=== Uninstalling RPM ==="
dnf remove -y gemacast-pc

# ── Verify Uninstall ──────────────────────────────────────────────────────
echo ""
echo "=== Verifying RPM Uninstall ==="
ERRORS=0
if command -v gemacast-pc &>/dev/null; then
  echo "FAIL: gemacast-pc still on PATH after uninstall"; ERRORS=$((ERRORS+1))
else
  echo "PASS: gemacast-pc removed from PATH"
fi

if [ -f /etc/sysctl.d/99-gemacast.conf ]; then
  echo "FAIL: sysctl drop-in survived uninstall"; ERRORS=$((ERRORS+1))
else
  echo "PASS: postrm scriptlet ran"
fi

if [ $ERRORS -gt 0 ]; then exit 1; fi
echo "All Fedora RPM uninstall checks passed"
