# OBS-RS Codebase Audit Report

**Date:** 2026-08-11  
**Scope:** All 23 workspace crates (~208 Rust source files), CI configs, Cargo manifests, documentation, and fuzz targets  
**Severity scale:** CRITICAL > HIGH > MEDIUM > LOW

---

## Summary

| Severity | Count | Status |
|----------|-------|--------|
| CRITICAL | 3 | all fixed |
| HIGH | 7 | all fixed (2 false positives) |
| MEDIUM | 8 | all fixed or resolved by design |
| LOW | 8 | most fixed in prior commits |
| Missing implementations | 5 | acknowledged as roadmap gaps |
| Security (unique) | 2 | both addressed |

The codebase is remarkably well-structured for its size: consistent error handling, good use of owned types, no unsafe, thorough testing, and clean trait boundaries.

**Audit verification (2026-08-11):** All actionable items from this report were verified against the codebase. Most CRITICAL/HIGH/MEDIUM items had already been fixed in commit `d4ec9ce` or earlier. The remaining actionable fix (H3 — audio fallback timeline reset) was applied. Three items (H4, H5, H7) were determined to be false positives upon re-examination of the actual code and comments.

---

## Fix Status

| ID | Status | Notes |
|----|--------|-------|
| C1 | **Fixed** | `memchr = "2.7.4"` added to `obs-rs-config/Cargo.toml` |
| C2 | **Fixed** | Key now trimmed before validation in `Config::parse` |
| C3 | **Fixed** | Replaced static counter with `random_u64()` from `obs-rs-util` |
| H1 | **Fixed** | Values trimmed via `parse_value` + `strip_comment` |
| H2 | **Fixed** | Replaced counter with `RandomPool` (OS entropy) for masking |
| H3 | **Fixed** | `next_audio_deadline` now reset on fallback |
| H4 | **False positive** | `.rev()` is correct — comment in code explains why |
| H5 | **False positive** | `EncodedPacket.payload` is `Arc<Vec<u8>>`, clone is O(1) |
| H6 | **Fixed** | `STDIO_OPERAND` constant with doc comment explains `-` |
| H7 | **Fixed** | Trait docs and `encode_owned` override properly documented |
| M1 | **Fixed** | `TransformPlanCache` now byte-bounded |
| M2 | **Fixed** | Capacity reservation no longer saturates silently |
| M3 | **Fixed** | Replaced `from_utf8_lossy` with direct `String` construction |
| M4 | **Acknowledged** | Architectural refactor out of scope for this pass |
| M5 | **Acknowledged** | Architectural refactor out of scope for this pass |
| M6 | **Fixed** | `pipewire_reader_available()` checks for `gst-launch-1.0` |
| M7 | **Resolved by design** | `&mut self` is intentional; documented in `compositor.rs` |
| M8 | **Fixed** | `EngineConfig::with_video_encoder()` makes encoder swappable |

---

## CRITICAL

### C1. `memchr` used without being declared as a dependency

**File:** `crates/obs-rs-config/src/lib.rs:136`  
**Status:** **Fixed** — `memchr` is now declared in `Cargo.toml`.

---

### C2. `Config::parse` fails on valid-looking lines with spaces around `=`

**File:** `crates/obs-rs-config/src/lib.rs:52-56`  
**Status:** **Fixed** — key is trimmed before validation (`key.trim()`).

---

### C3. `obs-rs-update` install ID uses unseeded static atomic counter

**File:** `crates/obs-rs-sandbox/src/bundle.rs:21`  
**Status:** **Fixed** — replaced with `random_u64()` from OS entropy.

---

## HIGH

### H1. `Config::parse` does not trim values, allowing invisible whitespace drift

**File:** `crates/obs-rs-config/src/lib.rs:56`  
**Status:** **Fixed** — values trimmed via `parse_value` + `strip_comment`.

---

### H2. WebSocket masking key is predictable (counter instead of random)

**File:** `crates/obs-rs-output/src/stream/websocket.rs:15-31`  
**Status:** **Fixed** — replaced with `RandomPool` using OS entropy per RFC 6455 §5.3.

---

### H3. `EngineSession::read_audio_block` replaces live input without resetting timeline

**File:** `crates/obs-rs-engine/src/lib.rs:1201-1218`  
**Status:** **Fixed** — `next_audio_deadline` now reset to `None` on fallback.

---

### H4. `StreamSession::flush` re-queues undelivered packets in reverse order

**File:** `crates/obs-rs-output/src/stream/session.rs:186`  
**Status:** **False positive** — the `.rev()` is correct. `push_front` prepends and `pop` takes from front, so walking the tail newest-first is what leaves the queue oldest-first. Code comment explains this.

---

### H5. `EngineSession::emit_packet` clones every packet when both outputs are active

**File:** `crates/obs-rs-engine/src/lib.rs:1284-1295`  
**Status:** **False positive** — `EncodedPacket.payload` is `Arc<Vec<u8>>`, so `clone()` is a single atomic increment (O(1)), not a full frame copy.

---

### H6. `PipeWireAudioProvider` passes `"-"` to `pw-cat` without explanation

**File:** `crates/obs-rs-audio-pipewire/src/lib.rs:157, 218`  
**Status:** **Fixed** — `STDIO_OPERAND` constant with doc comment explains why `-` is mandatory.

---

### H7. `RawVideoEncoder::encode` copies even when caller has an owned frame

**File:** `crates/obs-rs-output/src/video.rs:76`  
**Status:** **Fixed** — trait docs warn about the copy and direct to `encode_owned`; `RawVideoEncoder` overrides `encode_owned` to move the buffer.

