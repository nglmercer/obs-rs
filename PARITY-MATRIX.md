# OBS 32.2.2 parity matrix

**Baseline date:** 2026-08-20  
**Baseline commit:** `7afb7fa` (Phase 0 evidence; implementation progress is
committed separately by phase.)
**Reference:** OBS Studio `32.2.2` (`obs --version` verified on this host)

This is the first executable parity inventory for the project. It describes what
was observed in the repository and in the available runtime probes; it does not
copy status from `03-roadmap.md` or `07-functional-todo.md`. Code, tests, and
runtime behavior remain authoritative.

`Complete` means complete for the narrowly named behavior and its current test
scope. It does not mean that the surrounding OBS subsystem has reached product
parity. A row remains `Partial` when a basic slice exists but an OBS workflow,
platform, or recovery path is missing. `Broken` means the available evidence
shows a failing path. `Missing` means no implementation was found.

The live desktop comparison is currently limited by this execution environment:
the reference version is installed, but no usable Wayland compositor or X11
server is available to this process. The deterministic Slint screenshot harness
did produce English and Spanish fixtures for all nine settings pages in
`artifacts/baseline/screenshots/`.

The latest audio packet makes the typed standard channel-layout metadata
functional at the provider-to-mix boundary: the bounded resampler maps Mono,
Stereo, 2.1, Quad, 5.1, and 7.1 by speaker role, while unknown `Discrete`
layouts retain the index-based fallback. This does not claim per-source audio
routing, adaptive clock correction, or multiple recording tracks.

## Latest verified package: Mixer options opens Audio settings

On 2026-08-26, the Mixer dock's settings button now follows a typed callback
through docked and floating dock boundaries and opens Settings directly on the
Audio page (category 4). The generic Settings action remains unchanged for the
navbar and Controls dock, and both paths reload the same bounded draft before
showing the window. The GUI fixture verifies the page target through the real
`SettingsController`; this does not claim complete OBS advanced-audio parity.

## Latest verified package: source clipboard keyboard workflow

On 2026-08-26, the main-window keyboard boundary now maps OBS's local `Ctrl+C`
and `Ctrl+V` source workflow to the existing Rust clipboard. `Ctrl+C` copies
the selected stable source path, while `Ctrl+V` uses the existing reference
paste command and selects the newly created scene item. Root, leaf, and nested
group target resolution remain owned by the same project/UI command path; the
GUI testing-backend fixture drives both keys and verifies the pasted item
references the original source. Paste Duplicate remains an explicit context
menu action, and global OS hotkey registration remains incomplete.

## Latest verified package: source-order keyboard workflow

On 2026-08-26, the editable canvas and focused Sources dock now map
`Ctrl+Up`, `Ctrl+Down`, `Ctrl+Home`, and `Ctrl+End` to the existing bounded
source-order commands. Arrow moves use the current parent-local order, Home
and End move to its boundaries, and first/last presses are consumed without a
canvas nudge. Rust retains lock validation, nested target resolution, project
history, and persistence; the GUI fixture verifies all four actions and
cleans up its temporary sources. Configurable/global source-order hotkeys
remain incomplete.

## Latest verified package: Sources-dock clipboard and select-all shortcuts

On 2026-08-26, the focused Sources dock now maps `Ctrl+C`, `Ctrl+V`, and
`Ctrl+A` through the existing typed callbacks. Copy uses the selected stable
source path, paste keeps the established reference mode and selects the new
scene item, and select-all uses the bounded visible-source projection. The
GUI fixture verifies the workflow with dock focus and cleans up its temporary
items; Paste Duplicate and global OS hotkey registration remain incomplete.

## Latest verified package: native slideshow directory Browse picker

On 2026-08-26, `SOURCE-007` extended the source-properties Browse boundary to
`image_slideshow`. The `paths` row now opens a bounded native directory chooser
on Linux (`zenity`/`kdialog`), macOS (`osascript`), and Windows (PowerShell),
starting near the first configured path when possible. The selected directory
returns through the existing local draft and is committed only by OK; typed
multiple paths and file selection remain available as the explicit fallback.
Picker command tests and the integrated GUI properties/commit fixture pass.

The project codec now writes schema 8 for the bounded Stinger resource
reference; schema 7 remains a supported read-only compatibility version for
documents created before that field existed.

## Latest verified package: bounded Stinger runtime and explicit Take

On 2026-08-25, `STUDIO-002` gained a bounded runtime slice for Stinger
transitions. `StingerClip` validates 1..=256 decoded RGBA frames, matching
formats, per-frame durations, a 120-second total-duration ceiling, a 256 MiB
resident-pixel ceiling, and a normalized transition point. `TransitionSnapshot`
and the UI command carry the same preloaded clip without duplicating scene
state; the preview worker uses it for both the viewport-sized preview and the
full program/output target, scaling only at the bounded presentation boundary.
Media, UI, and GUI tests cover frame selection, the scene cut, invalid inputs,
and preview/output geometry. This does not claim full OBS Stinger parity:
schema 8 now persists a bounded resource path, transition point, preload flag,
and hardware-decode preference through a scene-owned `StingerSpec` and typed
project command. Scene Properties edits those fields as one undoable command.
Worker-side decoding, track mattes, and fade/audio monitoring policy remain
incomplete; the current GUI packet now supplies the bounded asynchronous
picker described below.
The native GStreamer adapter now probes the installed decoder registry before
exposing the hardware preference. Scene Properties enables that checkbox only
when a known hardware decoder and the `decodebin` selection property are
available; the loader sets `force-sw-decoders` explicitly for software versus
hardware preference and returns typed `DecoderUnavailable` instead of silently
accepting a hardware request without a native path. Codec-specific hardware
selection and other platform adapters remain incomplete.
The separate resource-worker boundary is now bounded at one pending request
and one result, supports non-blocking polling and typed request IDs, and
honors cooperative cancellation. The optional native GStreamer adapter now
decodes local file/container resources through negotiated RGBA caps and
bounded preloading, returning typed unreadable/decoder/frame/timeout failures;
the one-slot result policy discards stale request IDs without blocking submit
or poll callers. Engine/UI application is now connected for selected-scene
preload and scene-properties persistence; codec-specific hardware selection,
other platform adapters remain incomplete. Scene Properties now launches a
capability-backed asynchronous file picker on Linux (`zenity`/`kdialog`),
macOS (`osascript`), and Windows (PowerShell) when available; the selected
path returns through the Slint event loop with bounded UTF-8/path-size checks.
The toolkit-neutral `StingerLoadSession` now owns only the transient current
request/clip, preserves state on a full queue, validates the target format,
and invalidates old completions when the canvas format changes. The GUI now
constructs the native GStreamer loader behind the engine boundary and
preloads only the selected scene's `preload=true` resource from the refresh
timer; scene-properties fields are synchronized and validated without blocking
the UI. The docked and floating Transition panels expose an explicit `Take
Stinger` action that parses the configured duration, clones only the ready
worker-published clip, and dispatches the existing typed `TakeStinger` command.
Not-ready, typed decode failure, stopped-loader, invalid-duration, and state
dispatch errors are visible in the status surface. The picker reports
unavailable, already-open, cancelled, spawn, and selection errors without
blocking the UI. When a persisted resource is not ready, the first Take can
now submit a bounded on-demand request through the same worker; the refresh
cadence keeps the duration as a transient intent and dispatches automatically
when the matching clip becomes ready. Codec-specific hardware selection and
non-GStreamer adapters remain incomplete.

The asynchronous picker boundary is implemented in
`crates/obs-rs-gui/src/callbacks/stinger_picker.rs`; the capability is
disabled when no supported desktop chooser is discoverable on `PATH`.

## Latest verified package: project Save As workflow

On 2026-08-25, `PROFILE-001` gained a bounded project `Save As...` workflow.
The File menu and bilingual project dialog expose a separate mode that writes
the current document through the existing atomic `ProjectFileStore`, changes
the active project path and selection key only after the rename succeeds, and
keeps the existing document state when the target is empty or is a different
already-existing file. The new path is then used by later Save, Load, Recover,
and session-persistence operations. Callback, persistence, conflict, and GUI
dialog-mode tests pass. The broader collection/recovery UX remains open.

## Latest verified package: native project Save As picker

On 2026-08-26, `PROFILE-001` reused the bounded asynchronous file-picker
boundary for project Save As. Linux (`zenity`/`kdialog`), macOS (`osascript`),
and Windows (PowerShell) use native save-dialog commands when discoverable;
the selected path returns through the Slint event loop and is still validated
by the atomic Save As callback before any document state changes. The picker
has separate path limits for Stinger resources and project paths, shares one
activity gate across both dialogs, and exposes a bilingual manual-path message
when no chooser is available. Picker-command, GUI wiring, and the existing
Save As persistence tests pass.

## Latest verified package: target-aware nested source context actions

On 2026-08-26, `SOURCE-001` and `SCENE-002` gained target-aware `Rename` and
`Remove` behavior for nested Sources-dock rows. Root rows preserve OBS
multi-selection removal, while a nested row removes only its clicked stable
group/Scene-reference path; nested leaf rename is enabled and remains tied to
that path even if the canvas selection changes before confirmation. The GUI
drag/drop fixture covers nested removal, rename, selection preservation, and
locked-container recovery. Interact and the remaining context-menu actions
remain incomplete.

## Latest verified package: target-aware nested monitor selection

On 2026-08-26, `SOURCE-001`/`SOURCE-003` extended the shared monitor picker to
accept the stable `SourceTarget` resolved by Source Properties. A nested screen
leaf therefore opens the picker for its owning source even if an unrelated root
row becomes selected while the properties dialog is open; monitor refresh and
accept use the same target rather than re-reading the current canvas selection.
The production properties installer shares the existing picker controller, so
this does not create a second monitor or project state owner. The GUI fixture
covers the nested path and source identity. Live display enumeration,
multi-monitor/DPI evidence, and Windows/macOS capture adapters remain partial.

## Latest verified package: native image-source Browse picker

On 2026-08-26, `SOURCE-007` added a capability-backed asynchronous Browse action
to the existing `image_source` properties workflow. The path row now uses the
same bounded chooser boundary on Linux (`zenity`/`kdialog`), macOS
(`osascript`), and Windows (PowerShell), with image filters and manual-path
fallback when no supported chooser is discoverable. A selected path returns
through the properties draft's existing `edit-property` callback and changes
the project only after OK; slideshow paths and directory expansion remain
outside this packet. Picker-command tests and the GUI properties/commit fixture
pass.

## Latest verified package: native project Open picker

On 2026-08-26, `PROFILE-001` completed the native chooser slice for opening a
project. The existing dirty-session confirmation now leads to an explicit
Open-project dialog instead of loading the current path immediately. Linux
(`zenity`/`kdialog`), macOS (`osascript`), and Windows (PowerShell) use native
open-dialog commands when discoverable; the selected path returns through the
Slint event loop with the same bounded UTF-8/path-size validation as Save As.
The dialog has separate bilingual copy and keeps a manual-path fallback when
no chooser is available. Project store construction now rejects unsupported
extensions while retaining the shipped `.json` path as a legacy compatibility
form; native project filters advertise both `.obsrproj` and `.json`. Recovery
now opens a bilingual review modal before replacing the in-memory document; it
shows the active and temporary paths, preserves the `.tmp` on cancel, keeps the
modal open after a parse failure, and leaves a successful recovery dirty until
the user saves it. The broader recovery policy remains incomplete. The focused
GUI snapshot fixture and the picker-command tests pass.

## Latest verified package: project recovery review

On 2026-08-26, `PROFILE-001` stopped the Recover-project action from replacing
the active document immediately. A complete temporary project now opens a
bilingual confirmation modal showing both paths; confirmation uses the existing
typed `ProjectFileStore` recovery boundary, cancellation leaves the temporary
file untouched, and parse failures keep the modal available for retry. A
successful recovery closes the modal, refreshes the scene/editor projections,
and remains dirty until an explicit save publishes the document. The GUI
fixture covers rendering, successful recovery, and the failure/retry path.

## Latest verified package: native collection import/export pickers

On 2026-08-26, `PROFILE-001` extended the bounded asynchronous chooser to the
existing collection Export and Import dialog modes. Export uses a native save
dialog and Import uses a native open dialog on Linux (`zenity`/`kdialog`),
macOS (`osascript`), and Windows (PowerShell), with an OBS-RS collection
extension filter and separate path state. Selection returns through the Slint
event loop; the existing collection callbacks still own validation, atomic
writes, and document switching. The bilingual dialogs retain manual-path
fallbacks when no chooser is available, and picker-command plus GUI snapshot
tests pass.

## Latest verified package: portable Luma Wipe transition

On 2026-08-25, `STUDIO-002` gained a bounded portable Luma Wipe slice. The
typed `FrameTransition`/`TransitionSpec` model persists horizontal and vertical
linear luminance patterns, inversion, and softness in the 0..=1000 milli-range.
The CPU/reference compositor writes directly into the destination buffer and
does not allocate a second full-frame mask. Scene overrides, the console,
docked/floating transition controls, and the bilingual scene-properties dialog
use the same typed callbacks and persistence boundary. Media, project, UI, and
GUI workflow tests pass. OBS asset-backed mask patterns, external pattern
resources, and the full Stinger workflow remain incomplete.

## Latest verified package: bounded plugin dock metadata

On 2026-08-25, the plugin contract gained validated dock descriptors with
bounded titles and an optional `Plugin::dock_descriptors()` contribution. The
headless runtime registers them atomically under the `(plugin, dock)` namespace,
rejects duplicate declarations and lists above the per-plugin limit, exposes
deterministic metadata and usage counts, and keeps toolkit/native handles out
of the plugin boundary. Existing source-only plugins remain compatible. A
dynamic Slint dock surface, persisted custom-dock tree IDs, permissions, and
subprocess UI hosting remain incomplete.

## Latest verified package: built-in Stats dock

On 2026-08-25, the sixth built-in dock (ID 5) was added to the validated dock
tree and persisted layout. The Stats panel is available docked, tabbed, or
floating, has English and Spanish labels, and projects the existing status,
capture-capability, preview, and output diagnostics at the existing bounded UI
refresh cadence. It does not request work from render, capture, audio, or
output hot paths. Plugin-provided dynamic dock surfaces remain a separate open
capability.

## Latest verified package: Scene-dock drag/drop reorder

