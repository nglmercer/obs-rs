# Incremental Rust Adoption Roadmap

## Roadmap objective

This roadmap targets **incremental, ABI-preserving Rust adoption**, not a 100% Rust rewrite.

Each phase must preserve the existing C plugin ABI and cross-platform behavior. Advancement is based on measured engineering evidence, not percentage-of-lines-converted targets.

## Phase 0: Tooling and CI

### Scope

Establish a supported Rust toolchain without migrating production logic.

Potential build integration approaches include:

- CMake integration through Corrosion.
- Direct CMake custom-target integration around Cargo for narrowly scoped crates.
- `cxx` where a controlled C++ bridge is appropriate for internal frontend/native integration.
- `cbindgen` for generated C headers when generated declarations are appropriate and ABI review remains explicit.

Create a Cargo workspace for Rust crates and add Rust formatting, linting, unit-test, and build jobs alongside existing CI. The Rust toolchain version or release channel must be pinned according to an OBS-maintainer policy rather than following an unconstrained local default.

### Entry criteria

- Maintainer agreement that Rust is being evaluated as an additive language.
- A documented minimum supported Rust version or pinned toolchain policy.
- Agreement on supported target triples corresponding to OBS-supported Windows, macOS, and Linux builds.
- A selected CMake/Cargo integration approach with reproducible local and CI behavior.

### Exit criteria

- A minimal Rust crate can be configured and built from the normal OBS build flow on all required platforms.
- `cargo fmt`, Clippy, and Rust tests run in CI.
- Debug and release artifacts link reproducibly into a native test target.
- Symbol visibility, runtime-library packaging, and platform signing/notarization implications are documented.
- No behavior change occurs in production OBS code.

### Risk factors

- Longer configure/build times.
- Toolchain bootstrap complexity for contributors and packaging systems.
- Differences between Cargo target selection and CMake architecture configuration.
- Windows CRT/linker mismatches or macOS deployment-target mismatches.
- Rust dependency vendoring and offline/reproducible-build requirements.

### Required regression testing

- Existing C/C++ builds remain unchanged when Rust support is disabled or unused.
- CI matrix coverage for Windows, macOS, and Linux.
- Debug/release and supported architectures link successfully.
- Packaging smoke tests verify no unintended runtime dependency is introduced.

## Phase 1: Leaf utility code

### Scope

Migrate only self-contained utility code with no external native SDK dependency and no direct real-time media ownership.

Candidate categories, subject to repository-specific review, include:

- Isolated data-structure helpers.
- String parsing/validation utilities.
- UUID parsing, formatting, or generation helpers.
- Other deterministic utility routines with simple input/output boundaries.

Existing C callers should use C-ABI wrappers around the Rust implementation. Public C headers should remain unchanged unless an additive internal-only header is explicitly approved.

### Entry criteria

- Phase 0 tooling is stable on all supported platforms.
- Candidate has a narrow API and strong unit-test potential.
- Candidate is not on a frame-critical or audio-callback hot path, or measurements prove the boundary cost is negligible.
- Ownership and allocation behavior can be expressed unambiguously at the C/Rust boundary.

### Exit criteria

- Behavior matches the previous implementation for normal and invalid inputs.
- Existing C/C++ call sites do not require semantic changes.
- FFI wrappers have unit/integration tests for null pointers, lengths, allocation/free behavior, and error translation.
- Sanitizer/native tests and Rust tests show no ownership regressions.
- Performance is equal or acceptably close for the utility's actual call pattern.

### Risk factors

- String encoding or null-termination mismatches.
- Allocator mismatches across C and Rust.
- Accidental behavior changes in edge cases.
- Excessive FFI granularity causing needless call overhead.

### Required regression testing

- Golden/compatibility tests against the prior C behavior where practical.
- Fuzz/property testing for parsers and structured data utilities.
- Cross-platform tests for path and string behavior.
- Allocation/leak testing at repeated FFI boundaries.

## Phase 2: Self-contained subsystems

### Scope

Evaluate subsystems with stronger internal state but clear external boundaries and limited native SDK coupling. Examples for investigation include:

- Internal configuration parsing/validation components.
- Portions of hotkey management that can be separated from platform key-state backends.

The phrase "port hotkeys" must not be interpreted as rewriting OS-specific hotkey backends blindly. `libobs/obs-internal.h` shows hotkey state tied to platform context, thread state, mutexes, callbacks, bindings, and signals. A viable migration would separate platform/native edges from testable policy/state logic.

All existing public C headers and ABI-visible behavior must be preserved.

### Entry criteria

- Phase 1 demonstrates stable FFI and build practices.
- Subsystem boundaries and thread ownership are documented.
- Existing tests or new characterization tests cover current behavior.
- The subsystem can be divided into a safe Rust core plus narrow C/native adapters.

### Exit criteria

- Public C API/ABI remains compatible.
- Threading, callback ordering, and lifecycle semantics match existing behavior.
- Configuration round-tripping or hotkey behavior matches across platforms.
- No measurable regression appears in startup, shutdown, UI interaction, or media-thread responsiveness.

### Risk factors

- Hidden coupling through global `obs` state.
- Callback reentrancy and lifetime mistakes.
- Lock-order changes and deadlocks.
- Platform behavior divergence.
- Configuration compatibility regressions with existing profiles/scenes/settings.

### Required regression testing

