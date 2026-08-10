# Rust Code Style and FFI Conventions

## Status

This guide defines conventions for any future Rust code introduced as part of incremental OBS Studio Rust adoption. It does not authorize a source migration by itself.

The overriding compatibility rule is: **new Rust code must not require existing C/C++ callers or third-party plugins to change their ABI-visible behavior unless a separate compatibility decision explicitly approves that change.**

## Formatting

Use standard `rustfmt` formatting with default settings unless the repository later adopts a checked-in `rustfmt.toml` for a specific reason.

Required baseline:

```text
cargo fmt --all -- --check
```

Avoid style-only deviations from `rustfmt`; consistency is more valuable than local preferences.

## Lints

Use the following Clippy baseline for Rust crates:

```rust
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
```

CI should run Clippy with warnings treated as errors for code covered by the migration policy:

```text
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

`clippy::pedantic` exceptions are acceptable when they improve FFI correctness, readability, or performance, but each crate should keep exceptions narrow and documented rather than globally suppressing broad lint groups.

## Module and crate naming

Use `snake_case` Rust module names.

Where a Rust module maps directly to an existing C subsystem, mirror the existing conceptual name when that improves navigation. Examples:

- `libobs/obs-source.c` -> Rust module `obs_source`
- `libobs/obs-output.c` -> Rust module `obs_output`
- `libobs/obs-encoder.c` -> Rust module `obs_encoder`

Do not force one-to-one file mirroring where the C file is too large or spans multiple responsibilities. Rust crate/module boundaries should follow ownership and dependency boundaries, while retaining recognizable OBS terminology.

Crate names should be explicit and repository-scoped, for example `obs-rs-util` or another naming scheme approved by maintainers. Do not publish internal migration crates to crates.io by default.

## Public documentation

All public Rust items must have `///` documentation comments that describe:

- Purpose and semantics.
- Ownership and lifetime expectations.
- Thread-safety assumptions where relevant.
- Error conditions.
- Safety requirements for `unsafe` functions.
- ABI constraints for exported FFI items.

Public FFI functions must document nullability, pointer ownership, buffer lengths, valid enum ranges, callback lifetime, and whether the function may be called from real-time threads.

## Error handling

Inside Rust, prefer typed errors and `Result<T, E>` rather than C-style integer return codes or sentinel pointers.

Use `thiserror` for structured internal error types where it reduces boilerplate and improves diagnostics.

Example internal pattern:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid configuration key: {0}")]
    InvalidKey(String),

    #[error("invalid value for {key}: {value}")]
    InvalidValue { key: String, value: String },
}
```

At a C ABI boundary, translate `Result` into the existing OBS-compatible convention. Existing callers should not be forced to understand Rust error types.

For example, a Rust implementation replacing a C function that historically returns `bool`, `int`, or a nullable pointer should preserve that externally visible contract and log or expose detailed errors through an established OBS mechanism where appropriate.

## Panic policy

Rust panics must not unwind across an `extern "C"` boundary.

FFI entry points must be designed so that panics are prevented or contained. Depending on the final toolchain policy, use one or more of:

- `panic = "abort"` where acceptable for the crate/application profile.
- `std::panic::catch_unwind` around non-real-time FFI entry points when recovery is safe and well-defined.
- Internal APIs that make panics unlikely by avoiding `unwrap()`/`expect()` in externally driven paths.

Never rely on unwinding into C or C++ frames.

## `unsafe` policy

Treat `unsafe` as a localized implementation detail, not as a general escape hatch.

Requirements:

- Keep `unsafe` blocks as small as practical.
- Put pointer validation and conversion near the boundary.
- Document invariants with `// SAFETY:` comments.
- Prefer safe Rust types after crossing the FFI boundary.
- Do not store borrowed raw C pointers longer than the documented C lifetime permits.
- Do not infer thread safety from raw pointers; explicitly model `Send`/`Sync` requirements.