On 2026-08-25, the Scenes dock gained a real before/after DragArea/DropArea
workflow for persisted profile scene order. Each row carries a bounded typed
`DataTransfer`; Rust resolves the active profile, adjusts the insertion index
after removing the dragged row, rejects malformed payloads/modes/targets, and
selects the moved Preview scene through the existing state owner. Docked and
floating surfaces share the same callback chain. Pure index tests and a
testing-backend pointer fixture cover upward, downward, same-row, and invalid
drop cases. Scene-specific hotkeys, global registration, and broader
collection lifecycle behavior remain open.

## Latest verified package: persisted Toggle Studio Mode hotkey

On 2026-08-25, `HOTKEY-001` gained a typed `ToggleStudioMode` action. The
action is included in the bounded `DesktopState` shortcut table, has a
persisted and validated settings field, appears in the View menu and bilingual
Hotkeys page, and routes through the same Rust-owned callback that toggles the
existing Studio/single-canvas view state. The GUI fixture covers action code
18, callback entry/exit, settings round-trip, and conflict-table inclusion.
The default remains unbound to avoid stealing a desktop/window-manager key;
global OS registration, push-to-talk/mute, and the remaining OBS action set
remain incomplete.

## Latest verified package: selected-source visibility hotkey

On 2026-08-25, `SOURCE-001`/`HOTKEY-001` gained a typed
`ToggleSelectedSourceVisibility` action. The bounded shortcut table, persisted
Hotkeys field, English/Spanish settings row, action code 19, and Slint
execution path reuse the existing Rust source-visibility callback. The action
refuses an empty or `none` selected target at the UI boundary; its default
remains unbound. Global registration, Interact, and the remaining source
context-menu actions remain incomplete.

## Latest verified package: selected-source lock hotkey

On 2026-08-25, `SOURCE-001`/`HOTKEY-001` gained a typed
`ToggleSelectedSourceLock` action. The bounded shortcut table, persisted
Hotkeys field, English/Spanish settings row, action code 20, and Slint
execution path reuse the existing Rust source-lock callback. The action
requires a non-empty, non-`none` selected target and remains unbound by
default. Global registration, Interact, and the remaining source action
catalog remain incomplete.

## Latest verified package: selected-source projector hotkey

On 2026-08-25, `STUDIO-003`/`HOTKEY-001` gained a typed
`ToggleSelectedSourceProjector` action. The bounded shortcut table, persisted
Hotkeys field, English/Spanish settings row, action code 21, and Slint
execution path reuse the existing shared-feed source-projector callback.
Empty selections, groups, and Scene references are rejected at the UI
boundary; the default remains unbound. Live multi-monitor evidence and global
registration remain incomplete.

## Latest verified package: Preview-scene projector hotkey

On 2026-08-25, `STUDIO-003`/`HOTKEY-001` gained a typed
`TogglePreviewSceneProjector` action. The bounded shortcut table, persisted
Hotkeys field, English/Spanish settings row, action code 22, and Slint
execution path reuse the existing scene-projector callback with the current
Preview scene target. Empty or `none` Preview targets are rejected at the UI
boundary; the default remains unbound. Live multi-monitor evidence and global
registration remain incomplete.

## Earlier verified package: Slide and Swipe transition core

On 2026-08-25, `STUDIO-002` gained bounded portable Slide and Swipe
transitions. The CPU/reference compositor moves source/destination pixels in
place without allocating a second frame: Slide moves both layers, while Swipe
moves the source and leaves the destination stationary in the revealed area.
`TransitionSpec` persists the typed kind and left direction; scene properties,
the Transition dock, and the console expose both options. Media correctness,
project round-trip, UI parser, localized labels, and GUI workflow tests pass;
the timing reports remain ignored benchmarks. Stinger, luma, `swipe_in`, and
the visual direction selector remain incomplete.

## Latest verified package: Slide and Swipe direction expansion

On 2026-08-25, `STUDIO-002` expanded the typed portable direction model to
`left`, `right`, `up`, and `down` for both Slide and Swipe. The in-place CPU
compositor now handles horizontal and vertical axes with direction-safe
destination traversal; JSON accepts all four identifiers and the console keeps
the old `slide|swipe <progress>` syntax while adding
`slide|swipe <direction> <progress>`. Media 2×2 reference cases and project/UI
parser round-trips pass.

## Latest verified package: Slide and Swipe visual direction selector

On 2026-08-25, `STUDIO-002` gained a localized direction selector for Slide
and Swipe in the docked Transition panel, floating Transition panel, and
scene-properties dialog. English and Spanish expose Left, Right, Up, and Down;
the selected bounded index reaches the typed transition callback and persisted
scene override, and the refresh projection restores it when the dialog opens.
Legacy three-argument callbacks remain compatible and continue to mean left.
The GUI workflow fixtures cover right and up selections. Stinger, luma,
`swipe_in`, and the remaining transition catalog/assets/plugins remain open.

## Latest verified package: Swipe In transition mode

On 2026-08-25, `STUDIO-002` gained the OBS-compatible Swipe In mode. The typed
`FrameTransition`/`TransitionSpec` model distinguishes outgoing Swipe from an
incoming destination layer, and the CPU compositor keeps the source stationary
under the incoming destination without allocating a second frame. Project JSON
persists `swipe_in` for new documents while older Swipe records default to
`false`; the bounded console accepts `swipe in ...`, `swipe out ...`, and the
`swipe_in ...` alias. English and Spanish dock/scene-property checkboxes pass
the mode through normal and floating callback paths. Media, project, console,
runtime-label, and GUI workflow tests pass. Stinger, luma, and other transition
catalog/assets/plugins remain open.

## Latest verified package: exact nested leaf crop/rotation slice

On 2026-08-25, nested group and Scene-reference flattening gained a bounded
exact slice for leaf transforms. Crop is preserved across an axis-aligned
parent, including parent mirroring, and leaf rotation is preserved when the
crossed parent scale is uniform and unmirrored. Media, project, GUI inverse,
and canvas projection tests cover the behavior. Parent crop/rotation and a
rotated leaf under non-uniform or mirrored ancestry still return an explicit
unsupported result; those cases require an intermediate-scene clipping/shear
model.

## Latest verified package: local Cut transition hotkey

On 2026-08-25, Studio Mode gained a typed local Cut transition hotkey. The
setting is an optional, persisted shortcut with the same bounded parser and
conflict validation as the existing actions. Rust resolves it to a stable GUI
action code and Slint invokes the existing Cut callback, so the hotkey sends
the selected Preview scene to Program through the same transition path as the
button. The default remains unbound; global registration and scene-specific
hotkey actions remain open.

## Latest verified package: ordered Preview scene navigation hotkeys

On 2026-08-25, the local hotkey table gained persisted Previous Preview Scene
and Next Preview Scene actions. They resolve against the active profile's
explicit scene order, wrap at either end, clear an in-progress transition, and
refresh the same source selection boundary used by manual scene selection.
Invalid navigation directions are rejected by the toolkit-neutral state
machine; the GUI only bridges stable action codes 16 and 17. The defaults are
`F6` and `F7`, and they participate in the existing bounded conflict and
canonicalization checks. Global OS registration and scene-specific bindings
remain open.

## Latest verified package: keyboard Scene-dock navigation

On 2026-08-25, the Scenes dock gained a focused keyboard boundary shared by
docked and floating panels. Up/Down select the previous/next scene with the
same circular profile-order policy as the local navigation hotkeys, while Home
and End select the first/last persisted scene. Rust remains the only owner of
the Preview selection and source-selection refresh; Slint only forwards the
bounded direction. The toolkit-neutral edge/navigation tests and the real
testing-backend GUI fixture pass. Native screen-reader and platform focus
behavior remain open.

## Latest verified package: scene-item reparenting

On 2026-08-24, `SCENE-002`/`SOURCE-001` gained an atomic
`MoveSceneItemToParent` command. A root or nested item can move to the scene
root or any existing group while retaining its stable ID, transform, visibility,
and lock state. The Sources dock exposes bounded, localized Move to group
destinations through the same command for docked and floating panels, and
selection follows the item’s new path. Cycles, locked ancestors, duplicate
destination IDs, and invalid order positions fail without mutation. Project
tests cover root/group and group/group moves plus failure atomicity; the GUI
fixture covers destination projection, callback execution, selection recovery,
and cleanup. The later Scene-reference package extends this same contract
across owner scenes; transformed-boundary crop/rotation and full pointer
drag/drop evidence remain open.

## Latest verified package: scene properties

On 2026-08-24, the Scene properties dialog now edits the scene display name
and its optional Program transition override through one atomic
`SetSceneProperties` project command. The dialog exposes inheritance from the
desktop transition, Cut, Fade, and Fade to color with bounded duration and
RGBA color parsing. Project history records the name and transition as one
undo step; invalid names or transition values leave the project unchanged.
Refresh projection is versioned and does not overwrite fields while the modal
is being edited. Project tests cover persistence, inheritance clearing, undo,
and atomic rejection; the GUI fixture covers cross-fade, fade-to-color,
inheritance, and the single undo step. The existing dock transition controls
continue to use the same parser and runtime state.

## Latest verified package: keyboard source deletion

On 2026-08-24, an unmodified `Delete` key press from either the focused Sources
dock or the main canvas focus boundary now routes the selected source through
the existing Rust removal callback. That keeps nested path resolution, locked
item validation, project history, and refresh ownership in one boundary rather
than duplicating deletion state in Slint. The GUI fixture proves that an
unlocked source is removed from the editable canvas focus and that a locked
source remains when the Sources dock is focused, with the expected status
error. Modifier combinations reserved for shortcuts or alternate actions
remain outside this packet.

## Latest verified package: atomic multi-selection Delete

On 2026-08-24, Delete now removes the complete bounded `DesktopState`
selection through one `RemoveSceneItems` project command. Root and nested paths
are validated before mutation, selecting both a group and a descendant removes
the group once, and a locked target or locked group ancestor rejects the whole
gesture without partial removal. The command therefore creates one undo/redo
boundary instead of one history entry per selected row. The callback is wired
through the canvas, Sources dock button, SourceContextMenuArea, dock workspace,
dock slot, and floating-dock boundaries.
Project tests cover root/nested removal, ancestor collapse, lock rejection, and
one-step undo/redo. The GUI fixture reaches the new keyboard scenario before an
existing native capture-device assertion fails on this host because no device
row is available; the overall GUI fixture is therefore not green in this
environment.

## Latest verified package: source-dock modifier selection

On 2026-08-24, left-clicks in the visible Sources rows now carry the actual
PointerEvent modifier state to the Rust-owned selection boundary. Plain click
replaces the bounded selection, Shift-click selects the contiguous depth-first
row range from the active source, and Ctrl-click toggles one source in or out;
docked, floating, and context-menu surfaces forward the same typed callback.
The GUI fixture covers replacement, ascending and descending range selection,
and toggle after each model refresh. Drag-box selection and complete nested-row
pointer evidence remain open. The full fixture still stops later at the host's
unavailable native capture-device row.

The follow-up keyboard packet now applies the same contiguous range resolver to
Shift+Up/Down/Home/End in the focused Sources dock. The navigation fixture
proves range replacement and Ctrl toggle after keyboard movement; plain
navigation remains replacement and does not wrap at list edges.

## Latest verified package: canvas drag-box pointer fixture

On 2026-08-24, the GUI fixture now drives the editable `CanvasEditor::surface`
through the testing backend's real pointer path. It computes letterboxed canvas
coordinates from the current fit zoom and transient pan, starts on empty space,
selects one intersecting source with a plain drag, adds a second intersecting
source with Ctrl-drag, and verifies both middle-button and Space+drag pan. The
fixture removes its temporary sources, restores the starter background,
selection, pan, and wheel-zoom state, and leaves transform/presentation state
unchanged. Nested group/Scene-reference handle coverage is now provided by a
separate fixture; live DPI and the later native capture-device prerequisite
remain open.

The same canvas pointer fixture also inserts two overlapping temporary items:
a plain click selects the top layer, a second plain click walks to the selected
layer underneath, and Ctrl-click toggles the top layer back into the ordered
selection. This covers the live pointer path for the existing hit-stack rule.

It also inserts a temporary selected source, resolves the Rust-published
bottom-right handle coordinates, and drags that handle through the real
`TouchArea`. The resulting scale change is observed in project state before
the temporary item is removed; rotation, crop, and live DPI pointer evidence
remain open.

The fixture separately resolves the published rotation handle, drags it to a
new canvas angle, and observes the changed fixed-point rotation after the
single commit on release. Crop and live DPI pointer evidence remain open.

It also holds Alt while dragging the published left-middle handle inward and
observes a larger `crop_left` with the horizontal scene scale unchanged. This
covers the real modifier path for crop; live DPI evidence remains open.

The same GUI fixture now inserts a temporary group at a visible row boundary,
clicks its nested child through the real SourceContextMenuArea pointer target,
and verifies the stable `group/child` selection path before removing the group.

## Latest verified package: nested canvas pointer transform handles

On 2026-08-25, the GUI canvas fixture now drives the real testing-backend
pointer path through a nested group leaf and a leaf below a Scene-reference.
Each leaf is selected by its stable flattened path, its bottom-right resize
handle changes the owning local transform, and the enclosing group or scene
reference remains unchanged. The fixture removes both temporary roots and the
child scene after the assertion. This adds nested pointer/handle evidence
without claiming transformed-boundary crop/rotation or live DPI parity.

## Latest verified package: nested canvas body gestures

On 2026-08-25, the same fixture now covers body drags for both nested targets.
The pointer controller defers select-underneath until release, so a real drag
keeps the selected leaf instead of retargeting its group/reference ancestor or
an overlapping source. The drag commits one local transform change while the
enclosing transform remains unchanged; the existing overlapping-source click
fixture still proves plain select-underneath and Ctrl-toggle behavior.
Transformed-boundary crop/rotation and live DPI remain open.

## Latest verified package: nested group canvas projection

On 2026-08-24, the editable canvas now consumes the same flattened, stable
`group/child` paths as the Sources dock for visible nested group leaves. Hit
testing, drag-box selection, snapping guides, selection overlays, keyboard
nudge, and transform drafts operate on the effective profile-canvas rectangle;
release converts the draft back to the child's local transform and commits all
selected paths through one atomic `SetSceneItemTransforms` command. Locked
ancestors are respected. Scene-reference leaves are covered by the follow-up
package below; crop or rotation at a transformed group boundary still returns
an explicit unsupported result.

