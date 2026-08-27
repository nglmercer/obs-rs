# OBS-RS performance baseline

**Baseline date:** 2026-08-20  
**Baseline commit:** `7afb7fa` (Phase 0 evidence)  
**Latest measurement:** 2026-08-21 (bounded WGPU composition allocation packet)
**Reference:** OBS Studio `32.2.2` is installed and reports that version.  
**Machine:** Linux `x86_64`, AMD BC-250, 12 logical CPUs, 14 GiB RAM, Rust/Cargo `1.97.1`.

This is a reproducible OBS-RS baseline, not a performance sign-off. The
benchmark ran in a managed session with no usable X11 server, Wayland
compositor, camera, or PipeWire graph. No direct OBS-vs-OBS-RS number is claimed
until both programs can run on the same pinned reference machine with the same
scenes and capture devices.

## Commands and gate results

| Probe | Result | Notes |
| --- | --- | --- |
| `cargo fmt --all -- --check` | Pass | No formatting drift. |
| `cargo check --workspace --all-targets --all-features` | Pass | Completed in 45.75 s in the warm workspace. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Pass | Baseline lint drift was cleaned up while preserving behavior; this is now a required phase gate. |
| `cargo test --workspace --all-targets` | Pass with explicit environment ignores | Native production-sink tests and one native-window GUI test are explicitly ignored because this managed session has neither dependency; the remaining workspace tests pass. |
| `cargo test -p obs-rs-gui --bin obs-rs-gui -- --test-threads=1` | Pass with explicit ignores | 106 pass; one compositor-dependent GUI test and one timing probe are ignored. |
| `cargo run -p obs-rs-gui -- --smoke` | Pass | Constructs the window and render path without entering the event loop. |
| `cargo run -p obs-rs-app --bin obs-rs-linux-check` | Mixed | A/V soak passes; X11/window/camera/PipeWire checks skip due session capabilities. |
| `cargo run -p obs-rs-app --bin obs-rs-benchmark --release` | Pass as a measurement | The harness completes, but its deadline metrics do not meet the future acceptance gate. |
| `obs --version` | Pass | `OBS Studio - 32.2.2`. |

## Existing release benchmark

Command:

```text
cargo run -p obs-rs-app --bin obs-rs-benchmark --release
```

Observed report:

```text
render_samples=120
render_p50_ns=772671
render_p95_ns=1131076
render_max_ns=2090687
frame_owned_buffers=0
frame_owned_bytes=0
frame_shared_clones=480
frame_cow_buffers=120
frame_copied_bytes=110592000
rss_before_kib=6420
rss_after_kib=9912
requested=120
processed=120
cancelled=false
empty=0
dropped_oldest=0
dropped_newest=0
missed=120
lateness_ns=180437012
max_lateness_ns=2895770
wait_ns=3787514488
paced_render_ns=180254360
produced_bytes=110592000
peak_queued_bytes=921600
remaining=0
renders=250
source_requests=500
source_frames=500
empty_sources=0
transformed=250
filtered=250
blends=250
elapsed_ms=3967
multi_workers=2
multi_requested=60
multi_processed=60
multi_missed=60
multi_lateness_ns=8332885
multi_produced_bytes=55296000
multi_peak_queued_bytes=921600
multi_elapsed_ns=967025892
```

The fixture is the historical 640x360@30 workload. The latest measured render
p95 is 1.131 ms, but the current wall-clock deadline accounting reports a miss for all
120 single-worker frames and all 60 multi-worker frames in this session. That
is a baseline finding, not evidence that a 60 FPS production path is accepted.
The bounded queue footprint is one 921,600-byte RGBA frame in this fixture.

The 110,592,000 copied bytes are exactly 120 RGBA frames at 640x360. This is the
reference engine workload, not the GUI presentation copy. The GUI worker now
requests separate 1048x590 preview and program-view targets for a 1920x1080
canvas (and proportionally bounded targets for 4K), so their Slint copies are
separated and counted as `frame_copy_bytes` in the live metrics string.

The WGPU compositor allocation probe exercises 10,000 single-layer 1920x1080
compositions with an explicit `wait_idle()` after each frame. Before the
iterator change, the debug test profile reported:

```text
gpu_frames=10000 p95_ns=1298776 dropped=0 readbacks=0 gpu_bytes=33177600 pooled_textures=2
```

After removing the duplicate per-composition layer-descriptor `Vec` (the
compositor now consumes the exact-size iterator directly), the same probe
reported:

