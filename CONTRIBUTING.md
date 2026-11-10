# Contributing to Gemacast

[Architecture](ARCHITECTURE.md) · [Repository rules](CLAUDE.md) ·
[Website](web/README.md) · [Container checks](docker/README.md)

## Toolchain

| Component | Version or requirement |
| --- | --- |
| Rust, rustfmt, Clippy | [rust-toolchain.toml](rust-toolchain.toml) |
| Bun | CI uses 1.3.14 |
| Native builds | Clang and CMake; Android also uses Ninja |
| Android Java | JDK 17 |
| Android NDK | 25.2.9519653 |
| Android CMake | 3.22.1 |
| Android build-tools | 34.0.0 |
| Android SDK platform | 36 |
| Website Node.js | `^22.18.0` or `>=24.12.0` |

Android versions and target checks are defined in [CI](.github/workflows/ci.yml).
Frontend commands are defined in each application's `package.json`.

## Setup

```bash
git clone https://github.com/Kazzaky/gemacast.git
cd gemacast
```

Install dependencies for the frontend being developed:

| Working directory | Command |
| --- | --- |
| `gemacast-mobile/` | `bun install --frozen-lockfile` |
| `web/` | `bun install --frozen-lockfile` |

### Desktop dependencies

| Host | Requirements |
| --- | --- |
| Windows | MSVC C++ build tools, Windows SDK, Clang, CMake; WebView2 for the Tauri desktop UI |
| macOS | Xcode command-line tools and CMake; ScreenCaptureKit uses a Swift bridge |
| Linux | PipeWire, WirePlumber, ALSA, GTK3, AppIndicator, WebKit2GTK, and their development packages |

Ubuntu 22.04's PipeWire headers are too old for the current Rust bindings.
CI builds PipeWire 1.2.7 and WirePlumber 0.5.15 from source. Use the
[CI setup](.github/workflows/ci.yml) or [Linux container](docker/README.md).
Linux workspace checks also require the Tauri libraries listed in that workflow.

### Android SDK

Install the Android SDK packages listed above. USB transport uses Android USB tethering; USB debugging and ADB are not required by the app. Configure:

| Variable | Value |
| --- | --- |
| `JAVA_HOME` | JDK 17 directory |
| `ANDROID_HOME` | Android SDK directory |
| `NDK_HOME`, `ANDROID_NDK_HOME`, `ANDROID_NDK_ROOT` | SDK directory followed by `ndk/25.2.9519653` |

Accept the Android SDK licenses. From the repository root:

```bash
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
```

ARM64-only builds require only `aarch64-linux-android`. For USB transport testing,
enable **USB tethering** in Android settings and confirm that the PC and phone
receive addresses on the tethered network. USB debugging is not part of the
connection protocol.

## Build

| Target | Working directory | Command |
| --- | --- | --- |
| PC development run | Repository root | `cargo run -p gemacast-pc` |
| PC release binary | Repository root | `cargo build --release -p gemacast-pc` |
| Android development run | `gemacast-mobile/` | `bunx tauri android dev` |
| Android debug APK, ARM64 | `gemacast-mobile/` | `bunx tauri android build --debug --apk --target aarch64` |
| Android universal APK | `gemacast-mobile/` | `bunx tauri android build --apk` |
| Android ARM64 APK | `gemacast-mobile/` | `bunx tauri android build --apk --target aarch64 --split-per-abi` |
| Mobile frontend only | `gemacast-mobile/` | `bun run dev` |
| Website development | `web/` | `bun run dev` |
| Website production build | `web/` | `bun run build` |

PC release output is `target/release/gemacast-pc` (`.exe` on Windows).
APK output is under `gemacast-mobile/src-tauri/gen/android/app/build/outputs/apk/`.
The mobile Vite server alone provides no native audio or Tauri services.
The tracked Android project contains hand-written Kotlin; retain it when building.

## Validate

### Local CI runner

Run the local counterpart of the CI workflow from the repository root. It checks the pinned dependency audit, mobile and website production builds, release-script syntax, Rust formatting, Clippy, a host build (including the PC release binary), and the full test suite for the current host.

Python 3.10 or newer is required.

Windows (PowerShell):

```powershell
py -3 scripts/ci-local.py
```

Linux and macOS:

```bash
python3 scripts/ci-local.py
```

Add `--android` to also run the Android ARM64 Clippy check and Android JVM unit tests. The Android SDK, JDK 17, NDK 25.2.9519653, and `cargo-ndk` must already be installed.

Optional switches: `--skip-audit`, `--skip-frontend`, and `--skip-rust`. They are useful for focused iterations; use the default command before committing.

The script validates only the operating system on which it is run; it cannot replace the Windows/macOS/Linux CI matrix or installer install/uninstall smoke tests. It syntax-checks all release shell scripts when Bash is available and PowerShell scripts when PowerShell is available. On Linux, it expects the PipeWire 1.2.7 and WirePlumber 0.5.15 development/runtime dependencies described above to be installed.

### Rust

