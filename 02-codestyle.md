# Rust-Only Engineering Rules

## Safety policy

Every portable engine crate must begin with:

```rust
#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
```

Unsafe code is not a performance shortcut. If an operating-system or device
integration eventually needs it, that code belongs in a separately named adapter
crate with a written safety invariant, safe wrapper API, tests, and maintainer
approval. The core crates must remain safe and must not expose raw pointers.

The repository does not use native headers, generated bindings, foreign-function
interfaces, C-compatible layouts, or build-time code generation. Build logic should
remain in Cargo manifests and Rust source.

## API design

- Prefer small newtypes (`Identifier`, `Timestamp`, `SourceId`) over primitive
  values with implicit meaning.
- Validate at construction boundaries; make valid states representable afterward.
- Return `Result` for recoverable failures and `Option` for an expected absence.
- Never use `unwrap` or `expect` in library behavior. Tests may use them only when
  the fixture is part of the test's proof.
- Keep ownership visible in signatures. Do not return references whose lifetime is
  secretly tied to mutable global state.
- Use `BTreeMap` or explicitly ordered collections where deterministic output is a
  requirement.
- Keep trait objects object-safe and put object-safe APIs behind immutable contracts.

## Naming and modules

Crates use the `obs-rs-*` prefix. Modules use `snake_case`; types use `UpperCamelCase`;
methods and values use `snake_case`. Public types and methods require rustdoc that
explains invariants, failure behavior, and whether an operation is suitable for a
media callback.

One module should have one ownership responsibility. A large subsystem is split by
state owner and timing behavior, not by mirroring another project's file names.

## Error policy

Errors are typed, comparable where useful, and descriptive enough for logs. Library
errors must not contain process termination, logging side effects, or hidden retries.
The application layer decides how to display or persist them.

Panics are bugs in the engine. Boundary code must validate inputs before mutation so
that an error leaves the runtime usable. Long-lived worker tasks will report failure
through a channel and shut down in a known state rather than unwinding across an
uncontrolled boundary.

## Real-time rules

Code called from an audio callback, video scheduler, or render loop must document:

- allocation behavior;
- lock behavior and maximum wait;
- copy behavior and buffer ownership;
- timestamp and back-pressure semantics.

The first headless compositor is not yet a real-time path. It is deliberately a
simple CPU reference implementation against which later optimized paths can be
tested.

## Testing rules

Every public invariant gets a unit test. Subsystems add:

- malformed-input and boundary tests;
- deterministic golden fixtures;
- lifecycle tests for create/update/remove;
- property or stress tests when state is concurrent or time-dependent;
- benchmarks before and after media-path changes.

The CI gate is:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
```

## Dependencies

Use the standard library for small, stable primitives. Each external crate needs a
reason, license review, maintained versions, and a test that demonstrates why it is
needed. The lockfile is committed. New dependencies must not silently introduce a
native build step into the portable workspace.
