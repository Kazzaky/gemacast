#!/usr/bin/env python3
"""Run the repository's CI checks locally on the current host."""

from __future__ import annotations

import argparse
import json
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
cargo test --workspace --locked
if wpctl get-volume @DEFAULT_AUDIO_SINK@ >/dev/null 2>&1; then
  export GEMACAST_EXPECT_OS_VOLUME=1
fi
cargo test -p gemacast-core output_volume --locked -- --nocapture
cargo test -p gemacast-pc volume_watcher --locked -- --nocapture
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

    run("Cargo workspace tests", ["cargo", "test", "--workspace", "--locked"])

    volume_env: dict[str, str] = {}
    if system == "Windows":
        run(
            "Windows output-volume tests (core)",
            ["cargo", "test", "-p", "gemacast-core", "output_volume", "--locked", "--", "--nocapture"],
        )
        run(
            "Windows volume-watcher tests (PC)",
            ["cargo", "test", "-p", "gemacast-pc", "volume_watcher", "--locked", "--", "--nocapture"],
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
            ["cargo", "test", "-p", "gemacast-core", "output_volume", "--locked", "--", "--nocapture"],
            env=volume_env,
        )
        run(
            "macOS volume-watcher tests (PC)",
            ["cargo", "test", "-p", "gemacast-pc", "volume_watcher", "--locked", "--", "--nocapture"],
            env=volume_env,
        )


def verify_version_sync() -> None:
    cargo_text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    cargo_match = re.search(
        r'(?ms)^\[workspace\.package\]\s+.*?^version\s*=\s*"([^"]+)"',
        cargo_text,
    )
    if not cargo_match:
        raise ValidationError("Could not read the workspace version from Cargo.toml.")
    versions = {
        "Cargo.toml": cargo_match.group(1),
        "version.txt": (ROOT / "version.txt").read_text(encoding="utf-8").strip(),
        "gemacast-mobile/package.json": json.loads(
            (ROOT / "gemacast-mobile" / "package.json").read_text(encoding="utf-8")
        )["version"],
        "gemacast-mobile/src-tauri/tauri.conf.json": json.loads(
            (ROOT / "gemacast-mobile" / "src-tauri" / "tauri.conf.json").read_text(
                encoding="utf-8"
            )
        )["version"],
    }
    unique_versions = set(versions.values())
    if len(unique_versions) != 1 or not next(iter(unique_versions), ""):
        details = ", ".join(f"{name}={version!r}" for name, version in versions.items())
        raise ValidationError(f"Application versions are out of sync: {details}")

    print_step(f"Version metadata is synchronized (v{next(iter(unique_versions))})")


def run_audit() -> None:
    lock_path = ROOT / "Cargo.lock"
    lock_text = lock_path.read_text(encoding="utf-8")
    if GLIB_LOCK_SOURCE not in lock_text:
        raise ValidationError(
            "Cargo.lock does not contain the pinned glib security-backport revision "
            f"{GLIB_REV}. Check the lockfile before running the dependency audit."
        )
    require_command("cargo")
    require_command(
        "cargo-audit",
        "Install the local audit subcommand with: cargo install cargo-audit --locked",
    )
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
    run("Frontend production build", [bun, "run", "build"], cwd=mobile_dir)
    run("Bun unit tests", [bun, "test"], cwd=mobile_dir)

    website_dir = ROOT / "web"
    run("Install website dependencies", [bun, "install", "--frozen-lockfile"], cwd=website_dir)
    run("Website format check", [bun, "run", "format:check"], cwd=website_dir)
    run("Website lint (read-only)", [bun, "run", "lint:check"], cwd=website_dir)
    run("Website production build (type-check + Vite)", [bun, "run", "build"], cwd=website_dir)


