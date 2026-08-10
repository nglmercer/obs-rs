# OBS-RS Implementation Roadmap

## Objective

Deliver a complete OBS-like application from zero in Rust. The roadmap is organized
by usable capabilities and verification evidence, not by copying source files or
counting converted lines.

The current repository implements MVP slices in Phases 0–7 and reference recording
and transport fixtures in Phase 6. All later work remains active; this document is the
source of truth for sequencing.

## Status legend

- **Done**: code and tests are present in this repository.
- **Active**: the next implementation target.
- **Planned**: prerequisites are not complete.
- **Blocked**: an explicit decision or external capability is missing.

## Phase 0 — Rust foundation (Done)

### Deliverables

- pinned Rust toolchain and Cargo workspace;
- Rust-only CI for formatting, checking, linting, and tests;
- repository policy forbidding unsafe code in portable crates;
- deterministic local commands and a clean dependency baseline.

### Exit evidence

The workspace builds with the pinned toolchain, CI runs no native-language compiler,
and all current crates pass the quality gates.

## Phase 1 — Domain values and persistence (Done for MVP)

### Deliverables

- validated identifiers and typed IDs;
- deterministic settings parser and serializer;
- timestamps, frame rates, video formats, and owned RGBA frames;
- explicit error types and boundary tests.

### Exit evidence

Malformed settings are rejected without partial mutation, frame buffers cannot be
constructed with the wrong size, rational frame rates reject zero values, and all
serialization/checksum fixtures are repeatable.

## Phase 2 — Headless engine vertical slice (Done for MVP)

### Deliverables

- Rust plugin manifest and source traits;
- source factory registry with duplicate detection;
- runtime-owned source instances and named scenes;
- ordered scene composition with alpha blending;
- built-in color source;
- executable end-to-end demo.

### Exit evidence

The demo registers a plugin, creates a scene, renders a known frame, and tests plugin,
source, scene, format, and error lifecycles without global mutable state.

## Phase 3 — Video engine (Active)

### Goal

Turn the reference compositor into a clocked video engine suitable for live work.

### Work

- monotonic clock abstraction and frame schedule (scheduler, monotonic clock, deadline
  observation, injected-clock pacing, and cancellation-aware `VideoWorker` MVP
  implemented in `obs-rs-video`, with a thread-safe cancellation token);
- bounded frame queues with explicit drop policy (MVP implemented with both oldest
  and newest drop modes);
- source update and render phases with cancellation;
- color conversion and additional packed/planar formats (RGBA/BGRA/RGB/gray and
  I420 conversion MVP implemented in `obs-rs-media`);
- reference CPU renderer plus benchmark harness;
- scene transitions, filters, cropping, scaling, and transforms (cut/cross-fade
  transitions, transform, and basic grayscale/brightness/opacity filter MVP
  implemented in `obs-rs-media` and `obs-rs-core`);
- resource accounting and frame diagnostics (the runtime exposes compositor counters
  for source requests/results, transforms, in-place filters, and blends; the CPU render
  backend exposes bounded texture lifecycle, movement, and peak-byte metrics).

### Entry criteria

The Phase 2 contracts are stable and a benchmark fixture defines frame dimensions,
source count, target frame rate, and acceptable allocation behavior.

### Exit criteria

The engine can render a sustained scene at its configured frame rate, reports missed
deadlines and dropped frames, has deterministic tests for queue pressure, and keeps
the reference renderer as a correctness oracle.

### Current evidence

`obs-rs-video` proves deterministic deadlines, monotonic-clock behavior, injected-clock
pacing, deadline observation, cancellation, queue capacity, format rejection, both
drop policies, callback-driven render outcomes, render/drop counters, and a 120-frame
output-draining sustained-run fixture. The release benchmark exercises the worker for
120 wall-clock-paced frames and reports observed misses/lateness plus compositor work
counts, wait time, render time, and worst lateness. `obs-rs-media` validates packed/planar input buffers and deterministic
conversion; scene-item scale/translation/flip/opacity transforms, filters, cuts, and
cross-fades are covered by `obs-rs-media` and `obs-rs-core` tests. The compositor
applies filter chains in place, borrows scene/filter definitions without per-frame
snapshots, bypasses identity transforms, and avoids creating a transparent
accumulator for the first layer. The CPU render backend reports bounded texture
lifecycle, movement, and peak-byte metrics. The remaining work is wall-clock deadline
measurement in a long-running multi-worker design, allocation tracing beyond those
resource counters, more conversion formats, and performance tuning.

## Phase 4 — Audio engine

### Goal

Add a sample-clocked audio graph that can mix sources and remain synchronized with
video.

### Work

- `AudioFormat`, owned sample buffers, and channel layout (MVP implemented);
- bounded audio ring buffers and underflow/overflow policy (complete-buffer queue MVP
  implemented);
