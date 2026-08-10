# OBS Studio Rust Migration Planning

This directory contains planning documentation and the initial Cargo workspace for
**incremental, ABI-compatible Rust adoption** in OBS Studio.

It does **not** define a goal of rewriting OBS Studio 100% in Rust. OBS is built around a native `libobs` core, a C plugin ABI, a Qt/C++ desktop frontend, and many native platform/media/vendor dependencies. A realistic migration keeps stable C ABI boundaries and uses Rust only where isolated components can be migrated without breaking third-party plugins or real-time media behavior.

On current `master`, the Qt frontend is under `frontend/`; older references may call this area `UI/`.

## Documents

1. [00-executive-summary.md](00-executive-summary.md) — feasibility, scope, native dependency constraints, realistic target, and explicit non-goals.
2. [01-current-architecture.md](01-current-architecture.md) — `libobs` core, source/output/encoder/module subsystems, plugin structure, frontend boundary, and architecture diagram.
3. [02-codestyle.md](02-codestyle.md) — Rust formatting, linting, error handling, `unsafe`, panic, documentation, and C ABI/FFI conventions.
4. [03-roadmap.md](03-roadmap.md) — phased tooling, utility, subsystem, and plugin evaluation roadmap with entry/exit criteria, risks, and regression testing.
5. [04-risks-and-open-questions.md](04-risks-and-open-questions.md) — plugin ABI, real-time performance, cross-platform parity, mixed-toolchain, ownership, panic, and supply-chain risks.

## Core principle

**Full 100% Rust migration is not an achievable goal under the stated compatibility constraints.** The roadmap targets incremental adoption while preserving public/native compatibility and retaining FFI for Qt, native platform APIs, FFmpeg/x264, hardware encoder SDKs, and other dependencies where a pure-Rust replacement is not viable.

## Referenced code areas

The planning package is grounded in current repository areas including:

- `libobs/obs.c`
- `libobs/obs-internal.h`
- `libobs/obs-source.c`
- `libobs/obs-output.c`
- `libobs/obs-encoder.c`
- `libobs/obs-module.c`
- `libobs/obs-video.c`
- `libobs/obs-audio.c`
- `plugins/CMakeLists.txt`
- `plugins/`
- `frontend/`

The Rust workspace is intentionally limited to Phase 0 scaffolding. It does not yet
change `libobs/`, plugins, the frontend, or the native OBS build. The first crate is
`obs-rs-util`, a placeholder for a future leaf-utility evaluation; it currently has no
OBS production logic.

## Rust workspace

The project uses the Rust toolchain declared in [rust-toolchain.toml](rust-toolchain.toml)
and keeps Rust crates under `crates/`.

Run the Phase 0 checks from this directory:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
```

The same checks run in [`.github/workflows/rust.yml`](.github/workflows/rust.yml).

## Phase status

- Phase 0 — tooling and workspace: complete.
- Phase 1 — leaf utility evaluation: complete for the isolated
  `obs-rs-util` identifier candidate. Its C ABI contract is declared in
  [`include/obs_rs_util.h`](include/obs_rs_util.h), with Rust-side tests covering
  invalid input, error translation, and paired allocation/free behavior. The C
  contract is additionally exercised by [`tests/ffi_smoke.c`](tests/ffi_smoke.c).
  No OBS call sites were changed because this workspace does not contain the native
  OBS source tree; integration remains a later, repository-level step.
- Phase 2 — self-contained subsystem evaluation: complete for the isolated
  [`obs-rs-config`](crates/obs-rs-config/) component. It provides deterministic
  parsing, validation, round-tripping, explicit buffer ownership, and an opaque
  non-thread-safe C handle. No global OBS state or platform hotkey backend is
  present in this workspace, so those native boundaries remain untouched.
