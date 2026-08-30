# OBS-RS Windows packaging

The portable package contains the GUI, headless app, Windows acceptance check,
and `obs-rs-capture-windows-helper.exe`. The GUI searches beside its executable
before `%LOCALAPPDATA%\obs-rs\bin`, `%LOCALAPPDATA%\obs-rs`, and the matching
`%APPDATA%` directories, so launching it from another working directory does
not break capture.

Build it from a Windows PowerShell prompt with the pinned Rust toolchain and
the MSVC target installed:

```powershell
.\packaging\windows\package.ps1
```

For a local portable smoke package without release optimization, use the
incremental `dev-fast` profile. It writes and reads `target\dev-fast` for both
the workspace binaries and the separate capture helper:

```powershell
.\packaging\windows\package.ps1 -Configuration dev-fast
```

For faster local helper iteration, use the matching development profile:

```powershell
cargo helper
```

The `cargo gui` alias uses the bounded-memory `dev-fast-gui` profile. Build the
helper with the same profile when iterating on both components:

```powershell
cargo helper-gui
```

The helper has a separate target directory. Reclaim only its profile that is
consuming space with `cargo clean-helper-fast`, `cargo clean-helper-gui`, or
`cargo clean-helper-release`; these do not remove the root workspace cache.

Use `cargo gui-run` or `cargo gui-smoke` when launching the rebuilt GUI locally;
both commands select the same bounded-memory profile, so an older `target\\debug`
executable cannot be mistaken for the current Windows build.

The GUI searches the helper's `target\dev-fast-gui` and `target\dev-fast`
directories before the ordinary debug and release directories, so a local
fast build is picked up automatically.

The script creates `packaging/windows/dist/obs-rs-windows-<version>-x86_64.zip`
and a matching `.sha256` file. The archive includes `VERSION.txt`,
`SHA256SUMS.txt` (covering every payload file), `THIRD-PARTY-NOTICES.md`,
`WINDOWS-README.md`, the two app
entry points, the acceptance scripts, and the helper. Extract the `obs-rs`
directory and run `install.ps1` once if a per-user uninstall entry is wanted.
The entry points to the included `uninstall.ps1` and does not require
administrator access.

If the archive was built with `-ProductionGStreamer`, start the GUI with
`.\run-obs-rs.ps1 gui`. The launcher configures the bundled native DLLs,
approved plugin directory, and plugin scanner. The native adapter also
discovers the same `gstreamer` directory when an entry point is launched
directly. A default archive remains reference-output-only and does not include
GStreamer.

For a real interactive-session check after extraction:

```powershell
.\obs-rs-windows-check.exe
```

`pass`, `skip`, and `fail` are intentionally distinct. Hardware or privacy
conditions are reported as typed skips; protocol, frame-format, lifecycle, and
cleanup errors fail the command. On a production package, the native recording
check also runs the bundled `gst-discoverer-1.0.exe` against the preserved
Matroska artifact and requires both video and audio streams to be discoverable.

The first result is `capture_helper`. It verifies the packaged helper's
OBSRWIN1 protocol and compatible major version before any display or window
probe runs. The package verifier additionally requires the helper's exact
version to match `VERSION.txt`, so a stale minor-version helper cannot be
shipped in an otherwise valid archive. The acceptance script requires this
check, so a package with a missing or mismatched helper cannot be reported as a
hardware pass.

For a release-package hardware acceptance run, use the bundled script. It first
verifies every packaged payload, launches the extracted GUI through the bundled
runtime launcher, records machine/GPU/display/audio metadata, requires the
physical display, window, microphone, loopback, and monitor-output checks, and
writes bounded soak telemetry:

```powershell
.\acceptance.ps1 -SoakSeconds 1800
```

The runner keeps the human-readable `windows-check.txt` and also writes
`windows-check.json`. The JSON artifact includes the overall exit status,
pass/skip/fail counts, every parsed check with its detail, and any unexpected
launcher output. This makes an accepted run auditable without scraping console
text and makes hardware-limited skips visible in CI artifact viewers.

The default run validates the reference Windows package and does not require
the optional native GStreamer media source or production output backend. Use
`-RequireProduction` to require those native media/output checks.

Add `-RequireCamera` on a machine with a connected camera. A provisioned
production-output runner can pass `-RequireProduction` to require native output
capabilities and a real Matroska recording. Add `-ProductionStreamUrl` to also
require a real RTMP, RTMPS, SRT, or RIST endpoint; this switch implies the
production capability and recording checks. The endpoint may use RTMP, RTMPS,
SRT, RIST, or an unauthenticated `whip://`/`webrtc://` alias for an HTTPS WHIP
endpoint. It is used only for the live check and is not written to the telemetry
artifact. When native recording is enabled, the resulting
`production-recording.mkv` is kept in the acceptance artifact directory for
independent playback inspection. Add `-RequireProductionHls` to require the
local HLS playlist/segment check as well; it implies the native-runtime package
requirement but does not need a network endpoint.