- mixer with per-source gain, mute, pan, and monitoring taps (gain/mute/clamp and
  stereo-pan MVP implemented);
- resampling and timestamp reconciliation;
- test signal source and offline WAV-like reference output;
- A/V drift, latency, and long-duration synchronization tests.

### Exit criteria

An offline fixture and a real-time simulation produce stable sample counts, bounded
latency, and documented behavior under underflow, source removal, and clock drift.

### Current evidence

`obs-rs-audio` validates finite interleaved samples, bounds queue memory, exposes
drop-oldest behavior, mixes registered inputs deterministically with gain/mute/stereo
pan controls, resamples equal-channel buffers with explicit unknown, duplicate, format,
and rate errors, schedules exact sample-clock deadlines, reports signed A/V drift
with a tolerance-based action, exposes bounded post-mix monitoring taps, and
reconciles early/late buffers with sample-level trimming or inserted silence.
`AvSyncMonitor` adds bounded long-run observation counts, maximum absolute drift, and
saturating total drift diagnostics. Its injectable `AudioClock`/`AudioPacer` contract
covers block-level pacing, while `AudioWorker` adds thread-safe cancellation, exact
format/frame/timestamp contracts, underflow/drop accounting, and post-callback
deadline diagnostics. `obs-rs-clock::MediaTimeline` now advances matching rational
video/audio boundaries, `MonotonicMediaClock` implements both worker clock traits,
and `MediaSession` runs synchronized bounded audio/video ticks with aggregate
diagnostics. A 10,000-tick exact-boundary soak test keeps both rational domains at
zero measured drift. Real device clocks and long-duration synchronization under
independent hardware clocks are modeled by `IndependentMediaClock`, which applies
bounded signed ppm rates to separate audio/video domains while preserving monotonic
wait semantics. A 3,000-tick test proves accumulated drift is classified and remains
observable through `AvSyncController`; the demo runs the same fixture for 300 ticks.
Actual OS device clock adapters and correction against hardware callbacks still
remain incomplete.
`obs-rs-output` adds a canonical PCM16 WAV reference writer for offline inspection and
an interoperable pure-Rust PNG screenshot encoder using deterministic zlib stored
blocks. The PNG path proves a standards-based image artifact without introducing a
native codec dependency; it is not a production video codec. `Y4mRecording` now
emits a standards-based YUV4MPEG2 stream with deterministic RGBA-to-4:2:0 conversion,
even-dimension and timestamp validation, and bounded recording size. It is an
uncompressed reference container, not a production compression path.

## Phase 5 — Capture and render backends

### Goal

Provide useful screen, window, camera, and display capture plus an accelerated
renderer while keeping the core contracts portable.

### Work

- Rust traits for device discovery, permissions, hot-plug, and frame delivery;
- Linux, macOS, and Windows adapters selected per target;
- CPU fallback for every supported capture path;
- GPU abstraction for texture upload, composition, readback, and loss recovery;
- backend capability reporting and automated fixtures where hardware is absent.

The current repository implements the portable capture contract in `obs-rs-capture`:
device descriptors/provider discovery, atomic catalog refresh, permission and
hot-plug events, start/stop state, format validation, timestamped frames, and
deterministic animated test backends for test-pattern, screen, window, and camera
devices. `StreamCaptureDevice<R>` adds a bounded `OBSFRM01` packet stream that can
carry frames from a separate Rust process over a pipe or TCP reader. On Linux,
`X11CaptureDevice` speaks the local X11 setup and `GetImage` protocol directly over
the Unix socket, performs magic-cookie authentication when an Xauthority record is
available, converts TrueColor masks to RGBA, and is wired into the built-in
`x11_screen_capture` source. The parser and pixel conversion are covered by protocol
fixtures; an X server is still required for a live capture run. macOS and Windows
discovery remain separate future adapters.

`obs-rs-render` now supplies the portable render-backend contract and a deterministic
CPU fallback for texture allocation, upload, ordered composition, readback, resource
limits including aggregate byte accounting, raw packed/planar upload conversion, and
simulated context-loss recovery. Hardware acceleration, native device contexts, and
zero-copy resources remain separate integrations.

### Exit criteria

Each supported backend has a device lifecycle test, a permission/error matrix, a
fallback path, and frame/timestamp measurements on supported hardware. No backend
leaks platform details into `obs-rs-core`. The remaining Phase 5 work is real platform
discovery/devices, hardware contexts, and zero-copy/GPU resource integrations.

## Phase 6 — Encoding, muxing, recording, and streaming

### Goal

Produce files and network output with back-pressure, recovery, and observable state.

### Work

- encoder and packet traits;
- software codec implementations or reviewed Rust integrations;
- container writer and fragmented output recovery;
- file recording with atomic finalization;
- streaming protocol client, reconnect policy, and bounded queues;
- bitrate, keyframe, congestion, and error telemetry.