- Characterization tests before migration.
- Stress tests for create/destroy/register/unregister cycles.
- Thread-safety and deadlock-focused tests.
- Cross-platform behavior tests.
- Full OBS launch/stream/record smoke tests to detect indirect regressions.

## Phase 3: Plugin-by-plugin evaluation

### Scope

Evaluate plugins individually rather than assuming all plugins are migration candidates.

Potential candidates are plugins whose core logic is dominated by portable protocol or data-processing code with production-grade Rust ecosystem support and only a narrow libobs/native edge.

Plugins tightly coupled to vendor SDKs, device APIs, or platform multimedia frameworks should be classified as:

- **Out of scope for full rewrite**, or
- **Rust orchestration with native FFI**, only where that delivers a clear benefit.

Examples of native-heavy plugins include:

- `plugins/obs-nvenc/` for NVIDIA encoder integration.
- `plugins/obs-qsv11/` for Intel Quick Sync.
- `plugins/mac-videotoolbox/` for Apple VideoToolbox.
- `plugins/mac-avcapture/` for AVFoundation capture.
- `plugins/linux-pipewire/` and `plugins/linux-v4l2/` for Linux capture stacks.
- `plugins/win-dshow/` and `plugins/win-capture/` for Windows capture APIs.
- `plugins/obs-ffmpeg/` and `plugins/obs-x264/` where the essential codec/media implementation remains native.

A Rust-authored plugin must still export the native module ABI expected by `libobs/obs-module.c`, including `obs_module_load`, `obs_module_set_pointer`, and `obs_module_ver`.

### Entry criteria

- Stable Rust plugin build/package pattern exists.
- ABI symbol checks run in CI.
- Candidate plugin has a documented dependency map.
- A clear migration benefit exists beyond line-count conversion.

### Exit criteria

- Plugin loads through the unchanged native module loader.
- Existing scene collections and settings remain compatible.
- Functional parity is demonstrated on every platform the plugin supports.
- CPU, GPU, memory, latency, and frame pacing meet existing expectations.
- Distribution/package signing works through normal OBS release pipelines.

### Risk factors

- Native SDK wrapper unsafety merely moves rather than disappears.
- Driver/version-specific behavior becomes harder to diagnose through additional FFI layers.
- Packaging complexity increases per platform.
- Plugin ABI or settings migration mistakes break user configurations.

### Required regression testing

- Plugin load/unload/reload tests.
- Settings compatibility tests.
- Device enumeration and hot-plug tests where applicable.
- Long-duration streaming/recording tests.
- Encoder/capture quality, latency, and dropped-frame benchmarks.
- Driver-version and supported-OS matrix testing for native SDK plugins.

## Phase 4: Permanent native boundaries — not fully achievable

### Status

**A 100% Rust end state is not fully achievable under the compatibility and dependency constraints of OBS Studio.**

This phase is therefore documentation and boundary stabilization, not a promise to finish converting all remaining native code.

### Expected permanent native/FFI areas

#### Qt frontend

The Qt desktop application under `frontend/` remains C++/Qt unless the project separately chooses to replace its GUI framework. Rewriting the entire UI is a product rewrite, not an incremental language migration.

#### Vendor hardware encoder SDKs

NVENC, Intel Quick Sync, AMD/vendor platform encoder interfaces, and Apple VideoToolbox are native SDK/API boundaries. Rust may wrap them, but wrapping does not eliminate the native dependency.

#### Platform capture APIs

DirectShow, Media Foundation, Windows Graphics Capture and related Windows APIs; AVFoundation and related Apple APIs; PipeWire, V4L2, and similar Linux APIs remain operating-system/native boundaries.

#### Media libraries

FFmpeg and x264 are foundational native media dependencies. Rust orchestration can call them through bindings, but reimplementing them is outside the scope and risk tolerance of an OBS migration.

#### Third-party binary plugins

The C plugin ABI must remain available so existing third-party binaries can load. Even if more internals become Rust, a stable native compatibility layer remains necessary.

### Entry criteria

- Earlier phases demonstrate that Rust provides measurable value in selected components.
- Maintainers have an explicit inventory of permanent native dependencies.
- ABI boundaries are documented and tested.

### Exit criteria

There is no "100% Rust" exit criterion.

This phase is complete when:

- Permanent C/C++/FFI boundaries are explicitly documented.
- ABI compatibility tests protect plugin loading and public C interfaces.
- Remaining native code is treated as intentional architecture rather than migration debt.
- Future Rust work is prioritized by safety, maintainability, performance, and user value rather than language-percentage targets.

### Risk factors

- Organizational pressure to equate migration success with line-count percentage.
- Rewriting stable native adapters without measurable benefit.
- Underestimating third-party plugin compatibility requirements.
- Treating FFI as temporary when it is actually the correct long-term boundary.

### Required regression testing

- Binary plugin compatibility tests.
- Full streaming/recording/capture/encoding scenario matrix.
- Long-duration soak tests for A/V sync and resource leaks.
- Cross-platform release-build and packaging validation.
- Performance baselines for CPU, GPU, memory, render time, encode time, dropped frames, and audio buffering.

## Program-wide go/no-go rule

No phase should proceed because a roadmap date has arrived. A migration candidate proceeds only when its boundary is understood, compatibility can be preserved, regression testing is sufficient, and the expected safety/maintenance benefit outweighs the mixed-toolchain and FFI cost.
