//! Interactive scene-item editing on the preview canvas.
//!
//! OBS positions a source by dragging it on the canvas rather than by typing a
//! transform, so this turns pointer deltas into
//! [`obs_rs_media::FrameTransform`] edits. Drags mutate a draft that the
//! preview renders immediately; the project command is dispatched once, when
//! the pointer is released, so a drag is one undoable edit rather than a
//! hundred and the project revision does not churn per mouse move.

use std::{cell::RefCell, rc::Rc};

use obs_rs_media::FrameTransform;
use obs_rs_project::SceneItemSpec;
use obs_rs_ui::{DesktopState, UiCommand};
use slint::ComponentHandle;

use crate::{preview::TransformDraft, MainWindow, PreviewSurface};

/// The smallest on-canvas size a drag may leave a source at.
///
/// A scene item shrunk to nothing has no handles left to grab, so resizing
/// stops here rather than letting the item become unrecoverable.
const MINIMUM_ITEM_PIXELS: i64 = 16;

/// The scale a transform stores for a source that fills the canvas.
const UNIT_SCALE_MILLI: i64 = 1_000;

/// The zoom values exposed by the canvas control. `FitToWindow` is kept as a
/// distinct value because the fit scale depends on the live widget geometry;
/// it is not a project setting and must not be rounded into a fixed percent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CanvasZoom {
    FitToWindow,
    Percent(u16),
}

impl CanvasZoom {
    /// Returns the compact UI representation: zero means fit-to-window.
    pub(crate) const fn ui_value(self) -> i32 {
        match self {
            Self::FitToWindow => 0,
            Self::Percent(percent) => percent as i32,
        }
    }

    /// Parses the bounded values offered by the zoom control.
    pub(crate) const fn from_ui_value(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::FitToWindow),
            25 => Some(Self::Percent(25)),
            50 => Some(Self::Percent(50)),
            100 => Some(Self::Percent(100)),
            200 => Some(Self::Percent(200)),
            _ => None,
        }
    }

    /// Moves one position through the OBS-style bounded preset list.
    pub(crate) const fn stepped(self, direction: i32) -> Self {
        const PRESETS: [CanvasZoom; 5] = [
            CanvasZoom::FitToWindow,
            CanvasZoom::Percent(25),
            CanvasZoom::Percent(50),
            CanvasZoom::Percent(100),
            CanvasZoom::Percent(200),
        ];
        let current: usize = match self {
            Self::Percent(25) => 1,
            Self::Percent(50) => 2,
            Self::Percent(100) => 3,
            Self::Percent(200) => 4,
            // The constructor is private to this module, but keeping the
            // fallback makes the stepping contract total if another preset is
            // added later.
            Self::FitToWindow | Self::Percent(_) => 0,
        };
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else if direction > 0 {
            if current + 1 >= PRESETS.len() {
                PRESETS.len() - 1
            } else {
                current + 1
            }
        } else {
            current
        };
        PRESETS[next]
    }
}

/// Transient canvas viewport state owned by the canvas controller.
///
/// Zoom and pan are presentation state, not project data. Keeping them beside
/// the transform draft gives the UI one owner while leaving scene commands and
/// persisted documents free of widget-specific values. Pan is introduced here
/// so the next canvas packet can extend the same state without creating a
/// second viewport model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CanvasState {
    zoom: CanvasZoom,
    pan: (i32, i32),
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            zoom: CanvasZoom::FitToWindow,
            pan: (0, 0),
        }
    }
}

impl CanvasState {
    pub(crate) const fn zoom(self) -> CanvasZoom {
        self.zoom
    }

    #[allow(
        dead_code,
        reason = "pan is consumed by the next canvas interaction packet"
    )]
    pub(crate) const fn pan(self) -> (i32, i32) {
        self.pan
    }

    pub(crate) const fn with_zoom(self, zoom: CanvasZoom) -> Self {
        Self { zoom, ..self }
    }

    #[allow(
        dead_code,
        reason = "pan is written by the next canvas interaction packet"
    )]
    pub(crate) const fn with_pan(self, pan: (i32, i32)) -> Self {
        Self { pan, ..self }
    }
}

