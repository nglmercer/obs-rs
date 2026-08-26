# Architecture gaps blocking OBS parity

This document translates the parity matrix into dependency-aware engineering
gaps. It is intentionally about ownership and boundaries, not a second feature
roadmap. Each gap should become one or more narrow work packets with an
independent reviewer.

## Latest verified packet

The source clipboard keyboard packet now maps OBS's local `Ctrl+C` and
`Ctrl+V` workflow through the existing `DesktopState` clipboard. The main
window forwards only the selected stable source path; Rust keeps the copied
scene item and the existing reference-paste command owns target resolution,
selection, history, and nested-group behavior. The GUI fixture drives both
keys through the testing backend and proves the new reference item is selected
after paste. Paste Duplicate remains an explicit context-menu action, and
global OS hotkeys remain a separate capability.

The source-order keyboard packet now maps `Ctrl+Up`, `Ctrl+Down`, `Ctrl+Home`,
and `Ctrl+End` from both the editable canvas and the focused Sources dock to
the existing Rust order callbacks. Boundary presses are consumed as no-ops;
the callback retains lock checks, nested target resolution, project history,
and the bounded parent-local order. The GUI fixture verifies the four moves
and leaves the temporary sources removed. Configurable/global source-order
hotkeys are still outside this packet.

The Sources-dock clipboard follow-up now routes `Ctrl+C`, `Ctrl+V`, and
`Ctrl+A` through its focused `FocusScope` to the existing Rust callbacks.
It reuses the same selected target, reference-paste command, bounded
select-all projection, and single project state owner as the canvas path. The
GUI fixture proves copy/reference-paste/select-all while the dock owns focus;
Paste Duplicate remains context-menu-only and global registration remains a
separate capability.

The canvas clipboard boundary now applies the same modifier policy as the
Sources dock: local copy, reference-paste, and select-all require plain
`Ctrl`, while `Ctrl+Shift` and `Ctrl+Alt` are left available for configured
actions. The fixture proves modified clipboard keys neither replace the
Rust-owned clipboard nor create a scene item, and then verifies the original
clipboard still pastes a reference. No project or UI state is duplicated.

The source-deletion keyboard boundary now accepts both `Delete` and
`Backspace` from the editable canvas and the focused Sources dock. Both keys
still use the existing Rust multi-selection removal callback, so locking,
nested targets, history, and failure notices retain one owner. The GUI fixture
covers both keys on both focus surfaces; configurable/global hotkey
registration remains separate.

The Sources-dock rename packet now accepts unmodified `F2` for the selected
row and opens the existing typed rename callback with that stable target. The
modal still owns only its transient draft; Rust resolves the source or nested
group and commits the project edit. The GUI fixture verifies the draft and
commit through the real modal before removing its temporary item. macOS
Return-key behavior and the broader global hotkey catalog remain open.

The Scenes-dock rename packet now accepts unmodified `F2` for the selected
preview scene and opens the existing scene-properties modal. The dock reuses
the Rust-owned preview selection and scene-properties command, leaving the
modal's name and transition fields as transient UI drafts. The GUI fixture
verifies the real modal, commit, and restoration of the shared scene fixture.
macOS Return-key behavior and the broader global hotkey catalog remain open.

The modal keyboard packet now gives the active `ModalShell` its own focused
boundary. Unmodified `Escape` closes without committing the transient draft,
unmodified `Enter` invokes the existing acceptance callback, and otherwise
unhandled keys are consumed so main-window output/project shortcuts cannot
fire through an open dialog. The GUI fixture covers cancel-without-commit and
Enter-based scene-property commit; text-entry-specific and native
accessibility focus behavior remain open.

The Mixer options packet now keeps settings navigation contextual without
creating another settings state owner. The Mixer dock's gear button traverses
the docked or floating `DockSlot` boundary as a typed callback and opens the
existing Settings window on the Audio page; the generic Settings actions still
preserve their current page. The integrated GUI fixture verifies category 4
through the real controller, while advanced audio/device-graph parity remains
open.

The projector/multiview evidence was reconciled against the current runtime:
`ProjectorFeed::Multiview` is included in the same geometry and monitor arrays
as Program, Preview, Source, and Scene. Its fullscreen default, F11 toggle,
persisted geometry, display menu, monitor move, restart restore, and bounded
multiview render-demand path are covered by headless tests. The remaining gap
is live compositor-backed multi-monitor/DPI evidence plus per-scene audio and
source-specific tile behavior; the old claim that multiview monitor selection
was absent was stale.

The slideshow Browse packet extends the properties picker to the
`image_slideshow` `paths` row without creating a second source state owner. A
bounded asynchronous native directory chooser returns one selected directory
through the existing local draft, so the normal OK command remains the only
project mutation. Linux (`zenity`/`kdialog`), macOS (`osascript`), and Windows
(PowerShell) command shapes are covered, as is the integrated GUI commit path;
typed multiple paths and file selection remain the fallback for workflows the
directory chooser does not cover.

The image-source Browse packet now keeps the chooser at the properties
boundary: only `image_source`'s `path` row exposes the capability-backed
asynchronous native picker, and its result returns through the existing local
draft before the normal OK command commits it. Linux (`zenity`/`kdialog`),
macOS (`osascript`), and Windows (PowerShell) use bounded image filters when
available; typed paths remain the explicit fallback, and picker-command plus
GUI draft/commit tests pass.

