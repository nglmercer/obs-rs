# OBS-RS

OBS-RS is a new, Rust-first broadcasting and recording engine inspired by OBS
Studio. It is being built from zero as a standalone project; it is not a line-by-
line translation of the existing OBS implementation.

The repository deliberately has no C or C++ source, headers, ABI shims, generated
bindings, build scripts, or C-based smoke tests. The core crates forbid `unsafe`
code. Platform and media integrations will be added only behind Rust interfaces,
with a separate review when an external dependency is unavoidable.

## What exists today

The current vertical slice is a deterministic engine with headless and desktop
control surfaces:

- `obs-rs-util` provides validated identifiers and small shared value types.
- `obs-rs-config` provides bounded, deterministic settings documents.
- `obs-rs-media` defines timestamps, video formats, packed/planar input buffers,
  RGBA conversion, owned frames, filters, and deterministic transitions. The
  composition primitives carry measured fast paths — an identity transform is a
  copy, unscaled rows move as one memmove, opaque blends skip the alpha
  arithmetic, and division by 255 is an exact multiply-shift — with an ignored
  timing report to re-measure them.
- `obs-rs-audio` defines owned sample buffers, bounded audio queues, a reference
  mixer, a deterministic linear resampler, sample-clock pacing, reconciliation,
  monitoring taps, bounded long-run A/V drift telemetry, and a cancellation-aware
  `AudioWorker` with exact block contracts. Callback timestamp observation rejects
  device-clock regressions and applies bounded ppm correction; mixer peak telemetry
  is available to the desktop state.
- `obs-rs-audio-pipewire` provides the Linux PipeWire process adapter with stable
  `pw-dump` node enumeration, bounded raw `f32` input blocks, an optional output
  sink, and typed discovery/start/read/stop failures.
- `obs-rs-audio-wasapi` provides the Windows shared-mode WASAPI/CPAL adapter for
  microphone input, render-device loopback, and bounded local monitoring output.
- `obs-rs-capture` defines Rust capture-device lifecycle, permission, hot-plug
  catalog/provider contracts, atomic discovery refresh, deterministic animated test
  backends, a direct Linux X11 screen adapter with `RandR` monitor enumeration and
  per-monitor cropping, a safe-Rust session-bus client driving the
  `org.freedesktop.portal.ScreenCast` handshake for Wayland capture, and a bounded
  `OBSFRM01` RGBA frame-stream adapter for Rust pipes/TCP readers.
- `obs-rs-plugin-api` defines versioned Rust plugin and source interfaces.
- `obs-rs-sandbox` adds a bounded subprocess extension boundary: versioned
  `OBSRPLUGIN1` manifests, bounded manifest probing before source creation,
  direct no-shell process launch, fixed environment negotiation, bounded
  `OBSFRM01` frame packets, a two-frame handoff queue, and frame-delivery
  timeouts.
- `obs-rs-builtins` provides the built-in color, test-pattern, screen, window,
  media, and Nokhwa-backed camera factories plus the Linux `x11_screen_capture`
  and portal-backed `wayland_screen_capture` sources. Media playback uses the
  optional production GStreamer boundary; without it the source stays explicit
  and unavailable rather than substituting an image decoder. A camera that is unplugged,
  busy, or missing leaves its source in the scene, reports why, and reconnects
  on its own instead of failing the project load.
- `obs-rs-core` owns the plugin registry, sources, scenes, CPU compositor, and
  compositor-work counters. It also enforces explicit plugin/source/scene/filter
  quotas and exposes resource usage for diagnostics.
- `obs-rs-video` provides rational frame scheduling, callback-driven rendering,
  bounded frame transport, render/drop/timing metrics, and a sustained-run benchmark
  fixture plus a cancellation-aware wall-clock `VideoWorker`.
- `obs-rs-clock` coordinates rational audio/video deadlines, aggregates shared A/V
  drift diagnostics, provides one monotonic clock implementation for both worker
  traits, models independent device-clock drift deterministically, and runs bounded
  synchronized `MediaSession` ticks.
