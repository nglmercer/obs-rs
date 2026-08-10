# Risks and Open Questions

## Guiding constraint

This planning effort assumes **incremental, ABI-preserving Rust adoption only**. A full 100% migration is not considered an achievable end state while OBS retains its current third-party plugin ecosystem and native platform/media dependencies.

## Risk: breaking the third-party plugin ABI

### Why it matters

`libobs/obs-module.c` dynamically resolves required C symbols from plugin shared libraries, including `obs_module_load`, `obs_module_set_pointer`, and `obs_module_ver`. `libobs/obs-internal.h` stores these callbacks in `struct obs_module` and associates modules with registered source, output, encoder, and service types.

Third-party plugins may be closed source, independently maintained, or built with toolchains that the OBS project does not control. A migration that requires recompiling every plugin is not ABI-preserving.

### Mitigations

- Preserve existing exported C symbols and calling conventions.
- Keep public C headers stable unless a normal OBS compatibility policy explicitly allows change.
- Add automated symbol/ABI checks for Rust-produced libraries.
- Test against representative external binary plugins where legally and operationally practical.
- Prefer opaque handles and compatibility shims over exposing Rust layouts.
- Never unwind Rust panics across C ABI boundaries.

### Open questions

- What automated ABI-check tooling should become authoritative across Windows, macOS, and Linux?
- Which historical plugin binaries should be part of compatibility testing?
- Which public APIs are guaranteed at binary level versus source compatibility only?

## Risk: real-time audio/video performance regressions

### Why it matters

OBS is a real-time media application. `libobs/obs-video.c` and `libobs/obs-audio.c` sit on latency-sensitive paths, while `libobs/obs-source.c`, `libobs/obs-output.c`, and `libobs/obs-encoder.c` participate directly in rendering, audio mixing, encoding, packet delivery, and synchronization.

Rust does not automatically make these paths faster. Poor boundary design can add allocations, copies, locks, bounds checks in unsuitable locations, or callback/FFI overhead.

### Mitigations

- Keep early migration work off per-frame/per-audio-block hot paths.
- Establish performance baselines before touching media-critical code.
- Avoid allocations and blocking synchronization in real-time callbacks.
- Use zero-copy or ownership-transfer designs where the existing native API permits them.
- Benchmark FFI crossing frequency and batch operations where appropriate.
- Run long-duration A/V sync and dropped-frame tests.

### Open questions

- Which benchmark scenes and encoder/capture combinations should define acceptance thresholds?
- What regressions are acceptable for CPU, render time, encode time, audio callback timing, and memory?
- Which thread classes should be formally designated "real-time sensitive" for Rust code review rules?

## Risk: cross-platform parity

### Why it matters

OBS supports multiple operating systems and uses substantially different native stacks on each platform. The plugin tree includes Linux-specific modules such as `plugins/linux-pipewire/` and `plugins/linux-v4l2/`, macOS-specific modules such as `plugins/mac-avcapture/` and `plugins/mac-videotoolbox/`, and Windows-specific modules such as `plugins/win-dshow/` and `plugins/win-capture/`.

A Rust crate that works well on one platform may have incomplete APIs, different threading semantics, or different packaging constraints elsewhere.

### Mitigations

- Require Windows, macOS, and Linux support for shared core crates unless a crate is explicitly platform-specific.
- Keep platform-native adapters separate from portable Rust logic.
- Avoid selecting Rust dependencies based on single-platform convenience.
- Validate target triples, deployment targets, CRT choices, and linker behavior in CI.
- Preserve existing platform-specific native code when replacing it would add risk without user benefit.

### Open questions

- Which Rust target triples map exactly to OBS-supported build architectures?
- How will dependencies be vendored or mirrored for reproducible downstream packaging?
- How should platform-specific Rust crates be organized to avoid excessive conditional-compilation complexity?

## Risk: mixed C/C++/Rust build complexity

### Why it matters

OBS is currently built with CMake across the application, `libobs`, plugins, and platform-specific components. Adding Cargo introduces another dependency resolver, artifact graph, compiler toolchain, cache model, and set of platform target rules.

A migration that improves memory safety but makes OBS substantially harder to build, package, debug, or contribute to may not be a net win.

### Mitigations

- Integrate Cargo through a single documented CMake strategy.
- Pin toolchain expectations.
- Keep the number of crates and third-party Rust dependencies small initially.
- Ensure offline/reproducible packaging is possible.
- Make IDE/debugger instructions work for mixed-language call stacks.
- Track build-time and artifact-size regressions as first-class metrics.

### Open questions

- Corrosion, direct Cargo invocation, `cxx`, `cbindgen`, or a combination: which integration has the lowest maintenance cost for OBS?
- Will OBS vendor Cargo dependencies, rely on lockfiles plus network fetches, or use distribution-specific packaging?
- How will sanitizer builds interact with Rust code on each platform?
- How will symbol files and crash-reporting pipelines represent mixed Rust/C/C++ stacks?

## Risk: ownership and allocator mismatches

### Why it matters

OBS native code uses established allocation, reference-counting, object-context, callback, and weak-reference conventions. `libobs/obs-internal.h` contains shared context, weak-reference, mutex, list, and array state that Rust code must not reinterpret casually.

### Mitigations

- Make allocation ownership explicit at each FFI function.
- Prefer opaque handles.
- Pair every cross-boundary allocation with a defined destroy function.
- Do not mix Rust allocator ownership with native freeing unless the contract explicitly supports it.
- Characterize existing object lifetime and callback order before replacing implementations.

### Open questions

- Which OBS allocator functions, if any, should Rust wrappers use for objects returned to C?
- Which internal structs must remain C-owned indefinitely because their layout is shared across translation units or plugins?

## Risk: panic and exception boundary behavior

### Why it matters

A Rust panic crossing into C/C++ is not an acceptable error strategy. Conversely, C++ exceptions or platform callbacks must not violate assumptions in Rust code.

### Mitigations

- Define a repository panic policy before production Rust is introduced.
- Prevent unwinding across FFI.
- Map failures into existing OBS result/logging conventions.
- Keep FFI functions small and auditable.

### Open questions

- Should release Rust crates use `panic = "abort"`, boundary `catch_unwind`, or a mixed policy by subsystem?
- How should fatal versus recoverable Rust failures be surfaced in OBS logs and crash reports?

## Risk: dependency and supply-chain growth

### Why it matters

Introducing Rust can unintentionally introduce large transitive dependency graphs. This affects security review, build reproducibility, licensing, and update burden.

### Mitigations

- Review every new crate and its transitive graph.
- Commit and enforce `Cargo.lock` according to the chosen workspace model.
- Run dependency/license/security auditing in CI once Rust production code exists.
- Prefer standard-library solutions for small utilities.

### Open questions

- Which dependency-audit tools and policies should be mandatory?
- What license allowlist matches OBS distribution requirements?
- What is the process for responding to RustSec advisories in transitive dependencies?

## Decision questions before any code migration

Before Phase 1 begins, maintainers should resolve at least these questions:

1. What concrete engineering problem is Rust solving in the first candidate module?
2. What exact C ABI surface must remain stable?
3. What is the ownership model across that boundary?
4. Is the code called from a real-time or latency-sensitive thread?
5. What before/after tests establish behavioral equivalence?
6. What before/after benchmarks establish acceptable performance?
7. Does the candidate depend on a native SDK that will remain FFI-backed permanently?
8. Can the candidate be rolled back independently if the mixed-language cost is too high?

If these questions do not have precise answers, the candidate is not ready for migration.