The nested source context-action packet now routes `Rename` and `Remove` from
the exact stable row path. Root rows retain OBS multi-selection removal, while
nested rows remove only the clicked group/Scene-reference child; nested leaf
rename is enabled and remains independent from a later canvas-selection change.
The Sources-dock GUI fixture covers both paths and locked-container behavior.
Interact, the remaining context-menu actions, and global OS hotkey registration
remain open.

The target-aware monitor-selection packet now carries that same stable
`SourceTarget` from nested Source Properties into the shared monitor picker.
Changing the canvas selection while the dialog is open cannot redirect a
nested screen source to an unrelated root item; the picker resolves the owning
source name and settings document once, and refresh/accept continue using that
target. The GUI fixture covers a nested screen leaf, an unrelated selected root,
and the resulting monitor-window source identity. Live Wayland/X11 display
enumeration, multi-monitor behavior, and non-Linux adapters remain open.

The project-lifecycle packet now includes a bounded `Save As...` callback.
The GUI dialog writes the current serialized document through the existing
atomic `ProjectFileStore`, rejects a different existing destination before
touching the session, and changes the active path plus selection key only after
the write succeeds. Save, Load, Recover, and exit persistence therefore follow
the new document path without introducing a second project state store. The
same bounded asynchronous desktop picker boundary now offers a Save dialog for
the active project on Linux (`zenity`/`kdialog`), macOS (`osascript`), and
Windows (PowerShell), returning the selected path through the Slint event loop;
unsupported desktops keep the manual path field and an explicit unavailable
message. Project path validation now accepts the canonical `.obsrproj` extension
and the shipped legacy `.json` extension, while rejecting other file types
before store construction. Recover project now opens a bilingual review modal,
preserves the temporary file on cancel, and keeps the modal open after a parse
failure. Discard recovery now opens a separate confirmation modal and removes
only the exact temporary file after confirmation; broader recovery UX is still
open.

Opening a project now follows the same dirty-session guard into a separate
mode-4 dialog: the native chooser is asynchronous, uses open-dialog semantics
on Linux (`zenity`/`kdialog`), macOS (`osascript`), and Windows (PowerShell),
and only the selected bounded path reaches the existing load callback.
Unsupported desktops keep the manual path field and an explicit unavailable
message. Project file-type validation now rejects unsupported extensions while
retaining the shipped `.json` compatibility path. The Recover action now shows
the active and temporary paths in a confirmation modal before replacement;
broader recovery policy remains open.

The same picker boundary now covers collection transfer: mode 1 uses a bounded
native Save dialog for Export and mode 2 uses a bounded native Open dialog for
Import, writing only the collection-transfer field before the existing
filesystem callbacks validate and commit the operation. Collection extension
filtering is complete; the project-recovery review is now explicit for the
guarded Recover action, while broader recovery policy and other project
lifecycle polish remain open.

The bounded Stinger runtime packet now accepts already-decoded RGBA frames as
one validated, preloaded clip. Frame count, per-frame duration, total duration,
transition point, format consistency, and resident RGBA storage are all
bounded before the clip enters the runtime. The same immutable clip can feed
the full program target and the smaller GUI preview target; the preview worker
scales the selected frame at the presentation boundary, and render-time file
or decoder I/O is impossible. This is intentionally only a runtime slice:
persistent resource resolution, asynchronous decoding, track matte layout,
fade/audio monitoring policy remain open. The scene-properties dialog now
exposes the persisted path, transition point, preload flag, and hardware-decode
preference through the same undoable project command. Scene Properties now has
an asynchronous desktop file picker for the resource path on Linux
(`zenity`/`kdialog`), macOS (`osascript`), and Windows (PowerShell) when the
platform capability is present; the chooser process is bounded and never runs
on the UI thread.

The persistent Stinger configuration packet now adds a bounded resource path,
normalized transition point, preload flag, and hardware-decode preference to
the scene model. `SetSceneStingerOverride` owns the mutation and schema 8
round-trips it while schema 7 documents remain readable. This is still
metadata only: no project parse or command performs filesystem or decoder I/O,
and the project/UI workflow still does not own a resolved clip cache.

The worker-boundary packet now provides a capacity-one request queue and
capacity-one result queue, non-blocking submission/polling, typed request IDs,
cooperative cancellation, and a decoder-independent `StingerResourceLoader`
contract. Its thread is detached during teardown so UI/render owners never
join native resource work. The optional native GStreamer adapter now resolves a
local file/container into negotiated RGBA frames, with a bounded sample count,
resident memory budget, polling cancellation, and typed resource failures.
The result slot now discards completions whose request ID is no longer current
and never blocks submit/poll callers. The GUI now constructs the native loader
behind the engine boundary and preloads only the selected scene's
`preload=true` resource from the refresh timer, reporting ready and typed
failure states without waiting on the worker. Scene properties now edit the
resource metadata through one `SetSceneProperties` history entry. The docked
and floating Transition panels now route an explicit `Take Stinger` callback
through the ready clip only; invalid duration, not-ready, typed failure,
stopped-loader, and dispatch errors are visible without callback-side I/O.
The native GStreamer adapter now probes known hardware decoder factories and
the `decodebin` selection property before enabling the Scene Properties
preference. It sets the bounded `force-sw-decoders` control explicitly and
returns a typed unavailable failure when a requested hardware path cannot be
selected. Codec-specific hardware selection and non-GStreamer platform
adapters remain open. The file-picker workflow is now available through the
capability-backed Scene Properties button; unsupported desktops keep the
bounded manual-path field and an explicit unavailable state.

