# OBS-RS Windows packaging

The portable package keeps `obs-rs-capture-windows-helper.exe` beside
`obs-rs-gui.exe`; the GUI searches that directory before the per-user
application directories. Build it from a Windows PowerShell prompt with:

```powershell
.\packaging\windows\package.ps1
```

The script creates `packaging/windows/dist/obs-rs-windows.zip`. Extract the
`obs-rs` directory and run `install.ps1` once if a per-user uninstall entry is
wanted. The entry points to the included `uninstall.ps1` and does not require
administrator access.

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
Then build with the opt-in feature:

```powershell
cargo build -p obs-rs-gui --features production-gstreamer --release
```

The Windows CI job intentionally runs the default feature set, so a machine
without those native GStreamer packages still has a complete check, test, and
GUI smoke path.
