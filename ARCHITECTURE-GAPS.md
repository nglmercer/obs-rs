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
* The preview timer remains capacity-one and now requests up to 60 Hz for
  visible GUI/projector consumers, 5 Hz for a minimized idle window, and no
  render work for a hidden idle window; active output stays independent of GUI
  demand.
* WGPU composition consumes exact-size layer iterators directly, removing the
  duplicate per-frame source-descriptor allocation in both layer submission
  paths. A 10,000-frame local acceptance probe completed without drops or
  implicit readbacks before and after this change.

The remaining P0 work is native surface presentation, GPU target fan-out
without duplicate transfers, and complete end-to-end allocation profiling.
Parameter buffers/bind groups, capture/audio blocks, encoder staging, and the
Slint compatibility bridge still need attribution. These are intentionally
still open; passing a viewport test or a local composition probe is not the
same as zero-copy or final performance certification.

## Priority gaps

| ID | Gap | Current evidence | Consequence | Required target | Dependencies / first packet |
| --- | --- | --- | --- | --- | --- |
| GAP-001 | Render-target ownership is conflated | `RenderTarget`/`RenderTargetRole` and role-keyed WGPU targets now separate program, preview, projector, and encoder ownership; GUI preview no longer targets the full canvas by default. | Native fan-out and projector lifetimes are not yet connected, and the compatibility readback remains explicit CPU storage. | Finish role-specific fan-out and native target import while preserving the CPU oracle and output scaling tests. | Work 005–006; preserve `obs-rs-media` CPU oracle and existing output scaling tests. |
| GAP-002 | No replaceable preview presenter | A safe `PreviewPresenter` trait now owns Slint conversion behind one boundary; the current implementation is `SlintPreviewPresenter`. | A native/WGPU surface implementation is still unavailable, so one viewport-sized copy remains. | Add native/WGPU presenter implementations and keep copy-byte accounting for the fallback. | Work 005; depends on GAP-001. |
| GAP-003 | GPU and encoder conversion are not a single fan-out graph | Composition can submit to WGPU, but the GUI preview uses RGBA readback and the encoder uses a separate NV12 readback bridge. | Duplicate transfers and target lifetimes make latency/copy accounting unclear. | One composed source texture feeds preview, projector, encoder conversion, and optional recording targets; each transfer is explicit and bounded. | Work 005; GPU Agent plus Output Agent review. |
| GAP-004 | Rendering cadence is not fully demand-aware | The worker coalesces requests and typed desktop demand now distinguishes visible/projector consumers (up to 60 Hz), minimized idle windows (5 Hz), hidden idle windows (no render), and output-only work. Static invalidation and live multi-monitor cadence are not yet measured. | Static scenes and multi-monitor/projector demand can still do more work than necessary, and end-to-end cadence remains unverified. | Complete event-driven invalidation plus audio-meter/diagnostic cadence and live multi-window measurements. | Work 006; depends on GAP-001 and preview metrics. |
| GAP-005 | Hot-path allocation evidence is incomplete | Preview target dimensions and Slint-copy bytes are observable; WGPU composition no longer allocates a duplicate per-frame layer-descriptor vector, and the 10,000-frame local probe completed without drops or readbacks. Parameter buffers/bind groups, capture/audio/filter blocks, encoder staging, and diagnostics are not attributed end-to-end. | Regressions can hide behind bounded queues while allocating in capture/audio/filter/diagnostic paths. | Add scoped allocation/copy counters or an approved profiler harness for capture, audio, filters, WGPU staging, encoder, Slint models, and diagnostics. | Work 007; Performance Agent. |
| GAP-006 | Canvas interaction state is implicit in UI properties | `CanvasState` owns bounded transient zoom, pan, drag-box interaction, and the active snapping policy; `DesktopState` owns one bounded ordered selection shared by the dock and canvas. The persisted settings document owns one validated 1–100 canvas-pixel snap distance and one Show safe areas flag, applying both through the settings-to-studio boundary. The active guide set now includes bounded EBU/ITU Action, Graphics, and 4:3 safe-area edges, and the editable preview draws the same three rectangles. Crop/rotation and group transforms remain in typed drafts and are committed once per gesture; rotated Alt-crop deltas are converted through the same source-local frame as the media renderer. Ordinary resize preserves aspect, Shift enables free resize, Ctrl suppresses snapping, arrow-key nudges and typed Transform-menu commands commit through the same project path. | New canvas features can duplicate state or accidentally persist pointer drafts; fine-resize semantics, rotated visual-handle orientation, exact guide styling, and live pointer/DPI fixtures remain incomplete. | Keep the toolkit-neutral `CanvasState { zoom, pan, selection_box, snapping, interaction }` and `DesktopState` selection as the single cross-surface owner; keep the persisted snap value and guide visibility in `AppSettings`, clamp again at the runtime boundary, keep drafts transient, parse menu actions into a closed Rust command set, and commit one batch command per gesture/key action. | Work 009–013; Canvas Agent. |
| GAP-007 | Scene-item identity is weaker than OBS | `SceneItemSpec` has typed source, scene, and embedded group targets; schema v6 recursively persists group children and explicit profile scene order, command/codec validation bounds group depth, path-addressed group visibility/locking/order/removal/duplicate/transform/copy-paste commands are atomic, the Sources dock recursively projects group children with encoded paths and routes visibility/locking/order/duplicate/copy/paste through those commands, and engine/GUI preview flatten visible nested scenes/groups into ordered runtime items while retaining one shared source definition per capture device. Stable project paths now cross flattening, runtime attachment, compositor layers, and GUI transform drafts; runtime transforms can be addressed by item ID rather than only by ordered index. Direct safe Transform-menu actions, target-aware source filter/properties editing, target-aware modal transform editing for nested non-group rows, top-level scene move commands, validated startup and per-profile Preview/Program selection restoration, profile-switch history restoration, bounded document-keyed Preview/Program restoration across collection switches and guarded Load/Recover flows, atomic current-document-to-new-collection duplication, stable collection-root resolution, atomic collection rename/export, validated external collection import, guarded Load/Recover replacement actions, native main-window close guarding, Save/Don't save/Cancel continuation, and bounded durable per-document/per-profile scene-selection snapshots now route through the same typed project/UI boundaries. | The transform command validates only the current axis-aligned nested composition boundary; crop, rotation, and flips at transformed group boundaries are rejected rather than approximated. Durable collection-scoped active-scene persistence now records each visited profile within bounded document/profile records; the legacy index APIs remain incomplete/compatibility-only, while startup restoration ignores stale IDs without mutating project history. The remaining save/discard/recovery gap is the broader lifecycle beyond the guarded close/load/recover/save paths. | Complete nested transform semantics without reopening shared captures; keep stable IDs through any future reorder and interaction state. Complete the remaining save/discard/recovery lifecycle. | Scene/Engine Agent: nested transform oracle, profile/collection scene workflow, and full nested-item editing semantics; affects project codec, compositor, and UI. |
| GAP-008 | Dock interaction and floating geometry are incomplete | `DockNode::Split`, `DockNode::Tabs`, and `DockNode::Dock` provide one bounded, validated tree with versioned persistence; Rust emits pane and splitter projections, header drags resolve tab/directional targets, and detached windows restore scale-aware physical geometry. The existing monitor capability now supplies a virtual-desktop clamp for restored positions, while platforms without monitor bounds retain the saved coordinates. | OS monitor identity, per-monitor selection, and compositor-backed multi-monitor/DPI evidence are still absent; floating custom-dock/plugin surfaces are not yet supported. | Add platform monitor identity/capability adapters and live multi-monitor fixtures while retaining the shared bounded geometry/clamp helper and legacy migration. | Work 014–015; Dock Agent, Platform Agents, QA Agent. |
| GAP-009 | Filter persistence is ahead of filter execution | The project model accepts named filter kinds/settings, while runtime compilation recognizes only the small reference set. | A persisted filter can be silently absent from the render/audio graph. | Typed `AudioFilter`, `VideoFilter`, `AsyncVideoFilter`, and `GpuVideoFilter` nodes with explicit unavailable capability and CPU/reference oracle. | Work 018; Filter Agent and project/UI review. |
| GAP-010 | Audio is a reference mixer, not a full graph | Mixer, resampler, monitor taps, pacing, PipeWire discovery, deterministic fallback, transient active-device identity, bounded recovery for configured and automatic microphone/desktop routes, scheduled A/V drift counters, a sample-reusing positive per-channel sync-delay boundary, validated/persisted global microphone/desktop offset controls projected through the Audio settings page, typed per-source `AudioMonitorMode` output/monitor bus routing, engine/worker monitor-mode and sink-selection commands, a dedicated bounded `AudioOutputWorker` sink boundary, persisted Audio-page controls for the optional monitor-output device plus microphone/desktop monitor modes, and idle-boundary audio-format reconfiguration now exist. Full device graph, default-device policy, adaptive hardware clock correction, source-level sync properties, channel-layout routing, and multiple tracks do not. | Advanced audio workflows and production synchronization are incomplete; the current drift counters describe the portable scheduler rather than measured independent device clocks, settings control only positive global channel delay, two global monitor modes, and a bounded sample-rate/channel-count rebuild, and the monitor sink has no default-device policy or complete per-source/advanced-audio workflow. | Model device/source/filter/gain/pan/bus/track/monitor routing with bounded blocks and clock ownership; add measured clock feedback and adaptive resampling, then expose channel-layout/source-property routing and multiple tracks without unbounded latency; keep device recovery off UI paths and preserve explicit fallback state. | Work 019; Audio Agent, then UI and platform agents. |
| GAP-011 | Production output boundary is not yet product-complete | `OBSRPKT1` and reference writers are robust fixtures; GStreamer provides an optional native boundary, while live production-sink tests are explicitly environment-ignored here. | The application can describe production profiles without a trustworthy cross-platform output path. | Separate `Encoder -> EncodedPacket -> Muxer -> Output`; capability negotiation must produce typed unavailable results and never silently fall back. | Work 022–024; Output Agent and Platform Agents. |
| GAP-012 | Services and transports are coupled at the product boundary | A bounded compile-time service catalog now separates stable service IDs/display names, first ingest defaults, protocol selection, and redacted stream-key settings from the target boundary. `StreamingTransport` gives reference packet and native GStreamer sessions one typed poll/reconnect/close lifecycle contract; `ReconnectPolicy` adds a capped exponential schedule whose deferred outcome never sleeps or grows queues. Media submission, native state mapping, and service metadata remain separate. There is still no signed catalog update path or full transport media/session abstraction. | Congestion, keyframe, auth lifecycle, and service-specific diagnostics cannot yet evolve independently of all native transport details. | Extend the lifecycle contract into a bounded network worker and transport capability model; retain the catalog as replaceable metadata and keep encoder/media ownership outside service configuration. | Work 024; Output Agent. |
| GAP-013 | Platform crates are boundaries more than implementations | macOS and Windows crates return typed unavailable behavior off-platform; Linux live paths are environment-unverified. | Cross-platform parity cannot be claimed from portable tests. | Each platform agent supplies capture, audio, encoder, virtual-camera, permission, device-loss, and recovery evidence behind safe Rust traits. | Work 027–030; platform matrix and hardware CI. |
| GAP-014 | Plugin ecosystem stops short of product distribution | Versioned Rust API, manifest validation, signatures, quotas, and subprocess frames exist. | Dynamic source/filter/output/service/UI extensions, permission prompts, update policy, crash supervision, and diagnostics are incomplete. | Versioned manifests with permissions/resource quotas, isolated process supervision, signature/update model, and extension-facing dock contracts. | Work 031; Plugin Agent, Security Reviewer. |
| GAP-015 | Settings and hotkeys are not fully typed at the behavior boundary | Settings persist several output fields and hotkey display strings; the toolkit-neutral `Shortcut` parser validates bounded Ctrl/Shift/Alt combinations, canonicalizes aliases/order, preserves empty unbindings, and Settings Apply atomically compiles one bounded action table owned by `DesktopState`. The GUI event path now parses the Slint label in Rust, resolves the typed action, and lets Slint invoke the established confirmation/output/project callbacks. | Global registration, runtime effects for future actions, restart requirements, and the remaining action set are still incomplete; menu shortcut labels remain display projections and there is no OS-level hotkey capability model. | Structured setting schema and `KeyCombination` model with capability, validation, effect, and restart metadata, followed by runtime/platform registration. | Work 025–026; UI/Platform Agents. |
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