The toolkit-neutral `StingerLoadSession` now owns the transient current request
and resolved clip around that worker. It clears the renderable clip only after
an accepted request, preserves the current state when the bounded queue is
full, rejects a loader result whose format does not match the active canvas,
and invalidates old completions when the target format changes. It exposes the
typed failure without moving resource metadata or decoded pixels into a second
project state store; the GUI resource session is now connected to the native
adapter, and the scene-properties fields are synchronized from the project.
The file picker is now asynchronous and capability-backed. Explicit Take can
now submit a persisted `preload=false` resource through the same bounded worker
when no ready clip exists; the refresh cadence keeps the validated duration as a
transient intent and dispatches automatically after the matching clip is ready.
Codec-specific hardware selection, non-GStreamer adapters, and the exact OBS
workflow remain open.

The portable Luma Wipe packet now adds a typed luminance-mask transition to the
media, project, UI, and GUI boundaries. Linear horizontal and vertical masks
support inversion and bounded 0..=1000 softness, and the CPU/reference path
blends directly into the destination buffer without allocating a full-frame
mask. Scene overrides, console commands, bilingual controls, and persistence
round-trip through the same `TransitionSpec`; media, project, UI, and GUI
workflow tests cover the implemented slice. OBS's asset-backed mask catalog,
external pattern resources, and the full Stinger workflow remain open
capabilities.

The provider-to-mix `AudioResampler` now consumes the typed standard layout
metadata instead of treating every channel count as an unlabeled index list.
Speaker-role conversion is bounded and covered by a release timing probe;
`Discrete` provider layouts retain the compatibility fallback. This closes
only the low-level conversion slice. The project still lacks canonical
per-source audio identity and routing, so source properties and multiple tracks
remain intentionally open gaps.

The built-in Stats dock now completes the sixth core dock boundary: ID 5 is
part of the validated Rust-owned dock tree, layout persistence, menu visibility,
tab projection, and floating-window forwarding. It reads existing status,
capture-capability, preview, and output diagnostics on the established UI
cadence, so it adds no render/capture/audio/output hot-path work. Dynamic
plugin-rendered dock surfaces and persisted custom dock IDs remain open.

The Scenes dock now carries a bounded typed drag payload per row and routes
before/after drops through the existing `MoveScene` project command. Rust owns
profile/target validation, index adjustment after source removal, selection of
the moved scene, and failure reporting; docked and floating panels share the
same callback boundary. Pure index tests and a real testing-backend pointer
fixture cover both directions and invalid drop inputs. Scene collection
drag/drop across documents and native accessibility behavior remain open.

The hotkey table now includes a persisted `ToggleStudioMode` action. Settings
validation, conflict detection, action code 18, the View-menu projection, and
the Rust-owned callback all use the same bounded binding. The callback toggles
the existing view-mode state without introducing a second Studio-mode owner;
the default is intentionally unbound. Global OS registration and the remaining
OBS action catalog remain open.

Focused-window microphone push-to-talk and push-to-mute now have persisted,
conflict-checked bindings and stable action codes 23/24. Slint sends press and
release through the exact `SetMixerMute` command, so the mixer remains the one
mute-state owner and repeated key events are idempotent. The main canvas,
Scenes, and Sources focus boundaries also release a held microphone push action
when the window deactivates and ignore a delayed key release. This remains a
local event-boundary capability: global OS registration, arbitrary native-focus
control recovery, and push actions for other channels remain open.

The selected-source visibility hotkey now reuses the existing typed source
callback. Bounded action code 19 is persisted, conflict-checked, and rejected
when the UI has no valid selected source, so source visibility remains owned by
the project command path. Interact, context-menu completion, and global OS
registration remain open.

The selected-source lock hotkey now reuses the existing typed source-lock
callback. Bounded action code 20 is persisted, conflict-checked, and rejected
when the UI has no valid selected source, so lock state remains owned by the
project command path. Interact, context-menu completion, and global OS
registration remain open.

The selected-source projector hotkey now reuses the existing shared-feed
projector callback. Bounded action code 21 is persisted and conflict-checked;
the UI refuses empty selections, groups, and Scene references before opening
the source feed. Multi-monitor evidence and global OS registration remain
open.

The Preview-scene projector hotkey now reuses the existing target-bearing
scene-projector callback. Bounded action code 22 is persisted and
conflict-checked, and the UI refuses an empty or `none` Preview target before
opening the feed. Multi-monitor evidence and global OS registration remain
open.

The transition catalog now includes bounded Slide and Swipe samples in the four
reference directions. The portable renderer moves source/destination pixels in
place: Slide moves both layers, Swipe moves only the source for the outgoing
variant, and `swipe_in` moves the destination over the stationary source.
Project JSON, scene properties, the Transition dock, and the console share one
typed `TransitionSpec`; missing `swipe_in` fields in older documents default to
the outgoing behavior. Stinger, luma, and other slide variants remain open; the
640x360 timing reports are intentionally ignored until they are promoted to the
pinned performance suite.

The portable transition direction model now accepts left, right, up, and down
for both Slide and Swipe, with axis-aware in-place traversal and bounded JSON
and console parsing. The Transition dock, floating Transition dock, and
scene-properties dialog expose localized bounded direction selectors and pass
the selected index through typed callbacks. Persisted scene overrides project
their selected direction back into the scene-properties dialog, while legacy
callback entry points remain compatible by defaulting to left. Stinger, luma,
and other slide variants remain open.

