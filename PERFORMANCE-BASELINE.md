# OBS-RS performance baseline

**Baseline date:** 2026-08-20  
**Repository commit:** `9b39072d62864700807c5e6f3f74c624429c45e2`  
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
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Fail | Existing lint drift in `obs-rs-project` and `obs-rs-benchmark`; see `KNOWN-BUGS.md`. |
| `cargo test --workspace --all-targets` | Fail | Most tests pass; two production GStreamer engine tests fail during native state transition. |
| `cargo test -p obs-rs-gui --bin obs-rs-gui` | Fail | 81 pass, 1 fails because the winit backend cannot find a Wayland compositor. |
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
render_p50_ns=813969
render_p95_ns=1093100
render_max_ns=1510380
frame_owned_buffers=0
frame_owned_bytes=0
frame_shared_clones=480
frame_cow_buffers=120
frame_copied_bytes=110592000
rss_before_kib=5796
rss_after_kib=9272
requested=120
processed=120
cancelled=false
empty=0
dropped_oldest=0
dropped_newest=0
missed=120
lateness_ns=168948548
max_lateness_ns=2655480
wait_ns=3799855926
paced_render_ns=168427665
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
elapsed_ms=3968
multi_workers=2
multi_requested=60
multi_processed=60
multi_missed=60
multi_lateness_ns=8757094
multi_produced_bytes=55296000
multi_peak_queued_bytes=921600
multi_elapsed_ns=967146375
```

The fixture is the historical 640x360@30 workload. Its measured render p95 is
1.093 ms, but the current wall-clock deadline accounting reports a miss for all
120 single-worker frames and all 60 multi-worker frames in this session. That
is a baseline finding, not evidence that a 60 FPS production path is accepted.
The bounded queue footprint is one 921,600-byte RGBA frame in this fixture.

The 110,592,000 copied bytes are exactly 120 RGBA frames at 640x360. This is the
known reference path cost; it does not yet include the full-resolution GUI
presentation cost at 1080p or 4K.

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

The GUI preview currently follows this path:

```text
scene source frames
  -> WGPU composition at profile canvas format
  -> GPU RGBA readback into VideoFrame
  -> SharedPixelBuffer<Rgba8Pixel> copy
  -> Slint Image
```

`obs-rs-render-wgpu` also has a GPU NV12 conversion/readback path for encoder
compatibility. The WGPU backend itself proves that readback is explicit, but the
GUI preview still requires a CPU frame. This is the first performance target.

The preview worker is bounded: pending requests and published results are
capacity-one latest-value slots. That bound must remain while replacing the
presentation path.

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

