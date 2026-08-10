# OBS-RS Executive Summary

## Mission

Build a complete broadcasting, recording, and live-production application in Rust
from zero, using OBS Studio as the product reference while keeping the new engine's
ownership model, extension model, and tests Rust-native.

The project is not an incremental language migration. It does not promise source,
binary, or plugin compatibility with the existing OBS implementation. That choice
is intentional: it lets the new design use Rust ownership and traits at every
internal boundary instead of preserving historical native interfaces.

## Product target

The finished product should provide a headless engine and a desktop application with:

- scenes and ordered scene items;
- live sources, filters, transitions, and source settings;
- a clocked video pipeline with deterministic frame scheduling;
- an audio graph with mixing, monitoring, and synchronization;
- local recording and network streaming;
- hardware-accelerated rendering and encoding where a reviewed Rust integration is
  available;
- a Rust extension model with versioned, testable plugin contracts;
- portable profiles, scene collections, logs, recovery, and packaging.

The first milestone is deliberately smaller: a usable headless engine that can
register a source, build a scene, render a frame, and test all ownership and error
paths without a native host.

## Non-negotiable principles

1. **Rust owns the state.** Core state is represented by Rust types and borrowed or
   owned through explicit APIs. Shared mutable global state is prohibited.
2. **Safe by default.** Core crates use `#![forbid(unsafe_code)]`. An integration
   that cannot meet this rule is isolated, reviewed, and does not leak into the
   engine API.
3. **Rust interfaces, not historical ABI shims.** Plugins implement Rust traits and
   are registered at compile time in the first release. A future versioned dynamic
   format may use a sandboxed Rust-compatible boundary; it must not dictate unsafe
   layouts to the core.
4. **Deterministic behavior.** Configuration serialization, identifiers, scene
   ordering, timestamps, and test fixtures must be reproducible.
5. **Real-time discipline.** The media path must avoid unbounded allocation, locks of
   unknown duration, and blocking I/O. Every change to a hot path needs a benchmark.
6. **Evidence before expansion.** A phase advances only when its acceptance tests,
   benchmarks, and failure behavior are present in the repository.

## Scope boundary

This repository implements a new engine. It does not copy the existing OBS source
tree, preserve its native plugin ABI, or keep a compatibility layer as an implicit
requirement. Existing OBS scene/config formats may be supported later through
explicit Rust parsers and migration tools, but compatibility is a product feature,
not an internal architectural constraint.

The initial implementation also avoids platform SDKs and codec libraries. Capture,
rendering, encoding, and output backends will be introduced as separate Rust crates
after the portable contracts are stable.

## Definition of success

The project is successful when a fresh checkout can build and test the complete
application with the pinned Rust toolchain, run a desktop production workflow on
the supported platforms, and demonstrate equivalent functional behavior across
capture, scene composition, audio, recording, and streaming scenarios. The proof
must include long-running synchronization tests, resource accounting, reproducible
configuration recovery, plugin contract tests, and release artifacts.

Line count converted is not a success metric. A feature is complete when its Rust
behavior is usable, measured, documented, and maintainable.