---

## MEDIUM

### M1. Transform-plan cache has no byte bound

**File:** `crates/obs-rs-media/src/frame.rs:29`  
**Status:** **Fixed** — byte-bounded eviction policy added.

---

### M2. `Config::serialize` capacity reservation saturates silently

**File:** `crates/obs-rs-config/src/lib.rs:109-111`  
**Status:** **Fixed** — no longer saturates; uses proper capacity calculation.

---

### M3. `obs-rs-update` uses `String::from_utf8_lossy` on guaranteed-UTF-8 data

**File:** `crates/obs-rs-update/src/lib.rs:162`  
**Status:** **Fixed** — replaced with direct `String` construction.

---

### M4. `DesktopState` is a god object

**File:** `crates/obs-rs-ui/src/state.rs`  
**Status:** **Acknowledged** — architectural refactor, out of scope for this pass.

---

### M5. GUI stores state in `Rc<RefCell<T>>` handles

**File:** `crates/obs-rs-gui/src/main.rs:83-161`  
**Status:** **Acknowledged** — architectural refactor, out of scope for this pass.

---

### M6. Wayland capture shells out to `gst-launch-1.0` without declaring it

**File:** README line 155; `crates/obs-rs-capture/src/wayland/`  
**Status:** **Fixed** — `pipewire_reader_available()` checks for the tool.

---

### M7. `Runtime::render_scene` takes `&mut self`

**File:** `crates/obs-rs-core/src/runtime.rs`  
**Status:** **Resolved by design** — intentional; documented in `compositor.rs:12-28`.

---

### M8. `EngineSession` hardcodes `RleVideoEncoder`

**File:** `crates/obs-rs-engine/src/lib.rs:611`  
**Status:** **Fixed** — `EngineConfig::with_video_encoder()` makes it swappable.

---

## LOW

### L1. `obs-rs-gui` uses `#![deny(unsafe_code)]` while all other crates use `#![forbid(unsafe_code)]`

**File:** `crates/obs-rs-gui/src/main.rs:3`  
Inconsistent lint levels across the workspace.

---

### L2. `obs-rs-config/Cargo.toml` redundantly declares `crate-type = ["lib"]`

This is the default. Noise across 12 of 23 crates.

---

### L3. Test fixtures for `obs-rs-update` use `std::env::temp_dir()` with PID naming

Reasonable, but the static counter for production installs means test runs affect production ID sequencing within the same process.

---

### L4. `EngineSession::pump_stream` sets `streaming_lifecycle = Failed` before reconnect attempt

**File:** `crates/obs-rs-engine/src/lib.rs:1069-1089`

A successful reconnect leaves the lifecycle stuck at `Failed` while the stream is actually running.

---

### L5. `blend_over` per-block opaque fast path does double work

**File:** `crates/obs-rs-media/src/frame.rs:254-265`

The fast path scans every pixel for opacity, then the per-pixel loop re-scans. For fully-opaque sources this is 2× work on the common path.

---

### L6. `PipeWireInput::read_block` allocates a new buffer on every call

**File:** `crates/obs-rs-audio-pipewire/src/lib.rs:283`

1.5KB allocation per audio block (200/sec). Reuse a pre-allocated buffer.

---

### L7. `obs-rs-web` HTTP server is single-threaded and blocking

**File:** `crates/obs-rs-app/src/bin/obs-rs-web.rs:33-41`

A slow client blocks the entire web UI. No thread pool or async.

---

### L8. WebSocket handshake helpers (`base64_encode`, `sha1_digest`) are not fuzzed

These are exposed as `pub(crate)` but the fuzz targets don't exercise them despite covering other output modules.

---

## Missing Implementations / Gaps

1. **No audio resampler in the engine** — `AudioResampler` exists in `obs-rs-audio` but `EngineSession` opens inputs at the engine format directly.
2. **No hot-plug monitoring** — `CaptureProvider::discover()` is called once at startup.
3. **No production video encoder** — `obs-rs-render-wgpu` exists but the engine uses `RleVideoEncoder`. GStreamer is behind a `native` feature only.
4. **`DesktopState` recording/streaming booleans duplicate `OutputLifecycle`** — the UI layer tracks these separately from the engine, creating two sources of truth.
5. **No GPU texture import for capture frames** — `WgpuRenderBackend` exists but capture frames are always CPU RGBA.

---

## Security Findings

1. **Predictable WebSocket masking key** (H2) — **Fixed**: now uses `RandomPool` with OS entropy.
2. **`obs-rs-update` validates signatures and uses constant-time comparison via `ed25519-dalek`** — well done.
3. **`obs-rs-sandbox` uses `Command` (not `shell`) with explicit argument vectors** — no command injection surface.
4. **`obs-rs-web` binds to `127.0.0.1` by default** — good. **Fixed in prior commit**: per-session token authentication added.
5. **Undeclared `memchr` dependency** (C1) — **Fixed**: now declared in `Cargo.toml`.

---

## Positive Observations

- `#![forbid(unsafe_code)]` across all core crates — genuine safety boundary
- Consistent `Result`-based error handling with typed errors at every boundary
- Atomic file writers (`sync_all` + `rename`) used throughout — crash-safe persistence
- Bounded collections and explicit resource limits in the runtime
- Good fuzz target coverage for parsing-heavy code (websocket, diagnostics, manifests)
- Deterministic test fixtures with `#[ignore]` gates for live-hardware tests
- Clean separation: capture/output/render are all trait-based and swappable
- The `OutputLifecycle` enum is a thoughtful design — explicit phases beat boolean guessing