/// A scene item's rectangle in canvas pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ItemRect {
    pub(crate) x: i64,
    pub(crate) y: i64,
    pub(crate) width: i64,
    pub(crate) height: i64,
}

impl ItemRect {
    /// Returns whether `(x, y)` lies inside this rectangle.
    fn contains(self, x: i64, y: i64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// Returns where a transform places a source of `canvas` size on the canvas.
///
/// Sources render at canvas size, so the transform's scale is exactly the
/// item's size relative to the canvas and its translation is the top-left
/// corner.
pub(crate) fn item_rect(transform: FrameTransform, canvas: (u32, u32)) -> ItemRect {
    let width = i64::from(canvas.0) * i64::from(transform.scale_x_milli()) / UNIT_SCALE_MILLI;
    let height = i64::from(canvas.1) * i64::from(transform.scale_y_milli()) / UNIT_SCALE_MILLI;
    ItemRect {
        x: i64::from(transform.translate_x()),
        y: i64::from(transform.translate_y()),
        width,
        height,
    }
}

/// Rebuilds a transform from an edited rectangle, keeping flips and opacity.
fn transform_for_rect(base: FrameTransform, rect: ItemRect, canvas: (u32, u32)) -> FrameTransform {
    let scale = |extent: i64, canvas: u32| {
        let canvas = i64::from(canvas).max(1);
        let milli = extent.saturating_mul(UNIT_SCALE_MILLI) / canvas;
        u32::try_from(milli.clamp(1, i64::from(FrameTransform::MAX_SCALE_MILLI))).unwrap_or(1)
    };
    let translate = |value: i64| i32::try_from(value).unwrap_or(0);
    FrameTransform::new(
        scale(rect.width, canvas.0),
        scale(rect.height, canvas.1),
        translate(rect.x),
        translate(rect.y),
        base.flip_x(),
        base.flip_y(),
        base.opacity(),
    )
    .and_then(|transform| {
        transform.with_crop(
            base.crop_left(),
            base.crop_top(),
            base.crop_right(),
            base.crop_bottom(),
        )
    })
    .unwrap_or(base)
}

/// Applies one pointer drag to a rectangle.
///
/// `handle` is 0 for a move and 1-8 for the resize handles, numbered clockwise
/// from the top-left corner. Edge handles move one axis only, which is what
/// makes a side drag change width without nudging the item vertically.
pub(crate) fn drag_rect(rect: ItemRect, handle: i32, dx: i64, dy: i64) -> ItemRect {
    if handle == 0 {
        return ItemRect {
            x: rect.x + dx,
            y: rect.y + dy,
            ..rect
        };
    }
    let (left, top, right, bottom) = match handle {
        1 => (true, true, false, false),
        2 => (false, true, false, false),
        3 => (false, true, true, false),
        4 => (false, false, true, false),
        5 => (false, false, true, true),
        6 => (false, false, false, true),
        7 => (true, false, false, true),
        8 => (true, false, false, false),
        _ => return rect,
    };
    let mut edited = rect;
    if left {
        // Dragging a left or top edge moves the origin and shrinks the extent,
        // so the opposite edge stays where the user left it.
        let dx = dx.min(rect.width - MINIMUM_ITEM_PIXELS);
        edited.x = rect.x + dx;
        edited.width = rect.width - dx;
    }
    if right {
        edited.width = (rect.width + dx).max(MINIMUM_ITEM_PIXELS);
    }
    if top {
        let dy = dy.min(rect.height - MINIMUM_ITEM_PIXELS);
        edited.y = rect.y + dy;
        edited.height = rect.height - dy;
    }
    if bottom {
        edited.height = (rect.height + dy).max(MINIMUM_ITEM_PIXELS);
    }
    edited.width = edited.width.max(MINIMUM_ITEM_PIXELS);
    edited.height = edited.height.max(MINIMUM_ITEM_PIXELS);
    edited
}

/// Holds the transform a drag is editing before it reaches the project.
///
/// The preview timer reads [`CanvasController::draft`] on every tick and hands
/// it to the compositor directly, so the picture follows the pointer without a
/// single project revision being produced.
pub(crate) struct CanvasController {
    draft: RefCell<Option<TransformDraft>>,
    state: RefCell<CanvasState>,
}

impl CanvasController {
    /// Returns the drag in progress, if the pointer is down on an item.
    pub(crate) fn draft(&self) -> Option<TransformDraft> {
        self.draft.borrow().clone()
    }

    /// Returns the transient viewport state used by the canvas presentation.
    pub(crate) fn canvas_state(&self) -> CanvasState {
        *self.state.borrow()
    }
}

/// Connects the UI's transient zoom controls to the one canvas state owner.
fn install_zoom_callbacks(ui: &MainWindow, controller: &Rc<CanvasController>) {
    ui.set_canvas_zoom(controller.canvas_state().zoom().ui_value());

    let weak = ui.as_weak();
    let zoom_controller = Rc::clone(controller);
    ui.on_canvas_zoom_changed(move |value| {
        let Some(zoom) = CanvasZoom::from_ui_value(value) else {
            return;
        };
        zoom_controller
            .state
            .replace(zoom_controller.canvas_state().with_zoom(zoom));
        if let Some(ui) = weak.upgrade() {
            ui.set_canvas_zoom(zoom.ui_value());
        }
    });

    let weak = ui.as_weak();
    let zoom_controller = Rc::clone(controller);
    ui.on_canvas_zoom_step(move |direction| {
        let zoom = zoom_controller.canvas_state().zoom().stepped(direction);
        zoom_controller
            .state
            .replace(zoom_controller.canvas_state().with_zoom(zoom));
        if let Some(ui) = weak.upgrade() {
            ui.set_canvas_zoom(zoom.ui_value());
        }
    });
}

/// Installs the canvas selection and transform callbacks.
pub(crate) fn install_canvas_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) -> Rc<CanvasController> {
    let controller = Rc::new(CanvasController {
        draft: RefCell::new(None),
        state: RefCell::new(CanvasState::default()),
    });
    install_zoom_callbacks(ui, &controller);

    let weak = ui.as_weak();
    let drag_state = Rc::clone(state);
    let drag_controller = Rc::clone(&controller);
    ui.on_transform_dragged(move |handle, dx, dy| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if selected_is_locked(&drag_state) {
            return;
        }
        let Some((scene, item)) = dragged_item(&drag_state) else {
            return;
        };
        let canvas = canvas_size(&ui);
        let base = drag_controller
            .draft
            .borrow()
            .as_ref()
            .filter(|draft| draft.scene == scene && draft.item == item)
            .map(|draft| draft.transform)
            .or_else(|| selected_transform(&drag_state))
            .unwrap_or(FrameTransform::IDENTITY);
        let rect = drag_rect(
            item_rect(base, canvas),
            handle,
            i64::from(dx),
            i64::from(dy),
        );
        let transform = transform_for_rect(base, rect, canvas);
        // The drag stays out of the project until the pointer is released: the
        // preview timer feeds this straight to the compositor, and the overlay
        // handles follow it here. A revision per mouse move would fill the undo
        // history with a hundred entries for one gesture — and, before the
        // runtime learned to update in place, restart every capture device in
        // the scene along the way.
        drag_controller.draft.replace(Some(TransformDraft {
            scene,
            item,
            transform,
        }));
        push_item_rect(&ui, item_rect(transform, canvas));
    });

