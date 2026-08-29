# Windows full-compatibility matrix

This document is the verification baseline for the `windows-full-compatibility`
work. It deliberately separates a Windows build from a usable feature. A row
is not complete until its last column has a recorded result from a real Windows
acceptance run.

Baseline: branch `windows-full-compatibility`, audited on 2026-08-28. The
implementation pass is recorded below; the final column remains open until an
archived Windows hardware run proves the complete behavior.

## Status legend

- `✅` verified by the current source/tests or a deterministic live probe.
- `◐` an implementation or probe exists, but the repository does not yet prove
  the complete user-facing behavior.
- `—` no applicable discovery step or no Windows implementation is present.
- `☐` no repeatable, recorded Windows end-to-end acceptance result exists.

The `Actually works` column is intentionally conservative. In particular,
`obs-rs-windows-check` is a runtime probe, but its hardware-dependent checks
may report `skip`; a successful process exit therefore does not mean that all
Windows features passed.

## Feature matrix

| Feature | Builds | Discovers | Actually captures/works | E2E tested on Windows | Evidence / next acceptance |
| --- | --- | --- | --- | --- | --- |
| Display capture | ✅ | ✅ | ◐ | ☐ | `obs-rs-capture-windows` launches the WGC helper and the Windows check probes one display. Capture at 30/60 fps on 1080p, 1440p, and 4K. |
| Window capture | ✅ | ✅ | ◐ | ☐ | The helper enumerates top-level windows and emits PID+HWND IDs that survive title changes and resize; legacy title/process IDs remain resolvable. Verify minimize, close/reopen, cloaking, and process restart. |
| Camera capture | ✅ | ✅ | ◐ | ☐ | Nokhwa is used with the Windows Media Foundation input feature. Verify integrated, USB/UVC, capture-card, replug, and mode negotiation cases. |
| Microphone input | ✅ | ✅ | ◐ | ☐ | WASAPI/CPAL input and format fallback are implemented. Record with a physical microphone and verify timestamp continuity. |
| Desktop/system audio | ✅ | ✅ | ◐ | ☐ | WASAPI output endpoints are opened as loopback inputs. Verify audible desktop playback, silence handling, and default-render-device changes. |
| Audio monitoring/output | ✅ | ✅ | ◐ | ☐ | WASAPI output sinks and the monitor worker exist. Verify monitoring while recording, format conversion, and unplug/replug recovery. |
| Image source | ✅ | — | ✅ | ◐ | Portable source and GUI tests cover the path. Run the packaged GUI and load PNG/JPEG/WebP files from a Windows path. |
| Image slideshow | ✅ | — | ✅ | ◐ | Portable slideshow tests cover timing and selection. Verify directory/file dialogs and long-running playback in the Windows GUI. |
| Text source | ✅ | — | ✅ | ◐ | Portable text rendering is covered. Verify fonts, Unicode text, and Windows DPI scaling in the packaged GUI. |
| Media source | ✅ | — | ◐ | ☐ | `media_source` is registered with an explicit idle/unavailable portable path and an optional GStreamer playbin/appsink path. Verify local MP4/WebM/H.264/AAC playback in the production package. |
| Scene compositing | ✅ | — | ✅ | ◐ | Core compositor and transform tests pass portably. Verify the WGC/camera/audio sources in a real Windows scene. |
| Transforms/crop/scale | ✅ | — | ✅ | ◐ | Core transform/scaler tests pass portably. Verify mixed-DPI source sizes and output scaling in the GUI. |
| Preview | ✅ | — | ◐ | ☐ | GUI smoke tests exercise wiring and the renderer has portable tests. Verify real WGC frames reach the visible preview. |
| Recording | ✅ | — | ◐ | ☐ | Reference `OBSRPKT1` recording works; normal Windows builds remain reference-only unless the optional native GStreamer runtime is supplied. Verify a playable production file. |
| Streaming | ✅ | — | ◐ | ☐ | Reference packet transports exist; production RTMP/SRT/etc. require the optional native GStreamer feature/runtime. Verify a real endpoint. |
| Source persistence | ✅ | — | ✅ | ◐ | Project round-trip tests preserve source settings and target IDs. Reload a Windows project and capture the same selected display/window. |
| Monitor/window hotplug | ✅ | ◐ | ◐ | ☐ | Discovery can be refreshed and capture loss triggers bounded reopen attempts; no hardware hotplug acceptance is recorded. |
| Audio device hotplug | ✅ | ✅ | ◐ | ☐ | WASAPI discovery snapshots, default-route refresh, and bounded engine reconnect logic exist; the Windows probe now checks stable IDs/default metadata. Change default devices and unplug the active route while recording. |
| Diagnostics | ✅ | — | ◐ | ☐ | Windows version, GPU backend/adapter, and helper version are included in the diagnostics path. Export and inspect a packaged diagnostic bundle. |
| Updater | ✅ | — | ◐ | ☐ | Windows-safe atomic publish logic exists. Exercise update, rollback/recovery, and locked-file behavior on a clean installation. |
| Packaging | ✅ | — | ◐ | ☐ | The ZIP contains the GUI, app, check binary, helper, manifests, and checksums. Install/run it on a clean Windows 10/11 machine. |

