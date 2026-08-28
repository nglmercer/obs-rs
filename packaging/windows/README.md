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

The script creates `packaging/windows/dist/obs-rs-windows-<version>-x86_64.zip`
and a matching `.sha256` file. The archive includes `VERSION.txt`,
`SHA256SUMS.txt`, `THIRD-PARTY-NOTICES.md`, `WINDOWS-README.md`, the two app
entry points, the acceptance checker, and the helper. Extract the `obs-rs`
directory and run `install.ps1` once if a per-user uninstall entry is wanted.
The entry points to the included `uninstall.ps1` and does not require
administrator access.

If the archive was built with `-ProductionGStreamer`, start the GUI with
`.\run-obs-rs.ps1 gui`. The launcher configures the bundled native DLLs,
approved plugin directory, and plugin scanner. A default archive remains
reference-output-only and does not include GStreamer.

For a real interactive-session check after extraction:

```powershell
.\obs-rs-windows-check.exe
```

`pass`, `skip`, and `fail` are intentionally distinct. Hardware or privacy
conditions are reported as typed skips; protocol, frame-format, lifecycle, and
cleanup errors fail the command.

## Platform requirements and limitations

The supported minimum is 64-bit Windows 10 version 1809, because the helper
uses Windows Graphics Capture. Display/window capture needs an unlocked,
interactive desktop. Windows may deny or blank minimized, occluded, protected,
DRM, secure-desktop, or privacy-restricted content. Window target IDs are
session-stable; display IDs use Windows monitor device names where available.
Per-monitor DPI and negative virtual-desktop coordinates are retained.

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
  -GStreamerRuntimeDirectory 'C:\Program Files\gstreamer\1.0\msvc_x86_64'
```

The runtime directory must contain `bin`, `lib\gstreamer-1.0`, and, when
provided by the installer, `libexec\gstreamer-1.0\gst-plugin-scanner.exe`.
The matching development package is still required at build time. The
packager copies the runtime DLLs and plugins into the archive and writes the
selected runtime into `GSTREAMER-RUNTIME.txt`.

The Windows CI job intentionally runs the default feature set, so a machine
without those native GStreamer packages still has a complete check, test, and
GUI smoke path.