The nested transformed-leaf packet now preserves crop when a leaf crosses an
axis-aligned group or Scene-reference boundary, and preserves leaf rotation
when every crossed parent scale is uniform and unmirrored. Media, project,
canvas inverse, and GUI projection tests cover the supported slice. Crop or
rotation on a parent boundary, plus rotated leaves under non-uniform or
mirrored ancestry, still fail explicitly because those cases need an
intermediate-scene clipping/shear representation.

The Studio Mode hotkey packets now include optional persisted local Cut and
Previous/Next Preview Scene bindings. They use the existing bounded `Shortcut`
table and stable GUI action codes without renumbering existing bindings.
Previous/Next resolve the active profile's persistent scene order with
wrap-around and refresh the shared selection owner. Global registration,
scene-specific actions, and the remaining OBS hotkey catalog remain open.

The audio-filter editor packet now exposes the already-compiled Gain, Invert
Polarity, Limiter, Compressor, and Expander kinds with bounded localized
property schemas, alongside Noise Gate. This closes the GUI configuration
slice only; automatic source-level audio-filter routing, sidechains, and the
remaining filter graph are still separate gaps.

The same filter catalog now projects English and Spanish names from the active
UI locale, including safe fallback for persisted unknown kinds. Existing
filter instance names remain user-editable project data and are not rewritten
when the locale changes.

The scene-dock accessibility packet now gives each scene row an explicit button
role and stable scene-ID label, matching the existing source-row contract. A
real testing-backend fixture finds the Preview scene by that label; the full
screen-reader and native platform audit remains open.

The scene-item identity packet now also covers atomic root/group reparenting:
`MoveSceneItemToParent` validates source and destination paths before moving the
owned item, and the Sources dock projects the same destinations for nested rows.
Stable paths are recomputed after the move so selection does not point at the
old parent. Its flattened-target adapter now resolves both source and
destination owners across Scene-reference boundaries and commits cross-scene
moves transactionally, including lock, collision, depth, and cycle validation.
Sources-dock pointer drag/drop is now covered for group and leaf targets with
bounded before/after insertion and locked-container rejection. Canvas and
Scene-reference pointer drag/drop, the remaining full transformed-boundary
crop/rotation semantics, and the broader save/recovery lifecycle remain open.

The Scene properties packet now keeps scene name and optional transition
override in the same Rust-owned `SetSceneProperties` command. The dialog
projects inherited, Cut, cross-fade, and fade-to-color policies, while the
existing bounded transition parser remains the single validation boundary.
Refresh versioning prevents background UI ticks from replacing an in-progress
modal edit, and the project plus GUI fixtures prove one undo step and clean
inheritance. Per-scene transition selection still does not replace the broader
Studio Mode transition workflow or production output parity.

The nested Scene-reference grouping packet now resolves the selected flattened
targets to one owning scene and group path before invoking the existing atomic
group/ungroup mutations. The project command, toolkit-neutral selection
validation, Sources-dock availability projection, and GUI callback all retain
the parent reference while changing the owner scene. Mixed-owner selections
remain rejected; the remaining parent-boundary crop/rotation semantics remain
a separate gap.

The nested Scene-reference reparenting packet now resolves both source and
destination paths through the same owner resolver. A bounded two-scene
transaction moves the existing item without cloning it, while the UI projects
referenced scenes and their group destinations and restores the flattened
selection path. Sources-dock pointer drag/drop is covered for group and leaf
targets; canvas/Scene-reference GUI drag/drop and the remaining full
transformed-boundary crop/rotation semantics remain separate gaps.

The keyboard source-deletion packet keeps the Delete key as a presentation
boundary only: the focused Sources dock and the editable canvas `FocusScope`
call the existing Rust `remove-source` callback. The canvas editor explicitly
requests that scope on pointer-down, so keyboard delivery does not depend on a
child `TouchArea` becoming focused accidentally. Nested paths, locked-item
rejection, project history, and refresh therefore continue to have one owner.
The GUI fixture covers the successful unlocked canvas path and the locked dock
failure path; global hotkey registration and broader source context-menu parity
remain separate gaps.

The atomic multi-selection Delete packet now routes the full bounded
`DesktopState` selection through `RemoveSceneItems`. Root and nested targets are
validated before mutation, selected group descendants are subsumed by their
ancestor, and locked targets or ancestors reject the complete operation. The
project tests prove root/nested removal, atomic failure, and one-step undo/redo;
the callback is forwarded through the SourceContextMenuArea, docked and
floating panels, as well as the canvas. The GUI fixture reaches the keyboard
scenario, then fails later at the existing native capture-device choice
assertion because this host exposes no device row; the popup-specific testing
harness does not expose a stable menu element, so its interaction remains a
follow-up fixture rather than a false green result.

The Sources dock modifier-selection packet now maps plain, Shift, and Ctrl
pointer clicks through one typed callback into the existing Rust selection
owner. Docked, floating, and context-menu boundaries share that callback, so
selection is not duplicated in Slint. The GUI fixture proves replacement,
ascending/descending contiguous range selection, and toggle removal while
re-querying rows after each model refresh. Drag-box selection and complete
nested-row pointer evidence remain open. The fixture still fails later when
this host exposes no native capture-device row.

