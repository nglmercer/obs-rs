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
  RGBA conversion, owned frames, filters, and deterministic transitions.
- `obs-rs-audio` defines owned sample buffers, bounded audio queues, a reference
  mixer, a deterministic linear resampler, sample-clock pacing, reconciliation,
  monitoring taps, bounded long-run A/V drift telemetry, and a cancellation-aware
  `AudioWorker` with exact block contracts. Callback timestamp observation rejects
  device-clock regressions and applies bounded ppm correction; mixer peak telemetry
  is available to the desktop state.
- `obs-rs-audio-pipewire` provides the Linux PipeWire process adapter with stable
  `pw-dump` node enumeration, bounded raw `f32` input blocks, an optional output
  sink, and typed discovery/start/read/stop failures.
- `obs-rs-capture` defines Rust capture-device lifecycle, permission, hot-plug
  catalog/provider contracts, atomic discovery refresh, deterministic animated test
  backends, a direct Linux X11 root-screen adapter, and a bounded `OBSFRM01` RGBA
  frame-stream adapter for Rust pipes/TCP readers.
- `obs-rs-plugin-api` defines versioned Rust plugin and source interfaces.
- `obs-rs-sandbox` adds a bounded subprocess extension boundary: versioned
  `OBSRPLUGIN1` manifests, bounded manifest probing before source creation,
  direct no-shell process launch, fixed environment negotiation, bounded
  `OBSFRM01` frame packets, a two-frame handoff queue, and frame-delivery
  timeouts.
- `obs-rs-builtins` provides the built-in color, test-pattern, screen, window, and
  camera CPU-fallback factories plus the Linux `x11_screen_capture` source.
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
- `obs-rs-render` defines portable texture/composition contracts and a deterministic
  CPU backend with bounded texture bytes, lifecycle/readback metrics, and context-loss
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
  command dispatch, dirty-state tracking, deterministic escaped persistence, and
  atomic project-file save/load/recovery. Source visibility and lock state are
  persisted with backward-compatible parsing.
- `obs-rs-diagnostics` provides bounded deterministic project/UI/runtime bundles,
  strict decoding, and atomic recovery-file finalization.
- `obs-rs-ui` provides a toolkit-neutral desktop state machine for preview/program
  selection, transitions, output lifecycle, shortcuts, notices, project commands,
  real preview-to-program takes, mixer peak telemetry, deterministic bilingual
  labeled accessibility snapshots, strict terminal/HTTP command parsers, and an
  accessible browser page.
- `obs-rs-gui` provides the first Slint desktop control room: preview/program
  status cards with CPU-rendered RGBA scene frames, scene selection, transitions,
  recording/streaming controls, scene/source ordering and visibility/lock controls,
  a mixer with gain/mute/peak state, source properties, crash-safe project
  save/load/recover, platform-capture capability reporting, output telemetry, and
  PipeWire/fallback status, and a visible bilingual accessible state snapshot
  backed by the same `DesktopState` commands.
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
selected-device persistence/hot-plug monitoring, synchronization/staging for
edits during active output, and full hardware capture coverage. The
`obs-rs-linux-check` command reports pass/skip/fail for X11, PipeWire, and the
300-tick A/V soak. The project intentionally does not claim feature
parity with OBS Studio: macOS/Windows capture, GPU/zero-copy rendering,
production codecs and protocols, full GUI localization/property dialogs, signed
plugin distribution, signing, and update channels remain roadmap work.
