#!/usr/bin/env python3
"""Run the repository's CI checks locally on the current host."""

from __future__ import annotations

import argparse
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Mapping, Sequence

ROOT = Path(__file__).resolve().parents[1]
BUN_VERSION = "1.3.14"
GLIB_REV = "ea720152f28e293ef4362ee844ee5cc499f32d2a"
GLIB_LOCK_SOURCE = (
    "git+https://github.com/jcfs/gtk-rs-core?rev="
    f"{GLIB_REV}#{GLIB_REV}"
)
ANDROID_TARGETS = (
    "aarch64-linux-android",
    "armv7-linux-androideabi",
    "i686-linux-android",
    "x86_64-linux-android",
)


class ValidationError(RuntimeError):
    pass


def print_step(title: str) -> None:
    print(f"\n{'=' * 12} {title} {'=' * 12}", flush=True)


def require_command(name: str, install_hint: str | None = None) -> str:
    resolved = shutil.which(name)
    if not resolved:
        detail = f"\n{install_hint}" if install_hint else ""
        raise ValidationError(f"Required command not found: {name}.{detail}")
    return resolved


def run(
    title: str,
    command: Sequence[str],
    *,
    cwd: Path = ROOT,
    env: Mapping[str, str] | None = None,
) -> None:
    print_step(title)
    print("$ " + subprocess.list2cmdline(list(command)), flush=True)
    child_env = os.environ.copy()
    if env:
        child_env.update(env)
    try:
        completed = subprocess.run(list(command), cwd=cwd, env=child_env, check=False)
    except OSError as exc:
        raise ValidationError(f"Could not start {command[0]}: {exc}") from exc
    if completed.returncode:
        raise ValidationError(
            f"Step failed with exit code {completed.returncode}: {title}"
        )


def run_linux_rust_tests() -> None:
    for command in ("dbus-run-session", "pipewire", "wireplumber", "wpctl"):
        require_command(command)

    runtime_dir = Path(tempfile.mkdtemp(prefix="gemacast-runtime-"))
    runtime_dir.chmod(0o700)
    shell_script = r'''set -eu
PIPEWIRE_LOG_LEVEL=2 pipewire 2>/dev/null &
pw_pid=$!
wp_pid=
cleanup() {
  if [ -n "$wp_pid" ]; then kill "$wp_pid" 2>/dev/null || true; fi
  kill "$pw_pid" 2>/dev/null || true
  wait "$pw_pid" 2>/dev/null || true
}
trap cleanup EXIT
sleep 2
kill -0 "$pw_pid"
PIPEWIRE_LOG_LEVEL=2 wireplumber 2>/dev/null &
wp_pid=$!
sleep 2
kill -0 "$wp_pid"
cargo test --workspace
if wpctl get-volume @DEFAULT_AUDIO_SINK@ >/dev/null 2>&1; then
  export GEMACAST_EXPECT_OS_VOLUME=1
fi
cargo test -p gemacast-core output_volume -- --nocapture
cargo test -p gemacast-pc volume_watcher -- --nocapture
'''
    try:
        run(
            "Rust tests with PipeWire + WirePlumber (Linux)",
            ["dbus-run-session", "--", "sh", "-c", shell_script],
            env={"XDG_RUNTIME_DIR": str(runtime_dir), "PIPEWIRE_LOG_LEVEL": "2"},
        )
    finally:
        shutil.rmtree(runtime_dir, ignore_errors=True)


