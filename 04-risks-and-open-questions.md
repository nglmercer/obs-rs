# Risks and Open Questions

## Active risks

### Scope and schedule

A complete broadcasting application is a multi-year systems project. The main
mitigation is to keep a usable headless engine at every milestone and to make each
phase independently testable. The current MVP is intentionally not presented as
feature parity.

### Real-time behavior

Video and audio workloads punish unbounded allocations, hidden locks, and accidental
copies. The first compositor is a correctness reference, not a production scheduler.
Every move into a hot path needs queue-pressure tests, allocation measurements, and
long-duration synchronization evidence.

### Platform coverage

Screen capture, cameras, window enumeration, permissions, GPU contexts, and audio
devices differ by operating system. Portable traits must be designed before adapters;
each adapter needs a CPU/test fallback and explicit capability reporting.

### Codec and protocol availability

Pure Rust implementations may not yet cover every codec, hardware encoder, container,
or streaming protocol needed for a production release. The roadmap treats these as
separate capability decisions. An integration can be delayed without contaminating
the portable engine or pretending that a wrapper is a native Rust implementation.

### Plugin safety and compatibility

Trait objects are safe inside one build, but a Rust trait is not automatically a stable
binary ABI between independently compiled libraries. The first plugin model therefore
uses compile-time registration. Dynamic plugins require a versioned format, a threat
model, and a compatibility test suite before they are enabled.

### Memory and resource lifetime

Scenes, sources, frame queues, GPU resources, devices, and outputs have different
lifetime rules. Runtime-owned IDs and explicit removal are preferred to shared global
references. Each new resource type must have create, update, stop, and destroy tests.

### Dependency supply chain

Every dependency increases build, license, audit, and portability cost. The lockfile
is committed; external crates need a reason and an owner. A native build script or
unreviewed foreign binding is not allowed into the portable workspace.

## Decisions already made

- The project is a clean-room Rust implementation, not an in-place translation.
- Portable crates forbid unsafe code.
- The initial plugin model is compile-time Rust trait registration.
- The reference media format is owned RGBA8 on the CPU.
- The first runtime is single-threaded to make ownership and lifecycle behavior
  inspectable.
- The core has no native host dependency.

## Questions to answer before each phase

1. What is the smallest public Rust contract that proves the capability?
2. Which state owns each buffer, clock, device, and task?
3. What happens on missing data, back-pressure, cancellation, and shutdown?
4. What is the deterministic reference implementation?
5. What benchmark and soak fixture detects a regression?
6. Can the capability be tested on a machine without the target hardware?
7. Does a new dependency introduce a native build step or an unsafe boundary?

## Current open questions

- Which desktop UI toolkit gives the best Rust-native accessibility and platform
  support for Phase 7?
- Which video/audio formats should be first-class before GPU work begins?
- Which software encoders and containers can meet distribution and licensing goals
  without a native build dependency?
- Should dynamic plugins be sandboxed processes, WebAssembly modules, or remain
  compile-time only for the first release?
- What are the minimum supported CPU, GPU, OS, and device capabilities for a useful
  first release?
