# OBS-RS performance baseline

**Baseline date:** 2026-08-20  
**Baseline commit:** `7afb7fa` (Phase 0 evidence)  
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
| `cargo test -p obs-rs-gui --bin obs-rs-gui` | Pass with 1 explicit environment ignore | 85 pass, 1 ignored because the winit backend cannot find a Wayland/X11 compositor. |
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
render_p50_ns=831798
render_p95_ns=1203533
render_max_ns=1691590
frame_owned_buffers=0
frame_owned_bytes=0
frame_shared_clones=480
frame_cow_buffers=120
frame_copied_bytes=110592000
rss_before_kib=5820
rss_after_kib=9340
requested=120
processed=120
cancelled=false
empty=0
dropped_oldest=0
dropped_newest=0
missed=120
lateness_ns=146789986
max_lateness_ns=1959319
wait_ns=3820858614
paced_render_ns=146632308
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
multi_lateness_ns=7654986
multi_produced_bytes=55296000
multi_peak_queued_bytes=921600
multi_elapsed_ns=967007923
```

The fixture is the historical 640x360@30 workload. Its measured render p95 is
1.204 ms, but the current wall-clock deadline accounting reports a miss for all
120 single-worker frames and all 60 multi-worker frames in this session. That
is a baseline finding, not evidence that a 60 FPS production path is accepted.
The bounded queue footprint is one 921,600-byte RGBA frame in this fixture.

The 110,592,000 copied bytes are exactly 120 RGBA frames at 640x360. This is the
reference engine workload, not the GUI presentation copy. The GUI worker now
requests a 1048x590 preview for a 1920x1080 canvas (and a proportionally bounded
target for 4K), so its Slint copy is separated and counted as `frame_copy_bytes`
in the live metrics string.

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
produces a 1048x590 preview while the program frame remains 1920x1080. The
remaining CPU readback is deliberate compatibility behavior behind
`PreviewPresenter`; a native surface presenter is still future work.

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

The program/output path keeps its profile canvas target. Encoder conversion is
an explicit consumer of that target (the encoder-role contract is reserved for
future fan-out) and remains bounded, although it still uses a CPU-compatible
NV12 readback until native encoder texture import exists.

`obs-rs-render-wgpu` also has a GPU NV12 conversion/readback path for encoder
compatibility. The WGPU backend proves that readback is explicit, and the GUI
preview now requires only a viewport-sized CPU frame. A native surface presenter
remains the next performance target.

The preview worker is bounded: pending requests and published results are
capacity-one latest-value slots. The desktop timer requests at most 60 Hz while
an output is active and 30 Hz while idle; visibility/minimized suspension is
still a follow-up demand-state packet.

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