## C ABI compatibility

### Exported functions

Rust functions intended to satisfy existing C callers must use a stable C-compatible ABI and unmangled exported symbol names.

Typical pattern:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn obs_example_operation(arg: *mut obs_example_t) -> bool {
    // Validate and translate the pointer, then call safe Rust internals.
    true
}
```

The exact attribute syntax depends on the Rust edition/toolchain selected by the project. The build must verify exported symbol names on all supported platforms.

### Types

FFI-visible structs and enums must use explicit C-compatible representations where a Rust definition mirrors C layout:

```rust
#[repr(C)]
pub struct ObsExample {
    // ABI-stable fields only.
}
```

Do not expose ordinary Rust enums, `String`, `Vec<T>`, trait objects, references, slices, `Option<T>` with non-guaranteed layout, or Rust-owned generic containers directly to C.

Prefer opaque handles across the boundary. Keep allocation and destruction paired so ownership is unambiguous.

### Strings

Use C strings at the ABI boundary and convert immediately to validated Rust representations when possible.

Rules:

- Treat incoming `const char *` pointers as potentially null unless the existing C contract guarantees otherwise.
- Respect OBS's existing encoding expectations.
- Do not return a pointer to a temporary Rust `String`.
- When Rust allocates memory returned to C, provide a matching C-callable free function or use an existing OBS allocator contract when explicitly compatible.

### Callbacks

C callbacks must be represented as `extern "C"` function pointers with explicit userdata pointers matching existing OBS conventions.

Do not hold callback userdata after the documented registration lifetime. Do not invoke callbacks from different threads unless the existing API permits that behavior.

## Existing plugin ABI

The module loader in `libobs/obs-module.c` expects C symbols such as `obs_module_load`, `obs_module_set_pointer`, and `obs_module_ver` from plugin shared libraries.

If a plugin implementation is written partly or wholly in Rust, it must continue to export the same required symbols with the same C ABI and semantics. A Rust plugin must remain loadable by the existing native module loader without requiring the loader or third-party plugin ecosystem to adopt a Rust-specific ABI.

Conversely, if an internal `libobs` implementation is migrated to Rust, public C entry points and public headers must remain compatible so existing in-tree C/C++ code and already-compiled third-party plugins can continue to link and call into OBS unchanged, subject to the compatibility guarantees already provided by OBS.

## FFI wrapper architecture

Prefer a three-layer design:

1. **C ABI shim**: minimal pointer checks, representation conversion, error-code mapping, and symbol exports.
2. **Safe Rust core**: owns business logic and uses Rust types, `Result`, and explicit ownership.
3. **Native dependency adapter**: narrow `unsafe` FFI wrappers for platform APIs, FFmpeg, x264, vendor SDKs, or other native libraries.

This structure prevents native ABI concerns from spreading through the Rust implementation.

## Concurrency and real-time paths

OBS contains real-time audio/video paths. Rust code called from those paths must avoid accidental latency sources.

Do not introduce without measurement:

- Blocking mutexes on hot media threads.
- Heap allocation in per-frame or per-audio-block loops.
- Unbounded channels or queues.
- Logging in steady-state hot paths.
- Implicit copies of large audio/video buffers.
- Panic-catching or expensive validation on every frame when validation can occur at setup time.

Any migration touching `libobs/obs-video.c`, `libobs/obs-audio.c`, source rendering, encoder packet flow, or output packet flow requires before/after performance data.

## Dependencies

Add Rust crates conservatively.

For each dependency, evaluate:

- Maintenance status and release cadence.
- License compatibility.
- MSRV/toolchain requirements.
- Transitive dependency cost.
- Cross-platform support.
- Binary size and build-time impact.
- Whether the dependency executes on real-time paths.
- Whether a small local implementation is safer than a large dependency graph.

Avoid dependencies that silently pull in alternative async runtimes or platform stacks unless the subsystem explicitly needs them.