```text
gpu_frames=10000 p95_ns=1222958 dropped=0 readbacks=0 gpu_bytes=33177600 pooled_textures=2
```

Both runs completed all frames without readback or drops. This is a local
allocation-path comparison in the debug WGPU test profile, not a hardware
performance sign-off; end-to-end capture/audio/encoder allocation tracing is
still required.

## Crop/Pad, key, color, Scroll, and Render Delay evidence

The Crop/Pad, Color Key, Luma Key, Chroma Key, Sharpen, Color Multiply/Add, and
Scroll packets keep their effects
in-place and bounded: Crop/Pad clears edge pixels without changing frame
geometry, Color Key adjusts alpha from a normalized RGB distance, Luma Key
applies bounded smooth luminance thresholds, and Chroma Key applies a
current-pixel YCbCr distance with feathering and spill reduction, and Sharpen
uses a clamped 3x3 neighbour kernel, and Color Multiply/Add applies bounded RGB
multiply/add matrix values while preserving alpha. Scroll uses a timestamp-derived
integer source offset and either wraps or clears out-of-range pixels. Key filters
canonicalize fully transparent pixels. The WGPU path carries every filter in a fixed
seven-word record so the
shader does not parse variable-length data.

```text
cargo test --release -p obs-rs-media -- --ignored --nocapture composition_primitives_timing_report
```

The ignored timing report now includes `clone + crop-pad`, `clone +
color-correction`, `clone + color-key`, `clone + luma-key`, `clone +
chroma-key`, `clone + sharpen`, and `clone + color-multiply-add` beside the existing transform, blend,
grayscale, and solid-frame measurements. CPU and
WGPU correctness are covered separately by the media filter tests and WGPU
CPU-oracle/readback tests; this is evidence for repeatable local measurement,
not a 60 FPS acceptance result. On 2026-08-21 in this workspace's release
profile, the 640x360 samples averaged `195.213 µs` for Crop/Pad, `702.582 µs`
for Color Correction, `1.026162 ms` for Color Key, `1.059995 ms` for Luma Key,
`4.905796 ms` for Chroma Key, `1.047188 ms` for Sharpen, and `343.155 µs`
for Color Multiply/Add over 200 runs.
Chroma Key is materially more
expensive in this reference implementation because each pixel performs the
nonlinear sRGB and YCbCr distance math; the later performance phase must decide
whether a bounded lookup-table/vectorized path is needed. Sharpen's CPU
snapshot is also a deliberate allocation/copy point that the performance agent
must replace with bounded reusable scratch storage before high-resolution use.
These are local samples rather than acceptance thresholds; full-resolution CPU
filter performance still requires the Phase 16 comparison suite.

The Scroll-specific release probe is:

```text
cargo test --release -p obs-rs-media scroll_filter_timing_report -- --ignored --nocapture
scroll filter: 120 frames x 640x360 = 64.63851ms total (about 538.654µs/frame), checksum=3840
```

The CPU reference path copies a source snapshot once per active frame so an
ordered filter pass cannot read pixels it has already overwritten. The WGPU
path carries the timestamp-derived offset in its bounded record and the
integration test verifies it against that CPU oracle; the readback in that test
is explicit test instrumentation, not the preview path. Reusable CPU scratch
storage or GPU-only routing is still required before treating CPU Scroll as a
high-resolution real-time implementation.

The Render Delay queue is bounded by 32 retained frame slots and 256 MiB of
RGBA storage per source. It owns timestamps in the source runtime, returns no
frame during warm-up, and shares the delayed frame with every scene item that
references that source. The queue-only release probe is:

```text
cargo test --release -p obs-rs-media render_delay_buffer_timing_report -- --ignored --nocapture
render delay buffer: 120 timestamped 640x360 pushes = 6.313µs total (about 52ns/push), buffered=6, checksum=3648
```

This probe uses shared pixel storage to isolate queue/state cost; capture
allocation, high-resolution memory pressure, GPU texture history, and the
ordered temporal/pixel filter graph are not included. A history that exceeds
the CPU budget reports a bounded runtime failure and does not silently shorten
the requested delay.

The audio Gain timing probe processes 200 reusable 480-frame stereo blocks
through one fixed-capacity chain without allocating per block. Its release
measurement is recorded by:

```text
cargo test --release -p obs-rs-audio gain_filter_block_timing_report -- --nocapture
```

The observed release result was `112.129 µs` total, or `560 ns` per
480-frame stereo block on this host. This is a local primitive measurement;
full audio-graph and device-clock performance still require the Phase 16
matrix.