    let weak = ui.as_weak();
    let commit_state = Rc::clone(state);
    let commit_surface = Rc::clone(surface);
    let commit_controller = Rc::clone(&controller);
    ui.on_transform_committed(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(draft) = commit_controller.draft.borrow_mut().take() else {
            return;
        };
        // The gesture commits to the item it started on. A drag can outlive the
        // selection that began it — a dock click, a scene switch — and moving
        // whatever is selected at release time instead would move the wrong
        // source, having shown the user the right one moving the whole way.
        let Some(target) = crate::source_target(&commit_state.borrow(), &draft.item) else {
            ui.set_status_message("Source transform failed: the source is gone".into());
            return;
        };
        if target.scene != draft.scene {
            ui.set_status_message("Source transform failed: the scene changed".into());
            return;
        }
        crate::apply_source_transform_to(
            &ui,
            &commit_state,
            &commit_surface,
            &target,
            &crate::source_transform_document(draft.transform),
        );
    });

    let weak = ui.as_weak();
    let click_state = Rc::clone(state);
    let click_surface = Rc::clone(surface);
    ui.on_canvas_clicked(move |x, y| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let canvas = canvas_size(&ui);
        // Clicking selects the topmost item under the pointer, which is the
        // last one in the scene's draw order.
        let Some(id) = source_at(&click_state, canvas, i64::from(x), i64::from(y)) else {
            return;
        };
        if click_state.borrow().selected_source() == Some(id.as_str()) {
            return;
        }
        crate::dispatch_and_refresh(
            &ui.as_weak(),
            &click_state,
            &click_surface,
            UiCommand::SelectSource { id },
        );
    });

    controller
}

