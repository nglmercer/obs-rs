# Modularization TODO

This is the refactor backlog for reducing large Rust source files into modules
with one ownership responsibility each. It is an inventory first: behavior,
public APIs, file formats, and the Rust-only/native-boundary policy must remain
unchanged while the code is moved.

## Inventory

The line counts below were measured on 2026-08-10 with generated `target/`
artifacts excluded. “Large” means at least 500 lines. Tests are included in the
counts because they are currently embedded in the same production files and
should move with the module they prove.

| Priority | File | Lines | Main responsibilities currently mixed together |
| --- | --- | ---: | --- |
| P0 | `crates/obs-rs-output/src/lib.rs` | 3,648 | packets, queues, video/audio encoders, WAV/Y4M containers, muxing, recording, TCP, WebSocket, file writers, wire helpers |
| P0 | `crates/obs-rs-gui/src/main.rs` | 2,837 | Slint view, output runtime, callback wiring, project/scene/source commands, UI refresh, preview rendering, fixtures, tests |
| P0 | `crates/obs-rs-audio/src/lib.rs` | 2,447 | formats/buffers, clocks, pacing, queues, workers, mixer, monitoring, A/V correction, errors, tests |
| P0 | `crates/obs-rs-ui/src/lib.rs` | 1,954 | locale labels, desktop state, actions, shortcuts, console parser, web parser, snapshots, errors, tests |
| P1 | `crates/obs-rs-project/src/lib.rs` | 1,618 | project model, scene/source commands, sessions, persistence, text codec, transforms/filters, validation, errors, tests |
| P1 | `crates/obs-rs-video/src/lib.rs` | 1,508 | frame queue, clocks, deadlines, pacing, pipeline, metrics, workers, cancellation, soak runner, errors, tests |
| P1 | `crates/obs-rs-capture/src/lib.rs` | 1,331 | catalogs, provider contracts, platform fallback, frame-stream protocol, devices, simulated devices, source factories, settings, tests |
| P1 | `crates/obs-rs-core/src/lib.rs` | 1,249 | IDs/limits, registry, source and scene state, compositor, runtime lifecycle, quotas, errors, tests |
| P1 | `crates/obs-rs-media/src/lib.rs` | 1,036 | timestamps, frame rates/formats, pixel layouts, raw frames, RGBA frames, conversion, transforms, filters, transitions, errors, tests |
| P1 | `crates/obs-rs-clock/src/lib.rs` | 984 | timelines, wall clocks, clock rates, independent device-clock model, session coordination, cancellation, reports, errors, tests |
| P1 | `crates/obs-rs-sandbox/src/lib.rs` | 895 | manifest validation/codec, process discovery, plugin adapter, process source, bounded frame reader, lifecycle, errors, tests |
| P1 | `crates/obs-rs-capture/src/x11.rs` | 771 | X11 socket protocol, setup/authentication, geometry, image parsing, frame conversion, adapter errors, tests |
| P2 | `crates/obs-rs-render/src/lib.rs` | 670 | backend contract, capabilities, metrics, errors, CPU texture store, composition operations, conversion, tests |
| P2 | `crates/obs-rs-diagnostics/src/lib.rs` | 612 | bundle model, bounded codec, cursor/reader, atomic writer, errors, tests |
| P2 | `crates/obs-rs-app/src/main.rs` | 524 | demo orchestration, media fixtures, project fixtures, diagnostics export, sandbox discovery, output summary |

### Watchlist below the large-file threshold

These files are not in the 500-line inventory, but should be reviewed when
their neighboring crate is split:

| File | Lines | Follow-up |
| --- | ---: | --- |
| `crates/obs-rs-builtins/src/lib.rs` | 444 | separate portable factory registration from the Linux X11 adapter |
| `crates/obs-rs-plugin-api/src/lib.rs` | 310 | separate plugin contracts from registry/validation helpers if the API grows |
| `crates/obs-rs-config/src/lib.rs` | 267 | separate typed values from the bounded text codec if configuration expands |
| `crates/obs-rs-app/src/bin/obs-rs-web.rs` | 210 | move HTTP parsing/response helpers into a reusable app/web module |

## Refactor rules and definition of done

- [ ] Keep each crate’s current public API stable during moves. The existing
  `lib.rs` files should become small facades that declare modules and re-export
  public types/functions where callers already expect them.
- [ ] Give each module one ownership responsibility. Split by state owner,
  lifecycle, wire format, or timing behavior; do not split only at arbitrary
  line ranges.
- [ ] Move unit tests beside the implementation they prove. Keep integration
  behavior tests at the facade only when they exercise cross-module behavior.
- [ ] Keep the portable crates free of `unsafe` and foreign/native bindings.
  Platform-specific code remains behind explicit adapter modules/crates.
- [ ] Keep parsers, file writers, network transports, and subprocess readers
  bounded and typed while moving them.
- [ ] Target no production Rust source file above 500 lines after each crate is
  complete, except where a reviewed cohesive boundary makes that impractical.
- [ ] After each crate split, run `cargo fmt --all -- --check`, workspace
  check/clippy, and that crate’s tests before starting the next dependent split.