## Current verification commands

The repository currently exercises the following layers:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p obs-rs-gui -- --smoke
cargo build --manifest-path packaging/windows/capture-helper/Cargo.toml --release
cargo test --manifest-path packaging/windows/capture-helper/Cargo.toml
cargo clippy --manifest-path packaging/windows/capture-helper/Cargo.toml -- -D warnings
cargo run -p obs-rs-app --bin obs-rs-windows-check
OBSR_RS_REQUIRE_AUDIO_DEVICE_STABILITY=1 cargo run -p obs-rs-app --bin obs-rs-windows-check
packaging/windows/package.ps1 -Configuration release -OutputDirectory <directory>
packaging/windows/package.ps1 -Configuration dev-fast -OutputDirectory <directory>
packaging/windows/verify-package.ps1 -PackageDirectory <extracted-package>
packaging/windows/acceptance.ps1 -PackageDirectory <extracted-package> -RequireProduction
```

The Windows check should be run with `OBSR_CAPTURE_HELPER` pointing at the
release helper. Its output must be archived with the Windows build, including
`pass`, `skip`, and `fail` results, rather than reduced to the process exit
code. The first check also verifies that the packaged built-in plugin registers
the canonical `screen_capture`, `window_capture`, and `camera_capture` kinds;
this catches a mismatched/stale GUI package before a project can surface a
misleading "source kind is not registered" error.

On Windows, the source-properties display field belongs to the native
`screen_capture` source and lists the WGC display IDs returned by the helper.
The automatic entry means the primary display; Windows Graphics Capture does
not expose the X11-style whole-virtual-desktop target through this helper.
Projects imported from older Linux sessions migrate legacy Wayland/X11 screen
and window kinds when they are opened. Restart the rebuilt application and
reopen or reload the project before inspecting those properties; an already
running older binary can still show the legacy `Screen capture (Wayland)`
dialog.

## Implementation pass

The branch now covers the Windows user-facing boundaries that were previously
only scaffolded:

- projects loaded, recovered, imported, or switched on Windows migrate legacy
  `wayland_*`/`x11_*` capture kinds to Windows Graphics Capture while preserving
  source IDs and scene references; the document is left dirty so the target can
  be reviewed and saved;
- the screen and window property forms expose real WGC target discovery,
  refresh controls, cursor capture, and the WGC border policy; a missing saved
  target remains visible as unavailable instead of silently selecting another
  device;
- development builds find the separate helper under
  `packaging/windows/capture-helper/`, while packaged builds keep the helper
  beside the entry points and report its version in diagnostics;
- the Windows package includes `run-obs-rs.ps1` and
  `verify-package.ps1`; `-ProductionGStreamer` can copy a matching native
  runtime, plugin tree, and plugin scanner into a self-contained archive;
- the built-in `media_source` is registered on every platform and reports an
  explicit unavailable capability without the optional native GStreamer
  feature; production builds use a bounded playbin/appsink video path;
- the Windows check verifies canonical built-in capture-source registration,
  immediate display/window discovery stability, target-ID project round trips,
  and four-frame display runs at both 30 and 60 FPS; its audio stability check
  verifies endpoint identity/default-route invariants;
- native capture retries use media-time schedules, the helper publishes only
  its newest complete frame, bounded shutdown failures remain retryable, and
  live window IDs are tied to the owning PID/HWND rather than a mutable title;
- `.github/workflows/hardware-soak.yml` provides a self-hosted Windows lane
  for the real display, window, audio, reference-output, and cleanup probes.

These changes improve the runtime path but do not mark hardware or production
output rows complete. Those rows still require archived Windows 10/11 runs,
real media artifacts, and the GPU/audio/DPI matrix below.

## Latest local probe

On 2026-08-28, this Windows host passed display capture, window capture,
captured-frame reference recording, microphone input, monitor output, the
desktop loopback, the two-second A/V soak, and three capture start/stop cycles.
It also passed stable capture discovery, target persistence, and the 30/60 FPS
display probe. It skipped camera capture because no camera was connected. This
is host-specific evidence; it does not close the cross-version, cross-GPU,
camera, or production-output acceptance rows.

## Acceptance record format

Every hardware run should record at least:

```text
OS build: Windows 10 22H2 or Windows 11 <build>
GPU: Intel | AMD | NVIDIA, driver version
Displays: count, resolution, refresh rate, DPI, layout
Audio: microphone and render endpoint identifiers
Capture: display/window, start time, duration, frames, dropped frames
Output: recording profile/path or streaming protocol/endpoint
Lifecycle: resize, close/reopen, device changes, sleep/resume
Result: pass | fail, with the archived check output and media artifact
```

The P0 exit criteria are real display/window capture, microphone and desktop
loopback capture, helper recovery, and production recording/streaming. A green
compile, a reference packet, or a skipped hardware probe is not sufficient for
full Windows parity.