The repository now has validated `EncodedPacket`, `VideoEncoder`, and `PacketMuxer`
contracts, byte-bounded packet transport, deterministic raw and lossless RLE video
reference encoders with a decoder fixture, interoperable pure-Rust PNG screenshot
and YUV4MPEG2 reference recording writers, an in-memory muxer, explicit
finalized/aborted recording sessions, atomic standard-library raw/Y4M-file writers,
raw audio encoding, a canonical PCM16 WAV reference writer, a reconnectable
packet-transport session with a memory fixture, an atomic interleaved `OBSRPKT1`
packet-container writer with timestamp-order validation, an explicit length-framed
standard-library TCP transport, and an uncompressed `OBSRRAW1` reference recording
format in `obs-rs-output` for correctness and recovery fixtures. The TCP framing is
a real Rust transport path, not a claim of RTMP/SRT/WebRTC compatibility.
It is not yet a production codec, protocol client, or hardware/network streaming
implementation.

### Exit criteria

Recorded fixtures can be reopened and inspected, interrupted writes recover safely,
network failures do not block capture, and long-running tests preserve timestamps,
memory bounds, and A/V sync.

## Phase 7 — Product application and UX

### Goal

Expose the engine as a usable desktop production application.

### Work

- Rust application state and command model;
- scene/profile editors, source properties, preview/program views;
- keyboard shortcuts, accessibility, localization, and safe recovery;
- project migration/importers implemented as explicit Rust parsers;
- crash-safe persistence and diagnostic bundles.

The first Rust application-state slices are now present in `obs-rs-project` and
`obs-rs-ui`: profiles, ordered scenes and sources, transform/filter state, validated
commands, dirty-state tracking, deterministic escaped persistence, preview/program
selection, transitions, output lifecycle, bounded notices, and shortcut bindings.
They remain toolkit-neutral control-plane foundations; `obs-rs-gui` is the first
desktop adapter over them. `DesktopState` now renders a deterministic
labeled text snapshot, `obs-rs-console` provides a scriptable terminal presentation,
and `obs-rs-web` provides an accessible loopback browser presentation with validated
scene, transition, recording, and streaming commands. `obs-rs-gui` adds the first
Slint desktop control room with preview/program status cards, scene actions,
transition controls, output lifecycle buttons, and a visible snapshot backed by
those same commands. The preview/program surfaces now render project scene sources
through `obs-rs-core::Runtime` into Slint images on a bounded UI timer. They are CPU
reference previews rather than platform capture/device-backed feeds. The desktop
also has a small scene/source editor plus project save/load controls backed by the
crash-safe `ProjectFileStore`; richer property editing, crash recovery dialogs,
localization, and crash-report collection are still outstanding.

`ProjectFileStore` adds atomic standard-library project-file save/load semantics and
keeps the session dirty when a write fails.

`obs-rs-diagnostics` adds a bounded deterministic `OBSRDG01` bundle containing
project, UI, and runtime sections, with strict decoding and atomic recovery-file
finalization. The headless demo creates and reopens this artifact. A complete desktop
workflow and crash-report collection policy are still outstanding; the initial Slint
presentation is covered by the GUI crate's smoke mode and unit tests.

The plugin contract also carries an explicit API major/minor version, and
`obs-rs-core` rejects newer incompatible manifests before registering any factories.

### Exit criteria

A new user can create a scene, configure sources, preview, record, stream, recover
after restart, and understand all actionable errors without editing files manually.

## Phase 8 — Plugin ecosystem and release hardening

### Goal

Make extensions and releases sustainable without weakening Rust ownership.

### Work

- versioned Rust plugin contract and compatibility policy;
- compile-time plugin registry for the first release;
- optional sandboxed dynamic extension format after threat-model review;
- permission model, resource quotas, and plugin diagnostics;
- reproducible release builds, signing, update channels, and support tooling;
- fuzzing, sanitizers where applicable, dependency audits, and soak testing.

### Exit criteria

Third-party Rust authors can build a documented plugin, incompatible versions fail
clearly, a malicious or faulty plugin cannot corrupt core state, and release artifacts
can be reproduced and verified.

## Current priority order

1. Wall-clock video timing/diagnostics, resource profiling, and longer soak runs.
2. Device-clock audio behavior and long-duration synchronization tests.
3. Platform capture discovery/permissions behind the existing Rust capture traits.
4. Production codec/container/protocol decisions beyond the raw and RLE references.
5. Capture-backed desktop preview/editor/recovery workflows and plugin/release hardening.

## Go/no-go rule

Do not advance because a date or line-count target was reached. Advance only when the
phase's public contracts, failure behavior, tests, and performance evidence exist.
If a feature requires an unsafe or native integration, isolate it behind a Rust-safe
trait and keep the reference implementation usable without it.
