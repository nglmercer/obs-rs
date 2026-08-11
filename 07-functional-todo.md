# V1 functional roadmap and executable TODO

This is the delivery checklist for the first useful release: Linux, X11 first,
CPU/Rust media paths, X11 screen plus test-pattern sources, PipeWire audio with a
deterministic fallback, and the OBS-RS `OBSRPKT1` file/TCP/WebSocket boundary.
It is intentionally narrower than OBS Studio parity. Every unchecked item has an
owner boundary, dependencies, tests, and an acceptance condition.

## Product decisions

| Area | V1 decision |
| --- | --- |
| Target | Linux vertical slice |
| Video platform | X11 first; Wayland and other OS adapters follow later |
| Media boundary | Rust/CPU reference path; no new native ABI in the core |
| Video sources | X11 full-screen capture and animated test pattern |
| Audio | PipeWire `pw-dump`/`pw-cat` adapter, deterministic fallback |
| Output | `OBSRPKT1` file with video plus raw `f32` audio; existing TCP/WebSocket transport |
| GUI | Slint control room backed by the existing Rust state machine |

## Implemented in this delivery

- [x] Add `obs-rs-engine` with project/runtime assembly, coordinated rational
  audio/video deadlines, audio fallback, packet encoding, atomic recording,
  reconnectable streaming, snapshots, counters, and input gain/mute controls.
  Files: `crates/obs-rs-engine/`.
  Tests: engine monotonic-tick, fallback, and A/V packet-container tests.
- [x] Add typed `AudioDeviceInfo`, `AudioInput`, and provider contracts plus a
  deterministic signal provider. Files: `crates/obs-rs-audio/src/device.rs`.
- [x] Add the reviewed Linux PipeWire process adapter. It discovers a default
  source and reads bounded raw `f32` blocks; failure is typed and recoverable.
  Files: `crates/obs-rs-audio-pipewire/`.
- [x] Connect GUI output to the engine. Recording now commits one `OBSRPKT1`
  file containing both media kinds; stream packets are queued before pumping.
  Files: `crates/obs-rs-gui/src/output.rs`, GUI callbacks and settings.
- [x] Keep preview animation alive while idle and expose live audio/output
  status in the timer path.
- [x] Make GUI desktop-audio gain/mute controls affect the output mixer.
- [x] Capture the complete X11 root, resize with aspect-preserving letterbox,
  and use a test-pattern fallback/reconnect path. Files:
  `crates/obs-rs-capture/src/x11/` and `crates/obs-rs-builtins/src/x11.rs`.
- [x] Add deterministic tests for resize geometry, output A/V decoding, engine
  timestamp order, provider lifecycle, and fallback source creation.

## P0 — close the Linux vertical slice

### P0.1 — Move output work completely off the Slint callback thread

- [ ] Add `OutputWorker` around `EngineSession` with a bounded command/frame
  channel and an explicit shutdown acknowledgement.
  Dependencies: `obs-rs-engine` output API.
  Files: `crates/obs-rs-engine/src/worker.rs`,
  `crates/obs-rs-gui/src/output.rs`.
  Tests: bounded frame drop, worker cancellation, recording finalization after
  shutdown, stream reconnect while frames continue arriving.
  Acceptance: Slint callbacks only enqueue commands/frames; a stalled TCP peer
  cannot stall preview, scene edits, or shutdown.
- [ ] Add output lifecycle events (`Starting`, `Running`, `Failed`, `Stopping`)
  and reconcile them with `DesktopState` instead of optimistic booleans.
  Dependencies: P0.1.
  Tests: connect failure, remote close, failed recording path, stop during start.
  Acceptance: the GUI never says “streaming” after the engine reports failure.

### P0.2 — Complete PipeWire device lifecycle

- [ ] Enumerate stable input/sink node IDs from `pw-dump`, not only the default
  route; add selected-device configuration and hot-plug refresh.
  Dependencies: `obs-rs-audio-pipewire` contract.
  Files: adapter, `obs-rs-ui` audio commands, GUI settings.
  Tests: fake `pw-dump` snapshots, duplicate/removed nodes, selected-node loss.
  Acceptance: a user can select an input, see availability changes, and fall
  back without losing the recording timeline.