- `obs-rs-render` defines portable texture/composition contracts, explicit
  program/preview/projector/encoder render-target roles, and a deterministic CPU
  backend with bounded texture bytes, lifecycle/readback metrics, and context-loss
  recovery.
- `obs-rs-output` provides validated video/audio packet encoders, muxer contracts,
  bounded packet back-pressure, a lossless Rust RLE video reference codec, a
  standards-based pure-Rust PNG screenshot encoder, a pure-Rust YUV4MPEG2 reference
  recording writer, atomic raw/Y4M-file and interleaved packet-container finalization,
  a canonical PCM16 WAV reference writer,
  timestamp-order validation, a reconnectable memory transport fixture, and a
  length-framed standard-library TCP transport plus an RFC 6455 WebSocket client
  with reconnect/drop telemetry and an explicit `OBSRWS01` packet envelope.
- `obs-rs-engine` coordinates project/runtime rendering, rational A/V deadlines,
  PipeWire-or-fallback audio, `OBSRPKT1` audio/video recording, and bounded
  TCP/WebSocket output for headless and GUI hosts.
- `obs-rs-project` provides Rust-owned profiles, ordered scenes/source definitions,
  command dispatch, dirty-state tracking, deterministic JSON persistence, and
  atomic project-file save/load/recovery.
- `obs-rs-diagnostics` provides bounded deterministic project/UI/runtime bundles,
  strict decoding, and atomic recovery-file finalization.
- `obs-rs-ui` provides a toolkit-neutral desktop state machine for preview/program
  selection, transitions, output lifecycle, shortcuts, notices, project commands,
  real preview-to-program takes, mixer peak telemetry, deterministic bilingual
  labeled accessibility snapshots, strict terminal/HTTP command parsers, and an
  accessible browser page.
- `obs-rs-gui` provides the first Slint desktop control room: viewport-sized
  preview/program status cards with a replaceable presentation boundary, scene
  selection, transitions,
  recording/streaming controls, scene/source ordering and visibility/lock controls,
  a mixer with gain/mute/peak state, a typed OBS-style source properties form with
  a display picker, a live microphone channel whose fader, mute, and meter drive
  the engine's capture input, crash-safe project save/load/recover, session
  restore for the project and the dock layout, platform-capture capability reporting, output
  telemetry, and PipeWire/fallback status, and a visible bilingual accessible state
  snapshot backed by the same `DesktopState` commands.
- `obs-rs-app` runs a small end-to-end demo, a scriptable accessible terminal
  frontend, and a loopback-only accessible browser control surface without a native
  host dependency.

This is a usable Linux/X11 vertical slice with Rust reference codecs and the
OBS-RS packet boundary, not yet a production codec/protocol stack or full OBS
Studio parity. The executable V1 backlog and acceptance gates are in
[07-functional-todo.md](07-functional-todo.md).