/// Moves the selection overlay with the pointer during a drag.
///
/// The dock refresh derives these from the project, which the drag has not
/// reached yet, so the handles are placed from the draft instead.
fn push_item_rect(ui: &MainWindow, rect: ItemRect) {
    ui.set_item_x(i32::try_from(rect.x).unwrap_or(0));
    ui.set_item_y(i32::try_from(rect.y).unwrap_or(0));
    ui.set_item_width(i32::try_from(rect.width).unwrap_or(0));
    ui.set_item_height(i32::try_from(rect.height).unwrap_or(0));
}

/// Returns the scene and scene item a drag applies to.
fn dragged_item(state: &Rc<RefCell<DesktopState>>) -> Option<(String, String)> {
    let state = state.borrow();
    Some((
        state.preview_scene()?.to_owned(),
        state.selected_source()?.to_owned(),
    ))
}

/// Returns the canvas size the studio is currently rendering at.
fn canvas_size(ui: &MainWindow) -> (u32, u32) {
    (
        u32::try_from(ui.get_canvas_width()).unwrap_or(1_920).max(1),
        u32::try_from(ui.get_canvas_height())
            .unwrap_or(1_080)
            .max(1),
    )
}

/// Returns the selected source's current transform.
fn selected_transform(state: &Rc<RefCell<DesktopState>>) -> Option<FrameTransform> {
    let state = state.borrow();
    let scene = state.preview_scene()?;
    let item = state.selected_source()?;
    let session = state.project_session();
    session
        .project()
        .active_profile_spec()?
        .scene(scene)?
        .item(item)
        .map(SceneItemSpec::transform)
}

/// Returns whether the selected source is locked against editing.
fn selected_is_locked(state: &Rc<RefCell<DesktopState>>) -> bool {
    let state = state.borrow();
    let Some(scene) = state.preview_scene() else {
        return false;
    };
    let Some(item) = state.selected_source() else {
        return false;
    };
    let session = state.project_session();
    session
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene))
        .and_then(|scene| scene.item(item))
        .is_some_and(SceneItemSpec::locked)
}

