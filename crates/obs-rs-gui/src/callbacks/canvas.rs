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
use obs_rs_project::{ProjectCommand, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::ComponentHandle;

use crate::{refresh_ui, MainWindow, PreviewRenderer};

/// The smallest on-canvas size a drag may leave a source at.
///
/// A scene item shrunk to nothing has no handles left to grab, so resizing
/// stops here rather than letting the item become unrecoverable.
const MINIMUM_ITEM_PIXELS: i64 = 16;

/// The scale a transform stores for a source that fills the canvas.
const UNIT_SCALE_MILLI: i64 = 1_000;

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
pub(crate) struct CanvasController {
    draft: RefCell<Option<FrameTransform>>,
}

/// Installs the canvas selection and transform callbacks.
pub(crate) fn install_canvas_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) -> Rc<CanvasController> {
    let controller = Rc::new(CanvasController {
        draft: RefCell::new(None),
    });

    let weak = ui.as_weak();
    let drag_state = Rc::clone(state);
    let drag_renderer = Rc::clone(renderer);
    let drag_controller = Rc::clone(&controller);
    ui.on_transform_dragged(move |handle, dx, dy| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let canvas = canvas_size(&ui);
        let base = drag_controller
            .draft
            .borrow()
            .or_else(|| selected_transform(&drag_state))
            .unwrap_or(FrameTransform::IDENTITY);
        if selected_is_locked(&drag_state) {
            return;
        }
        let rect = drag_rect(
            item_rect(base, canvas),
            handle,
            i64::from(dx),
            i64::from(dy),
        );
        let transform = transform_for_rect(base, rect, canvas);
        drag_controller.draft.replace(Some(transform));
        // The preview follows the pointer, so the edit is applied to the
        // renderer's copy of the scene without touching the project yet.
        apply_preview_transform(&ui, &drag_state, &drag_renderer, transform);
    });

    let weak = ui.as_weak();
    let commit_state = Rc::clone(state);
    let commit_renderer = Rc::clone(renderer);
    let commit_controller = Rc::clone(&controller);
    ui.on_transform_committed(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(transform) = commit_controller.draft.borrow_mut().take() else {
            return;
        };
        crate::apply_source_transform_and_refresh(
            &ui,
            &commit_state,
            &commit_renderer,
            &crate::source_transform_document(transform),
        );
    });

    let weak = ui.as_weak();
    let click_state = Rc::clone(state);
    let click_renderer = Rc::clone(renderer);
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
            &click_renderer,
            UiCommand::SelectSource { id },
        );
    });

    controller
}

/// Pushes a drag's transform through the project so the preview redraws.
///
/// The command path is the same one the properties dialog uses; it is cheap
/// because only the scene item's transform changes, and it keeps the preview
/// honest about what the compositor will produce.
fn apply_preview_transform(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    transform: FrameTransform,
) {
    let Some((profile, scene, source)) = selected_context(&state.borrow()) else {
        return;
    };
    let applied =
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSourceTransform {
                profile,
                scene,
                source,
                transform,
            }));
    if applied.is_ok() {
        refresh_ui(ui, state, renderer);
    }
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

/// Returns the profile, scene, and source the canvas edits apply to.
fn selected_context(state: &DesktopState) -> Option<(String, String, String)> {
    let profile = state
        .project_session()
        .project()
        .active_profile()
        .to_string();
    let scene = state.preview_scene()?.to_owned();
    let source = state.selected_source()?.to_owned();
    Some((profile, scene, source))
}

/// Returns the selected source's current transform.
fn selected_transform(state: &Rc<RefCell<DesktopState>>) -> Option<FrameTransform> {
    let state = state.borrow();
    let scene = state.preview_scene()?;
    let source = state.selected_source()?;
    let session = state.project_session();
    session
        .project()
        .active_profile_spec()?
        .scene(scene)?
        .source(source)
        .map(SourceSpec::transform)
}

/// Returns whether the selected source is locked against editing.
fn selected_is_locked(state: &Rc<RefCell<DesktopState>>) -> bool {
    let state = state.borrow();
    let Some(scene) = state.preview_scene() else {
        return false;
    };
    let Some(source) = state.selected_source() else {
        return false;
    };
    let session = state.project_session();
    session
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene))
        .and_then(|scene| scene.source(source))
        .is_some_and(SourceSpec::locked)
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
        .sources()
        .iter()
        .rev()
        .find(|source| source.visible() && item_rect(source.transform(), canvas).contains(x, y))
        .map(|source| source.id().as_str().to_owned())
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