## Latest verified package: nested scene-reference canvas editing

On 2026-08-24, flattened canvas targets below `Scene` sources now resolve
through groups and scene references. Effective canvas transforms, ancestor
locks, live preview drafts, inverse local conversion, and one atomic batch
commit are shared with nested groups. The project command writes the leaf into
the referenced scene that owns it, preserving the scene-reference transform
and stable runtime path. Axis-aligned scale/translation/opacity/mirroring are
supported; crop or rotation crossing a transformed scene/group boundary still
returns an explicit unsupported result.

The standalone Transform dialog now resolves the same flattened paths. It
displays and edits the leaf's local transform, routes the commit to the owning
referenced scene, preserves the parent Scene item, and rejects edits when a
Scene/group ancestor is locked. The GUI fixture covers the successful commit
and inherited-lock failure path.

The standalone Source Properties and Filters dialogs now resolve those same
flattened paths to the profile-wide source definition. A nested Scene leaf
therefore edits the shared source settings/filter list instead of failing as an
unknown target or editing the Scene-reference item. Source-projector
availability uses the same resolver, so nested Scene leaves remain eligible
without opening another capture runtime. The GUI fixture covers nested
properties and filter commits; target lock behavior remains enforced by the
existing flattened transform/filter command paths.

## Latest verified package: nested scene-reference item callbacks

On 2026-08-24, Sources-dock visibility and lock actions now send one stable
target path through the generic scene-item commands. The project command
adapter resolves group and `Scene`-reference segments to the scene that owns
the leaf, so `scene-ref/leaf` changes the child item without mutating the
parent reference. Project and GUI fixtures cover both toggles and leave
remove/reorder/duplicate of referenced leaves explicitly outside this packet.

## Latest verified package: nested scene-reference source rename

On 2026-08-24, the source rename modal now resolves a flattened
`scene-ref/leaf` target to the profile-wide source definition. Direct group
rename behavior remains unchanged, while nested Scene-reference leaves update
the shared source name through the existing `SetSourceName` command. The GUI
fixture covers opening the modal, editing the name, and observing the profile
source update.

## Latest verified package: nested scene-reference group rename

On 2026-08-24, `SetGroupName` now accepts the same stable flattened path when
the addressed group is below a `Scene` source. The command resolves the owner
scene and local group path before mutating the group display name, preserving
the parent Scene-reference item. The rename modal resolves the nested group
name through the canvas target resolver; project and GUI fixtures cover the
owner-scene mutation and modal workflow.

> SCENE-002/SOURCE-001 reconciliation: the row's older nested group-name
> limitation is superseded by this package; the remaining transformed-boundary
> gap is nested crop/rotation semantics.

## Latest verified package: nested scene-reference Transform menu

On 2026-08-24, the OBS-style Transform submenu and flip callback now read
flattened `scene-ref/leaf` targets through the canvas resolver before issuing
the existing atomic transform commands. Centering and horizontal flip update
the leaf in its owning scene while preserving the parent Scene-reference
transform. The GUI fixture covers center alignment, flip round-trip, and the
existing inherited-lock dialog path.

## Latest verified package: nested scene-reference item removal

On 2026-08-24, `RemoveSceneItem` now accepts a stable flattened
`scene-ref/leaf` path and resolves it to the owning scene before mutation. The
Sources-dock callback uses that generic command for root, group, and
Scene-reference rows, so removing a referenced leaf preserves the parent
Scene-reference item and its transform. Project and GUI fixtures cover the
owner-scene mutation; nested reorder and duplicate remain outside this packet.

> SCENE-002/SOURCE-001 reconciliation: the earlier limitation “nested
> Scene-reference remove/reorder/duplicate” is now narrowed to reorder and
> duplicate; nested remove is covered by this package.

## Latest verified package: nested scene-reference item duplication

On 2026-08-24, `DuplicateSceneItem` now resolves a flattened
`scene-ref/leaf` target to the owner scene/group before applying the existing
reference or source-clone mode. The Sources-dock callback uses that generic
command for root, group, and Scene-reference rows. The project and GUI
fixtures cover source cloning in the referenced scene, preserving the parent
reference, and removing only the original leaf afterward; nested reorder is
the remaining item-management gap in this sequence.

## Latest verified package: nested scene-reference item reordering

On 2026-08-24, `MoveSceneItem` now resolves a stable flattened
`scene-ref/leaf` target to the scene or group that owns the leaf before applying
the existing same-parent reorder operation. The Sources-dock callbacks use the
same generic command for root, group, and Scene-reference rows, while the
owner's ordered child list is used for bounded target-index validation. The
project fixture proves the parent reference remains unchanged; the GUI fixture
proves duplicate, reorder, and remove as one nested workflow. Reparenting a
Scene-reference leaf across scene boundaries remains intentionally outside this
packet and continues to use the separate `MoveSceneItemToParent` contract.

> SCENE-002/SOURCE-001 reconciliation: same-owner nested Scene-reference
> reorder and cross-owner reparenting are now covered. Pointer drag/drop
> evidence is now covered for Sources-dock group/leaf rows; canvas and
> Scene-reference GUI drag/drop remain open, alongside crop/rotation semantics
> at transformed boundaries.

## Latest verified package: nested scene-reference reparenting

On 2026-08-24, `MoveSceneItemToParent` now resolves both the source item and
destination container through flattened group/`Scene`-reference paths. A move
can therefore cross from a parent scene into a referenced scene (or back),
without reopening sources or duplicating scene state. The command validates
crossed locks, local destination bounds, duplicate IDs, group-depth/cycle
constraints, and scene-reference cycles before publishing a transactional
two-scene mutation. Move-target projection includes referenced scenes and
their nested groups; the project and GUI fixtures cover both directions,
selection-path recovery, locked-boundary rejection, and self-descendant
rejection.

> SCENE-002/SOURCE-001 reconciliation: cross-owner nested Scene-reference
> reparenting is now covered. Sources-dock group/leaf pointer drag/drop is
> covered; canvas/Scene-reference GUI drag/drop and transformed-boundary
> crop/rotation remain open.

## Latest verified package: Sources-dock pointer drag/drop

On 2026-08-25, visible source rows gained typed `DataTransfer` payloads and
Slint `DragArea`/`DropArea` input boundaries. Dropping on a group or
Scene-reference container inserts at the front; dropping on a leaf uses
bounded before/after zones. The Rust callback resolves flattened paths and
indexes, then dispatches the existing `MoveSceneItemToParent` command so the
project remains the source of truth. Selection follows the new path after a
successful move, while locked destinations are rejected without mutation.

The GUI fixture covers real vertical pointer drags into a group and a nested
leaf, order and selection-path recovery, and locked-container rejection. The
Sources viewport disables mouse-drag panning so its Flickable cannot capture
the gesture; wheel scrolling remains available. Canvas and Scene-reference
GUI drag/drop, plus transformed-boundary crop/rotation, remain open.

## Latest verified package: nested scene-reference grouping

On 2026-08-24, `GroupSceneItems` and `UngroupSceneItem` now resolve flattened
targets through `Scene` references before applying the existing atomic
same-owner operation. Leaves below one referenced scene can be grouped or
ungrouped while preserving the parent reference and the owner's order,
transforms, locks, and visibility. Mixed-owner selections are rejected before
mutation. The Sources-dock availability check and toolkit-neutral selection
resolver now validate the same paths; project, UI-state, and GUI fixtures cover
selection, grouping, ungrouping, owner-scene mutation, and parent preservation.

> SCENE-002/SOURCE-001 reconciliation: same-owner nested Scene-reference
> grouping, ungrouping, and cross-owner reparenting are now covered. Full
> Sources-dock group/leaf pointer drag/drop is now covered; full
> canvas/Scene-reference GUI drag/drop and transformed-boundary crop/rotation
> remain open.

## Latest verified package: dock-header pointer drag

On 2026-08-24, the GUI dock fixture now drives a visible `DockHeader` through
the testing backend's real pointer path. It verifies drag start, directional
right-zone hit testing over the final pane, tree mutation on release, and
restores the default dock tree before the remaining layout assertions. The
fixture still does not provide compositor-backed multi-monitor/DPI evidence.

The same layout fixture now drags a visible `VerticalSplitter` through pointer
events and verifies that the pane count stays constant while one projected
boundary changes. The tree is restored before the legacy projection checks.

## Latest verified package: main-window modal modularization

On 2026-08-24, the 1,024-line `main.slint` component was split at the
window-level modal boundary. `MainModals` now owns only dialog presentation and
two-way field bindings; `MainWindow` remains the single owner of state and
callbacks. The extracted component preserves setup scrim, confirmation,
project/scene/output, collection, recovery, remux, and rename dialog flows.
`main.slint` is now 900 lines and no tracked Rust/Slint source file exceeds the
1,000-line modularization threshold.

## Core, scheduling, and rendering

### Matrix correction — 2026-08-25

The `CANVAS-003` and `CANVAS-004` rows below predate the latest GUI fixture
wording. Their current evidence includes nested group/Scene-reference body
drags and resize handles, plus deferred select-underneath semantics; only
transformed-boundary crop/rotation and live DPI remain open.

| ID | Feature | OBS behavior | OBS-RS observed behavior | Status | Platform | Tests / performance evidence | Files involved | Dependencies |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CORE-001 | Project model and persistence | Projects preserve profiles, scenes, sources, item transforms, filters, and output settings. | Rust-owned project/profile/source/item model with deterministic JSON, validation, atomic save/load/recovery, dirty state, and bounded undo/redo. | Partial | All | `obs-rs-project` tests pass; no OBS import/export parity. | `crates/obs-rs-project`, `crates/obs-rs-config` | Scene collections, full settings schema |
| CORE-002 | Runtime source lifecycle | A source remains in the scene while a device fails, and recovers without disturbing unrelated sources. | Runtime owns source instances and isolates source failures; live-scene tests cover failing camera/screen sources and no reopen during a transform. | Complete | Portable/Linux slice | `crates/obs-rs-core/tests/live_scene.rs`; compositor counters. | `crates/obs-rs-core`, `crates/obs-rs-plugin-api` | Native capture adapters |
| VIDEO-001 | Frame scheduling | Video follows a monotonic clock, bounded queues, explicit drop policy, and target cadence. | Rational scheduler, cancellation-aware workers, capacity-bounded frame transport, drop/lateness metrics, and a sustained-run fixture exist. | Complete | Portable | `obs-rs-video` tests; release benchmark reports queue depth and deadline data. | `crates/obs-rs-video`, `crates/obs-rs-clock` | Performance acceptance on pinned hardware |
| VIDEO-002 | Preview request coalescing/cadence | Editing the UI does not create an unbounded render backlog, and hidden demand does not force output-rate rendering. | Preview worker uses a capacity-one latest-request/result slot. Visible GUI/projector consumers request up to 60 Hz, minimized idle windows request 5 Hz, and hidden idle windows submit no render request; active output stays at 60 Hz but requests only its output consumer. | Partial | Desktop | `preview_worker` queue tests plus pure demand tests for hidden, minimized, output-only, and visible Studio states; live multi-window cadence still needs measurement. | `crates/obs-rs-gui/src/preview_worker.rs`, `crates/obs-rs-gui/src/callbacks/{mod.rs,menu.rs}` | Live multi-monitor/projector cadence fixture |
| RENDER-001 | CPU scene compositor | Ordered source layers, transforms, opacity, filters, and alpha composition render predictably. | CPU reference compositor supports transforms, crop, flips, opacity, basic filters, cut, and cross-fade. | Partial | Portable | `obs-rs-media` and `obs-rs-core` correctness tests; no full OBS filter set. | `crates/obs-rs-media`, `crates/obs-rs-core` | Filter graph, source parity |
| RENDER-002 | GPU composition | Preview and output use GPU textures where supported and avoid unnecessary transfers. | WGPU composition accepts canvas-space source layers into distinct role-keyed targets, including separate bounded Preview and ProgramPreview targets plus full Program and Encoder consumers. CPU readback remains the Slint/encoder compatibility bridge and direct external-surface providers are still unavailable. | Partial | GPU-dependent | WGPU test composes an 8x8 source into a 4x4 target; no GUI zero-copy path. | `crates/obs-rs-render-wgpu`, `crates/obs-rs-render` | Native presenter, fan-out graph |
| RENDER-003 | GUI presentation | Preview is presented at viewport size without requiring a full output-resolution CPU image. | GUI preview and program views request bounded aspect-preserving targets (1920x1080 -> 1048x590). The full canvas program target is rendered only for encoder consumers, output scaling, or CPU fallback; the `PreviewPresenter` isolates the remaining Slint-owned copy. | Partial | Desktop | Preview-format, worker split, and full-output-fallback tests; WGPU viewport-target test; copy bytes are measured separately from render output. | `crates/obs-rs-gui/src/preview.rs`, `crates/obs-rs-gui/src/preview_worker.rs`, `crates/obs-rs-render-wgpu/src/gpu.rs` | Native/WGPU presenter |
| VIDEO-003 | Output scaling separate from canvas | Canvas and encoded output may differ, with explicit scaling and filter behavior. | Settings and output runtime distinguish canvas/output geometry and tests cover resampling and staged changes. | Partial | Portable/desktop | `obs-rs-gui` settings/output tests pass; native encoder quality is unverified. | `crates/obs-rs-gui/src/settings_model.rs`, `crates/obs-rs-gui/src/output.rs`, `crates/obs-rs-media/src/scale.rs` | Production encoder path |

## Canvas editing

> Selection update: the Sources dock now projects and selects bounded nested
> group paths such as `overlay-group/background`; click, context-menu opening,
> and keyboard navigation use the same Rust-owned path selection. Canvas
> geometry now projects both group leaves and visible leaves below `Scene`
> sources through the runtime's stable flattened paths; the Sources dock still
> exposes group descendants only.

> Row reconciliation: the nested scene-reference package below supersedes the
> older `CANVAS-003`/`CANVAS-004` dependency wording. Axis-aligned scene-source
> leaves now participate in canvas selection, drafts, local commits, and the
> standalone Transform dialog. The real GUI fixture now covers nested group and
> Scene-reference selection plus resize handles; leaf crop and leaf rotation
> now compose across a uniform, unmirrored parent, while parent-boundary crop/
> rotation, body-drag behavior in the testing backend, and live DPI evidence
> remain open.