The standard-layout resampler probe converts 200 reusable 480-frame 5.1
blocks to stereo using speaker-role mapping. Its release measurement is:

```text
cargo test --release -p obs-rs-audio resampler_block_timing_report -- --nocapture
resampler: 200 blocks x 480 5.1 frames = 5.680215ms (28.401µs/block)
```

This is a bounded channel-mapping and linear-resampling primitive measurement;
device-clock correction, capture negotiation, and the complete audio graph are
not included.

Automatic route reconciliation now uses a capacity-one engine worker. Provider
discovery, candidate-format negotiation, and native input opening run on that
worker; the audio tick only polls a mutex-backed latest-result slot and queues
the next refresh with `try_send`. The route-change regression fixture verifies
that a healthy default switch is applied without waiting for provider I/O, while
explicit selections remain unchanged.

The same release probe for Invert Polarity is kept separate because it has no
settings or gain conversion; its result is recorded by:

```text
cargo test --release -p obs-rs-audio invert_polarity_block_timing_report -- --nocapture
```

The observed release result was `27.754 µs` total, or `138 ns` per
480-frame stereo block on this host.

The same release probe for the stateful Limiter processes a reusable 480-frame
stereo block with a fixed 1 ms attack and 60 ms release. Its envelope is kept
inside the filter instance across blocks; no per-block allocation is introduced.
The measurement is recorded by:

```text
cargo test --release -p obs-rs-audio limiter_block_timing_report -- --nocapture
```

The observed release result was `1.236304 ms` total, or `6.181 µs` per
480-frame stereo block on this host. This is a local primitive measurement;
full audio-graph and device-clock performance still require the Phase 16
matrix.

The stateful Compressor timing probe uses the source signal as its detector,
with a 10:1 ratio, −18 dB threshold, 6 ms attack, 60 ms release, and 0 dB
output gain. Its two-pass finite-output preflight keeps an overflowing positive
output-gain update atomic without allocating per block. The measurement is
recorded by:

```text
cargo test --release -p obs-rs-audio compressor_block_timing_report -- --nocapture
```

The observed release result was `3.70458 ms` total, or `18.522 µs` per
480-frame stereo block on this host. This is a local primitive measurement;
sidechain synchronization and full audio-graph performance still require the
Phase 16 matrix.

The peak Expander timing probe uses a 2:1 ratio, −40 dB threshold, 10 ms
attack, 50 ms release, and 0 dB output gain. Its gain ballistics remain in the
filter instance, and its finite-output preflight keeps overflow atomic without
allocating per block. The measurement is recorded by:

```text
cargo test --release -p obs-rs-audio expander_block_timing_report -- --nocapture
```

The observed release result was `4.05549 ms` total, or `20.277 µs` per
480-frame stereo block on this host. This is a local peak-detector primitive;
RMS/gate/knee/sidechain synchronization and full audio-graph performance still
require the Phase 16 matrix.

## Phase 1 render-target evidence

The first performance architecture packet is implemented and independently
exercised by these checks:

```text
cargo test -p obs-rs-render-wgpu --features gpu --lib gpu_upload_layer_submission_readback_and_recovery_are_explicit
cargo test -p obs-rs-gui --bin obs-rs-gui preview_format_is_bounded_and_preserves_canvas_aspect
cargo test -p obs-rs-gui --bin obs-rs-gui scene_composition_runs_on_the_preview_thread
```

The WGPU test submits an 8x8 canvas frame into a 4x4 target and verifies the
target-sized readback. The GUI worker test verifies that a 1920x1080 canvas
produces both a 1048x590 preview/program-view frame and an explicit
1920x1080 full-canvas fallback only when the output path requests RGBA. The
normal WGPU output path now submits the full Program target directly and reads
back only its encoder-oriented NV12 payload. The remaining CPU readback is
deliberate compatibility behavior behind `PreviewPresenter`; a native surface
presenter is still future work.

## Canvas transform timing evidence

The crop/rotation packet adds a release timing probe for the new rotated CPU
path. It keeps the existing identity, scaled, blend, filter, and solid-frame
measurements so rotation can be compared against the established primitives:

```text
cargo test --release -p obs-rs-media -- --ignored --nocapture composition_primitives_timing_report
```

Observed on this host with a 640x360 frame and 200 iterations per case:

```text
transformed(identity): 30ns
transformed(scaled): 451.958us
transformed(rotated-90deg): 2.907309ms
clone + blend_over: 327.057us
clone + grayscale: 221.171us
solid: 38.688us
```