def run_script_syntax_checks() -> None:
    scripts_dir = ROOT / ".github" / "scripts"
    shell_scripts = sorted(scripts_dir.glob("*.sh"))
    bash = shutil.which("bash")
    if not bash and platform.system() == "Windows":
        git_executable = shutil.which("git")
        if git_executable:
            git_path = Path(git_executable).resolve()
            for parent in git_path.parents:
                candidates = (
                    parent / "bin" / "bash.exe",
                    parent / "usr" / "bin" / "bash.exe",
                )
                bash = next((str(candidate) for candidate in candidates if candidate.is_file()), None)
                if bash:
                    break
    if shell_scripts and not bash:
        raise ValidationError(
            "Bash is required to syntax-check .github/scripts/*.sh. "
            "On Windows, install Git for Windows (including Git Bash)."
        )
    if shell_scripts:
        for script in shell_scripts:
            run(
                f"Shell syntax: {script.relative_to(ROOT)}",
                [bash, "-n", str(script)],
            )

    powershell_scripts = sorted(scripts_dir.glob("*.ps1"))
    powershell = shutil.which("pwsh") or shutil.which("powershell")
    if powershell and powershell_scripts:
        parser_command = (
            "$ErrorActionPreference = 'Stop'; "
            "Get-ChildItem -LiteralPath '.github/scripts' -Filter '*.ps1' | "
            "ForEach-Object { "
            "$tokens = $null; $parseErrors = $null; "
            "[System.Management.Automation.Language.Parser]::ParseFile("
            "$_.FullName, [ref]$tokens, [ref]$parseErrors) | Out-Null; "
            "if ($parseErrors.Count -gt 0) { "
            "$parseErrors | ForEach-Object { Write-Error $_ }; exit 1 "
            "} }"
        )
        run(
            "PowerShell release-script syntax",
            [powershell, "-NoProfile", "-NonInteractive", "-Command", parser_command],
        )
    elif powershell_scripts and platform.system() == "Windows":
        raise ValidationError("PowerShell is required to syntax-check .github/scripts/*.ps1.")


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
        ["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
    )
    run("Build workspace (host target)", ["cargo", "build", "--workspace", "--locked"])
    run(
        "PC release build",
        ["cargo", "build", "--release", "-p", "gemacast-pc", "--locked"],
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
    required_sdk_components = {
        "CMake 3.22.1": Path(sdk) / "cmake" / "3.22.1",
        "Android Build Tools 34.0.0": Path(sdk) / "build-tools" / "34.0.0",
        "Android Platform 36": Path(sdk) / "platforms" / "android-36",
    }
    missing_components = [
        name for name, path in required_sdk_components.items() if not path.is_dir()
    ]
    if missing_components:
        raise ValidationError(
            "Missing Android SDK components: "
            + ", ".join(missing_components)
            + ". Install the pinned versions documented in CONTRIBUTING.md."
        )

    bun = require_command("bun", "Install Bun 1.3.14 before building the Android APKs.")
    bun_version = subprocess.run(
        [bun, "--version"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if bun_version.returncode != 0 or bun_version.stdout.strip() != BUN_VERSION:
        raise ValidationError(
            f"Android packaging requires Bun {BUN_VERSION}; found "
            f"{bun_version.stdout.strip() or 'unknown'}."
        )

    java = require_command("java", "Install JDK 17 and configure JAVA_HOME.")
    java_version = subprocess.run(
        [java, "-version"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    java_output = java_version.stdout + java_version.stderr
    java_match = re.search(r'(?:java|openjdk)(?: version)?\s+"?(?:1\.)?(\d+)', java_output, re.IGNORECASE)
    if java_version.returncode != 0 or not java_match or java_match.group(1) != "17":
        raise ValidationError(
            "Android checks require JDK 17; detected "
            f"{java_output.strip() or 'an unknown Java version'}."
        )
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
        "BUILD_TOOLS_VERSION": "34.0.0",
    }
    if "JAVA_HOME" not in os.environ:
        java_home = Path(java).resolve().parent.parent
        android_env["JAVA_HOME"] = str(java_home)

    run(
        "Android Clippy (ARM64)",
        [
            "cargo", "ndk", "-t", "arm64-v8a", "clippy", "--workspace",
            "--exclude", "gemacast-pc", "--locked", "--", "-D", "warnings",
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

    mobile_dir = ROOT / "gemacast-mobile"
    run(
        "Android universal release APK build",
        ["bun", "x", "tauri", "android", "build", "--apk"],
        cwd=mobile_dir,
        env=android_env,
    )
    run(
        "Android ARM64 split release APK build",
        [
            "bun", "x", "tauri", "android", "build", "--apk",
            "--target", "aarch64", "--split-per-abi",
        ],
        cwd=mobile_dir,
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
        help="Also run Android ARM64 Clippy, JVM unit tests, and universal plus ARM64 release APK builds.",
    )
    args = parser.parse_args()

    print("Gemacast local CI validation")
    print(f"Repository: {ROOT}")
    print(f"Host: {platform.platform()}")
    print("Note: this validates only the current host, not the full Windows/macOS/Linux matrix.")

    try:
        require_command("git")
        run("Git whitespace check", ["git", "diff", "--check", "HEAD"])
        verify_version_sync()
        run_script_syntax_checks()
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