The Sources-dock pointer drag/drop packet gives each visible row a typed
`DataTransfer` payload and uses Slint `DragArea`/`DropArea` only as the input
boundary. The Rust callback resolves flattened source and destination paths,
dispatches the existing `MoveSceneItemToParent` command, and refreshes the
single project/UI state owner. Container rows insert at the front; leaf rows
use bounded before/after zones. The GUI fixture proves vertical drags into a
group and a nested leaf, stable selection paths, order changes, and rejection
when the destination container is locked. Mouse-drag panning is disabled for
the source-row viewport so the Flickable cannot steal the gesture; wheel
scrolling remains available. Canvas/Scene-reference GUI drag/drop and the
remaining full transformed-boundary crop/rotation semantics remain open.

The keyboard follow-up now sends Shift+Up/Down/Home/End through the same
contiguous range resolver instead of treating Shift as one-row additive state.
The navigation fixture proves replacement ranges in both directions and Ctrl
toggle after movement; plain navigation remains bounded and non-wrapping.

The canvas pointer-fixture packet now exercises the actual Slint testing-backend
pointer path against the editable surface. It maps the current fit zoom and
pan into letterboxed coordinates, proves blank-space drag-box replacement,
Ctrl-additive selection, both middle-button and Space+drag pan, and a selected
source's bottom-right resize handle, then removes temporary items and restores
the starter scene and transient pan. Transformed-boundary crop/rotation, live
DPI, and the native capture device prerequisite remain outside this evidence;
the same fixture also verifies a real wheel event changes continuous zoom and
restores the anchored viewport state, while the separate rotation-handle
gesture observes one fixed-point angle commit and the Alt left-middle handle
gesture changes source crop without changing horizontal scene scale.

The source-row follow-up inserts a temporary group at a visible boundary and
clicks its nested child through the actual SourceContextMenuArea target. The
selected `group/child` path is verified and the temporary group is removed;
nested canvas geometry is now covered by a separate projection packet.

The nested-group canvas packet projects visible leaf items from the same
flattened path model used by the runtime and Sources dock. Hit-testing,
drag-box selection, snapping guides, overlays, keyboard nudge, and transform
drafts use effective canvas coordinates; commit converts those drafts back to
local group coordinates and sends one atomic root/nested transform batch.
Locked ancestors are included in the edit guard. Scene-reference leaves now
resolve through the owning referenced scene for axis-aligned local commits;
the standalone Transform dialog uses the same flattened target resolver and
rejects inherited locks. Leaf crop and leaf rotation under a uniform,
unmirrored parent now compose through this boundary; parent crop/rotation and
rotated leaves under non-uniform or mirrored ancestry still fail explicitly
until a full intermediate-scene transform model exists.

The dock-header pointer packet now drives the visible `DockHeader` through the
testing backend rather than invoking the callback directly. It verifies drag
start, right-zone hit testing over the final pane, release-time tree mutation,
and restores the default tree before the legacy projection checks. This is
interaction evidence only; live multi-monitor/DPI and custom floating dock
surfaces remain open.

The dock-splitter pointer follow-up now drives a visible `VerticalSplitter`
through the testing backend and verifies a bounded boundary change without
changing pane count. It restores the default tree before the remaining layout
checks; horizontal main-window splitter, live DPI, and platform minimum-size
evidence remain separate.

The UI modularization packet extracted the window-root modal overlay into
`main_modals.slint`. It deliberately moved no project state or mutation logic:
the component receives bounded properties and forwards typed callbacks, while
`MainWindow` continues to own the Rust-facing boundary. The GUI fixture passes
after the extraction, and the repository size audit now reports no Rust or
Slint source file above 1,000 lines.

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

Selection projection update: nested Sources rows now resolve through the same
bounded `DesktopState` path selection as top-level rows. Clicks, context-menu
opening, and depth-first keyboard navigation can select targets such as
`group/child`; Ctrl+A uses the same bounded visible-row projection. Nested
canvas geometry now uses the same stable path projection for selection, body
gestures, and transform handles. Leaf crop and leaf rotation under a uniform,
unmirrored parent now compose through this boundary; parent crop/rotation and
rotated leaves under non-uniform or mirrored ancestry remain separate until an
intermediate-scene transform model exists. The canvas
pointer fixture now also uses two
overlapping temporary top-level items to prove top-layer selection,
select-underneath on the next plain click, and Ctrl-toggle of the top layer
through the real pointer boundary. The pure hit-test oracle and the live event
path now exercise the same ordered selection owner.

The nested canvas pointer packet now drives real body and resize-handle
gestures for a group leaf and a Scene-reference leaf. Each fixture asserts the
stable flattened selection path, local transform mutation, and unchanged
enclosing transform, then removes its temporary group/reference scene. The
pointer controller defers select-underneath until a no-movement release, so
body drags cannot retarget an ancestor or overlapping source. The supported
leaf crop/uniform-rotation slice is covered by project/media/canvas oracles;
full parent-boundary crop/rotation and live DPI evidence remain open.

The Sources dock now exposes one atomic same-owner grouping command. It
preserves the parent order and child transforms for root, nested-group, or
same-owner Scene-reference selections, rejects mixed-owner or locked
selections, keeps the new path-addressed group selected, and leaves the
performance/presentation path unchanged. Ungrouping is its inverse for root,
nested-group, and same-owner Scene-reference group paths: child IDs are
validated before mutation, children replace the group at its former owner
position, and the UI selects the resulting root or nested child paths.