/// Returns the topmost visible source covering a canvas point.
fn source_at(
    state: &Rc<RefCell<DesktopState>>,
    canvas: (u32, u32),
    x: i64,
    y: i64,
) -> Option<String> {
    let state = state.borrow();
    let scene = state.preview_scene()?;
    let session = state.project_session();
    let scene = session.project().active_profile_spec()?.scene(scene)?;
    scene
        .items()
        .iter()
        .rev()
        .find(|item| item.visible() && item_rect(item.transform(), canvas).contains(x, y))
        .map(|item| item.id().as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: (u32, u32) = (1_920, 1_080);

    fn rect() -> ItemRect {
        ItemRect {
            x: 100,
            y: 50,
            width: 400,
            height: 300,
        }
    }

    #[test]
    fn canvas_zoom_accepts_only_the_bounded_reference_presets() {
        assert_eq!(CanvasZoom::from_ui_value(0), Some(CanvasZoom::FitToWindow));
        assert_eq!(CanvasZoom::from_ui_value(25), Some(CanvasZoom::Percent(25)));
        assert_eq!(CanvasZoom::from_ui_value(50), Some(CanvasZoom::Percent(50)));
        assert_eq!(
            CanvasZoom::from_ui_value(100),
            Some(CanvasZoom::Percent(100))
        );
        assert_eq!(
            CanvasZoom::from_ui_value(200),
            Some(CanvasZoom::Percent(200))
        );
        assert_eq!(CanvasZoom::from_ui_value(75), None);
    }

    #[test]
    fn canvas_zoom_steps_stay_inside_the_preset_bounds() {
        assert_eq!(CanvasZoom::FitToWindow.stepped(-1), CanvasZoom::FitToWindow);
        assert_eq!(CanvasZoom::FitToWindow.stepped(1), CanvasZoom::Percent(25));
        assert_eq!(CanvasZoom::Percent(25).stepped(-1), CanvasZoom::FitToWindow);
        assert_eq!(
            CanvasZoom::Percent(100).stepped(1),
            CanvasZoom::Percent(200)
        );
        assert_eq!(
            CanvasZoom::Percent(200).stepped(1),
            CanvasZoom::Percent(200)
        );
    }

    #[test]
    fn canvas_state_keeps_viewport_state_outside_the_project_transform() {
        let state = CanvasState::default()
            .with_zoom(CanvasZoom::Percent(100))
            .with_pan((24, -12));

        assert_eq!(state.zoom().ui_value(), 100);
        assert_eq!(state.pan(), (24, -12));
    }

    #[test]
    fn an_identity_transform_covers_the_whole_canvas() {
        let rect = item_rect(FrameTransform::IDENTITY, CANVAS);

        assert_eq!(
            rect,
            ItemRect {
                x: 0,
                y: 0,
                width: 1_920,
                height: 1_080
            }
        );
    }

    #[test]
    fn dragging_the_body_moves_without_resizing() {
        let moved = drag_rect(rect(), 0, 25, -10);

        assert_eq!(moved.x, 125);
        assert_eq!(moved.y, 40);
        assert_eq!((moved.width, moved.height), (400, 300));
    }

    #[test]
    fn opposite_edges_stay_put_while_a_handle_is_dragged() {
        // Dragging the left edge right must not move the right edge.
        let edited = drag_rect(rect(), 8, 40, 0);
        assert_eq!(edited.x, 140);
        assert_eq!(edited.x + edited.width, 500);

        // Dragging the top edge down must not move the bottom edge.
        let edited = drag_rect(rect(), 2, 0, 30);
        assert_eq!(edited.y, 80);
        assert_eq!(edited.y + edited.height, 350);

        // The bottom-right corner moves only that corner.
        let edited = drag_rect(rect(), 5, 20, 15);
        assert_eq!((edited.x, edited.y), (100, 50));
        assert_eq!((edited.width, edited.height), (420, 315));
    }

    #[test]
    fn a_source_cannot_be_shrunk_past_its_handles() {
        let collapsed = drag_rect(rect(), 5, -1_000, -1_000);
        assert_eq!(collapsed.width, MINIMUM_ITEM_PIXELS);
        assert_eq!(collapsed.height, MINIMUM_ITEM_PIXELS);

        // Pulling a left edge past the limit must not drag the origin beyond
        // the opposite edge either.
        let collapsed = drag_rect(rect(), 8, 1_000, 0);
        assert_eq!(collapsed.width, MINIMUM_ITEM_PIXELS);
        assert_eq!(collapsed.x + collapsed.width, 500);
    }

    #[test]
    fn an_edited_rectangle_round_trips_through_the_transform() {
        let edited = ItemRect {
            x: 240,
            y: 135,
            width: 960,
            height: 540,
        };

        let transform = transform_for_rect(FrameTransform::IDENTITY, edited, CANVAS);

        assert_eq!(transform.scale_x_milli(), 500);
        assert_eq!(transform.scale_y_milli(), 500);
        assert_eq!(item_rect(transform, CANVAS), edited);
    }

    #[test]
    fn rebuilding_a_transform_keeps_flips_and_opacity() {
        let base = FrameTransform::new(1_000, 1_000, 0, 0, true, true, 128).expect("transform");

        let rebuilt = transform_for_rect(base, rect(), CANVAS);

        assert!(rebuilt.flip_x() && rebuilt.flip_y());
        assert_eq!(rebuilt.opacity(), 128);
    }

    #[test]
    fn hit_testing_uses_the_item_rectangle() {
        let rect = rect();

        assert!(rect.contains(100, 50));
        assert!(rect.contains(499, 349));
        assert!(!rect.contains(99, 50));
        assert!(!rect.contains(500, 350));
    }
}
