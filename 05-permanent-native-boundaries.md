# Rust-Native Boundary Policy

## Purpose

The old planning direction treated native boundaries as permanent and accepted a
mixed-language application as the end state. OBS-RS has a different objective: the
portable engine and product code are Rust, and no foreign ABI is allowed to become a
silent dependency.

This document defines how future integrations remain aligned with that objective.

## Required boundary shape

Every subsystem exposes a safe Rust trait or value contract to the rest of the
workspace. A backend may have platform-specific implementation details, but it must:

- live in a separately named crate;
- return typed errors and capabilities;
- have a CPU or simulated test implementation when practical;
- document ownership, threading, allocation, and shutdown behavior;
- avoid raw pointers and foreign layouts in the public API;
- pass the workspace quality gates and its own integration tests.

## Current evidence

The current workspace contains only Rust source and Cargo metadata. The portable
crates forbid unsafe code, and the CI workflow invokes Cargo checks and tests only.
The headless demo exercises the current path from plugin registration through
rendering, PNG output, an `OBSFRM01` frame-stream round trip, independent clock drift,
and recovery diagnostics. On Linux, the built-in `x11_screen_capture` source contains
a direct Rust X11 wire-protocol adapter with fixture-tested setup and pixel decoding.
The terminal and loopback browser frontends reuse the same validated Rust-owned UI
state. `obs-rs-gui` adds a Slint desktop control room whose callbacks dispatch into
that state; the GUI crate's smoke mode constructs the component without requiring a
long-running event loop.

This is evidence of a Rust-native foundation and safe integration seams, not evidence
that direct OS capture, GPU acceleration, production codecs/protocols, or native
desktop packaging are complete. Those capabilities remain on the roadmap and must
supply their own evidence.

## Integration decision matrix

| Integration | First implementation | Acceptance gate |
| --- | --- | --- |
| Video capture | simulated/test source, then direct Rust platform protocol | device lifecycle, permissions, timestamps, fallback |
| Audio input/output | offline buffers and simulated clock | sample count, drift, underflow, latency |
| GPU rendering | CPU reference renderer first | format parity, context loss, resource cleanup |
| Encoding | Rust packet/encoder traits | deterministic fixtures, quality, bounded back-pressure |
| Streaming | output trait with a fake transport | reconnect, cancellation, no capture-thread blocking |
| Plugins | compile-time Rust registration with API versioning | version checks, isolation, diagnostics |
| Desktop UI | Rust application state plus Slint control-room adapter | live preview/editor workflows, accessibility audit, recovery, cross-platform packaging |

## Prohibited shortcuts

Do not add a foreign ABI merely to make an unfinished subsystem appear complete. Do
not expose a backend's internal layout to the core. Do not call a wrapper “pure Rust”
when its correctness depends on an unreviewed native build. If a capability is not
ready, keep the reference implementation and mark the feature as unavailable.

## Exit condition

There is no claim of completion until the roadmap phases that are in scope for a
release have passed their acceptance gates. The project may ship a useful subset,
but its release notes must identify unsupported capture, codec, output, or UI
capabilities instead of hiding them behind an architectural exception.
