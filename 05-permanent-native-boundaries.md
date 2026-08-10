# Phase 4: Permanent Native Boundaries

## Status

Phase 4 is complete for this workspace as a boundary-stabilization record. It does
not promise a 100% Rust rewrite. The workspace contains migration-oriented Rust
components and ABI probes, but it does not contain the OBS native source tree, the
native build graph, or generated platform SDK bindings.

The required module export signatures are based on the authoritative OBS
[`libobs/obs-module.h`](https://github.com/obsproject/obs-studio/blob/master/libobs/obs-module.h).
The local probe verifies symbol shape and dynamic loading; it must receive the target
checkout's generated `LIBOBS_API_VER` before it can be evaluated as a real plugin.

## Boundary inventory

| Boundary | Why it remains native | Rust role allowed by this project | Acceptance guard |
| --- | --- | --- | --- |
| Qt frontend under `frontend/` | Qt widgets, `.ui` forms, platform integration, and the C++ application shell are product infrastructure. | Narrow C ABI or C++ bridge to isolated control-plane logic. | No frontend rewrite is implied; UI behavior and startup remain native. |
| Vendor encoders | NVENC, Intel Quick Sync, AMD/vendor interfaces, and Apple VideoToolbox are SDK contracts owned by platform or hardware vendors. | Safe orchestration around narrow, audited FFI adapters. | Driver/API matrix, encode quality, latency, and dropped-frame benchmarks. |
| Platform capture APIs | DirectShow, Media Foundation, Windows capture APIs, AVFoundation, PipeWire, and V4L2 expose OS/device behavior. | Portable policy/state code beside native adapters. | Device enumeration, hot-plug, supported-OS, and long-duration capture tests. |
| Media libraries | FFmpeg and x264 provide foundational codec/mux/protocol implementations. | Explicit bindings or orchestration; no speculative reimplementation. | Stream/record compatibility, A/V sync, CPU/GPU, and packaging tests. |
| Third-party plugin ABI | Existing binary plugins depend on C symbols, layouts, calling conventions, and native loader behavior. | Rust may implement a plugin behind the unchanged C ABI. | Required symbol checks, loader smoke tests, ABI review, and representative binary-plugin tests. |
| OBS process-wide runtime | `struct obs_core`, synchronization, callback tables, and native object lifetimes are shared by the C/C++ tree. | Migrate only isolated policy/state after characterization; keep opaque handles. | Ownership, callback order, lock behavior, and rollback evidence. |

## Local evidence

The current workspace protects the boundaries it can actually observe:

- `obs-rs-util` exposes paired allocation/free functions and tests nullability,
  lengths, error translation, and exported symbols.
- `obs-rs-config` keeps state ownership in Rust, exposes an opaque C handle, and
  tests round-tripping, buffer sizing, mutation, destruction, and invalid input.
- `obs-rs-plugin-probe` exports `obs_module_load`,
  `obs_module_set_pointer`, and `obs_module_ver`; `tests/plugin_abi_smoke.c` loads
  the shared library with the platform dynamic loader and checks the required
  symbols.
- `.github/workflows/rust.yml` runs formatting, workspace checks, Clippy, Rust
  tests, release builds, C header compilation, and ABI smoke tests.

These checks prove the local Rust/C contracts only. They do not prove compatibility
with an OBS binary or third-party plugin until this workspace is integrated into an
actual OBS checkout and tested against its generated headers and loader.

## Go/no-go rule

No later migration should remove a native boundary merely to increase a Rust line
count. A candidate must have a concrete safety or maintenance benefit, an explicit
ownership model, characterization tests, performance evidence when relevant, and a
rollback path. If those conditions are absent, the native implementation is an
intentional permanent boundary rather than unfinished migration work.