## Build and run

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo run -p obs-rs-app
cargo run -p obs-rs-app --bin obs-rs-linux-check
cargo run -p obs-rs-gui
cargo run -p obs-rs-gui -- --smoke
cargo run -p obs-rs-app --bin obs-rs-console
cargo run -p obs-rs-app --bin obs-rs-web
cargo run -p obs-rs-app --bin obs-rs-benchmark --release
scripts/release-artifacts.sh [dist]
```

### Fast local development

The default `dev` and `test` profiles keep assertions and overflow checks,
emit compact line-table debug information for workspace crates, and omit debug
information from dependencies. This keeps incremental artifacts much smaller
while preserving useful backtraces for code in this repository.

For an opt-in moderately optimized local build, use these Cargo aliases:

```text
cargo dev          # build the workspace with the incremental dev-fast profile
cargo gui          # build only the GUI with dev-fast
cargo check-fast   # quick library/bin check; does not build examples/tests/benches
cargo check-all-fast # check every workspace target with dev-fast
cargo test-fast    # run the normal workspace test set with dev-fast
cargo test-all-fast # include examples and benches in the test build
cargo clippy-fast  # quick clippy pass with -D warnings
cargo clippy-all-fast # clippy every workspace target with -D warnings
cargo gui-check    # check only GUI targets with dev-fast
cargo gui-test     # run the GUI test binary with dev-fast
cargo app          # build only the desktop app with dev-fast
cargo app-check    # check only the desktop app binary
cargo windows-check # build the Windows acceptance binary locally
cargo windows-check-check # check the Windows acceptance binary
cargo windows-check-target # cross-check it for x86_64-pc-windows-msvc
cargo clean-fast # remove only root dev-fast artifacts
cargo clean-release # remove only root release artifacts
cargo clean-workspace # remove every root workspace profile
```

Use the ordinary commands for canonical CI/release-style verification.
`cargo clean` (or `cargo clean-workspace`) removes all root profiles and their
incremental state when a toolchain or profile change makes the cache stale.
The standalone Windows capture helper has its own target directory, so clean
it explicitly when needed:

```powershell
cargo clean --manifest-path packaging/windows/capture-helper/Cargo.toml --profile dev-fast
# Full helper cleanup: omit --profile dev-fast
```

The local toolchain does not force a linker cache or alternate linker; if
`sccache` or a known-good Windows LLD installation is added later, it should be
configured locally rather than required by the repository.

The demo registers the built-in plugin, probes and registers the reference sandbox
source executable when it is available, creates a scene, adds two sources, applies a
scene-item transform/filter, renders through the bounded video pipeline and render
backend, mixes and paces audio blocks, muxes and streams one packet, round-trips one
raw recording, persists project state, commits and reopens a diagnostics bundle, and
encodes an interoperable PNG screenshot and YUV4MPEG2 frame, round-trips one
`OBSFRM01` capture packet, exercises independent audio/video clock drift, then prints
a stable summary.
`obs-rs-console` exposes the same Rust-owned desktop state
through line-oriented scene selection, preview/program swap, transitions, recording,
streaming, and snapshot commands. `obs-rs-web` serves the same state through an
accessible local browser page and bounded `POST /command` requests. The benchmark
runs cancellation-aware wall-clock video workers and reports deadline misses,
lateness, drops, elapsed time, owned-frame footprint, and compositor work counters. All
behavior is exercised through safe Rust APIs and Rust tests.
`obs-rs-gui` opens the native Slint control room; its `--smoke` mode constructs the
window, renders the project preview path, and binds the state without entering the
event loop, which keeps GUI wiring checkable in headless validation.

### Screen capture on Linux

The Add Source list only offers the screen source that can produce frames in the
current session, because the wrong one silently yields a black canvas:

- **X11 sessions** get `x11_screen_capture`. Its display picker lists the `RandR`
  monitors and crops capture to the chosen rectangle; the selection is stored as
  the source's `monitor` setting. A compositor that refuses direct `GetImage`
  falls back to an `ffmpeg x11grab` reader on the same rectangle, and then to the
  deterministic test pattern.
- **Wayland sessions** get `wayland_screen_capture`. There is no direct screen
  read on Wayland, so OBS-RS runs the `org.freedesktop.portal.ScreenCast`
  handshake over the session bus, and the compositor's own dialog is the display
  picker. The portal's restore token is stored in the source settings, so later
  sessions reopen the same screen without prompting. Frames are read from the
  `PipeWire` node the portal returns through `gst-launch-1.0 pipewiresrc`, which
  must be installed (`gst-plugin-pipewire`).

### Camera and microphone

The camera list in the source properties offers the real V4L2 nodes discovered
on the host, and selecting one starts a bounded `ffmpeg` reader for it. The
microphone is chosen on the settings window's Audio page; the engine captures
that device, and the mixer's input channel is named after it, so its fader,
mute, and meter act on the audio that reaches the recording and the stream. If
the configured microphone disappears, the engine keeps bounded deterministic
fallback audio and retries that same device at a one-second media-time
interval; an automatic microphone route rediscovers the first available input.
A configured desktop playback monitor similarly returns to silence
and retries only its selected route after a loss; an automatic route is
rediscovered from the first available output. `obs-rs-linux-check` reports whether
the live `PipeWire` capture is running or the deterministic fallback took over;
this managed host cannot provide live unplug/replug evidence.

Settings, the reopened project, and the dock layout are stored under
`$XDG_CONFIG_HOME/obs-rs` on Linux and `%APPDATA%\obs-rs` (falling back to
`%LOCALAPPDATA%\obs-rs`) on Windows. A file already present in the working
directory keeps being used, so existing installs are unaffected. Both restore
behaviours can be turned off on the settings window's Advanced page.

### Windows capture, audio, and packaging

Windows support targets 64-bit Windows 10 version 1809 or later with the MSVC
toolchain. Version 1809 is the minimum for the Windows Graphics Capture APIs
used by the Rust-built helper. A normal interactive desktop session is required
for display/window capture; a service or locked session may report a typed
unavailable result.

The Windows screen and window sources use the bundled
`obs-rs-capture-windows-helper.exe`, which is found beside the GUI executable or
under the per-user application directories, independent of the current working
directory. Build and run the portable path with:

```powershell
cargo build --target x86_64-pc-windows-msvc -p obs-rs-gui --release
cargo build --manifest-path packaging/windows/capture-helper/Cargo.toml --release
$env:OBSR_CAPTURE_HELPER = "packaging/windows/capture-helper/target/release/obs-rs-capture-windows-helper.exe"
cargo run -p obs-rs-app --bin obs-rs-windows-check
.\packaging\windows\package.ps1
```

The acceptance binary prints machine-readable `pass`, `skip`, and `fail`
records. A missing privacy grant, camera, microphone, render endpoint, helper,
or interactive capture session is an explicit `skip`; it is never represented
as a fake successful frame or audio stream.

Windows Graphics Capture follows the operating system's picker/session rules.
Minimized, occluded, protected, DRM, secure-desktop, and permission-restricted
content can be unavailable or black by design; the source keeps its target and
reports the loss so it can recover when the target returns. Window IDs are
stable for the current desktop session; display IDs use the Windows monitor
device name when available. Per-monitor DPI and negative virtual-desktop
coordinates are preserved by discovery.

The Audio settings page exposes the default or explicit microphone, desktop
loopback render device, and local monitor output. Microphone and loopback
streams use bounded shared-mode WASAPI queues; unplugged devices remain selected
and are retried with typed diagnostics. The Windows acceptance probe also checks
that endpoint IDs and default-route metadata are stable across immediate
snapshots. Windows camera sources use the shared
Nokhwa path, so there is one canonical camera catalog rather than a second helper
camera implementation. Microphone/camera privacy permissions are controlled by
Windows Settings.

The default Windows build uses the portable Rust reference output path and does
not require GStreamer. RTMP, SRT, WebRTC, HLS, and other production profiles
remain an explicitly optional GStreamer build; see
[packaging/windows/README.md](packaging/windows/README.md) for its separate
runtime/development prerequisites; the package script can bundle the matching
runtime and plugin scanner for a self-contained production archive. Settings and diagnostics use `%APPDATA%`
(or `%LOCALAPPDATA%` when needed) under `obs-rs`, and explicit user-supplied
paths are preserved.

### On-disk formats

Both documents OBS-RS writes are standard, human-editable formats rather than
bespoke text, so they can be inspected, diffed, and version-controlled with
ordinary tooling.

| File | Format | Written by |
|------|--------|------------|
| `obs-rs-settings.toml` | flat [TOML](https://toml.io) table of `key = value` pairs | `obs-rs-config` |
| `obs-rs-project.json` | JSON, tagged with `"format"` and `"version"` | `obs-rs-project` |

The GUI keeps `obs-rs-project.json` as the shipped default for compatibility.
Save As and collection workflows use `.obsrproj`; the GUI accepts both that
current extension and legacy `.json` project paths, and rejects other file
types before opening or writing them.

Both are serialized deterministically — keys sorted, no incidental whitespace —
so saving unchanged state twice produces byte-identical files and a project diff
shows only what actually changed.

The settings reader accepts the flat subset of TOML it writes: bare keys, basic
and literal strings, integers, and booleans. `[table]` headers, arrays, and
floats are reported as unsupported rather than quietly misread. Project
documents carry an explicit schema version, so a file written by a newer build
is refused with a clear message instead of being partially parsed.

## Repository documents

1. [00-executive-summary.md](00-executive-summary.md) — mission, scope, principles,
   and the definition of a successful Rust-native OBS engine.
2. [01-current-architecture.md](01-current-architecture.md) — the architecture we
   are implementing now, ownership rules, and the vertical slice.
3. [02-codestyle.md](02-codestyle.md) — Rust-only coding, safety, API, testing, and
   dependency rules.
4. [03-roadmap.md](03-roadmap.md) — the full implementation roadmap from foundation
   through capture, audio, output, UI, and release hardening.
5. [04-risks-and-open-questions.md](04-risks-and-open-questions.md) — active risks,
   decisions, and questions that must be answered before each phase advances.
6. [05-permanent-native-boundaries.md](05-permanent-native-boundaries.md) — the
   boundary policy and evidence checklist for keeping the project Rust-native.
7. [07-functional-todo.md](07-functional-todo.md) — the scoped Linux/X11 V1
   roadmap with dependencies, files, tests, and acceptance criteria.
8. [PARITY-MATRIX.md](PARITY-MATRIX.md) — the verified OBS Studio 32.2.2
   subsystem inventory. This is the current parity evidence ledger; roadmap
   checkboxes are not authoritative.
9. [PERFORMANCE-BASELINE.md](PERFORMANCE-BASELINE.md) — reproducible benchmark
   commands, measurements, and the performance sign-off gate.
10. [KNOWN-BUGS.md](KNOWN-BUGS.md) — observed failures and environment-limited
    evidence that must be resolved or classified.
11. [ARCHITECTURE-GAPS.md](ARCHITECTURE-GAPS.md) — dependency-aware gaps for the
    first performance and parity work packets.

## Current status

Phase 0 (workspace, policy, and pinned-toolchain CI), the Phase 1/2 vertical slice, the Phase 3 reference
render loop and multi-worker soak, Phase 4 callback-clock/audio-worker primitives,
the shared media-clock coordinator, the Phase 5 capture contract plus Linux X11
adapter, and the Phase 6 packet/muxer/atomic recording lifecycle contracts are
implemented, together with a growing Phase 7 project/GUI workflow and Phase 8
resource diagnostics. The current V1 integration adds `obs-rs-engine`, a bounded
background output worker, PipeWire input with deterministic fallback, full-root
X11 resize/letterbox capture with recovery, idle project revision
synchronization, and GUI A/V packet output. A bounded subprocess extension
contract and a tested WebSocket packet transport are also present as reference
boundaries.
The release profile, pinned-toolchain CI workflow, and checksum manifest script
are present. Remaining V1 gaps are explicit output lifecycle events,
hot-plug monitoring, synchronization/staging for edits during active output,
and the remaining performance matrix. The `obs-rs-linux-check` command reports
pass/skip/fail for X11, PipeWire, and the 300-tick A/V soak;
`obs-rs-windows-check` covers the Windows helper, camera, WASAPI, A/V, and
cleanup paths with explicit hardware skips. The project intentionally does not
claim feature parity with OBS Studio: macOS capture, GPU/zero-copy rendering,
full GUI localization/property dialogs, signed plugin distribution, signing,
and archived Windows production/hardware acceptance remain roadmap work. Native
production codecs and protocols are available only in builds supplied with the
approved GStreamer development/runtime boundary.
