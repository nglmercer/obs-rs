# Windows support plan

The build/runtime checklist in this document is historical scaffolding. The
authoritative verification baseline for full parity is
[`10-windows-full-compatibility.md`](10-windows-full-compatibility.md), which
keeps build, discovery, runtime behavior, and real Windows E2E acceptance as
separate states.

This is the checklist for bringing the Windows host to parity with the
Linux/X11 vertical slice: a green build and test suite without any Unix
dependency, native capture and audio adapters behind the existing Rust
interfaces, and CI coverage. It follows the same rules as the rest of the
repository: no C/C++ sources in-tree, no `unsafe`, and platform integrations
only through typed Rust boundaries.

## Current state (Phase 0, done on this branch)

- [x] Build the whole workspace on Windows with no `pkg-config`, GStreamer,
  or MSYS2 requirement. The native GStreamer adapter stays opt-in via the
  existing `production-gstreamer` feature; on Windows the GUI now compiles
  against the pure-Rust capability model (`obs-rs-output-gstreamer` without
  its `native` feature), which already reports reference-only capabilities.
  Files: `crates/obs-rs-gui/Cargo.toml`,
  `crates/obs-rs-engine/Cargo.toml`, `crates/obs-rs-engine/src/lib.rs`.
- [x] Import `PlatformCaptureAdapter` at the macOS/Windows call sites in the
  built-in plugin. Files: `crates/obs-rs-builtins/src/lib.rs`.
- [x] Compile the Linux-only live-check binary as a typed skip stub on other
  platforms. Files: `crates/obs-rs-app/src/bin/obs-rs-linux-check.rs`.
- [x] Gate the Wayland portal picker path to Linux where it belongs.
  Files: `crates/obs-rs-gui/src/callbacks/monitor.rs`,
  `crates/obs-rs-gui/src/fixtures.rs`, `crates/obs-rs-gui/src/main.rs`.
- [x] Close file handles before atomic publish renames; Windows refuses to
  rename an open file or a directory containing one.
  Files: `crates/obs-rs-update/src/lib.rs`.
- [x] Make GUI tests host-neutral: separator-safe recording-path assertion, a
  reconnect-failure wait budget that covers slow loopback refusals on Windows,
  and the X11 display-picker exercise gated to Linux until the native picker
  exists. Files: `crates/obs-rs-gui/src/settings.rs`,
  `crates/obs-rs-gui/src/tests.rs`.
- [x] Acceptance: `cargo check --workspace`, `cargo test --workspace`, and
  `cargo run` all pass on `x86_64-pc-windows-msvc`.

## Phase 1 - CI parity

- [x] Add a `windows-latest` job running `cargo fmt --all -- --check`,
  `cargo check --workspace --all-targets`, `cargo clippy --workspace
  --all-targets -- -D warnings`, and `cargo test --workspace`. Keep
  `--all-features` jobs Linux-only for now; `--all-features` implies the
  native GStreamer adapter, which needs the MSVC runtime installed.
- [x] Run the Slint GUI smoke test (`cargo run -p obs-rs-gui -- --smoke`)
  on the Windows job once it does not depend on X11-only fixtures.
- [x] Fix or explicitly allow the remaining Windows-only warnings in
  `obs-rs-capture` (`provider.rs` unused imports) and `obs-rs-engine`
  (unused `frame` parameter on the non-GStreamer path) so `-D warnings`
  passes.

## Phase 2 - Runtime parity

- [x] Audio input: add `obs-rs-audio-wasapi` implementing the existing
  `AudioInputProvider`/`AudioInput` contracts over WASAPI (shared mode,
  pull model via event handles is acceptable). Discovery reports stable
  device IDs; failure types map onto `AudioDeviceError`. The deterministic
  fallback provider stays the default until the adapter is injected.
- [x] Screen/window capture: implement the `obs-rs-capture-windows-helper`
  executable that `WindowsCaptureAdapter` already launches and speaks to
  (`OBSRWIN1` protocol): Windows Graphics Capture for screens/windows,
  writing bounded `OBSFRM01` RGBA packets to stdout. Cameras use the separate
  Nokhwa Media Foundation path in the main workspace; they are not routed
  through this helper. The helper lives outside the workspace (it owns the
  COM/D3D boundary) and ships as a separate binary; the repository keeps its
  no-native-source rule.
- [x] Display picker: give the monitor window a Windows backend that lists
  displays from the capture helper's discovery, then un-gate the
  `exercise_monitor_selection` GUI test for Windows.
- [x] Production output (optional): document installing GStreamer MSVC
  runtime + development packages and expose the `production-gstreamer`
  feature on Windows builds that want RTMP/SRT/WebRTC/HLS output. The
  reference packet outputs work everywhere without it.

## Phase 3 - Application-level polish

- [x] Settings: default recording directory and helper lookup paths use
  `%APPDATA%`/`%LOCALAPPDATA%` instead of `/tmp`-style defaults.
- [x] Packaging: an MSI or portable zip layout that places the capture
  helper next to the GUI executable and registers the uninstall entry.
- [x] Diagnostics: include Windows version, GPU adapter name, and capture
  helper version in the bounded diagnostics bundle.
- [ ] Acceptance: the GUI runs a real screen capture session end to end on
  Windows, records an `OBSRPKT1` file from it, and the full test suite
  passes in CI on both platforms. The repeatable Windows hardware lane is now
  defined in `.github/workflows/hardware-soak.yml`; this checkbox remains open
  until its archived run and production-output acceptance pass.

The real Windows desktop capture portion was exercised locally: the helper
captured the physical display and the engine committed a valid `OBSRPKT1`
file. The remaining acceptance dependency is the archived Windows hardware
run on both supported OS generations, including production recording and
streaming.