| ID | Gap | Current evidence | Consequence | Required target | Dependencies / first packet |
| --- | --- | --- | --- | --- | --- |
| GAP-001 | Render-target ownership is conflated | `RenderTarget`/`RenderTargetRole` and role-keyed WGPU targets now separate program, preview, projector, and encoder ownership; GUI preview no longer targets the full canvas by default. Selected-source and selected-scene projectors use the same worker runtime and bounded projector targets. | Native fan-out and projector lifetimes are not yet connected, and the compatibility readback remains explicit CPU storage. | Finish role-specific fan-out and native target import while preserving the CPU oracle and output scaling tests. | Work 005–006; preserve `obs-rs-media` CPU oracle and existing output scaling tests. |
| GAP-002 | No replaceable preview presenter | A safe `PreviewPresenter` trait now owns Slint conversion behind one boundary; the current implementation is `SlintPreviewPresenter`. | A native/WGPU surface implementation is still unavailable, so one viewport-sized copy remains. | Add native/WGPU presenter implementations and keep copy-byte accounting for the fallback. | Work 005; depends on GAP-001. |
| GAP-003 | GPU and encoder conversion are not a single fan-out graph | Composition can submit to WGPU, but the GUI preview uses RGBA readback and the encoder uses a separate NV12 readback bridge. | Duplicate transfers and target lifetimes make latency/copy accounting unclear. | One composed source texture feeds preview, projector, encoder conversion, and optional recording targets; each transfer is explicit and bounded. | Work 005; GPU Agent plus Output Agent review. |
| GAP-004 | Rendering cadence is not fully demand-aware | The worker coalesces requests and typed desktop demand now distinguishes visible/projector consumers (up to 60 Hz), minimized idle windows (5 Hz), hidden idle windows (no render), output-only work, and bounded selected-source/selected-scene projector work. Static invalidation and live multi-monitor cadence are not yet measured. | Static scenes and multi-monitor/projector demand can still do more work than necessary, and end-to-end cadence remains unverified. | Complete event-driven invalidation plus audio-meter/diagnostic cadence and live multi-window measurements. | Work 006; depends on GAP-001 and preview metrics. |
| GAP-005 | Hot-path allocation evidence is incomplete | Preview target dimensions and Slint-copy bytes are observable; WGPU composition no longer allocates a duplicate per-frame layer-descriptor vector, and the 10,000-frame local probe completed without drops or readbacks. Parameter buffers/bind groups, capture/audio/filter blocks, encoder staging, and diagnostics are not attributed end-to-end. | Regressions can hide behind bounded queues while allocating in capture/audio/filter/diagnostic paths. | Add scoped allocation/copy counters or an approved profiler harness for capture, audio, filters, WGPU staging, encoder, Slint models, and diagnostics. | Work 007; Performance Agent. |
| GAP-006 | Canvas interaction state is implicit in UI properties | `CanvasState` owns bounded transient zoom, pan, drag-box interaction, and the active snapping policy; `DesktopState` owns one bounded ordered selection shared by the dock and canvas. The persisted settings document owns one validated 1–100 canvas-pixel snap distance and one Show safe areas flag, applying both through the settings-to-studio boundary. The active guide set now includes bounded EBU/ITU Action, Graphics, and 4:3 safe-area edges, and the editable preview draws the same three rectangles. Crop/rotation and group transforms remain in typed drafts and are committed once per gesture; single-selection rotation keeps its pointer anchor in `CanvasController`, and multi-selection rotation snapshots every selected base transform and uses the initial bounding-box center as a transient pivot. Each sample is computed from those immutable bases and projects the ninth handle through the existing viewport mapping. Rotated Alt-crop and rotated resize deltas are converted through the same source-local frame as the media renderer, with the opposite visual edge solved in canvas space. Ordinary resize preserves aspect, Shift enables free resize, Ctrl suppresses snapping, Alt is carried as the typed crop modifier, arrow-key nudges and typed Transform-menu commands commit through the same project path. The visible top-level Sources rows now use a focused Slint boundary that sends Up/Down/Home/End, Shift range selection, and Ctrl-toggle navigation to the same Rust selection owner; the focused preview maps Ctrl+A to the same bounded select-all command. The editable canvas now has a real pointer fixture for plain and Ctrl-additive drag-box selection, while Rust owns the rotated single/group handle points and polygon path, which Slint only presents through the existing viewport mapping. | New canvas features can duplicate state or accidentally persist pointer drafts; nested-row selection projection, transform-handle pointer coverage, exact guide styling, live DPI, and native capture prerequisites remain incomplete. | Keep the toolkit-neutral `CanvasState { zoom, pan, selection_box, snapping, interaction }` and `DesktopState` selection as the single cross-surface owner; keep source-list focus transient, keep the persisted snap value and guide visibility in `AppSettings`, clamp again at the runtime boundary, keep drafts and rotation anchors transient, parse menu actions to a closed Rust command set, and commit one batch command per gesture/key action. | Work 009–013; Canvas Agent. |
| GAP-007 | Scene-item identity is weaker than OBS | `SceneItemSpec` has typed source, scene, and embedded group targets; schema v8 recursively persists group children, optional destination-scene transition policies, optional bounded Stinger resource references, and explicit profile scene order, command/codec validation bounds group depth, path-addressed group visibility/locking/order/removal/duplicate/transform/copy-paste/name commands are atomic, the Sources dock recursively projects group children with encoded paths and routes visibility/locking/order/duplicate/copy/paste/rename through those commands, and engine/GUI preview flatten visible nested scenes/groups into ordered runtime items while retaining one shared source definition per capture device. Stable project paths now cross flattening, runtime attachment, compositor layers, and GUI transform drafts; runtime transforms can be addressed by item ID rather than only by ordered index. Direct safe Transform-menu actions, target-aware visibility/locking and source filter/properties editing for nested group and Scene-reference leaves, target-aware modal transform editing for nested rows, top-level scene move commands, validated startup and per-profile Preview/Program selection restoration, profile-switch history restoration, bounded document-keyed Preview/Program restoration across collection switches and guarded Load/Recover flows, atomic current-document-to-new-collection duplication, stable collection-root resolution, atomic collection rename/export, validated external collection import, guarded Load/Recover replacement actions, native main-window close guarding, Save/Don't save/Cancel continuation, bounded durable per-document/per-profile scene-selection snapshots, and explicit confirmed removal of the exact project recovery temporary file now route through the same typed project/UI boundaries. Axis-aligned nested scale/translation/opacity and horizontal/vertical mirroring now use the profile canvas bounds during flattening and group-command validation; crop and rotation at transformed group boundaries still fail explicitly rather than being approximated. Durable collection-scoped active-scene persistence now records each visited profile within bounded document/profile records; the legacy index APIs remain incomplete/compatibility-only, while startup restoration ignores stale IDs without mutating project history. The remaining save/discard/recovery gap is the broader lifecycle beyond the guarded close/load/recover/save paths. | Complete nested crop/rotation semantics without reopening shared captures; keep stable IDs through any future reorder and interaction state. Complete the remaining save/discard/recovery lifecycle. | Scene/Engine Agent: nested transform oracle, profile/collection scene workflow, and full nested-item editing semantics; affects project codec, compositor, and UI. |
| GAP-008 | Dock interaction and floating geometry are incomplete | `DockNode::Split`, `DockNode::Tabs`, and `DockNode::Dock` provide one bounded, validated tree with versioned persistence; Rust emits pane and splitter projections, header drags resolve tab/directional targets, detached docks restore scale-aware physical geometry, and projector windows persist bounded physical geometry, fullscreen state, fixed-feed open state, source/scene target identities, and the observed platform monitor identity through versioned records, the same desktop clamp, and DPI scaling helpers. Projector right-click menus now expose the current platform monitor identities as typed rows; selecting one moves the existing projector and updates the persisted monitor record. The existing monitor capability supplies a virtual-desktop clamp for restored positions, while platforms without monitor bounds retain the saved coordinates. | Explicit projector assignment is implemented for enumerated platform monitors, but platform adapters beyond the current Linux/X11 slice, compositor-backed multi-monitor/DPI evidence, and floating custom-dock/plugin surfaces are not yet supported. A monitor disappearing between menu open and activation returns a bounded UI error rather than writing stale coordinates. | Extend platform monitor identity/capability adapters beyond the current Linux/X11 slice, add live multi-monitor fixtures, and extend the same explicit-target pattern to floating custom/plugin surfaces while retaining the shared bounded geometry/clamp helper and legacy migration. | Work 014–015 and projector persistence/selection follow-up; Dock Agent, Platform Agents, QA Agent. |
| GAP-009 | Filter persistence is ahead of filter execution | The project model accepts named filter kinds/settings, while runtime compilation recognizes only the small reference set; disabled filters are intentionally ignored, and unsupported categories/kinds or malformed settings now produce bounded engine/preview diagnostics. | An unavailable persisted filter is still absent from the render/audio graph, but the engine snapshot and exported preview diagnostics identify the source, filter, category, and reason. | Typed `AudioFilter`, `VideoFilter`, `AsyncVideoFilter`, and `GpuVideoFilter` nodes with explicit unavailable capability and CPU/reference oracle; retain the bounded diagnostic contract while execution coverage grows. | Work 018; Filter Agent and project/UI review. |
| GAP-010 | Audio is a reference mixer, not a full graph | Mixer, resampler, monitor taps, pacing, PipeWire discovery, deterministic fallback, provider-declared default-route selection, transient active-device identity, bounded recovery for configured and automatic microphone/desktop routes, a capacity-one engine route worker for automatic default changes, scheduled A/V drift counters, a sample-reusing positive per-channel sync-delay boundary, validated/persisted global microphone/desktop offset controls projected through the Audio settings page, typed per-source `AudioMonitorMode` output/monitor bus routing, engine/worker monitor-mode and sink-selection commands, a dedicated bounded `AudioOutputWorker` sink boundary, persisted Audio-page controls for the optional monitor-output device plus microphone/desktop monitor modes, idle-boundary audio-format reconfiguration, and typed standard channel-layout metadata/choices now exist. Full device graph, live unplug/replug graph reconciliation, adaptive hardware clock correction, source-level sync properties, per-source channel-layout routing, and multiple tracks do not. | Advanced audio workflows and production synchronization are incomplete; the current drift counters describe the portable scheduler rather than measured independent device clocks, automatic selection now tracks healthy provider default changes through a background discovery/opening boundary while explicit IDs remain pinned, settings control only positive global channel delay, two global monitor modes, a bounded sample-rate/channel-count rebuild, and standard layout metadata without per-source mapping, and the monitor sink has no complete per-source/advanced-audio workflow. | Model device/source/filter/gain/pan/bus/track/monitor routing with bounded blocks and clock ownership; add real device timestamps, measured clock correction, adaptive resampling, and full hotplug graph recovery, then expose channel-layout/source-property routing and multiple tracks without unbounded latency; keep device recovery off UI paths and preserve explicit fallback state. | Work 019; Audio Agent, then UI and platform agents. |
| GAP-011 | Production output boundary is not yet product-complete | `OBSRPKT1` and reference writers are robust fixtures; GStreamer provides an optional native boundary, while live production-sink tests are explicitly environment-ignored here. | The application can describe production profiles without a trustworthy cross-platform output path. | Separate `Encoder -> EncodedPacket -> Muxer -> Output`; capability negotiation must produce typed unavailable results and never silently fall back. | Work 022–024; Output Agent and Platform Agents. |
| GAP-012 | Services and transports are coupled at the product boundary | A bounded compile-time service catalog now separates stable service IDs/display names, first ingest defaults, protocol selection, and redacted stream-key settings from the target boundary. `StreamingTransport` gives reference packet and native GStreamer sessions one typed poll/reconnect/close lifecycle contract; `ReconnectPolicy` adds a capped exponential schedule whose deferred outcome never sleeps or grows queues. Media submission, native state mapping, and service metadata remain separate. There is still no signed catalog update path or full transport media/session abstraction. | Congestion, keyframe, auth lifecycle, and service-specific diagnostics cannot yet evolve independently of all native transport details. | Extend the lifecycle contract into a bounded network worker and transport capability model; retain the catalog as replaceable metadata and keep encoder/media ownership outside service configuration. | Work 024; Output Agent. |
| GAP-013 | Platform crates are boundaries more than implementations | macOS and Windows crates return typed unavailable behavior off-platform; Linux live paths are environment-unverified. | Cross-platform parity cannot be claimed from portable tests. | Each platform agent supplies capture, audio, encoder, virtual-camera, permission, device-loss, and recovery evidence behind safe Rust traits. | Work 027–030; platform matrix and hardware CI. |
| GAP-014 | Plugin ecosystem stops short of product distribution | Versioned Rust API, manifest validation, signatures, quotas, and subprocess frames exist. | Dynamic source/filter/output/service/UI extensions, permission prompts, update policy, crash supervision, and diagnostics are incomplete. | Versioned manifests with permissions/resource quotas, isolated process supervision, signature/update model, and extension-facing dock contracts. | Work 031; Plugin Agent, Security Reviewer. |
| GAP-015 | Settings and hotkeys are not fully typed at the behavior boundary | Settings persist several output fields and hotkey display strings; the toolkit-neutral `Shortcut` parser validates bounded Ctrl/Shift/Alt combinations, canonicalizes aliases/order, preserves empty unbindings, and Settings Apply atomically compiles one bounded action table owned by `DesktopState`. The GUI event path now parses the Slint label in Rust, resolves the typed action, and lets Slint invoke the established confirmation/output/project callbacks. Configurable local microphone and desktop mute actions, focused-window microphone push-to-talk/push-to-mute press/release with modifier-order-safe tracking, and circular Previous/Next Preview Scene navigation now use that same table and existing state/mixer callbacks, with no second state owner. The main canvas, Scenes, and Sources focus boundaries release a held microphone push action on window deactivation and ignore the delayed key release. | Global registration, push actions for other scopes/channels, arbitrary native-focus recovery, runtime effects for future actions, restart requirements, scene-specific bindings, and the remaining action set are still incomplete; menu shortcut labels remain display projections and there is no OS-level hotkey capability model. | Structured setting schema and `KeyCombination` model with capability, validation, effect, and restart metadata, followed by runtime/platform registration. | Work 025–026; UI/Platform Agents. |
| GAP-016 | Visual QA does not yet compare the same states | Slint can render deterministic settings fixtures in English and Spanish; live compositor screenshots and OBS fixture capture are unavailable in this environment. | Spacing, focus, hover, menus, dock proportions, and canvas behavior can regress without a measurable diff. | Reference fixture catalog, scripted workflows, screenshot diff thresholds, and platform/DPI/locale matrix. | Work 033; QA Agent. |
| GAP-017 | Reliability evidence is shorter than the product target | 300-tick A/V soak and bounded worker tests pass; no multi-hour stream/record/device/GPU/network soak is present. | Memory growth, reconnect storms, output failure recovery, and device loss remain unproven. | 30-minute and multi-hour fault-injection soaks with RSS, queue, deadline, copy, error, and recovery telemetry. | Work 035; Performance + QA Agents. |

Reconciliation note: the plugin contract now carries bounded dock descriptors,
and the core runtime registers them atomically under a plugin-scoped namespace
with usage accounting. Dynamic Slint/plugin surfaces, permissions, and
subprocess UI hosting remain open; this packet only closes the metadata and
runtime-registration boundary.

## Dependency graph

Latest nested Scene-reference verification covers source-name editing and
Transform-menu callbacks: the rename modal resolves a flattened leaf to the
profile-wide source, while geometry callbacks write the owning leaf and keep
the parent reference transform unchanged. Removal now follows the same owner
scene route and preserves the parent reference. Nested group rename remains
inside the supported command path through the same owner-scene resolver.
Duplication now follows the same route and can clone the profile source;
same-owner reorder now follows the same route. Grouping and ungrouping now use
the same-owner resolver as well, and reparenting resolves both source and
destination owners. Sources-dock pointer drag/drop now covers group and leaf
targets through the same command boundary, with selection recovery and locked
destination rejection. Full canvas/Scene-reference pointer drag/drop and the
remaining full transformed-boundary crop/rotation semantics remain open.

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
