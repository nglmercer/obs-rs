# OBS-RS

OBS-RS is a new, Rust-first broadcasting and recording engine inspired by OBS
Studio. It is being built from zero as a standalone project; it is not a line-by-
line translation of the existing OBS implementation.

The repository deliberately has no C or C++ source, headers, ABI shims, generated
bindings, build scripts, or C-based smoke tests. The core crates forbid `unsafe`
code. Platform and media integrations will be added only behind Rust interfaces,
with a separate review when an external dependency is unavoidable.

## What exists today

The current vertical slice is a deterministic, headless engine:

- `obs-rs-util` provides validated identifiers and small shared value types.
- `obs-rs-config` provides bounded, deterministic settings documents.
- `obs-rs-media` defines timestamps, video formats, packed/planar input buffers,
  RGBA conversion, owned frames, filters, and deterministic transitions.
- `obs-rs-audio` defines owned sample buffers, bounded audio queues, a reference
  mixer, and a deterministic linear resampler.
- `obs-rs-capture` defines Rust capture-device lifecycle, permission, hot-plug
  catalog contracts, and deterministic animated test backends for test-pattern,
  screen, window, and camera source kinds.
- `obs-rs-plugin-api` defines versioned Rust plugin and source interfaces.
- `obs-rs-builtins` provides the built-in color, test-pattern, screen, window, and
  camera CPU-fallback source factories.
- `obs-rs-core` owns the plugin registry, sources, scenes, and compositor.
- `obs-rs-video` provides rational frame scheduling, callback-driven rendering,
  bounded frame transport, render/drop metrics, and a sustained-run benchmark
  fixture plus a cancellation-aware wall-clock `VideoWorker`.
- `obs-rs-render` defines portable texture/composition contracts and a deterministic
  CPU backend with readback and context-loss recovery.
- `obs-rs-output` provides validated video/audio packet encoders, muxer contracts,
  bounded packet back-pressure, a lossless Rust RLE video reference codec, atomic
  raw-file finalization, a canonical PCM16 WAV reference writer, a reconnectable
  memory transport fixture, and a length-framed standard-library TCP transport.
- `obs-rs-project` provides Rust-owned profiles, ordered scenes/source definitions,
  command dispatch, dirty-state tracking, deterministic escaped persistence, and
  atomic project-file save/load.
- `obs-rs-ui` provides a toolkit-neutral desktop state machine for preview/program
  selection, transitions, output lifecycle, shortcuts, notices, and project
  commands.
- `obs-rs-app` runs a small end-to-end demo without a native host dependency.

This is an engine foundation, not yet a production recorder or streamer. The
complete target and its acceptance gates are described in the roadmap.

## Build and run

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo run -p obs-rs-app
cargo run -p obs-rs-app --bin obs-rs-benchmark --release
```

The demo registers the built-in plugin, creates a scene, adds two sources, applies a
scene-item transform/filter, renders through the bounded video pipeline and render
backend, mixes one audio buffer, muxes and streams one packet, round-trips one raw
recording, persists project state, and prints a stable summary. The benchmark runs
the cancellation-aware wall-clock worker for 120 frames and reports deadline
misses, lateness, drops, and elapsed time. All behavior is exercised through safe
Rust APIs and Rust tests.

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

## Current status

Phase 0 (workspace and policy), the Phase 1/2 vertical slice, the Phase 3 reference
render loop, injected-clock pacer, packed/planar conversion, transitions, and
sustained benchmark fixture, the first Phase 4 audio primitives including stereo
pan, monitoring taps, and actionable sample-clock/A/V reconciliation, the Phase 5
capture contract/test backend,
and the first Phase 6 packet/muxer/recording lifecycle contracts including atomic
file finalization are implemented, together with the first Phase 7 project
state/command/persistence slice. The project is intentionally not claiming feature
parity with OBS Studio. The next priority is platform capture, hardware rendering,
real codecs, network output, and desktop UX.