## Platform requirements and limitations

The supported minimum is 64-bit Windows 10 version 1809, because the helper
uses Windows Graphics Capture. Display/window capture needs an unlocked,
interactive desktop. Windows may deny or blank minimized, occluded, protected,
DRM, secure-desktop, or privacy-restricted content. Window target IDs are
session-stable; display IDs use Windows monitor device names where available.
Per-monitor DPI and negative virtual-desktop coordinates are retained.
The native capture helper embeds a PerMonitorV2 application manifest, so the
monitor rectangles and window coordinates it reports are physical desktop
coordinates even when displays use different scaling factors.

The Audio page selects a default or explicit WASAPI microphone, render-device
loopback route, and local monitor output. Windows privacy settings govern
microphone and camera access. The engine preserves a missing explicit ID and
retries it rather than silently changing to another physical device.

## Production GStreamer on Windows

The default Windows build uses the reference output path and does not require
GStreamer. Builds that need RTMP, SRT, WebRTC, or HLS should install matching
64-bit MSVC GStreamer runtime and development packages first:

- GStreamer runtime: the MSVC x86_64 installer, including the base and good
  plugin sets;
- GStreamer development: the matching MSVC x86_64 development installer;
- any additional bad, ugly, or libav plugin bundles required by the selected
  production output.

The runtime `bin` directory must be on `PATH`, and `PKG_CONFIG_PATH` (or the
GStreamer pkg-config environment supplied by the installer) must point at the
matching development package. Keep runtime and development versions identical.
Then build and package with the opt-in feature and a matching runtime tree:

```powershell
.\packaging\windows\package.ps1 -ProductionGStreamer `
  -GStreamerRuntimeDirectory 'C:\Program Files\gstreamer\1.0\msvc_x86_64' `
  -ProductionStreamProtocol rtmp
```

`-ProductionStreamProtocol` is optional for a recording-only package. It accepts
`rtmp`, `rtmps`, `srt`, `rist`, `hls`, `whip`, or `webrtc` (`whip` and `webrtc`
share the `webrtc` gate). Multiple gates can be requested with a PowerShell
array, for example `-ProductionStreamProtocol rtmp,hls`; each gate requires its
corresponding sink, parser, and encoder elements before packaging. The selected
gates are written to `GSTREAMER-RUNTIME.txt` and rechecked by
`verify-package.ps1`.

The verifier accepts the same array syntax, for example
`.\verify-package.ps1 -RequiredStreamProtocol rtmp,hls`, and requires every
requested gate to be present in the package marker.

The runtime directory must contain `bin\gstreamer-1.0-0.dll`,
`bin\gst-inspect-1.0.exe`, `bin\gst-discoverer-1.0.exe`,
`lib\gstreamer-1.0`, and
`libexec\gstreamer-1.0\gst-plugin-scanner.exe`.
The matching development package is still required at build time. The
packager copies the runtime DLLs, capability/recording probes, and plugins
into the archive and writes the selected runtime into `GSTREAMER-RUNTIME.txt`.
Its package-time capability probe is pointed at that exact runtime tree, so a
different system-wide GStreamer installation cannot produce a false plugin
failure. The
launcher sets `OBSR_GST_INSPECT` so capability discovery cannot accidentally
use a different GStreamer installation from `PATH`; direct entry-point
launches apply the equivalent bundled-runtime setup in the native adapter.

The Windows CI job intentionally runs the default feature set, so a machine
without those native GStreamer packages still has a complete check, test, and
GUI smoke path.

The production acceptance workflow is a separate manual GitHub Actions lane:
`.github/workflows/windows-production.yml`. Its self-hosted runner must be
labelled `obs-rs-production` and expose `GSTREAMER_1_0_ROOT` and
`GSTREAMER_1_0_DEVEL_ROOT` as runner environment variables pointing to matching
64-bit MSVC runtime and development installations. It packages the native
runtime, verifies the extracted archive, and runs the real display, audio,
recording, and cleanup checks. Set the optional repository/environment secret
`OBS_RS_PRODUCTION_STREAM_URL` to require a real RTMP, RTMPS, SRT, RIST, or
unauthenticated WHIP streaming acceptance; set the repository variable
`OBS_RS_REQUIRE_PRODUCTION_STREAMING` to `1` when that secret must be present.
The check also exercises local HLS playlist/segment output whenever the
packaged runtime exposes `hlssink2`; set `OBS_RS_REQUIRE_PRODUCTION_HLS=1` to
make that check mandatory on a production runner. The production workflow
forwards that repository variable as `-RequireProductionHls` and adds an HLS
package gate alongside any external streaming protocol gate.
