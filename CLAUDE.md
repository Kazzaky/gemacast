# Development constraints

Repository instructions for changes to Gemacast. Architecture is documented in
[ARCHITECTURE.md](ARCHITECTURE.md); setup and checks are in
[CONTRIBUTING.md](CONTRIBUTING.md).

## Source references

| Contract | Definition |
| --- | --- |
| Rust toolchain and workspace | `rust-toolchain.toml`, root `Cargo.toml` |
| Network ports | `gemacast-core/src/network/ports.rs` |
| Audio constants and PCM chunks | `gemacast-core/src/audio/` |
| Control messages and request types | `gemacast-core/src/control/messages.rs`, `types.rs` |
| Mobile commands and events | `gemacast-mobile/src/core/tauri-bridge.ts`, `controllers/app-events-controller.ts` |
| Build and release checks | `.github/workflows/ci.yml`, `prepare-release.yml`, `release.yml` |

## Module boundaries

- Core ports belong in `gemacast-core/src/ports/`; application ports belong in
  each app's `traits/` directory. Concrete implementations belong in `adapters/`
  and are wired at composition roots.
- Platform audio dependencies belong to `gemacast-core`. App crates use its
  facades rather than importing platform audio libraries directly.
- PC UI/background communication uses `TrayEvent` and `AppCommand` from
  `gemacast-pc/src/events.rs`. Route native dialogs through
  `gemacast-pc/src/dialog.rs`.
- Mobile Tauri commands delegate to services. Platform-specific Rust/JNI code
  belongs under `gemacast-mobile/src-tauri/src/services/platform/`.
- React controllers own workflows and side effects. Stores hold state;
  components render it. `tauri-bridge.ts` is the frontend `invoke()` boundary;
  `app-events-controller.ts` owns Tauri event subscriptions.
- Restrict `useEffect` to external lifecycle synchronization, with cleanup.
- Core errors use `thiserror` and `gemacast-core/src/domain/error.rs`. Preserve
  error categories and causes until conversion at a UI, IPC, or logging boundary.
- A task's owner handles cancellation, replacement, timeout, joining, and cleanup.
  Reconnect code must reject stale task generations.
- Refactors preserve behavior. Update all Rust, TypeScript, Kotlin, and test
  consumers when changing a shared API; preserve unrelated worktree edits.

## Protocol and authentication

- Preserve port numbers, packet layouts, serialized names, Tauri command/event
  names, retries, and shutdown order unless the change explicitly targets them.
  Protocol-breaking changes require both PC and phone updates.
- Mutating control routes use `authenticate_device`, binding the bearer token to
  the request's `device_id`. `/sources` and `/processes` use `authenticate_token`.
- Session tokens remain memory-only and rotate on connection. Preserve
  `SessionGeneration` checks during request, socket, and WebSocket cleanup.
- Persist trust only after approval. A changed PC certificate requires the user
  to forget that identity; a changed phone key requires re-pair approval.
- Changes to dispatcher error text must update `connect_error_code` mappings
  and their tests.
- Updater SHA-256 verification rejects missing or malformed digests. Detached
  GPG signatures are not verified by the application.

## Audio

- Audio callbacks and jitter hot paths must not allocate, block, perform I/O,
  or emit burst logging.
- Preserve 48 kHz stereo alignment: a 10 ms frame contains 480 samples per
  channel, or 960 interleaved samples.
- Capture may be shared by source; each phone retains its own encoder, bitrate,
  and session identity.
- Send each non-silent UDP PCM frame as four 978-byte datagrams in one batch on
  the 10 ms frame clock. Reassemble all four before jitter playback.
- Preserve the 40 ms UDP send-age limit and 100 ms incomplete-assembly expiry,
  including during silence and receive timeouts. Preserve sender generations.
  Layouts are in [Audio wire format](ARCHITECTURE.md#audio-wire-format).
- Keep jitter target calculation separate from time-scale adjustment. Adaptive
  statistics must decay or expire; target selection must not retain unbounded peaks.
- Preserve separate admission rules for acceleration, preemptive expansion,
  and concealment expansion. Concealment must produce replacement audio.
- Reset decoder packet-loss-concealment state on raw PCM, silence, resync,
  reset, and format transitions. It must describe the last played codec frame.
- Crossfades progress monotonically from outgoing to incoming audio.
- Preserve diagnostic counter semantics. Document the measurement supporting
  tuning constants and change one audio behavior per measurement round.
- Latency, jitter, and concealment changes require real-device captures over
  Wi-Fi and USB tethering in addition to unit tests.

## Android

- `gemacast-mobile/src-tauri/gen/android/` contains tracked, hand-written Kotlin.
  Do not delete or wholesale-regenerate it.
- Keep Android state transitions in JVM-testable reducers, with tests under
  `app/src/test/`.
- Kotlin methods called dynamically through JNI require explicit R8 keep rules
  in `proguard-rules.pro` and validation with a release APK.
- Keep generated package/library settings consistent with `tauri.conf.json` and
  the Rust library target. Native checks require the
  [Android build environment](CONTRIBUTING.md#android).
- Android Rust tracing uses the `tracing/log` bridge and `tauri-plugin-log`;
  do not install a second tracing subscriber.
- Shell scripts and `gradlew` retain LF line endings.

## Validation

- Run focused regression tests, the owning suite, and affected native-target
  checks from [CONTRIBUTING.md](CONTRIBUTING.md#validate).
- Pairing/session changes require Wi-Fi and USB-tethering coverage. JNI changes
  require a release APK; JVM tests do not exercise R8.
- Inject time in timing tests. Tests sharing PipeWire use `#[serial(pipewire)]`.
  Shared mocks belong in each crate's `testing.rs` under `#[cfg(test)]`.
- Check regressions against the prior behavior. Rate-limit tests must also
  establish that the operation ran.
- Run `git diff --check`. Report untested platforms and distinguish automated
  checks from device measurements.

## Dependencies and configuration

- Keep `.cargo/audit.toml` and CI's RustSec ignore list synchronized. Audit
  exceptions require documented dependency reachability.
- Preserve Tauri IPC endpoints in `connect-src` and the Vite HMR WebSocket in
  development CSP. Validate CSP changes in the native app.
- The PC macOS bundle uses `macos/Info.plist`; Screen Recording and Microphone
  permissions are separate. Preserve the Swift runtime linker configuration.
- Version metadata is synchronized manually across the root `Cargo.toml`,
  `gemacast-mobile/package.json`, `gemacast-mobile/src-tauri/tauri.conf.json`,
  and `version.txt`. CI and release workflows are manual-only; a release run
  consumes an existing version tag.