From the repository root:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Use `cargo test -p gemacast-core <test-filter>` for a filtered core test run.
Linux workspace tests require running PipeWire and WirePlumber services.
Host workspace checks compile the mobile crate's non-Android target.

### Mobile frontend

From `gemacast-mobile/`:

```bash
bun run format:check
bun run lint
bun run typecheck
bun test
```

### Android

Direct Cargo checks require the following variables to generate ignored
Gradle and Kotlin support files. Run from the repository root after SDK setup.

Bash:

```bash
export TAURI_ANDROID_PROJECT_PATH="$PWD/gemacast-mobile/src-tauri/gen/android"
export WRY_ANDROID_PACKAGE="com.apir.gemacast"
export WRY_ANDROID_LIBRARY="gemacast_mobile_lib"
export WRY_ANDROID_KOTLIN_FILES_OUT_DIR="$TAURI_ANDROID_PROJECT_PATH/app/src/main/java/com/apir/gemacast/generated"
mkdir -p "$WRY_ANDROID_KOTLIN_FILES_OUT_DIR"
cargo ndk -t arm64-v8a clippy --workspace --exclude gemacast-pc -- -D warnings
```

PowerShell:

```powershell
$env:TAURI_ANDROID_PROJECT_PATH = (Resolve-Path 'gemacast-mobile/src-tauri/gen/android').Path
$env:WRY_ANDROID_PACKAGE = 'com.apir.gemacast'
$env:WRY_ANDROID_LIBRARY = 'gemacast_mobile_lib'
$env:WRY_ANDROID_KOTLIN_FILES_OUT_DIR = Join-Path $env:TAURI_ANDROID_PROJECT_PATH 'app/src/main/java/com/apir/gemacast/generated'
New-Item -ItemType Directory -Force -Path $env:WRY_ANDROID_KOTLIN_FILES_OUT_DIR | Out-Null
cargo ndk -t arm64-v8a clippy --workspace --exclude gemacast-pc -- -D warnings
```

Then run JVM tests from `gemacast-mobile/src-tauri/gen/android/`:

| Shell | Command |
| --- | --- |
| Bash | `bash ./gradlew :app:testUniversalDebugUnitTest --no-daemon` |
| PowerShell | `./gradlew.bat :app:testUniversalDebugUnitTest --no-daemon` |

JVM tests cover Kotlin logic. They do not exercise device audio or release R8
minification. JNI changes require a release APK check. Shell scripts and
`gradlew` use LF line endings.

### Website and containers

Website commands are listed in [web/README.md](web/README.md). The CI frontend
job performs a locked dependency install and production build for the website;
there are currently no website unit-test files.

Linux and Android container commands are in [docker/README.md](docker/README.md).
Windows and macOS checks run on native CI hosts.

## Diagnostics

PC debug builds write tracing to stderr. From the repository root:

```bash
RUST_LOG=gemacast_core=debug,gemacast_pc=debug cargo run -p gemacast-pc
```

PowerShell:

```powershell
$env:RUST_LOG = 'gemacast_core=debug,gemacast_pc=debug'
cargo run -p gemacast-pc
```

PC and Android release builds compile out Rust tracing. Use a debug APK when
collecting Android diagnostic logs; release builds intentionally omit tracing.

PC panic reports overwrite `crash.log` in the configuration directory:

| OS | Directory |
| --- | --- |
| Windows | `%APPDATA%/gemacast/` |
| Linux | `$XDG_CONFIG_HOME/gemacast/`, defaulting to `~/.config/gemacast/` |
| macOS | `~/Library/Application Support/gemacast/` |

## Reporting bugs

Include these fields in an [issue](https://github.com/Kazzaky/gemacast/issues):

- PC and Android app versions or source commit.
- PC OS and audio device; phone model and Android version.
- Connection mode, capture source, codec/bitrate, buffer preset, and screen state.
- Reproduction steps, expected result, actual result, and relevant logs.

Audio measurements also need the measurement method and affected transport.
RTT and buffer depth do not measure total capture-to-output latency. Remove
credentials and private identity material from diagnostic attachments.

## Pull requests

Target `main`. Include the change, test commands and results, and affected
platforms. Protocol changes require matching PC and phone changes. Repository
constraints are in [CLAUDE.md](CLAUDE.md).

The CI workflow (`.github/workflows/ci.yml`) is manual-only and is not
automatically triggered by pushes or pull requests. Run it from the GitHub
Actions page with **Run workflow**, or use `scripts/ci-local.py` locally.

The release workflow (`.github/workflows/release.yml`) is also manual-only.
It builds and publishes an **existing version tag** supplied through
`workflow_dispatch`; it does not create a release tag automatically. Before
publishing, configure the repository secrets
`ANDROID_KEYSTORE_BASE64`, `ANDROID_KEY_ALIAS`,
`ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_PASSWORD`,
`GPG_PRIVATE_KEY`, `GPG_PASSPHRASE`, and the repository variable
`GPG_SIGNING_KEY_FINGERPRINT`.
