# Architecture gaps blocking OBS parity

This document translates the parity matrix into dependency-aware engineering
gaps. It is intentionally about ownership and boundaries, not a second feature
roadmap. Each gap should become one or more narrow work packets with an
independent reviewer.

## Priority gaps

| ID | Gap | Current evidence | Consequence | Required target | Dependencies / first packet |
| --- | --- | --- | --- | --- | --- |
| GAP-001 | Render-target ownership is conflated | `PreviewRenderer` renders the project canvas and `render_live_scene` reads it back as a CPU `VideoFrame` for GUI preview. | A small UI viewport pays for full canvas readback and CPU storage; output and GUI cannot be budgeted independently. | Add `ProgramRenderTarget`, `PreviewRenderTarget`, `ProjectorRenderTarget`, and `EncoderRenderTarget` concepts with explicit dimensions and consumers. | Work 004; preserve `obs-rs-media` CPU oracle and existing output scaling tests. |
| GAP-002 | No replaceable preview presenter | WGPU exposes `GpuFrameHandle`/`VideoSurface`, but the GUI directly chooses `readback` and `SharedPixelBuffer`. | A native/WGPU surface cannot replace the copy path without rewriting GUI callbacks. | Introduce a safe `PreviewPresenter` trait with CPU, Slint-copy, and future native-surface implementations. | Work 005; depends on GAP-001. |
| GAP-003 | GPU and encoder conversion are not a single fan-out graph | Composition can submit to WGPU, but the GUI preview uses RGBA readback and the encoder uses a separate NV12 readback bridge. | Duplicate transfers and target lifetimes make latency/copy accounting unclear. | One composed source texture feeds preview, projector, encoder conversion, and optional recording targets; each transfer is explicit and bounded. | Work 005; GPU Agent plus Output Agent review. |
| GAP-004 | Rendering cadence is not demand-aware | The preview worker coalesces requests, but the desktop preview timer keeps requesting frames while idle; no visibility/minimized/static cadence policy is centralized. | CPU/GPU work can continue when no consumer needs a frame. | Add demand state for visible/minimized/hidden/static scenes and separate audio meter/diagnostic cadence. | Work 006; depends on GAP-001 and preview metrics. |
| GAP-005 | Hot-path allocation evidence is incomplete | Safe owned-buffer counters and compositor counters exist; allocation tracing, copy attribution, and device-buffer reuse are not end-to-end. | Regressions can hide behind bounded queues while allocating every frame. | Add scoped allocation/copy counters or an approved profiler harness for capture, audio, filters, WGPU staging, encoder, Slint models, and diagnostics. | Work 007; Performance Agent. |
| GAP-006 | Canvas interaction state is implicit in UI properties | The stage computes fit scale/offset; selected item rectangle and drag draft live in separate callback/UI fields. Zoom, pan, snap, multi-selection, and keyboard state have no canonical owner. | New canvas features can duplicate state or accidentally persist pointer drafts. | Add toolkit-neutral `CanvasState { zoom, pan, selection, snapping, interaction }`; keep drafts transient and commit one command per gesture. | Work 008 onward; Canvas Agent. |
| GAP-007 | Scene-item identity is weaker than OBS | Runtime scene order is a list of `SourceId` values and deliberately collapses repeated references to the same source. | Groups, nested scenes, duplicate references, and per-instance transforms cannot be modeled correctly. | Resolve each scene item to a stable instance identity; let one source definition feed multiple scene-item instances. | Engine Agent before nested scenes/groups; affects project codec and compositor. |
| GAP-008 | Dock state is a flat row | Five parallel arrays represent order, weights, visibility, and floating membership. | Tabs, nested splits, insertion indicators, per-window geometry, and monitor/DPI restoration cannot be represented. | Implement `DockNode::Split`, `DockNode::Tabs`, and `DockNode::Dock` plus versioned layout persistence. | Work 014–015; Dock Agent. |
| GAP-009 | Filter persistence is ahead of filter execution | The project model accepts named filter kinds/settings, while runtime compilation recognizes only the small reference set. | A persisted filter can be silently absent from the render/audio graph. | Typed `AudioFilter`, `VideoFilter`, `AsyncVideoFilter`, and `GpuVideoFilter` nodes with explicit unavailable capability and CPU/reference oracle. | Work 018; Filter Agent and project/UI review. |
| GAP-010 | Audio is a reference mixer, not a full graph | Mixer, resampler, monitor taps, pacing, PipeWire discovery, and fallback exist; full device graph, hot plug, multiple tracks, and audio filters do not. | Advanced audio workflows and production synchronization are incomplete. | Model device/source/filter/gain/pan/bus/track/monitor routing with bounded blocks and clock ownership. | Work 019; Audio Agent, then platform agents. |
| GAP-011 | Production output boundary is not yet product-complete | `OBSRPKT1` and reference writers are robust fixtures; GStreamer provides an optional native boundary, but production startup tests fail in baseline. | The application can describe production profiles without a trustworthy cross-platform output path. | Separate `Encoder -> EncodedPacket -> Muxer -> Output`; capability negotiation must produce typed unavailable results and never silently fall back. | Work 022–024; Output Agent and Platform Agents. |
| GAP-012 | Services and transports are coupled at the product boundary | Typed protocol profiles and transport implementations exist, but there is no complete `StreamingService`/`StreamingTransport` split with service presets and auth lifecycle. | Reconnect, congestion, keyframe, service configuration, and diagnostics cannot evolve independently. | Define service configuration, transport session, bounded network worker, and redacted diagnostics as separate contracts. | Work 024; Output Agent. |
| GAP-013 | Platform crates are boundaries more than implementations | macOS and Windows crates return typed unavailable behavior off-platform; Linux live paths are environment-unverified. | Cross-platform parity cannot be claimed from portable tests. | Each platform agent supplies capture, audio, encoder, virtual-camera, permission, device-loss, and recovery evidence behind safe Rust traits. | Work 027–030; platform matrix and hardware CI. |
| GAP-014 | Plugin ecosystem stops short of product distribution | Versioned Rust API, manifest validation, signatures, quotas, and subprocess frames exist. | Dynamic source/filter/output/service/UI extensions, permission prompts, update policy, crash supervision, and diagnostics are incomplete. | Versioned manifests with permissions/resource quotas, isolated process supervision, signature/update model, and extension-facing dock contracts. | Work 031; Plugin Agent, Security Reviewer. |
| GAP-015 | Settings and hotkeys are not fully typed at the behavior boundary | Settings persist several hotkey display strings and many output fields; UI often maps display values directly. | Conflicts, global registration, runtime effects, and restart requirements are hard to validate centrally. | Structured setting schema and `KeyCombination` model with capability, validation, effect, and restart metadata. | Work 025–026; UI/Platform Agents. |
| GAP-016 | Visual QA does not yet compare the same states | Slint can render deterministic settings fixtures in English and Spanish; live compositor screenshots and OBS fixture capture are unavailable in this environment. | Spacing, focus, hover, menus, dock proportions, and canvas behavior can regress without a measurable diff. | Reference fixture catalog, scripted workflows, screenshot diff thresholds, and platform/DPI/locale matrix. | Work 033; QA Agent. |
| GAP-017 | Reliability evidence is shorter than the product target | 300-tick A/V soak and bounded worker tests pass; no multi-hour stream/record/device/GPU/network soak is present. | Memory growth, reconnect storms, output failure recovery, and device loss remain unproven. | 30-minute and multi-hour fault-injection soaks with RSS, queue, deadline, copy, error, and recovery telemetry. | Work 035; Performance + QA Agents. |

## Dependency graph

```text
truthful baseline / lint and test gate
                |
                v
render targets --> preview presenter --> demand-aware cadence
       |                  |                       |
       +----------> copy/allocation evidence <---+
                |
                v
canvas state --> scene-item identity --> source/filter graph
                                      |
                                      v
                              audio/output consumers
                                      |
                                      v
                     platform adapters / plugins / services
                                      |
                                      v
                         visual parity / soak certification
```

## Coordinator rules for closing a gap

1. The Code Auditor records current-state evidence before implementation.
2. The implementation packet names one owning crate/module and one observable
   behavior.
3. Hot-path changes include a benchmark and queue/allocation evidence.
4. Platform/native changes return typed capability or failure results and include
   a real-platform or explicitly unavailable test.
5. The Reviewer Agent compares the behavior with OBS 32.2.2 and does not write
   the implementation being reviewed.
6. The Coordinator updates `PARITY-MATRIX.md` only after implementation, tests,
   performance evidence, and review all pass.