> Transform reconciliation: the nested transform slice is exact for leaf crop
> across axis-aligned parents (including mirroring) and leaf rotation across
> uniform, unmirrored parents. A parent crop/rotation or a rotated leaf under
> non-uniform/mirrored ancestry remains an explicit unsupported capability.

> Profile reconciliation: `PROFILE-001` now includes a separate `Save As...`
> workflow and a shared capability-backed native save picker. The atomic
> project write completes before the active path and selection key change; an
> empty path or a different existing destination is rejected without
> overwriting the target or mutating the loaded document.

| ID | Feature | OBS behavior | OBS-RS observed behavior | Status | Platform | Tests / performance evidence | Files involved | Dependencies |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| CANVAS-001 | Viewport zoom | Fit to Window, fixed zoom presets, and wheel zoom change the view without changing project transforms. | Rust-owned `CanvasState` drives Fit to Window plus bounded 25%, 50%, 100%, and 200% presets. Wheel zoom now uses bounded continuous 10–800% levels and fixed-point cursor anchoring; the canvas image and selection overlay scale together and remain clipped to the viewport. | Partial | Desktop | `callbacks::canvas` preset, continuous-wheel, and anchor-invariant tests; GUI callback/snapshot coverage; testing-backend pointer fixture verifies a real wheel event changes and restores zoom/pan. | `crates/obs-rs-gui/ui/{canvas_editor.slint,stage.slint}`, `crates/obs-rs-gui/src/callbacks/canvas.rs`, `crates/obs-rs-gui/src/tests/{ui.rs,ui_canvas.rs}` | Live DPI/viewport evidence |
| CANVAS-002 | Pan | Middle-button or Space-drag pans a zoomed canvas. | Rust-owned transient pan coordinates drive middle-button and Space+drag movement in canvas pixels; pan is bounded and does not create a project/undo command. | Partial | Desktop | Canvas-state bound test; GUI callback coverage; real testing-backend pointer fixture verifies middle-button and Space+drag and restores the initial pan; live DPI/zoom-anchor evidence remains open. | `crates/obs-rs-gui/ui/canvas_editor.slint`, `crates/obs-rs-gui/ui/stage.slint`, `crates/obs-rs-gui/src/callbacks/canvas.rs`, `crates/obs-rs-gui/src/tests/ui_canvas.rs` | Live DPI, zoom anchor |
| CANVAS-003 | Selection | Click, Ctrl multi-select, drag-box, select-underneath, and locked-item rules work in the preview. | A plain click selects the topmost hit unless it is already selected, then walks down through the overlapping selected hit stack to reach the next source; Ctrl toggles the topmost hit; blank-space drag selects visible intersecting items; the bounded ordered selection is shared by the Sources dock and canvas union overlay. Locked selection blocks transform edits, including locked group ancestors. The Sources dock now accepts focus on click, handles bounded Up/Down/Home/End navigation for visible top-level rows, and removes the complete selected set on unmodified Delete through the shared atomic Rust command; the main canvas focus boundary accepts the same deletion action. Source-row Shift-click and Shift+keyboard navigation select the contiguous depth-first range from the active row, while Ctrl-click/Ctrl+keyboard navigation toggles through the same Rust selection boundary. The editable canvas fixture now proves plain and Ctrl-additive drag-box selection and overlapping hit-stack selection through the real pointer path. A nested Sources row is also clicked through the real pointer target and retains its stable path. Nested group and axis-aligned Scene-reference leaves now participate in canvas hit-testing, box selection, guides, overlays, and target-aware transform editing through their stable paths. The focused preview maps Ctrl+A to bounded top-level select-all through that same Rust owner. Transform-handle pointer coverage and live DPI evidence remain incomplete; arrow-key movement still acts on the current canvas selection. | Partial | Desktop | `callbacks::canvas` hit-stack, nested-projection, and geometry tests; `callbacks::scene` bounded navigation, select-all, and contiguous-range tests; `obs-rs-ui` multi-selection test; project batch-removal/undo/lock tests; GUI navigation/range/select-all/keyboard-delete and multi-delete/undo, source-row/nested-row mouse-selection, canvas drag-box, overlapping-hit pointer workflow, and nested Scene-reference Transform-dialog workflow. Nested canvas/live DPI fixture remains unavailable. | `crates/obs-rs-gui/src/{callbacks/canvas.rs,callbacks/canvas_controller.rs,callbacks/canvas_transform.rs,callbacks/source.rs,callbacks/source_transform.rs}`, `crates/obs-rs-ui/src/state.rs`, `crates/obs-rs-project/src/{commands.rs,commands/groups.rs}`, `crates/obs-rs-gui/ui/{source_dock.slint,canvas_editor.slint,main.slint}` | Transformed-boundary crop/rotation, transform/DPI pointer fixtures |
| CANVAS-004 | Move and resize | Move and eight handles support OBS modifier rules and preserve the correct opposite edge. | Body drag and eight handles work in canvas pixels; ordinary resize preserves the source/selection aspect ratio, Shift opts into free resize, Ctrl disables snapping, and Alt+handle drag crops source edges. The Slint boundary now carries Shift/Ctrl/Alt as one gesture mask instead of encoding crop as fake handle IDs. Single and multi-selection overlays expose a ninth rotation handle; Rust keeps the pointer anchor and immutable base transforms, rotates a group around the initial bounding-box center, applies Shift 15-degree absolute/delta snapping and Ctrl-disabled proximity snapping, and commits every selected item through one project command on release. Nested group and axis-aligned Scene-reference leaves now use effective canvas geometry during drafts and convert back to local path transforms on commit; `SetSceneItemTransforms` validates root and nested targets as one atomic batch. Rotation-aware bounds, source-local rotated Alt-crop deltas, Rust-owned oriented handles/path overlay, local-axis rotated handle resizing with opposite visual-edge anchoring, exhaustive pure eight-handle free/aspect/crop/minimum-size coverage, group move/resize drafts, arrow-key nudge, bounded snapping, one atomic project command, and target-aware standalone Transform-dialog editing are implemented. Transformed-group/Scene crop/rotation, pointer/DPI fixtures, and exact visual guide styling remain incomplete. | Partial | Desktop | `callbacks::canvas` exhaustive free/aspect/modifier/geometry and nested projection/inverse tests; rotated overlay quarter-turn, single/group rotation-handle tests, stable-base rotation and modifier tests, group-pivot transform tests, multi-selection, and local-axis resize tests; all eight crop-handle and minimum-size regression tests; project nested batch-transform atomicity test; GUI nudge and nested Scene-reference Transform-dialog coverage; ignored multi-selection timing report covers 16-item group rotation; no compositor-backed pointer fixture. The modifier policy follows [OBS 32.2.2 `OBSBasicPreview.cpp`](https://github.com/obsproject/obs-studio/blob/32.2.2/frontend/widgets/OBSBasicPreview.cpp). | `crates/obs-rs-gui/src/callbacks/{canvas.rs,canvas_controller.rs,canvas_geometry.rs,canvas_transform.rs,source.rs,source_transform.rs}`, `crates/obs-rs-gui/ui/{canvas_editor.slint,main.slint,stage.slint}`, `crates/obs-rs-project/src/{commands.rs,commands/groups.rs}` | Transformed-boundary crop/rotation, pointer/DPI fixture |
| CANVAS-005 | Transform dialog | Position, scale, rotation, crop, flip, alignment, and reset commands are available. | Transform dialog supports position, scale, integer-degree rotation backed by fixed-point media state, crop, opacity, flips, reset, and versioned persistence. The source Transform menu now implements Fit/Stretch to Screen, centering, and four edge-alignment commands through typed geometry. | Partial | Desktop | Media 32 pass/1 timing ignore; project 26 + sample fixture; canvas command tests; GUI suite 190 pass, 2 ignored. | `crates/obs-rs-media/src/transform.rs`, `crates/obs-rs-project/src/codec.rs`, `crates/obs-rs-gui/src/callbacks/canvas.rs`, `crates/obs-rs-gui/ui/context_menus.slint` | Full modifier matrix, visual reference fixtures |
| CANVAS-006 | Snapping and keyboard manipulation | Edges, centers, other sources, safe areas, configurable snap distance, and arrow-key movement work. | Bounded Rust-owned snapping aligns canvas edges/centers, visible source edges/centers, and EBU/ITU safe-area edges (Action 3.5%, Graphics 5%, 4:3 16.25% horizontal/5% vertical) and applies to the selected group. The persisted Settings > General > Canvas value controls a validated 1–100 canvas-pixel distance (default 10); Ctrl suppresses snapping for a move/resize gesture. The same page persists a Show safe areas toggle, and the editable preview draws the three matching guide rectangles. Arrow keys move unlocked selected items by 1px, or 10px with Shift, as one bounded project command. Compositor-backed pointer/DPI evidence and exact OBS guide styling remain unverified. | Partial | Desktop | Pure snap/command/modifier tests; safe-area margin regression test; settings round-trip/validation tests; GUI Apply/runtime propagation and settings-page rendering tests; `obs-rs-ui` multi-selection test; GUI keyboard nudge and transform-menu coverage. The margin constants follow [OBS 32.2.2 safe-area definitions](https://github.com/obsproject/obs-studio/blob/32.2.2/frontend/utility/display-helpers.hpp). | `crates/obs-rs-gui/src/{settings.rs,callbacks/canvas.rs,callbacks/settings.rs}`, `crates/obs-rs-gui/ui/{settings_window.slint,settings_pages.slint,canvas_editor.slint,stage.slint}`, `crates/obs-rs-ui/src/state.rs` | Pointer/DPI fixtures, exact guide styling |

## Docking and desktop windows

| ID | Feature | OBS behavior | OBS-RS observed behavior | Status | Platform | Tests / performance evidence | Files involved | Dependencies |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| DOCK-001 | Dock layout | Docks are arranged in a nested horizontal/vertical tree with tab groups. | A bounded, validated `DockNode` tree owns persistence; Rust emits normalized pane and splitter projections, and the Slint workspace renders horizontal/vertical splits, active tabs, and bounded split handles. Legacy order/weight models remain only for compatibility. | Partial | Desktop | `dock_tree` geometry/resize/round-trip tests; GUI snapshot, real vertical-splitter pointer, and splitter callback coverage. | `crates/obs-rs-gui/src/dock_tree.rs`, `crates/obs-rs-gui/ui/dock_workspace.slint`, `crates/obs-rs-gui/src/callbacks/docks.rs` | Platform minimum-size/monitor behavior |
| DOCK-002 | Tabs, insertion, re-docking | Docks can be dragged into split or tab insertion targets and re-docked without losing geometry. | Header drags use normalized pane hit-testing, show tab/left/right/top/bottom indicators, preserve flat-row reorder semantics, and commit typed tab/split mutations; floating re-docking remains functional. | Partial | Desktop | Tree drop-zone/atomic-mutation tests; GUI real DockHeader pointer drag/drop, drag-indicator, splitter, and re-dock coverage. | `crates/obs-rs-gui/src/dock_tree.rs`, `crates/obs-rs-gui/ui/dock_workspace.slint`, `crates/obs-rs-gui/src/callbacks/docks.rs` | Plugin/custom dock drop contracts |
| DOCK-003 | Floating windows | Floating docks retain position/size across restart, monitor changes, and DPI changes. | Detached windows share the studio models, reopen from persisted detached state, and persist bounded physical position/size plus scale-aware restoration. When the existing platform display capability reports a virtual desktop, restored positions are clamped so at least a 48px title-bar strip remains visible; when no monitor bounds are available, saved physical coordinates are preserved. Projector windows persist the platform monitor identity observed at their center, expose a right-click display menu backed by the current monitor capability, and restore within the selected monitor when it is still present, falling back to the current virtual desktop otherwise. Live multi-monitor/DPI evidence remains unavailable on this host. | Partial | Linux desktop slice | Settings geometry/monitor round-trip; GUI detach/re-dock capture; pure virtual-desktop clamp, projector-center monitor, and monitor-row projection tests; no compositor-backed multi-monitor test. | `crates/obs-rs-gui/src/{callbacks/docks.rs,callbacks/menu.rs,callbacks/monitor.rs,fixtures.rs}`, `crates/obs-rs-gui/ui/projector_window.slint`, `crates/obs-rs-gui/src/settings.rs` | Platform monitor capability/live multi-monitor fixture |
| DOCK-004 | Core dock coverage | Scenes, Sources, Mixer, Transitions, Controls, Stats, properties, and plugin docks are available. | Scenes, Sources, Mixer, Transitions, Controls, and the built-in Stats dock (ID 5) are available through the bounded tree, menu, persistence, tabs, and floating-window path. Stats projects existing diagnostics without adding hot-path work. Plugin/custom dock registration has bounded runtime metadata, but no dynamic Slint surface or persisted custom-dock IDs. | Partial | Desktop | GUI snapshot and callback tests; dock tree/persistence round-trip; six-header/floating layout coverage. | `crates/obs-rs-gui/ui`, `crates/obs-rs-gui/src`, `crates/obs-rs-plugin-api/src/lib.rs`, `crates/obs-rs-core/src/{runtime.rs,registry.rs}` | Dynamic plugin UI surface and extension permissions |

## Scenes, sources, and filters

> Nested-row update: source-row selection and depth-first keyboard navigation
> now include visible group descendants using the same bounded path targets as
> nested source actions. Ctrl+A uses that same visible-row projection and
> bounded selection limit; source and group rename now resolve the same target
> path, including root and nested group names.
> Group selected items is now one atomic same-owner project command: the dock
> keeps multi-selection on right-click, accepts root, nested-group, or
> same-owner Scene-reference siblings, rejects mixed-owner or locked
> selections, preserves parent order and item state, and selects the new
> path-addressed group after the command. Undo removes the grouping as one edit.
> Ungroup is the inverse atomic command for root, nested-group, and
> same-owner Scene-reference group paths: it validates lock and child-ID
> collisions, restores children at the former owner position, and selects the
> exposed root or nested child paths.
> This closes dock selection projection, but does not claim nested canvas
> geometry parity.

> Sources-dock pointer drag/drop now uses typed row payloads and the existing
> `MoveSceneItemToParent` command. Group and Scene-reference containers accept
> front insertion, leaf rows accept bounded before/after zones, and locked
> destinations reject the gesture without mutation. The GUI fixture covers
> real vertical drags into a group and nested leaf, order/selection recovery,
> and locked-container rejection. The source-row viewport disables mouse-drag
> panning so its Flickable cannot steal the gesture; wheel scrolling remains
> available. Canvas and Scene-reference GUI drag/drop remain open.

| ID | Feature | OBS behavior | OBS-RS observed behavior | Status | Platform | Tests / performance evidence | Files involved | Dependencies |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SCENE-001 | Scene lifecycle | Create, remove, rename, duplicate, reorder, switch, and persist scenes. | Create/remove/rename/duplicate/reorder and selection are wired through Rust project commands; newly created and duplicated scenes become the Preview scene through the existing UI state boundary; profile scene order is explicit state persisted in schema v7, v5 documents retain their serialized scene-array order, and Scene-dock context menus expose move up/down. The last valid Preview/Program choices are persisted in the desktop settings document and restored after a successful startup project load; switching profiles now restores each profile's session-scoped Preview/Program choices with first-scene fallback, including profile-switch undo/redo, without adding active-scene data to the project document. Switching collections and loading/recovering documents restores each document's choices through a bounded in-memory path-keyed cache, and exit-time settings persistence now serializes bounded typed document/profile-selection snapshots so collection choices survive a restart for every visited profile. Startup loads the project before applying the matching durable snapshot; stale, malformed, oversized, or missing choices fall back to the first scene without a project-history entry. Removing a scene still referenced by a nested scene item is rejected atomically; some collection workflows remain incomplete. | Partial | Desktop | `obs-rs-project` command/history, v5 compatibility, and round-trip tests; `obs-rs-ui` validated startup-selection, per-profile selection/history, keyed-document selection, and per-profile restart snapshot tests; GUI collection-creation preservation, settings escaping/round-trip, durable startup-restore, and scene-dock build coverage. | `crates/obs-rs-project/src/{model.rs,commands.rs,codec.rs}`, `crates/obs-rs-ui/src/{state.rs,types.rs,tests.rs}`, `crates/obs-rs-gui/src/{main.rs,settings.rs,callbacks/{menu.rs,project.rs,scene.rs,settings.rs},tests.rs}`, `crates/obs-rs-gui/ui/{context_menus.slint,scene_dock.slint,dock_workspace.slint}` | Collections, active-scene/profile switching, nested scene editing |
| SCENE-002 | Nested scenes and groups | Scene sources and nested groups can be composed, reordered, locked, and edited independently. | Project schema v7 persists typed `scene` targets, optional destination-scene transition policies, and embedded `group` targets with recursively ordered child items and explicit profile scene order. Commands validate unknown references, cycles, and bounded group depth; path-addressed group child visibility, locking, ordering, removal, reference/source-clone duplication, same-parent grouping and root/nested ungrouping, validated transform edits, and group-name edits are atomic; the Sources dock recursively projects group children with stable encoded paths and routes visibility/locking/order/duplicate/copy/paste/rename through those commands; engine/GUI preview flatten visible nested scenes and groups into one shared-source runtime without reopening captures. Project item paths now remain stable through flattening, runtime attachment, compositor layers, and preview transform drafts. Axis-aligned scale/translation/opacity plus horizontal/vertical mirroring now compose across nested scene/group boundaries using the profile canvas; nested-group and Scene-reference visibility/lock routing, recursive duplicate-source behavior, grouped Sources-dock actions, direct safe Transform-menu actions, target-aware source filter/properties editing, target-aware modal transform editing for nested group and Scene-reference leaves, nested Scene-reference remove/reorder/duplicate, and Sources-dock pointer drag/drop for group/leaf targets are covered. Canvas/Scene-reference GUI drag/drop and nested crop/rotation semantics at transformed boundaries remain unsupported. | Partial | All | Project group round-trip with mirrored nested groups, transition-policy round-trip, recursive duplication, depth-bound, child-command, child-removal, child-duplicate-mode, root/nested grouping and ungrouping, group-name, nested-transform, nested Scene-reference visibility/lock, and stable-path flattening tests; core stable-ID addressing and layer tests; media axis-aligned composition and renderer-oracle tests; engine group flatten/render test; GUI group render, nested filter/properties/transform-window/visibility/lock, and Sources-dock callback tests. | `crates/obs-rs-project/src/{model.rs,codec.rs,commands.rs,commands/groups.rs}`, `crates/obs-rs-media/src/transform.rs`, `crates/obs-rs-core/src/{registry.rs,runtime.rs,compositor.rs}`, `crates/obs-rs-engine/src/lib.rs`, `crates/obs-rs-gui/src/{preview.rs,refresh.rs,callbacks/{source.rs,source_filters.rs,source_properties.rs,source_transform.rs}}`, `crates/obs-rs-gui/ui/{source_dock.slint,context_menus.slint}` | Full nested crop/rotation semantics |
| SOURCE-001 | Basic source management | Existing/duplicate/copy-reference/copy-duplicate, visibility, lock, ordering, properties, filters, transform, interact, and projector commands are available. | Source create/duplicate/copy/paste/remove, visibility, lock, order, properties, filters, transform, and projectors have basic UI callbacks. Copy/paste now carries the row target through context-menu and floating-dock boundaries, so nested group children and groups paste into a validated group path while top-level paste keeps its existing selection behavior. Root-level Duplicate now selects the single newly-created item even when copy suffixes already exist; nested rows retain the current top-level-only canvas selection policy. The visible Sources rows now have click focus plus bounded Up/Down/Home/End navigation with Shift range selection and Ctrl-toggle selection routed through Rust, and unmodified Delete from the focused Sources dock or canvas removes the complete selected set through one atomic project command, including locked-target/ancestor rejection and one project-history boundary. The GUI fixture also clicks a nested source row and proves the stable group/child path. The rename modal applies source or group names to the stable row target that opened it, and source-row context menus expose the existing selected-source projector for source rows, including nested group and Scene-reference leaf paths while disabling it for groups and Scene references themselves. Standalone properties and filters now resolve Scene-reference leaf paths to the shared source definition; visibility and lock callbacks route nested Scene-reference leaves to their owner scene. Reference items can share one runtime source with independent scene transforms. Sources-dock rows now carry typed drag payloads and support group/Scene-reference container drops plus bounded leaf before/after drops through `MoveSceneItemToParent`; the GUI fixture covers real vertical drags, order and selection-path recovery, and locked-container rejection. Interact and the remaining context-menu actions are incomplete. | Partial | Desktop | GUI/source/project command tests; project and UI nested copy/paste and group-name tests; core and GUI duplicate-reference tests; bounded scene-navigation/range unit tests; keyboard-delete locked/unlocked and multi-delete/undo GUI fixture; nested-row pointer selection and `ui_source_drag_drop.rs`; nested Scene-reference properties/filter/visibility/lock fixture; projector lifecycle, dock callback/build, and reference snapshot workflow. | `crates/obs-rs-gui/src/{callbacks/scene.rs,callbacks/source.rs,callbacks/source_targets.rs,callbacks/source_batch.rs,callbacks/docks.rs,callbacks/menu_projectors.rs,tests/ui_sources.rs,tests/ui_source_drag_drop.rs,tests/ui_scene_reference.rs}`, `crates/obs-rs-gui/ui/{context_menus.slint,source_dock.slint,dock_slot.slint,dock_workspace.slint,floating_dock.slint,main.slint}`, `crates/obs-rs-ui/src/{state.rs,types.rs}`, `crates/obs-rs-project/src/{commands.rs,commands/types.rs,commands/groups.rs}`, `crates/obs-rs-core/src` | Full source model, nested canvas geometry |
| SOURCE-002 | Color and test-pattern sources | Solid color and test sources render deterministically and expose settings. | Color and test-pattern sources are implemented and exercised by the demo and built-in tests. | Complete | Portable | `obs-rs-builtins` tests; demo checksum. | `crates/obs-rs-builtins/src/portable.rs`, `crates/obs-rs-capture/src/simulated.rs` | None for reference behavior |
| SOURCE-003 | Display capture | X11, Wayland, and platform display capture expose selectable displays, permissions, reconnect, and correct geometry. | Linux has direct X11 protocol/cropping plus a Wayland portal/PipeWire boundary; current runtime probe skipped both because the managed session denied X11 and had no usable PipeWire graph. Windows/macOS are adapter boundaries only. | Partial | Linux slice; other platforms unavailable | X11/portal fixture tests; `obs-rs-linux-check` skips X11/Wayland. | `crates/obs-rs-capture/src/x11`, `crates/obs-rs-capture/src/dbus`, `crates/obs-rs-builtins/src/{x11,wayland}.rs` | Live platform sessions, native capture workers |
| SOURCE-004 | Window/game capture | Window lists, client-area behavior, game capture, reconnect, and platform-specific exclusions work. | Linux X11 window enumeration/cropping contract exists; game capture and Windows/macOS behavior are absent. | Partial | Linux X11 | X11 fixture tests; live window tests ignored without a local server. | `crates/obs-rs-capture/src/x11/window.rs`, `crates/obs-rs-builtins/src/x11.rs` | Platform agents |
| SOURCE-005 | Camera capture | Video capture device properties, native modes, hot plug, device loss, reconnect, and frame pacing work. | Shared Nokhwa capability/lifecycle seam and threaded camera source exist; the current Linux probe discovers `nokhwa-camera-0` with 16 native modes and reports a passing camera check. Hot-plug recovery and macOS/Windows behavior remain unverified. | Partial | Linux implementation; macOS/Windows seam | Camera mode/lifecycle tests; `obs-rs-linux-check` camera pass with 16 modes. | `crates/obs-rs-capture/src/nokhwa_camera.rs`, `crates/obs-rs-capture/src/threaded.rs` | Device matrix, live loss/recovery tests |
| SOURCE-006 | Audio input/output sources | Desktop audio and microphone sources expose device settings, monitoring, sync, and recovery. | PipeWire process adapter and deterministic fallback feed the engine; the GUI exposes microphone and desktop channels. The current Linux probe opens `pipewire-default` and completes a live 480-frame stereo capture check. Configured microphone and desktop IDs remain authoritative: if either disappears, the engine preserves bounded fallback/silence and retries that same ID once per second of media time, restoring the live backend when it returns. Automatic routes prefer a provider-declared default, retain transient active-device identity, and rediscover the default or first available route after loss; clock synchronization and the complete device graph remain incomplete. Live unplug/replug evidence is still unavailable. | Partial | Linux | Audio/PipeWire unit tests; selected-device authority, automatic-route identity/default-over-order, failure-continuity, bounded microphone/monitor retry, and restore tests; `obs-rs-linux-check` PipeWire pass; no live unplug/replug run. | `crates/obs-rs-audio-pipewire`, `crates/obs-rs-audio/src/{device.rs,tests.rs}`, `crates/obs-rs-engine`, `crates/obs-rs-gui/src/output.rs` | Full device graph, OS default-route reconciliation, clock sync, live hotplug run |
| SOURCE-007 | Image/media/browser/text/slideshow/VLC sources | Common OBS media and text source workflows are available with live update and recovery. | Portable `image_source` now decodes bounded PNG/JPEG/GIF/WebP/PNM files on create/update, resizes them into the configured frame, retains the last valid image sequence after a failed update, and exposes a localized path property. Its path row also exposes a capability-backed asynchronous native Browse action with image filters; a selected path returns through the local properties draft and is committed only by OK, while typed paths remain the fallback when no chooser is available. GIF image sources now retain a bounded timestamp-driven frame sequence with a minimum visible frame cadence and atomic replacement on update; static images remain one-frame sources. Portable `image_slideshow` now expands bounded newline-separated image files/directories, orders directory entries by name, optionally randomizes the expanded order, selects sequential slides from timestamped render requests, supports bounded slide time and loop/hold behavior, and can cross-fade into the next slide with a validated duration, including the loop boundary; its decoded set and timing policy replace atomically. Its `paths` row now exposes a capability-backed asynchronous native directory Browse action that returns one selected directory through the same local draft and commits only on OK; typed multiple paths and file selection remain available when the chooser is unavailable or insufficient. Portable `text_source` persists text, RGBA color, font size, and frame dimensions, updates atomically, and renders a bounded deterministic 5x7 bitmap. Manual/hotkey playback, swipe/slide transitions, media playlist, browser, VLC, font selection, rich layout, and full text rendering parity remain absent. | Partial | Portable image/slideshow/text slice | Built-in image decode/limit/atomic-update, animated GIF timestamp/loop, slideshow timestamp/directory/randomization/limit/failed-reload/cross-fade/validation, and text create/render/update/rejection tests; image/slideshow-picker command tests for Zenity/KDialog/AppleScript/PowerShell; GUI image and slideshow properties/path-commit plus Add Source fixture coverage. The slideshow render timing test covers the 640x360 hot path. | `crates/obs-rs-builtins/src/{image.rs,text.rs,factories.rs}`, `crates/obs-rs-gui/src/{properties.rs,fixtures.rs,callbacks/{add_source.rs,source_properties.rs,stinger_picker.rs}}`, `crates/obs-rs-gui/src/i18n/{english.rs,spanish.rs}`, `crates/obs-rs-gui/ui/{i18n.slint,source_properties_window.slint}` | Image/slideshow picker capability boundary, manual playback/hotkey boundary, media/browser runtimes, transition/media-clock boundary, font/resource boundary |
| FILTER-001 | Filter graph | Filters have ordered instances, categories, enable state, settings, validation, and independent properties. | Project model stores named ordered filter instances and categories; runtime compilation maps the small reference set, intentionally ignores disabled instances, and now publishes bounded diagnostics for unsupported categories/kinds and malformed settings instead of making unavailable filters indistinguishable from applied ones. A complete typed graph and source-level audio routing remain absent. | Partial | Portable/desktop | Project filter persistence/history tests; engine compilation-outcome, snapshot-diagnostic, and preview-diagnostic tests; no complete runtime graph. | `crates/obs-rs-project/src/model.rs`, `crates/obs-rs-engine/src/lib.rs`, `crates/obs-rs-gui/src/{preview.rs,preview_worker.rs,callbacks/project.rs}` | Typed filter graph |
| FILTER-002 | Video filters | Crop/pad, color correction/keying, sharpen, scale, scroll, mask, and delay behave like OBS. | Grayscale, brightness, opacity, the non-negative Crop/Pad edge-clearing slice, the six-control Color Correction slice, bounded RGB-distance Color Key, bounded four-control Luma Key, bounded current-pixel YCbCr Chroma Key core, bounded 3x3 Sharpen core, RGB Color Multiply/Add color wash, timestamp-driven Scroll, and a bounded timestamp-based Render Delay now run through the CPU oracle and WGPU compositor boundary. Scroll exposes bounded horizontal/vertical speeds (-500..500 pixels/second), loop/non-loop edge behavior, strict persistence validation, and a real filter-property toggle. Render Delay exposes OBS's 0..500 ms control range, warms up without showing current frames early, resets on timeline/source changes, and retains source-owned history behind frame-count and byte ceilings shared by every scene reference. The color-wash operation follows OBS's `color_multiply`/`color_add` matrix semantics in a portable RGBA8 value; it is exposed as a separate typed effect until the full color-property UI exists. HDR behavior, Chroma Key box filtering, color-space negotiation, optional Chroma Key color controls, Sharpen transformed or arbitrarily chained-neighbour semantics, Scroll width/height limiting and full ordered-chain semantics, color picking, masks, GPU-native delay textures, async 20-second video delay, scale-filter integration, and true pad-mode semantics remain absent. | Partial | Portable/WGPU slice | Media filter/history and timing tests; core delayed-source warm-up/recovery tests; engine/project compiler and legacy conversion tests; GUI filter-property/workflow coverage; WGPU CPU-oracle/readback tests; no OBS reference snapshots. The Scroll control range and offset/loop model are compared against [OBS's Scroll filter](https://github.com/obsproject/obs-studio/blob/master/plugins/obs-filters/scroll-filter.c); the Render Delay range and bounded texture-queue model are compared against [OBS's GPU delay filter](https://github.com/obsproject/obs-studio/blob/master/plugins/obs-filters/gpu-delay.c); the color control names and matrix order are documented in [OBS's color correction filter](https://github.com/obsproject/obs-studio/blob/master/plugins/obs-filters/color-correction-filter.c). | `crates/obs-rs-media/src/{delay.rs,filters.rs,frame.rs}`, `crates/obs-rs-core/src/{compositor.rs,registry.rs,runtime.rs}`, `crates/obs-rs-engine/src/lib.rs`, `crates/obs-rs-project/src/{commands.rs,tests.rs}`, `crates/obs-rs-render-wgpu/src/{gpu.rs,lib.rs}`, `crates/obs-rs-gui/src/{callbacks/source_filters.rs,filter_properties.rs,tests.rs}`, `crates/obs-rs-gui/ui/source_filters_window.slint` | GPU/reference implementations, full filter graph |
| FILTER-003 | Audio filters | Compressor, expander, gain, limiter, gate, suppression, polarity, and plugin filters process real audio. | Audio/video filter records remain persistent and the live engine now owns a bounded ordered audio chain; OBS-compatible Gain, Invert Polarity, a stateful Limiter, a stateful own-signal Compressor, a stateful peak Expander, and a stateful peak Noise Gate apply to desktop/microphone blocks before metering and mixing through explicit channel APIs. Limiter threshold/release, Compressor ratio/threshold/attack/release/output-gain, Expander ratio/threshold/attack/release/output-gain, and Noise Gate open/close threshold plus attack/hold/release use bounded fixed-point controls with continuous per-frame state. Noise Gate uses the channel's maximum absolute sample detector and hysteretic open/close thresholds; its safe Rust boundary requires nonzero attack/release times. Compressor/Expander/Gate sidechain selection, RMS detection, knee/upward-compressor modes, suppression, plugin processing, and automatic project-source routing remain absent. | Partial | Portable/live-channel slice | `obs-rs-audio` gain/polarity/limiter/compressor/expander/gate, bounds, continuity, overflow, capacity, and timing tests; engine compiler and live-channel meter/mix tests; worker command coverage. Reference: [OBS's 32.2.2 Noise Gate implementation](https://raw.githubusercontent.com/obsproject/obs-studio/32.2.2/plugins/obs-filters/noise-gate-filter.c), which uses a peak detector with open/close thresholds and attack/hold/release state. | `crates/obs-rs-audio/src/{filters.rs,error.rs}`, `crates/obs-rs-engine/src/{lib.rs,worker.rs}`, `crates/obs-rs-gui/src/{callbacks/source_filters.rs,filter_properties.rs}` | Stateful audio graph, detector/sidechain/source routing |

Reconciliation note: the source-filter editor now offers bounded localized
property schemas for all currently compiled audio kinds: Gain, Invert Polarity,
Limiter, Compressor, Expander, and Noise Gate. This is GUI configuration and
persistence coverage. The catalog and row kind labels also project bounded
English/Spanish names from the active locale without rewriting user-authored
instance names; automatic source-level routing remains intentionally open.

## Audio and production workflow

| ID | Feature | OBS behavior | OBS-RS observed behavior | Status | Platform | Tests / performance evidence | Files involved | Dependencies |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| AUDIO-001 | Mixer controls | Per-source gain, mute, pan/balance, meters, peak hold, and clipping state update live. | Reference mixer supports gain, mute, stereo pan, bounded monitor taps, peak hold, and clip indication; the engine, bounded worker, UI state, and mixer dock expose fixed-point gain (`0..2000`) and pan (`-1000..1000`), post-gain current/held peaks, and a bounded one-second clip indication. Configured and automatic microphone/desktop routes now recover through bounded rediscovery and same-device reopen attempts after a stream failure. The Mixer options button opens the Audio settings page through the same typed callback path for docked and floating docks. The remaining gap is the complete device graph and per-channel advanced audio workflow. | Partial | Portable/Linux slice | Audio mixer pan/gain-bound/mute/hold/clip tests; engine mixed-output pan, gain-bound, live-meter, selected/automatic-device identity, failure-continuity, and reattach tests; worker control/bounds test; UI pan/meter persistence tests; GUI workspace/settings-controller fixture verifies the Audio-page target; release mixer probe: 200 x 480-frame stereo blocks in 1.245 ms (6.225 us/block). Reference: [OBS 32.2.2 VolumeMeter](https://raw.githubusercontent.com/obsproject/obs-studio/32.2.2/frontend/components/VolumeMeter.cpp). | `crates/obs-rs-audio/src/{mixer.rs,lib.rs}`, `crates/obs-rs-engine/src/{lib.rs,worker.rs}`, `crates/obs-rs-ui/src/{commands.rs,state.rs,types.rs,snapshot.rs}`, `crates/obs-rs-gui/{src/{callbacks/docks.rs,callbacks/docks_forward.rs,callbacks/mod.rs,callbacks/settings_controller.rs,callbacks/output.rs,output.rs,refresh.rs,tests/ui_layout.rs},ui/{mixer_dock.slint,dock_slot.slint,dock_workspace.slint,floating_dock.slint,main.slint,models.slint}}` | Real device graph, automatic-route policy, advanced audio properties |
| AUDIO-002 | Device graph and hot plug | Multiple device inputs/outputs survive device loss, reconnect, and clock changes. | PipeWire enumeration, provider-declared default-route selection, fallback input, typed discovery errors, and bounded recovery for explicitly selected microphone/desktop IDs and automatic routes exist; the current Linux probe passes live `pipewire-default` capture. Automatic routes now refresh through a capacity-one engine route worker: provider discovery and compatible-device opening stay off the audio tick, a changed healthy default is reopened, and explicit IDs remain pinned. Full graph routing, clock-change handling, and live unplug/replug remain incomplete. | Partial | Linux | PipeWire discovery/default-route tests; engine selected/automatic-device identity, default-over-order, live-default-change, explicit-selection, failure-continuity, and reattach tests for input and monitor; `obs-rs-linux-check` live PipeWire pass; hotplug test remains unavailable. | `crates/obs-rs-audio-pipewire`, `crates/obs-rs-audio/src/{device.rs,tests.rs}`, `crates/obs-rs-engine/src/{audio_routes.rs,lib.rs}` | Full device graph, live graph/hotplug tests, clock synchronization |
| AUDIO-003 | Clock synchronization | Independent audio/video clocks resample and reconcile drift without unbounded latency. | Rational A/V timeline, deterministic drift model, callback clock correction, resampler, pacing, and long-run telemetry exist. Engine stats and GUI diagnostics now expose bounded observation, in-sync, behind, ahead, and maximum-delta counters; engine comparisons use the latest scheduled audio block for each video deadline, including externally supplied raw frames. Provider default-route reopening is now asynchronous, but current providers still stamp blocks at the requested media time, so no real-device clock correction or hardware drift measurement is claimed. | Partial | Portable | `obs-rs-clock` and `obs-rs-audio` long-run tests; engine tick/raw-frame telemetry test; GUI output-diagnostics test; 300-tick soak passes. | `crates/obs-rs-clock`, `crates/obs-rs-audio`, `crates/obs-rs-engine/src/{audio_routes.rs,lib.rs}`, `crates/obs-rs-gui/src/{output.rs,tests.rs}` | Real device timestamps, adaptive resampling policy |
| AUDIO-004 | Monitoring and recording tracks | Monitor off/only/both, channel layouts, sync offset, and multiple recording tracks are configurable. | The mixer has typed per-source `AudioMonitorMode` routing and a single-pass bounded `mix_buses`/`mix_buses_into` API: `Off` feeds output only, `MonitorOnly` feeds the monitor bus only, and `MonitorAndOutput` feeds both. Existing post-mix taps continue to observe the output bus. `AudioOutputWorker` opens a typed monitor sink on a dedicated thread and accepts only bounded non-blocking submissions with drop/failure telemetry. `EngineSession` and `EngineWorker` now apply per-channel monitor modes, select/clear the sink, route monitor blocks through the worker, and expose submission/drop/failure state in snapshots without blocking the engine. `AudioDelayLine` plus engine/worker controls implement a positive per-channel sync offset, quantized to sample frames and capped at 5,000 ms; changing the offset or losing a device clears only that channel's queued audio. `AppSettings` validates and persists both global offsets, the optional monitor-output device, and global microphone/desktop monitor modes. The Audio page exposes those controls in English and Spanish; startup and Apply route them through `OutputRuntime` and the engine worker boundary, retaining an unavailable selected output for explicit recovery. The selected sample rate/channel count now rebuilds the worker-owned timeline, mixer, device negotiation, monitor sink, and audio encoder while idle; changes made during recording/streaming/replay are bounded and staged until the next idle boundary. `AudioFormat` now carries typed standard layout metadata (mono, stereo, 2.1, quad, 5.1, and 7.1); the Audio page offers and persists those bounded choices while unknown positive counts remain a discrete fallback. Per-source channel-layout routing, source-level sync properties, device-policy/default-output selection, and multiple recording tracks remain absent. | Partial | Portable audio crate plus Linux engine/worker/settings slice | Audio delay order/bounds/format tests; mixer monitor-mode, asynchronous-worker, and format-reconfiguration tests; engine monitor-routing/sink-failure, delay/reconfiguration/bound, and idle-format-rebuild tests; worker monitor-control/sink-selection, format-replacement, and control-bound tests; audio format layout tests; GUI settings persistence/runtime-control/staged-format/layout round-trip tests; GUI workspace tests (190 passed, 2 ignored); audio timing probe: 200 x 480-frame stereo bus blocks in 70.558 ms (352.79 us/block) in the current debug run. | `crates/obs-rs-audio/src/{delay.rs,buffer.rs,error.rs,lib.rs,mixer.rs,output_worker.rs,tests.rs,types.rs}`, `crates/obs-rs-engine/src/{lib.rs,worker.rs}`, `crates/obs-rs-gui/{src/{settings.rs,callbacks/{mod.rs,settings.rs},output.rs,main.rs,i18n.rs,tests.rs},ui/{i18n.slint,settings_pages.slint,settings_window.slint}}` | Per-source channel-layout routing/mapping, source properties, default-device policy, production muxer, multiple tracks |
| STUDIO-001 | Studio Mode | Preview/program, Take, Cut, Fade, scene transitions, and output feed are separate but synchronized. | Preview/program selection, swap, Take, cut transitions, and the timed cross-fade path are owned by the toolkit-neutral state and single preview worker. A Take carries a validated 1–60,000 ms duration; progress is transient, bounded, and rendered into the GUI program view plus full output/projector feed without adding another runtime. Destination-scene overrides are validated project data, exposed by the dock, and resolve before the active transition starts; output lifecycle remains asynchronous. | Partial | Desktop | `obs-rs-ui` transient-duration/expiry and override-resolution tests; GUI worker test verifies the same transition sample reaches bounded program preview and full output frames; GUI dock callback fixture covers set/clear; engine transition tests. | `crates/obs-rs-ui`, `crates/obs-rs-project`, `crates/obs-rs-media`, `crates/obs-rs-gui/src/{callbacks/mod.rs,callbacks/docks.rs,callbacks/output.rs,preview.rs,preview_worker.rs,refresh.rs}`, `crates/obs-rs-gui/ui/{main.slint,transition_dock.slint,dock_slot.slint,dock_workspace.slint,floating_dock.slint}` | Full transition model, projectors, audio transition policy |
| STUDIO-002 | Transition catalog | Per-scene transitions, overrides, duration, stinger, swipe, slide, color, and luma transitions behave like OBS. | Cut and timed cross-fade/reference fade behavior now run through the single worker with a validated 1–60,000 ms duration. A bounded CPU/reference Fade to Color transition covers source → configured RGBA color → destination. Slide moves both layers without a second frame allocation; outgoing Swipe moves the source while the destination stays fixed, and Swipe In moves the destination over the stationary source without a second frame allocation. Both persist through typed `TransitionSpec` kinds and all four reference directions, and the console accepts legacy `transition/take slide|swipe <progress>` plus explicit direction/mode forms. The docked and floating Transition panels plus the scene-properties dialog expose localized Left/Right/Up/Down and Swipe In controls and pass them through typed callbacks and persisted scene overrides. Portable Luma Wipe now supports linear horizontal/vertical luminance masks, inversion, and bounded softness through the same media/project/UI/GUI path; it blends in place without a full-frame mask allocation. A bounded preloaded Stinger runtime validates decoded frames and feeds the same clip to viewport preview and full program/output rendering without render-time file I/O. A bounded `StingerSpec` persists a validated resource path, transition point, preload flag, and hardware-decode preference in schema 8 while schema 7 documents remain readable; Scene Properties edits those fields through one undoable command and the GUI preloads the selected scene's resource through the bounded worker. The generic loader boundary is capacity-one and cooperative; the optional native GStreamer adapter decodes local file/container resources into bounded RGBA clips with typed failures, and its one-slot result policy rejects stale request IDs without blocking submit/poll callers. The docked and floating Transition panels now expose `Take Stinger` for a ready preloaded clip, with visible failure states and no callback-side file/decoder I/O. Actual hardware-decode selection, file-picker support, track mattes, OBS asset-backed Luma Wipe patterns, external pattern resources, and the remaining transition catalog/assets/plugins are still incomplete. | Partial | Portable/desktop/native GStreamer | Media Slide/Swipe/Luma Wipe/Stinger correctness, project JSON round-trips, UI console/parser and runtime-label coverage, GUI fixtures covering direction, Swipe In, Luma Wipe, preview/output Stinger geometry, scene-properties edit/undo, explicit Take ready/not-ready behavior, native GStreamer loader fixture, and stale-result worker tests pass; the 640x360 Slide/Swipe/Stinger timing reports remain ignored benchmarks. Reference: [OBS 32.2.2 transition plugin sources](https://github.com/obsproject/obs-studio/tree/32.2.2/plugins/obs-transitions). | `crates/obs-rs-media/src/{transition.rs,frame_transitions.rs,stinger.rs,stinger_loader.rs,error.rs,media_tests_transitions.rs,media_tests_stinger.rs,media_tests_stinger_loader.rs}`, `crates/obs-rs-project/src/{codec.rs,model/scene.rs,commands.rs,commands/types.rs,codec.rs,project_tests_transition_stinger.rs,project_tests_round_trip.rs}`, `crates/obs-rs-ui/src/{types.rs,commands.rs,state.rs,ui_tests_shortcuts.rs}`, `crates/obs-rs-gui/src/{preview_render.rs,preview_worker.rs,preview_worker_tests.rs,callbacks/{output.rs,project.rs,docks.rs,docks_forward.rs},refresh.rs,refresh_transitions.rs,tests/{runtime.rs,ui_output.rs},callbacks/scene_tests.rs,i18n/{english.rs,spanish.rs}}`, `crates/obs-rs-output-gstreamer/src/{native_stinger.rs,native_stinger_tests.rs}`, `crates/obs-rs-gui/ui/{dialogs.slint,transition_luma.slint,transition_dock.slint,dock_slot.slint,dock_workspace.slint,floating_dock.slint,main.slint,main_modals.slint,i18n.slint}` | Capability-backed hardware-decode selection, file-picker workflow, track mattes, fade/audio policy, OBS asset-backed Luma Wipe patterns/resources, other transition assets/plugins |
| STUDIO-003 | Projectors | Preview/program/source/scene projectors can be fullscreen or windowed and reuse the engine feed. | Preview, program, multiview, selected-source, and selected-scene projector windows reuse worker-produced GUI feeds without opening another runtime. The selected-source projector captures the source item from the current Preview scene at open time, applies its persisted source-level filters, ignores scene-item geometry, and continues to follow that source if GUI selection changes. The scene dock context menu opens a stable scene projector target without changing the current preview selection. Program and multiview projectors default to fullscreen; preview, selected-source, and selected-scene projectors default to windowed. F11 toggles fullscreen on any projector, and versioned records persist fullscreen, bounded physical geometry, open state, source/scene target identities, and the observed platform monitor identity with DPI-aware restoration; windowed restores still use the selected monitor when available and otherwise the virtual-desktop visibility clamp. A projector right-click menu now enumerates current displays and moves the existing window through the typed monitor-selection callback; fixed and target-bearing projectors reopen after a clean restart only when their targets still resolve in the active project. Live multi-monitor evidence remains incomplete. | Partial | Desktop | Headless menu callback fixture verifies program, multiview, selected-source, and selected-scene projector toggles/fullscreen/reopen state; settings v1/v2/v3 geometry, bounded escaped target/monitor round-trip/rejection, pure projector-center monitor and monitor-row projection tests cover lifecycle state; render-demand tests verify hidden multiview/source/scene projectors request only their bounded feeds; renderer fixtures verify selected-source and complete-scene output; no live multi-monitor capture. | `crates/obs-rs-gui/src/{callbacks/menu.rs,callbacks/menu_projectors.rs,callbacks/docks.rs,callbacks/mod.rs,main.rs,preview.rs,preview_worker.rs,refresh.rs,settings.rs}`, `crates/obs-rs-gui/ui/{context_menus.slint,scene_dock.slint,projector_window.slint,main.slint,navbar.slint,i18n.slint}` | Platform monitor capability/live multi-monitor fixture |
| STUDIO-004 | Multiview | Multiview displays preview, program, scenes, and audio/status overlays. | View mode 2 requests a bounded ordered scene list from the single preview worker, renders at most 16 scene thumbnails into one 256px-class tile composite, overlays localized scene/Preview/Program labels with click-to-preview and double-click-to-program selection, and now projects one output-status strip with frame/drop/audio-block/queue counters plus a bounded mixed-audio dB meter through the existing 100 ms refresh cadence. The View menu can open the same bounded composite in a fullscreen multiview projector without another runtime. That projector has its own persisted fullscreen/geometry record and monitor identity, and the right-click display menu can move it through the same typed monitor-selection path as the other projector feeds. Per-scene audio meters, source-specific tile controls, live multi-monitor/DPI validation, and exact OBS tile styling remain absent. | Partial | Desktop | Render-demand test confirms multiview does not request the ordinary preview/program views and that a hidden multiview projector requests only the bounded grid; worker grid/format bounds, render-thread, composite, timing, output-telemetry, and GUI snapshot tests pass. Headless projector tests cover multiview fullscreen/toggle, monitor-row identity, and persisted geometry/monitor records; no compositor-backed multi-monitor capture. Debug timing report: 3 scenes x 20 renders = 100.945 ms (5.047 ms/render) on the baseline host. | `crates/obs-rs-gui/src/{callbacks/mod.rs,callbacks/menu.rs,callbacks/menu_projectors.rs,output.rs,preview.rs,preview_worker.rs,refresh.rs,tests.rs,tests/ui_navigation.rs}`, `crates/obs-rs-gui/ui/{main.slint,stage.slint,models.slint,navbar.slint,projector_window.slint,i18n.slint}` | Per-scene audio meters, source-specific tile controls, visual reference pass |

## Encoding, recording, and streaming

| ID | Feature | OBS behavior | OBS-RS observed behavior | Status | Platform | Tests / performance evidence | Files involved | Dependencies |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OUTPUT-001 | Output ownership and back-pressure | Capture/render/audio remain independent of bounded encoder/network work. | Engine worker owns output lifecycle and bounded packet queues; streaming and GUI recording start/finish now use nonblocking bounded requests, while lifecycle snapshots reconcile asynchronous setup/finalization failures back into desktop state. Synchronous worker methods remain available to control-plane and engine tests. | Partial | Portable/desktop | Engine worker lifecycle/queue tests, nonblocking recording start/finish barrier test, and GUI recording callback/container fixture; no production soak. | `crates/obs-rs-engine/src/worker.rs`, `crates/obs-rs-gui/src/{output.rs,callbacks/{output.rs,mod.rs}}`, `crates/obs-rs-output/src/queue.rs` | Native encoder isolation, production soak |
| OUTPUT-002 | Production encoders | Software and hardware H.264, HEVC, AV1, AAC, and Opus encoders are capability-negotiated. | GStreamer adapter can describe/negotiate several profiles and encoders; actual production availability and stable startup are not certified. | Partial | Linux adapter; other platforms absent | GStreamer capability tests; native session tests are environment-sensitive. | `crates/obs-rs-output-gstreamer`, `crates/obs-rs-output/src/codec.rs` | Platform encoder agents |
| OUTPUT-003 | Recording containers | MKV, MP4/remux, fragmented MP4, MOV, FLV, and multiple audio tracks are reliable and crash-safe. | Reference raw/Y4M/WAV/packet writers plus capability-negotiated GStreamer Matroska, H.264/AAC MP4, typed fragmented MP4, MOV, and FLV recording paths exist. Production containers finalize through hidden `.part` files and publish through a no-replace final-path link after EOS; native startup removes only the exact known `.part` artifact before opening a new container, while the normal lossless/reference path has bounded `.tmp`/`.part` cleanup. The native remux boundary consumes hidden H.264/AAC Matroska `.mkv.part` output on close and publishes MP4 through its own hidden `.mp4.part`; Settings > Output exposes opt-in automatic remux for unsplit Matroska and GUI/worker starts route it nonblocking. A bounded native/engine/worker scan now requires a matching durable JSON sidecar, finds deterministic non-empty unpublished candidates, and the GUI asynchronously presents them in a chooser before exact-path recovery. GUI startup also performs one bounded scan of the configured recording directory and reuses that chooser, without silently remuxing or overwriting a destination. Multiple audio tracks, broader codec/container coverage, and silent automatic resume remain absent. | Partial | Portable reference; Linux production adapter | Output profile/path-validation tests; GUI settings round-trip, runtime-routing, asynchronous recovery/no-candidate, candidate-discovery/startup-scan, and snapshot tests; engine Matroska, MP4, MOV, FLV, automatic-remux, worker recovery, and worker candidate-discovery tests; bounded native stale-artifact cleanup, manifest atomicity/matching, candidate-scan, and no-clobber/empty-artifact/publication tests; fragmented-MP4 native pipeline parse test; no complete container matrix. | `crates/obs-rs-output/src/{profile.rs,tests/profile.rs}`, `crates/obs-rs-output-gstreamer/{src/lib.rs,src/native.rs}`, `crates/obs-rs-engine/src/{lib.rs,worker.rs}`, `crates/obs-rs-gui/src/{main.rs,settings.rs,output.rs,refresh.rs,callbacks/{output.rs,settings.rs},tests.rs}`, `crates/obs-rs-gui/ui/{settings_pages.slint,settings_window.slint,controls_dock.slint,dock_slot.slint,dock_workspace.slint,floating_dock.slint,main.slint}` | Production codecs, multiple tracks, cross-platform remux |
| OUTPUT-004 | Replay buffer, split files, remux | Recording can be buffered, split, recovered, and automatically remuxed. | A bounded packetized replay buffer retains monotonic encoded audio/video history by byte and time limits, and saved reference snapshots begin at a retained video keyframe. The portable and native split paths use bounded policies, known-artifact cleanup, atomic numbered publication, and explicit unsupported-host errors; Settings > Output persists the split controls and routes production starts through the worker. Desktop replay start/stop/save requests are nonblocking and the Controls dock exposes the persisted bounded workflow. Automatic remux now consumes only unsplit H.264/AAC Matroska recordings, publishes MP4 through the native bounded remux boundary, persists a matching durable sidecar, and uses the chooser before recovering one exact candidate; startup performs one additional bounded directory scan and uses the same explicit recovery chooser. Replay remux and split/remux combinations remain absent. Save Replay plus local typed Start/Stop Replay hotkeys are implemented. | Partial | Portable/desktop; Linux native production boundary | Replay eviction/keyframe/timing tests; segmented writer and bounded stale-artifact tests; engine/worker reference and native split/remux/recovery/candidate-discovery tests; native manifest atomicity/matching tests; settings persistence and GUI automatic-remux/runtime/recovery/candidate-discovery/startup-scan fixtures; GUI Controls-dock asynchronous save/recovery and shortcut action/code fixtures. | `crates/obs-rs-output/src/{replay.rs,writers.rs,error.rs,tests/writers.rs}`, `crates/obs-rs-output-gstreamer/src/{lib.rs,native.rs}`, `crates/obs-rs-engine/src/{lib.rs,worker.rs}`, `crates/obs-rs-gui/src/{main.rs,settings.rs,output.rs,refresh.rs,callbacks/{output.rs,settings.rs,hotkeys.rs},tests.rs}`, `crates/obs-rs-gui/ui/{settings_pages.slint,settings_window.slint,controls_dock.slint,dock_slot.slint,dock_workspace.slint,floating_dock.slint,main.slint}` | Replay/remux variants, global hotkeys, output UI polish |
| OUTPUT-005 | Streaming protocols | RTMP/RTMPS, SRT, and intended WebRTC transports stream production encoded media. | Typed profiles and a GStreamer boundary exist; reference TCP/WebSocket transports use `OBSRPKT1`. The explicit native RTMP/RTMPS/SRT startup fixture now passes on this Linux host when run with its local-sink requirement, but no real remote endpoint, sustained media session, WebRTC session, or cross-platform proof exists. | Partial | Linux adapter; no cross-platform proof | Engine `production_schemes_create_native_stream_outputs` passes when explicitly run ignored; output-gstreamer protocol/metadata/native pipeline tests pass. | `crates/obs-rs-output-gstreamer`, `crates/obs-rs-output/src` | Real endpoint fixtures, sustained media soak, WebRTC runtime, cross-platform stream adapters |
| OUTPUT-006 | Services, authentication, reconnect, diagnostics | Service presets, keys/tokens, congestion, keyframes, reconnect, delay, and dropped-frame diagnostics are user-facing. | Settings now expose a bounded compile-time catalog of 82 RTMP-family choices (Custom plus 81 entries) derived from the pinned OBS 32.2.2 catalog. Stable service IDs resolve display names; selecting a service applies its first pinned-OBS ingest endpoint and protocol while keeping the server editable, and the server picker exposes the additional pinned regional/backup choices for Twitch, YouTube RTMPS, Loola.tv, and Restream.io. Stream keys remain secret-typed/redacted. The three pinned rows whose primary workflows are HLS or HTTP/API are intentionally omitted until matching typed targets exist. Reference and native sessions now share a typed lifecycle contract for bounded poll/reconnect/close control. Reconnect attempts use a capped exponential schedule and return a typed deferred outcome without sleeping, while native health polling may consume its own reconnect budget when it detects a live sink failure. Native telemetry now also reports submitted/dropped/reconnect counters, bounded video/audio appsrc queue bytes, and maximum submit latency through the engine snapshot and diagnostics strings. Signed catalog updates, the remaining services' regional/multiple-server choices, account auth, congestion policy, keyframe negotiation, and service-specific diagnostics remain absent. | Partial | Desktop; Linux native transport boundary | Output service-catalog/redaction/endpoint/lifecycle/backoff tests; catalog cardinality/ID/endpoint/additional-server tests; GStreamer native telemetry and engine production-stream bounds tests; GUI settings model/render and service-to-server callback coverage; no live service test. The catalog follows the [OBS 32.2.2 RTMP service catalog](https://raw.githubusercontent.com/obsproject/obs-studio/32.2.2/plugins/rtmp-services/data/services.json). | `crates/obs-rs-output/src/{config.rs,stream.rs,stream/session.rs,types.rs}`, `crates/obs-rs-engine/src/lib.rs`, `crates/obs-rs-output-gstreamer/src/native.rs`, `crates/obs-rs-gui/{src/{callbacks/settings.rs,tests.rs},ui/{settings_pages.slint,settings_window.slint}}` | Signed catalog updates, remaining regional/multiple-server choices, transport media/session split, auth lifecycle, congestion/keyframe diagnostics |

## Settings, profiles, and input

Reconciliation note: `HOTKEY-001` now also includes the optional local Cut
transition action and focused-window microphone push-to-talk/push-to-mute
actions. Their defaults are unbound, settings are persisted, and all compile
into the same bounded local action table; push actions use an exact mute
command on press/release. Global OS registration remains incomplete.

| ID | Feature | OBS behavior | OBS-RS observed behavior | Status | Platform | Tests / performance evidence | Files involved | Dependencies |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| SETTINGS-001 | Settings pages | General, Stream, Output, Audio, Video, Hotkeys, Accessibility, Advanced, and appearance settings have OBS-equivalent defaults and controls. | Nine pages render through a deterministic harness with bilingual catalogs and validated persistence. Controls are not yet equivalent page-by-page and some are capability placeholders. | Partial | Desktop | 18 screenshot fixtures; settings tests pass. | `crates/obs-rs-gui/ui/settings_pages.slint`, `crates/obs-rs-gui/src/settings.rs` | Reference comparison, capability model |
| SETTINGS-002 | Runtime effect and restart semantics | Every setting validates, persists, applies at the correct runtime boundary, or clearly requires restart. | Canvas/output staging, idle audio-format rebuild with active-output staging, audio input switching, monitor routing, project restore, and output lifecycle have tests; full setting-to-runtime coverage is incomplete. | Partial | Desktop | GUI output/settings tests, including staged audio-format application. | `crates/obs-rs-gui/src/callbacks/{mod.rs,settings.rs}`, `crates/obs-rs-gui/src/output.rs`, `crates/obs-rs-engine/src/{lib.rs,worker.rs}` | Complete settings matrix |
| PROFILE-001 | Profiles and scene collections | Profiles/collections can be created, duplicated, renamed, imported, exported, switched, and safely recovered. | Project profiles and file-backed collection discovery/switching exist. New collections start clean, Duplicate collection commits the current project through the atomic store, Rename collection saves then moves the current document into a validated managed sibling path without overwriting an existing target, Export collection writes the active serialized document to an explicit `.obsrproj` path without switching it, and Import collection validates an external `.obsrproj`, atomically copies it into the managed collection directory, and switches to the imported document after the shared discard guard. File-menu Load and Recover use that same guard before replacing the active document, and the native main-window close request now enters the same guard before exit. Save As writes the active document atomically before changing its path and selection key; its dialog reuses the bounded asynchronous native save picker on supported desktops and explains manual entry when unavailable. The project-store boundary accepts canonical `.obsrproj` and legacy `.json` paths and rejects other extensions before opening or writing. Recover project now opens a bilingual review modal before replacement, preserves the temporary file on cancel, keeps the modal open after parse failure, and leaves successful recovery dirty until an explicit save. Discard recovery now opens a separate confirmation modal and removes only the exact temporary file after confirmation. The dirty-project guard offers Save, Don't save, and Cancel; Save continues the pending action only after an atomic project save succeeds, while save failure leaves the guard active. Bounded per-document scene-selection snapshots for each visited profile persist in settings and restore after a successful startup project load. The broader save/discard/recovery workflow is still incomplete. | Partial | Desktop | Project/profile tests; menu collection-name/discovery tests; duplicate-copy, root-stability, rename-move/conflict, export-write/path-validation, import-copy/switch, invalid-import, and save-failure tests; Save As persistence/conflict and project-extension validation tests; native picker command tests for Zenity/KDialog plus GUI dialog-mode/render wiring; recovery review/discard modal render and success/failure-retry fixture; UI guard wiring, native close-response test, three-action dialog render fixture, settings escaping/round-trip, and per-profile durable selection restore tests. | `crates/obs-rs-project`, `crates/obs-rs-ui/src/{state.rs,types.rs}`, `crates/obs-rs-gui/src/{main.rs,settings.rs,callbacks/{menu.rs,project.rs,stinger_picker.rs},i18n.rs,tests.rs}`, `crates/obs-rs-gui/ui/{navbar.slint,dialogs.slint,main.slint,main_modals.slint,theme.slint}` | Collection format and recovery UX |
| HOTKEY-001 | Hotkeys | Structured key combinations support local/global hotkeys, conflicts, push-to-talk/mute, scene actions, outputs, transitions, and replay. | The toolkit-neutral `Shortcut` parses bounded `Ctrl`/`Shift`/`Alt` combinations, canonicalizes key aliases and modifier order, preserves empty unbindings, and rejects malformed/oversized input. Settings load and Apply compile one bounded action table owned by `DesktopState`, atomically replacing it after conflict validation. The GUI key event is now only an event bridge: Rust resolves the canonical label through that table, and Slint executes the existing confirmation/output/project callbacks for Swap, Previous/Next Preview Scene with circular profile-order navigation, recording/streaming, Undo/Redo, Save, Fade, Save Replay, guarded Start/Stop Replay, configurable local microphone/desktop mute actions, focused-window microphone push-to-talk/push-to-mute on press/release, and persisted Toggle Studio Mode. The main canvas, Scenes, and Sources focus boundaries release a held microphone push action on window deactivation, and a late key-release is ignored after that recovery. Scene navigation clears transient transitions and refreshes source selection through the same state owner. The mute, push, and Studio Mode actions route through typed callbacks without duplicating audio or view state; menu labels remain display-only projections of settings. The Toggle Studio Mode and push-action fields are opt-in and default to unbound. Global registration, push actions for other scopes/channels, scene-specific bindings, and the remaining OBS action set are incomplete. | Partial | Desktop | `obs-rs-ui` parser/table/atomic-replacement/frontend-action/mixer/scene-navigation tests; settings round-trip/fallback/canonicalization/conflict tests including toggle and push microphone bindings, Previous/Next, and Toggle Studio Mode fields; GUI action-code tests for codes 16/17/18/23/24; GUI Settings fixture/compile coverage, push mute state tests, Studio Mode callback fixture, and Controls fixture for replay lifecycle; GUI window-deactivation fixture proves PTT/PTM recovery and late-release idempotence; full workspace GUI tests and smoke; no global OS registration test. | `crates/obs-rs-ui/src/{lib.rs,types.rs,error.rs,state.rs,commands.rs,ui_tests_shortcuts.rs}`, `crates/obs-rs-gui/src/{main.rs,settings.rs,settings_tests.rs,callbacks/{docks.rs,hotkeys.rs,output.rs,settings_pages.rs,settings_commit.rs,settings_helpers.rs},tests/ui.rs}`, `crates/obs-rs-gui/ui/{main.slint,stage.slint,scene_dock.slint,source_dock.slint,dock_workspace.slint,dock_slot.slint,floating_dock.slint,hotkey_label.slint,navbar.slint,i18n.slint,settings_hotkeys.slint,settings_window.slint}` | Typed runtime registration and platform hooks |

## Platforms, plugins, and product hardening

| ID | Feature | OBS behavior | OBS-RS observed behavior | Status | Platform | Tests / performance evidence | Files involved | Dependencies |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PLATFORM-001 | Linux X11 | Display/window capture, V4L2, PipeWire audio, hardware encoders, virtual camera, and recovery work in a real X11 session. | Direct Rust X11 screen/window protocol and V4L2/Nokhwa/PipeWire seams exist; the current managed session still cannot connect to X11, while the Linux probe now passes camera discovery (16 modes) and live PipeWire capture. Hardware encoders, virtual camera, and hotplug recovery remain unverified. | Partial | Linux | X11/window checks still skip; `obs-rs-linux-check` camera and PipeWire checks pass. | `crates/obs-rs-capture/src/x11`, `crates/obs-rs-capture`, `crates/obs-rs-audio-pipewire` | Real Linux matrix, X11 session, hardware encoders, virtual camera, hotplug |
| PLATFORM-002 | Linux Wayland | Portal/ScreenCast, PipeWire node, permissions, restore token, and reconnect work across compositors. | Portal session-bus handshake and PipeWire process boundary exist; no live compositor acceptance evidence. | Partial | Linux | DBus protocol fixtures; live portal test ignored. | `crates/obs-rs-capture/src/dbus`, `crates/obs-rs-builtins/src/wayland.rs` | Real compositor matrix |
| PLATFORM-003 | Windows | Windows Graphics Capture, game capture, Media Foundation camera, WASAPI, hardware encoders, virtual camera. | Rust adapter crate and typed capability boundary exist, but no native implementation/runtime evidence was found on this Linux host. | Missing | Windows | Non-Windows typed-unavailable test only. | `crates/obs-rs-capture-windows` | Windows platform agent |
| PLATFORM-004 | macOS | ScreenCaptureKit, AVFoundation, CoreAudio, VideoToolbox, virtual camera. | Rust adapter crate and typed capability boundary exist, but no native implementation/runtime evidence was found on this Linux host. | Missing | macOS | Non-macOS typed-unavailable test only. | `crates/obs-rs-capture-macos` | macOS platform agent |
| PLUGIN-001 | Rust plugin API | Versioned source/filter/output/service/UI extensions can be installed and diagnosed safely. | Compile-time Rust plugin manifest/API versioning and a signed/bounded subprocess frame boundary exist. Dynamic source/filter/output/service/UI ecosystem is incomplete. | Partial | Portable | Plugin API, sandbox, manifest, and fuzz fixtures. | `crates/obs-rs-plugin-api`, `crates/obs-rs-sandbox` | Versioned manifests, permissions, quotas |
| PLUGIN-002 | Isolation and failure recovery | A plugin crash or invalid resource use does not take down the studio. | Subprocess protocol, resource limits, signatures, and bounded frame handoff exist; full crash supervision/update UX is incomplete. | Partial | Linux reference | Sandbox validation/failure tests. | `crates/obs-rs-sandbox` | Cross-platform supervisor |
| PRODUCT-001 | Accessibility and localization | Controls, focus, keyboard navigation, labels, and translated layouts remain usable. | Bilingual English/Spanish catalogs and accessible state snapshots exist; full focus audit, screen-reader behavior, and all translated dialogs are unverified. | Partial | Desktop | GUI bilingual snapshots and accessibility snapshot tests. | `crates/obs-rs-ui/src/snapshot.rs`, `crates/obs-rs-gui/src/i18n.rs` | Native accessibility audit |
| PRODUCT-002 | Visual 1:1 parity | Main window, docks, dialogs, menus, settings, focus, hover, disabled states, spacing, and icons match the reference. | A dark OBS-like Slint control room and settings fixtures exist; live side-by-side screenshots and pixel-diff thresholds are not established. | Partial | Desktop | Settings fixtures only; live screenshot capture blocked in baseline environment. | `crates/obs-rs-gui/ui`, `artifacts/baseline/screenshots` | Reference screenshot capture |
| PRODUCT-003 | Reliability and soak | Hours-long record/stream sessions survive device, GPU, network, and output failures with bounded memory. | A/V 300-tick soak and bounded worker tests pass; no multi-hour production output/device/GPU/network soak has been run. | Partial | Portable/Linux slice | `obs-rs-linux-check` A/V soak passes; release benchmark misses deadlines. | `crates/obs-rs-clock`, `crates/obs-rs-engine`, `crates/obs-rs-video` | Performance agent, fault-injection matrix |
| PRODUCT-004 | Packaging and updates | Signed installers, platform packaging, plugin updates, rollback, and diagnostics ship for all targets. | Release artifact script and signed update-manifest primitives exist; production packaging/signing/update channels are not complete. | Partial | All | Script/unit coverage only. | `scripts/release-artifacts.sh`, `crates/obs-rs-update` | Platform agents, release pipeline |

Reconciliation note: `SOURCE-001`/`SCENE-002` now include target-aware nested
Sources-dock `Rename` and `Remove` actions. Root removal retains the selected
set, while a nested row routes the clicked stable path and respects locked
ancestors; the GUI drag/drop fixture verifies selection preservation. Interact,
the remaining context-menu actions, and global OS registration remain partial.

Reconciliation note: scene-dock rows now expose an explicit button role and
stable scene-ID accessibility label, matching the existing source-row identity
contract. The testing backend discovers the Preview row by that label; native
screen-reader behavior and the remaining focus audit are still open.

Reconciliation note: SCENE-001 now also has real keyboard navigation and
testing-backend pointer drag/drop evidence for the persisted scene order in the
docked and floating-panel callback chain. `SceneRow.drag-data` remains a UI
projection; Rust owns validation and `ProjectCommand::MoveScene` mutation. The
broader collection lifecycle and native accessibility audit remain partial.

Reconciliation note: `DOCK-004`/`PLUGIN-001` now include bounded plugin dock
metadata registration and runtime diagnostics. This is an extension contract,
not yet a dynamic custom-dock UI; Slint surfaces still expose only the built-in
docks until the plugin-host packet is implemented.

Reconciliation note: `SOURCE-001`/`HOTKEY-001` now also include the optional
persisted selected-source visibility binding and GUI action code 19. It routes
through the existing source callback and leaves the default unbound; global
registration and the remaining source action catalog remain partial.

Reconciliation note: `SOURCE-001`/`HOTKEY-001` now also include the optional
persisted selected-source lock binding and GUI action code 20. It routes through
the existing source-lock callback and leaves the default unbound; global
registration and the remaining source action catalog remain partial.

Reconciliation note: `STUDIO-003`/`HOTKEY-001` now also include the optional
persisted selected-source projector binding and GUI action code 21. It reuses
the existing projector lifecycle and rejects groups/Scene references rather
than opening an invalid source feed; global registration remains partial.

Reconciliation note: `STUDIO-003`/`HOTKEY-001` now also include the optional
persisted Preview-scene projector binding and GUI action code 22. It reuses the
existing target-bearing scene-projector lifecycle and leaves the default
unbound; live multi-monitor evidence and global registration remain partial.

## Baseline conclusion

The first milestone is correctly identified as **fast OBS shell**: fix the
preview/presentation architecture, then implement real canvas state and a dock
tree before expanding source, audio, and output breadth. The first seven work
packets are therefore performance and evidence work, not additional UI polish.