- [ ] Add a PipeWire monitor/output sink behind the same typed boundary.
  Dependencies: P0.2 input catalog.
  Tests: format negotiation, underflow/overflow, stop/restart, no-device fallback.
  Acceptance: monitoring is optional and never blocks the program output.

### P0.3 — Make project/output synchronization explicit

- [ ] Synchronize active profile format and source revisions into the engine at a
  safe boundary; reject or stage edits while an output is running.
  Dependencies: P0.1 lifecycle events.
  Files: engine session, GUI refresh/project callbacks.
  Tests: scene edit while recording, profile resolution change, source update
  failure, rollback after rebuild failure.
  Acceptance: output format, preview format, and project profile cannot diverge.
- [ ] Add a V1 diagnostics section for engine/audio/output state and last typed
  failure, including backend (`PipeWire` or fallback), queue depth, drops, and
  reconnect count.
  Dependencies: existing `obs-rs-diagnostics` bundle.
  Tests: deterministic bundle round-trip and bounded error text.
  Acceptance: an exported diagnostic artifact explains every failed V1 output.

### P0.4 — Verify the supported Linux path on real services

- [ ] Add a live X11 acceptance command that captures one full-root frame when
  `DISPLAY` and authorization are usable; retain the ignored fixture for CI.
- [ ] Add a live PipeWire acceptance command that reads one complete block when
  a source exists; skip with a typed capability result when no source exists.
- [ ] Add a 300-tick A/V soak that writes/decodes `OBSRPKT1` and checks monotonic
  timestamps, packet counts, fallback transitions, and bounded memory.
  Acceptance: all three commands produce machine-readable pass/skip/fail output;
  CI remains deterministic without X11/PipeWire.

## P1 — useful capture and editing breadth

- [ ] X11 window selection and geometry tracking. Dependencies: P0.4.
  Tests: window ID discovery, destroyed window, resize, fallback.
- [ ] Linux camera input behind a reviewed Rust adapter. Dependencies: the
  platform capture contract and a chosen safe device API. Tests: permission,
  format negotiation, disconnect/reconnect.
- [ ] Source property forms for all V1 source settings, including display,
  crop, scale, opacity, and test-pattern controls. Dependencies: project command
  validation. Acceptance: every visible source setting round-trips through the
  project file and takes effect in preview.
- [ ] Real mixer channel graph: map desktop and microphone channels to separate
  engine sources and expose per-source peaks. Dependencies: P0.2.
- [ ] Guided recovery dialog and explicit “unsupported capability” states for
  projector, virtual camera, and platform-specific menu actions.

## P2 — post-V1 capabilities

- [ ] Wayland/PipeWire screen capture and macOS/Windows capture adapters.
- [ ] GPU backend and zero-copy paths while preserving the CPU renderer as the
  correctness oracle.
- [ ] Production codec/container and RTMP/SRT/WebRTC decisions after license,
  distribution, and native-boundary review.
- [ ] Signed dynamic plugins, packaging, updates, fuzzing, and long-duration
  hardware soak automation.

## Definition of done for V1

- [ ] A clean Linux session can create a project, select X11 or test-pattern
  sources, preview while idle, change preview/program, take a transition, and
  recover from a missing display/audio service.
- [ ] Recording produces an atomically committed `OBSRPKT1` file with both video
  and audio packets and monotonic timestamps.
- [ ] Streaming uses the existing TCP/WebSocket boundary with bounded queue,
  reconnect telemetry, and no GUI freeze under a stalled peer.
- [ ] Audio settings, gain, mute, backend/fallback state, and meters are visible
  and actionable.
- [ ] Project save/load/recovery and diagnostics include the engine state.
- [ ] `fmt`, workspace check, strict clippy, workspace tests, GUI smoke, app demo,
  and the Linux capability checks are green or explicitly skipped with a reason.

## Verification commands

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo run -p obs-rs-gui -- --smoke
cargo run -p obs-rs-app
```