Rotation is intentionally recorded as a separate, slower reference path. The
GPU oracle covers the corresponding shader operation; this is measurement
evidence, not a production performance sign-off.

The multi-selection packet also exposes a release timing probe for the bounded
group geometry path:

```text
cargo test --release -p obs-rs-gui -- --ignored --nocapture callbacks::canvas::tests::multi_selection_geometry_timing_report
```

Observed with 16 selected items and 200 pointer-sample iterations:

```text
multi-selection: items=16 runs=200 per_sample=435ns checksum=5611200
```

The probe covers group bounds, pointer translation, and transform rebuilding;
it does not claim the end-to-end compositor or UI callback budget.

The same GUI suite now covers bounded Transform-menu geometry (Fit/Stretch to
Screen, centering, and edge alignment) and keyboard nudge dispatch. Arrow-key
actions use one atomic project command per event; regular arrows move 1 canvas
pixel and Shift+arrows move 10 pixels. These are correctness/interaction
checks, not a hot-path performance sign-off.

## Linux capability and soak probe

Command:

```text
cargo run -p obs-rs-app --bin obs-rs-linux-check
```

Observed result:

```text
x11        skip: connect to /tmp/.X11-unix/X0: Operation not permitted
x11_window skip: connect to /tmp/.X11-unix/X0: Operation not permitted
camera     skip: no native camera is present
pipewire   skip: audio device unavailable; PipeWire discovery exited with 255
av_soak    pass: ticks=300 chunks=1 packets=1297 bytes=3856642
             audio_blocks=997 audio_fallback_blocks=0
             rss_initial_kib=4688 rss_warm_kib=12856
             rss_peak_kib=12856 rss_final_kib=5388 elapsed_ms=452
```

The soak demonstrates bounded deterministic A/V coordination. It is not a
device, GPU, or production-output soak.

## Current hot-path architecture measured

The GUI preview now follows this path:

```text
scene source frames
  -> WGPU composition into a bounded preview target (for example 1048x590)
  -> GPU RGBA readback of the viewport target
  -> PreviewPresenter
  -> SharedPixelBuffer<Rgba8Pixel> copy
  -> Slint Image
```

The program/output path keeps its profile canvas target. The GUI program view
uses a separate bounded ProgramPreview target, while encoder conversion is an
explicit consumer of the full Program target (the encoder-role contract is
reserved for future fan-out). It remains bounded, although it still uses a
CPU-compatible NV12 readback until native encoder texture import exists. A
full-canvas RGBA readback is requested only for output scaling or when the
accelerated compositor is unavailable.

Within each WGPU composition, prepared layer ownership is retained for texture
recycling while the compositor consumes an exact-size iterator over those
layers. The former duplicate source-descriptor vector is no longer allocated
per frame; parameter buffers and bind groups remain explicit GPU resources and
are still a follow-up profiling target.

`obs-rs-render-wgpu` also has a GPU NV12 conversion/readback path for encoder
compatibility. The WGPU backend proves that readback is explicit, and the GUI
preview now requires only a viewport-sized CPU frame. A native surface presenter
remains the next performance target.

The portable image slideshow render probe reports 100 timestamped 640x360
renders in the release build at 2026-08-21:

```text
cargo test --release -p obs-rs-builtins image_slideshow_render_timing_report -- --nocapture
image slideshow: 100 x 640x360 renders = 3.183 us total (about 31 ns/render)
```

The source keeps decoded frames in its bounded control-plane set and selects
the timestamped frame without a per-render decode or file read. It still
requires the existing preview presenter to copy the selected pixels for Slint.

The portable replay-buffer push probe reports 1,000 encoded packet pushes in
the release build at 2026-08-21:

```text
cargo test --release -p obs-rs-output replay_buffer_push_timing_report -- --nocapture
replay buffer: 1000 packet pushes = 117.382 us (117 ns/push)
```

The buffer evicts oldest packet references by timestamp, byte budget, and
packet-count bound; payload storage remains shared with the encoder fan-out.

The audio gate timing probe reports 200 blocks of 480 stereo frames in the
release build at 2026-08-21:

```text
cargo test --release -p obs-rs-audio noise_gate_block_timing_report -- --nocapture
noise gate: 200 blocks x 480 stereo frames = 840.058 us total (about 4.2 us/block)
```

This uses the allocation-free maximum-absolute-sample detector and stateful
linear attack/hold/release envelope from the bounded Noise Gate core. RMS,
sidechain, and native Speex/RNNoise suppression processing are not included.