def run_host_rust_tests() -> None:
    system = platform.system()
    if system == "Linux":
        run_linux_rust_tests()
        return

    run("Cargo workspace tests", ["cargo", "test", "--workspace"])

    volume_env: dict[str, str] = {}
    if system == "Windows":
        run(
            "Windows output-volume tests (core)",
            ["cargo", "test", "-p", "gemacast-core", "output_volume", "--", "--nocapture"],
        )
        run(
            "Windows volume-watcher tests (PC)",
            ["cargo", "test", "-p", "gemacast-pc", "volume_watcher", "--", "--nocapture"],
        )
    elif system == "Darwin":
        osascript = shutil.which("osascript")
        if osascript:
            probe = subprocess.run(
                [osascript, "-e", "output volume of (get volume settings)"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            if probe.returncode == 0:
                volume_env["GEMACAST_EXPECT_OS_VOLUME"] = "1"
        run(
            "macOS output-volume tests (core)",
            ["cargo", "test", "-p", "gemacast-core", "output_volume", "--", "--nocapture"],
            env=volume_env,
        )
        run(
            "macOS volume-watcher tests (PC)",
            ["cargo", "test", "-p", "gemacast-pc", "volume_watcher", "--", "--nocapture"],
            env=volume_env,
        )


def run_audit() -> None:
    lock_path = ROOT / "Cargo.lock"
    lock_text = lock_path.read_text(encoding="utf-8")
    if GLIB_LOCK_SOURCE not in lock_text:
        raise ValidationError(
            "Cargo.lock does not contain the pinned glib security-backport revision "
            f"{GLIB_REV}. Check the lockfile before running the dependency audit."
        )
    require_command("cargo")
    run(
        "Rust dependency audit (uses .cargo/audit.toml)",
        ["cargo", "audit"],
    )


def run_frontend() -> None:
    bun = require_command(
        "bun",
        "Install Bun 1.3.14 (the version pinned by .github/workflows/ci.yml).",
    )
    version = subprocess.run(
        [bun, "--version"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if version.returncode != 0:
        raise ValidationError("Could not determine the installed Bun version.")
    installed = version.stdout.strip()
    if installed != BUN_VERSION:
        raise ValidationError(
            f"Bun {BUN_VERSION} is required by CI, but {installed or 'unknown'} is installed."
        )

    mobile_dir = ROOT / "gemacast-mobile"
    run("Install mobile frontend dependencies", [bun, "install", "--frozen-lockfile"], cwd=mobile_dir)
    run("Prettier format check", [bun, "run", "format:check"], cwd=mobile_dir)
    run("ESLint", [bun, "run", "lint"], cwd=mobile_dir)
    run("TypeScript typecheck", [bun, "run", "typecheck"], cwd=mobile_dir)
    run("Bun unit tests", [bun, "test"], cwd=mobile_dir)


def run_rust_checks() -> None:
    require_command("cargo")
    require_command("rustc")
    version = subprocess.run(
        ["rustc", "--version"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if version.returncode != 0 or not re.search(r"\b1\.97\.1\b", version.stdout):
        actual = version.stdout.strip() or "unknown"
        raise ValidationError(
            f"The repository pins Rust 1.97.1 in rust-toolchain.toml; found {actual}."
        )
    run("Rustfmt", ["cargo", "fmt", "--check"])
    run(
        "Clippy (workspace, all targets, warnings are errors)",
        ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
    )
    run_host_rust_tests()


def run_android_checks() -> None:
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if not sdk:
        raise ValidationError(
            "--android requires ANDROID_HOME or ANDROID_SDK_ROOT to point to the Android SDK."
        )
    ndk = Path(sdk) / "ndk" / "25.2.9519653"
    if not ndk.is_dir():
        raise ValidationError(
            f"Pinned Android NDK was not found at {ndk}. Install NDK 25.2.9519653 first."
        )
    java = require_command("java", "Install JDK 17 and configure JAVA_HOME.")
    cargo_ndk = shutil.which("cargo-ndk")
    if not cargo_ndk:
        raise ValidationError("cargo-ndk is missing. Install it with: cargo install cargo-ndk")

    rustup = require_command("rustup")
    run("Install Android Rust targets", [rustup, "target", "add", *ANDROID_TARGETS])

    android_project = ROOT / "gemacast-mobile" / "src-tauri" / "gen" / "android"
    gradlew = android_project / ("gradlew.bat" if os.name == "nt" else "gradlew")
    if not gradlew.is_file():
        raise ValidationError(f"Gradle wrapper not found: {gradlew}")
    kotlin_output = (
        android_project
        / "app"
        / "src"
        / "main"
        / "java"
        / "com"
        / "apir"
        / "gemacast"
        / "generated"
    )
    kotlin_output.mkdir(parents=True, exist_ok=True)
    android_env = {
        "ANDROID_HOME": sdk,
        "ANDROID_NDK_HOME": str(ndk),
        "ANDROID_NDK_ROOT": str(ndk),
        "NDK_HOME": str(ndk),
        "TAURI_ANDROID_PROJECT_PATH": str(android_project),
        "WRY_ANDROID_PACKAGE": "com.apir.gemacast",
        "WRY_ANDROID_LIBRARY": "gemacast_mobile_lib",
        "WRY_ANDROID_KOTLIN_FILES_OUT_DIR": str(kotlin_output),
    }
    if "JAVA_HOME" not in os.environ:
        java_home = Path(java).resolve().parent.parent
        android_env["JAVA_HOME"] = str(java_home)

    run(
        "Android Clippy (ARM64)",
        [
            "cargo", "ndk", "-t", "arm64-v8a", "clippy", "--workspace",
            "--exclude", "gemacast-pc", "--", "-D", "warnings",
        ],
        env=android_env,
    )
    if os.name == "nt":
        gradle_command = ["cmd", "/c", str(gradlew), ":app:testUniversalDebugUnitTest", "--no-daemon"]
    else:
        gradle_command = ["bash", str(gradlew), ":app:testUniversalDebugUnitTest", "--no-daemon"]
    run(
        "Android JVM unit tests",
        gradle_command,
        cwd=android_project,
        env=android_env,
    )


def main() -> int:
    if sys.version_info < (3, 10):
        print("Python 3.10 or newer is required.", file=sys.stderr)
        return 2

    parser = argparse.ArgumentParser(
        description="Run Gemacast's CI checks locally on the current operating system."
    )
    parser.add_argument("--skip-audit", action="store_true", help="Skip cargo audit.")
    parser.add_argument("--skip-frontend", action="store_true", help="Skip Bun frontend checks.")
    parser.add_argument("--skip-rust", action="store_true", help="Skip host Rust checks and tests.")
    parser.add_argument(
        "--android",
        action="store_true",
        help="Also run the Android ARM64 Clippy check and Android JVM unit tests.",
    )
    args = parser.parse_args()

    print("Gemacast local CI validation")
    print(f"Repository: {ROOT}")
    print(f"Host: {platform.platform()}")
    print("Note: this validates only the current host, not the full Windows/macOS/Linux matrix.")

    try:
        if not args.skip_audit:
            run_audit()
        if not args.skip_frontend:
            run_frontend()
        if not args.skip_rust:
            run_rust_checks()
        if args.android:
            run_android_checks()
    except ValidationError as exc:
        print(f"\nFAILED: {exc}", file=sys.stderr)
        return 1

    print_step("All requested local checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
