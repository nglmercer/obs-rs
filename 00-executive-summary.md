# Rust Migration: Executive Summary

## Purpose

OBS Studio is a large native application whose core runtime and public extension model are built around C and C++. A literal, repository-wide "100% Rust" rewrite is not an achievable migration target without either breaking compatibility with the existing third-party binary plugin ecosystem or retaining substantial native code behind FFI boundaries.

The realistic engineering objective is **incremental, ABI-compatible Rust adoption**: introduce Rust in well-isolated components where it provides clear safety or maintainability benefits, while preserving the existing C ABI and retaining native FFI boundaries for platform APIs, vendor SDKs, and other dependencies that do not have viable pure-Rust replacements.

## Current native architecture constraints

The core OBS runtime is `libobs/`. The process-wide runtime state is represented by `struct obs_core` in `libobs/obs-internal.h`, with registries for sources, outputs, encoders, and services plus video, audio, data, hotkey, signal, and module state. `libobs/obs.c` owns the global `struct obs_core *obs` and coordinates initialization and media-runtime behavior.

OBS plugins are loaded as native shared libraries. `libobs/obs-module.c` resolves required symbols from each plugin at runtime, including:

- `obs_module_load`
- `obs_module_set_pointer`
- `obs_module_ver`

The corresponding module structure and function pointers are declared in `libobs/obs-internal.h`. This C ABI is a compatibility boundary used by in-tree plugins and third-party plugins, including third-party binaries for which OBS does not control the source code or toolchain.

The desktop frontend is Qt-based. On current `master`, the frontend lives under `frontend/` (older documentation and historical references may call this area `UI/`). Qt `.ui` forms and the C++ frontend remain native application infrastructure and are not an appropriate first target for a Rust migration.

The repository also relies heavily on native platform SDKs and media libraries. Representative examples include:

- FFmpeg integration through `plugins/obs-ffmpeg/`.
- x264 integration through `plugins/obs-x264/`.
- Windows capture and device integrations such as `plugins/win-dshow/`, `plugins/win-capture/`, and Windows Media Foundation / platform APIs used by Windows-specific code.
- macOS AVFoundation capture through `plugins/mac-avcapture/` and other Apple-native integrations.
- Linux PipeWire and V4L2 through `plugins/linux-pipewire/` and `plugins/linux-v4l2/`.
- NVIDIA encoder integration through `plugins/obs-nvenc/`.
- Intel Quick Sync integration through `plugins/obs-qsv11/`.
- AMD hardware encoder support where provided through FFmpeg/platform integrations and vendor-facing native interfaces.

These native dependencies are not made "Rust" simply by calling them from Rust. A Rust component that still depends on those APIs would necessarily use FFI or a binding layer.

## Why "100% Rust" is not a realistic end state

A literal 100% Rust rewrite would require at least one of the following:

1. Reimplement every native dependency and platform integration in Rust, including Qt-facing frontend functionality, media codec libraries, capture stacks, and vendor encoder SDKs; or
2. Continue calling those native libraries through FFI, in which case the resulting application is not meaningfully 100% Rust; or
3. Replace the existing C plugin ABI with a Rust-specific ABI or API, which would break compatibility with existing third-party binary plugins unless a complete compatibility layer were retained.

The third option is particularly damaging because the native plugin ABI is an established extension contract. Existing closed-source or independently built plugins cannot be mass-recompiled by the OBS project.

Accordingly, **full 100% migration is not a stated achievable goal of this roadmap**.

## Realistic target: incremental, ABI-compatible Rust adoption

The migration target is to introduce Rust only where module boundaries are sufficiently isolated and where the resulting component can continue to expose the same externally visible C ABI.

Key principles:

- Preserve public C headers and calling conventions.
- Preserve required plugin entry-point symbols and native dynamic-loading behavior.
- Prefer leaf utilities and self-contained internal subsystems before media hot paths.
- Keep FFI boundaries explicit and narrow.
- Treat platform APIs, Qt, FFmpeg, x264, and vendor SDKs as native dependencies unless a production-grade replacement is proven.
- Require regression evidence for latency, frame pacing, audio synchronization, memory ownership, and cross-platform behavior before expanding Rust into media-critical paths.

## Explicit non-goals

This planning effort does **not** propose:

- Rewriting the Qt desktop frontend (`frontend/`, historically referenced as `UI/`) in Rust.
- Dropping or intentionally breaking compatibility with the existing third-party C plugin ABI.
- Replacing vendor hardware encoder SDKs such as NVIDIA NVENC, Intel Quick Sync, or AMD vendor/platform encoder interfaces with speculative pure-Rust implementations.
- Reimplementing FFmpeg, x264, DirectShow, Media Foundation, AVFoundation, PipeWire, V4L2, or equivalent platform/media stacks in Rust.
- Performing any immediate source migration in `libobs/`, `plugins/`, `frontend/`, or build-system files as part of this documentation package.

## Success definition

A successful Rust adoption program would improve safety and maintainability in selected components **without changing observable ABI behavior or degrading real-time media performance**. The end state is expected to remain a mixed C/C++/Rust application with carefully designed FFI boundaries.
