# Architecture gaps blocking OBS parity

This document translates the parity matrix into dependency-aware engineering
gaps. It is intentionally about ownership and boundaries, not a second feature
roadmap. Each gap should become one or more narrow work packets with an
independent reviewer.

## Phase 1 progress

The first performance packet is now implemented:

* `RenderTarget`/`RenderTargetRole` makes Program, Preview, Projector, and
  Encoder ownership explicit at the portable render boundary.
* WGPU composition maps canvas-space source layers into a distinct target
  format, so a 1920x1080 or 4K canvas can feed a bounded 1048x590 preview.
* The GUI keeps program/output rendering at the profile canvas format while
  preview requests use the bounded viewport format.
* `PreviewPresenter` isolates the remaining Slint-owned RGBA copy, and live
  metrics count the bytes copied into presentation storage.
* The preview timer remains capacity-one and now requests up to 60 Hz during an
  active output and 30 Hz while idle.

The remaining P0 work is native surface presentation, visibility/minimized
demand state, GPU target fan-out without duplicate transfers, and complete
allocation profiling. The CPU compositor now avoids duplicate pulls when a
scene references one source more than once. These are intentionally still
open; passing a viewport test is not the same as zero-copy or final
performance certification.

## Priority gaps

| ID | Gap | Current evidence | Consequence | Required target | Dependencies / first packet |
| --- | --- | --- | --- | --- | --- |
| GAP-001 | Render-target ownership is conflated | `RenderTarget`/`RenderTargetRole` and role-keyed WGPU targets now separate program, preview, projector, and encoder ownership; GUI preview no longer targets the full canvas by default. | Native fan-out and projector lifetimes are not yet connected, and the compatibility readback remains explicit CPU storage. | Finish role-specific fan-out and native target import while preserving the CPU oracle and output scaling tests. | Work 005–006; preserve `obs-rs-media` CPU oracle and existing output scaling tests. |
| GAP-002 | No replaceable preview presenter | A safe `PreviewPresenter` trait now owns Slint conversion behind one boundary; the current implementation is `SlintPreviewPresenter`. | A native/WGPU surface implementation is still unavailable, so one viewport-sized copy remains. | Add native/WGPU presenter implementations and keep copy-byte accounting for the fallback. | Work 005; depends on GAP-001. |
| GAP-003 | GPU and encoder conversion are not a single fan-out graph | Composition can submit to WGPU, but the GUI preview uses RGBA readback and the encoder uses a separate NV12 readback bridge. | Duplicate transfers and target lifetimes make latency/copy accounting unclear. | One composed source texture feeds preview, projector, encoder conversion, and optional recording targets; each transfer is explicit and bounded. | Work 005; GPU Agent plus Output Agent review. |
| GAP-004 | Rendering cadence is not demand-aware | The worker coalesces requests and the desktop timer now uses 60 Hz active-output/30 Hz idle cadence; visibility/minimized/static demand is not centralized. | Hidden or minimized windows can still perform preview work, and static demand is not event-driven. | Add demand state for visible/minimized/hidden/static scenes and separate audio meter/diagnostic cadence. | Work 006; depends on GAP-001 and preview metrics. |
| GAP-005 | Hot-path allocation evidence is incomplete | Preview target dimensions and Slint-copy bytes are now observable; static frames use shared immutable storage, but allocation tracing and WGPU staging attribution are not end-to-end. | Regressions can hide behind bounded queues while allocating in capture/audio/filter/diagnostic paths. | Add scoped allocation/copy counters or an approved profiler harness for capture, audio, filters, WGPU staging, encoder, Slint models, and diagnostics. | Work 007; Performance Agent. |
| GAP-006 | Canvas interaction state is implicit in UI properties | `CanvasState` owns bounded transient zoom, pan, drag-box interaction, and fixed-policy snapping; `DesktopState` owns one bounded ordered selection shared by the dock and canvas. Crop/rotation and group transforms remain in typed drafts and are committed once per gesture. Arrow-key nudges and typed Transform-menu commands commit through the same project path. | New canvas features can duplicate state or accidentally persist pointer drafts; wheel zoom is still preset-stepped rather than cursor-anchored, select-underneath is absent, and crop handles do not yet support rotated geometry. | Keep the toolkit-neutral `CanvasState { zoom, pan, selection_box, snapping, interaction }` and `DesktopState` selection as the single cross-surface owner; keep drafts transient, parse menu actions into a closed Rust command set, and commit one batch command per gesture/key action. | Work 009–013; Canvas Agent. |
| GAP-007 | Scene-item identity is weaker than OBS | `SceneItemSpec` has typed source, scene, and embedded group targets; schema v5 recursively persists group children, command/codec validation bounds group depth, path-addressed group visibility/locking/order/removal/duplicate/transform commands are atomic, the Sources dock recursively projects group children with encoded paths and routes visibility/locking/order/removal/duplicate through those commands, and engine/GUI preview flatten visible nested scenes/groups into ordered runtime items while retaining one shared source definition per capture device. | Persistent runtime scene-item IDs and group child properties/filter/transform UI remain absent. The project transform command validates only the current axis-aligned nested composition boundary; crop, rotation, and flips at transformed group boundaries are rejected rather than approximated. Runtime transform addressing is still an ordered index, so reordering rebuilds scene-item references. | Carry stable project scene-item identity through runtime, groups, and nested scene sources, then provide nested transform UI and full transform semantics without reopening shared captures. | Scene/Engine Agent: stable runtime IDs, nested child command expansion/UI, nested transform oracle, and nested-item editing; affects project codec, compositor, and UI. |
| GAP-008 | Dock interaction and floating geometry are incomplete | `DockNode::Split`, `DockNode::Tabs`, and `DockNode::Dock` provide one bounded, validated tree with versioned persistence; Rust emits pane and splitter projections, header drags resolve tab/directional targets, and detached windows restore scale-aware physical geometry. | OS monitor identity/clamping and compositor-backed multi-monitor/DPI evidence are still absent; floating custom-dock/plugin surfaces are not yet supported. | Add platform monitor identity/capability adapters and live multi-monitor fixtures while retaining bounded geometry and legacy migration. | Work 014–015; Dock Agent, Platform Agents, QA Agent. |
| GAP-009 | Filter persistence is ahead of filter execution | The project model accepts named filter kinds/settings, while runtime compilation recognizes only the small reference set. | A persisted filter can be silently absent from the render/audio graph. | Typed `AudioFilter`, `VideoFilter`, `AsyncVideoFilter`, and `GpuVideoFilter` nodes with explicit unavailable capability and CPU/reference oracle. | Work 018; Filter Agent and project/UI review. |
| GAP-010 | Audio is a reference mixer, not a full graph | Mixer, resampler, monitor taps, pacing, PipeWire discovery, and fallback exist; full device graph, hot plug, multiple tracks, and audio filters do not. | Advanced audio workflows and production synchronization are incomplete. | Model device/source/filter/gain/pan/bus/track/monitor routing with bounded blocks and clock ownership. | Work 019; Audio Agent, then platform agents. |
| GAP-011 | Production output boundary is not yet product-complete | `OBSRPKT1` and reference writers are robust fixtures; GStreamer provides an optional native boundary, while live production-sink tests are explicitly environment-ignored here. | The application can describe production profiles without a trustworthy cross-platform output path. | Separate `Encoder -> EncodedPacket -> Muxer -> Output`; capability negotiation must produce typed unavailable results and never silently fall back. | Work 022–024; Output Agent and Platform Agents. |
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