- [ ] At the end, run the complete workspace gates, docs, release build, and
  release-artifact verification.

## Ordered work items

### P0 — output, desktop, and audio

- [x] `obs-rs-output`: split packet types/errors, queue, video/audio codecs,
  raw recording, muxing, file writers, codec helpers, stream session, socket
  transports, WebSocket protocol, and focused tests into physical modules.
  The facade is 68 lines, the largest implementation module is 485 lines, and
  all 25 output tests pass with clippy clean.
- [x] `obs-rs-gui`: make `main.rs` a 63-line bootstrap facade; split output,
  preview, refresh, fixtures, callback groups, tests, and the Slint view into
  physical modules. The view is composed from nine `.slint` components, the
  largest Rust module is 408 lines, and all eight GUI tests pass with strict
  clippy clean.
  - [x] UI-facing error and notice formatting remains owned by
    `obs-rs-ui/src/error.rs`; the GUI callback modules only translate errors at
    the application boundary.
- [x] `obs-rs-audio`: split formats/buffers/resampling, pacing/callback clocks,
  queues/workers, mixer/monitor, A/V sync, errors, and tests into physical
  modules. The largest source file is 464 lines and all 17 audio tests pass.

### P0 — toolkit-neutral UI state

- [x] `obs-rs-ui`: split toolkit-neutral state, snapshots, command mutation,
  helpers, types/actions, console parsing, web parsing, errors, and tests into
  physical modules. The largest implementation module is 328 lines; all 13 UI
  tests and strict clippy remain green.

### P1 — media engine and project state

- [x] `obs-rs-project`: split model, commands, session, persistence, codec,
  validation, errors, and tests into physical modules. The facade is 24 lines,
  the largest implementation module is 381 lines, and all seven project tests
  remain green.
- [x] `obs-rs-video`: split queue, clock/pacing, pipeline/metrics, worker,
  cancellation, soak, errors, and tests into physical modules. The largest
  implementation module is 341 lines and all ten video tests pass.
- [x] `obs-rs-media`: create `time.rs`, `format.rs`, `pixel.rs`, `frame.rs`,
  `transform.rs`, `filters.rs`, `transition.rs`, and `error.rs`. The facade is
  25 lines and the nine media tests remain colocated in `tests.rs`.
- [x] `obs-rs-clock`: split timeline, wall clock, device rates, cancellation,
  session, report, errors, and tests into physical modules. The largest module
  is 278 lines and all nine timing tests pass.

### P1 — runtime, capture, and sandbox boundaries

- [x] `obs-rs-core`: split IDs, limits, registry, runtime/compositor, metrics,
  errors, and tests into physical modules. The facade is 20 lines and all ten
  runtime tests pass with clippy clean.
- [x] `obs-rs-capture`: split catalog/types, provider, protocol, stream device,
  simulated devices, factories, settings, errors, and tests into physical
  modules. The facade is 40 lines and 15 portable tests pass.
- [x] `obs-rs-capture/src/x11.rs`: split protocol, connection, screen lifecycle,
  image decoding, errors, and tests into the Linux-only X11 subtree. The
  largest X11 module is 224 lines and the portable facade exposes no protocol
  details.
- [x] `obs-rs-sandbox`: split manifest/discovery, plugin/source process,
  frame reader, protocol limits, validation, settings, errors, and tests. The
  facade is 30 lines and all five sandbox tests pass with clippy clean.

### P2 — rendering, diagnostics, and application orchestration

- [x] `obs-rs-render`: split the backend contract, CPU implementation, shared
  types/metrics, errors, and tests into physical modules. The facade now owns
  only module declarations and public re-exports.
- [x] `obs-rs-diagnostics`: split bundle state/codec, cursor, atomic writer,
  errors, shared limits, and tests into physical modules. The largest module is
  226 lines and the four diagnostics tests pass.
- [x] `obs-rs-app/src/main.rs`: keep the 175-line startup/demo orchestration
  facade and move deterministic media/project fixtures into `fixtures.rs`,
  diagnostics export into `diagnostics.rs`, and sandbox probing into
  `sandbox.rs`. The app and its auxiliary binaries compile and strict clippy
  passes.
- [x] `obs-rs-builtins`: split factory registration, portable color source
  implementation, Linux X11 adapter, and tests into physical modules. The
  largest implementation module is 136 lines; all six built-in tests pass.

## Dependency-safe sequencing

1. Split leaf/value crates first: media, render, diagnostics, and clock.
2. Split audio and video while preserving their current public re-exports.
3. Split project and core, then capture and sandbox platform boundaries.
4. Split output after its media dependencies have stable facades.
5. Split toolkit-neutral UI, then the Slint GUI and application binaries.
6. Run the full verification matrix and remove any temporary compatibility
   modules only after downstream callers have moved.

## Verification checklist

- [ ] `git diff --check`
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace --all-targets --all-features`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace --all-targets`
- [ ] `cargo doc --workspace --all-features --no-deps`
- [ ] `cargo build --workspace --release`
- [ ] `scripts/release-artifacts.sh <output-directory>` and checksum validation
- [ ] Re-run the inventory and confirm no production Rust file remains above
  the agreed threshold without an explicit exception in this document.