The mixer telemetry timing probe reports 200 blocks of 480 stereo frames in
the release build at 2026-08-21:

```text
cargo test --release -p obs-rs-audio mixer_block_timing_report -- --nocapture
mixer: 200 blocks x 480 stereo frames = 1.245127 ms (6.225 us/block)
```

This uses the allocation-free caller-owned output path and includes bounded
peak, peak-hold, and clip-indicator bookkeeping. Monitor taps are intentionally
not registered in this probe because they retain shared snapshots by contract.

The preview worker is bounded: pending requests and published results are
capacity-one latest-value slots. Demand-aware scheduling now requests up to
60 Hz for visible GUI/projector consumers, 5 Hz for a minimized idle window,
and no render work for a hidden idle window. An active output remains at 60 Hz
but does not request unused GUI preview/program-view frames. The demand policy
has pure state tests; live multi-monitor/projector cadence measurement remains
part of the final performance matrix.

## Windows acceptance and soak evidence

The Windows capability checker was run in a real interactive session on
2026-08-27 with the release capture helper and the default Windows audio path:

```text
cargo run -p obs-rs-app --bin obs-rs-windows-check
check=display status=pass detail=device=wgc-screen-b39ba7a49ee19eea_size=320x180_timestamp_ns=191817600
check=window status=pass detail=device=wgc-window-25f744a65d648162_size=320x180
check=camera status=skip detail=no_native_Nokhwa_camera_is_present
check=microphone status=pass detail=device=wasapi:{0.0.1.00000000}.{d78d2df4-628b-4a0e-97ed-0e363151c501}_frames=480_channels=1
check=desktop_loopback status=skip detail=audio_device_unavailable:_WASAPI_input_stream_failed:_A_buffer_underrun_or_overrun_occurred.
check=monitor_output status=pass detail=render_device=wasapi:{0.0.0.00000000}.{bdde2538-865e-4129-9573-2e798f22586f}_frames=480
check=av_soak status=pass detail=seconds=2_frames=194_audio_blocks=198_audio_device=wasapi:{0.0.1.00000000}.{d78d2df4-628b-4a0e-97ed-0e363151c501}
check=cleanup_restart status=pass detail=three_capture_start/stop_cycles_joined_cleanly
```

The run proves the bounded display/window helper lifecycle, microphone input,
monitor output, two-second audio/video progress, and three clean capture
restart cycles. This host had no native camera and its render loopback endpoint
reported an idle WASAPI underrun, so those capabilities are explicit hardware
skips rather than synthetic passes. The checker is a smoke/soak gate at
320x180 for two seconds; it is not a sustained 1080p30/60 performance sign-off.
The full matrix below remains required for release-grade latency, resource,
copy, allocation, deadline, and multi-hour soak measurements.

## Acceptance measurements to add

The next benchmark suite must record these fields for each fixture and for both
OBS Studio 32.2.2 and OBS-RS on the same machine:

| Measurement | Required form |
| --- | --- |
| GUI preview resolution | viewport pixels and canvas/output pixels separately |
| Render latency | p50/p95/p99/max, in milliseconds |
| Presentation latency | render completion to visible frame, in milliseconds |
| UI callback latency | p95 and max, in milliseconds |
| Audio latency | capture to mixer/output, in milliseconds |
| Encoder latency | p50/p95/p99 and queue depth |
| CPU/GPU/RAM/GPU memory | sampled time series and peak |
| Copies | bytes copied per frame, split GPU-to-CPU and CPU-to-GUI |
| Allocations | allocations/sec and bytes/sec on hot paths |
| Drops/deadlines | queue drops, missed deadlines, late frames |
| Soak | 30-minute and multi-hour RSS/queue/error trend |

Required fixtures are: one camera, one display, camera+display, 10 sources, 20
sources, browser-heavy, filter-heavy, 4K canvas, 1080p60 stream, simultaneous
stream+recording, Studio Mode, multiview, and multiple audio devices.

## Performance gate

The future sign-off gate remains:

```text
no unbounded allocation growth
no blocking file/network I/O on UI, audio, render, or capture threads
preview and output queues bounded
UI callback p95 < 4 ms
canvas pointer feedback <= one display frame
composition p95 < 80% of the target frame interval
```

For reference, the frame budgets are 16.67 ms at 60 FPS and 33.33 ms at 30
FPS. The current baseline does not pass the deadline portion of that gate.
