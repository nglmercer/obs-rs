//! Interactive scene-item editing on the preview canvas.

use std::{cell::RefCell, rc::Rc};

use obs_rs_media::FrameTransform;
use obs_rs_project::SceneItemSpec;
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    preview::{TransformDraft, TransformDraftItem},
    settings::{CANVAS_SNAP_DISTANCE_DEFAULT, CANVAS_SNAP_DISTANCE_RANGE},
    MainWindow, PreviewSurface,
};

#[path = "canvas_controller.rs"]
mod canvas_controller;
#[path = "canvas_geometry.rs"]
mod canvas_geometry;
#[path = "canvas_model.rs"]
mod canvas_model;
#[path = "canvas_transform.rs"]
mod canvas_transform;
#[cfg(test)]
#[path = "canvas_tests.rs"]
mod tests;

#[cfg(test)]
use canvas_controller::{first_selectable_hit, map_rect_into_group};
pub(crate) use canvas_controller::{install_canvas_callbacks, CanvasController};
use canvas_geometry::{
    aspect_preserved_size, local_item_rect, oriented_handle_points, rounded_canvas_coordinate,
    selection_overlay_for_transforms, snap_delta, snap_rect, visible_source_extent, SnapGuides,
};
pub(crate) use canvas_geometry::{item_rect, ItemRect, SelectionOverlay};
use canvas_model::{
    CanvasResizeModifiers, SnapSettings, ACTION_SAFE_INSET, FOUR_BY_THREE_SAFE_X_INSET,
    GRAPHICS_SAFE_INSET, MAX_SNAP_GUIDES, MINIMUM_ITEM_PIXELS, SAFE_AREA_DENOMINATOR,
    UNIT_SCALE_MILLI,
};
pub(crate) use canvas_model::{CanvasState, CanvasTransformCommand, CanvasZoom};
#[cfg(test)]
use canvas_model::{
    MAX_PAN_PIXELS, MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT, RESIZE_MODIFIER_CONTROL,
    RESIZE_MODIFIER_SHIFT, SCALE_MICROS_PER_PERCENT, SCALE_MICROS_PER_UNIT,
};
use canvas_transform::{
    crop_transform, preserve_resize_aspect, rotate_canvas_delta, snap_rotated_resize_delta,
    transform_for_rect, transform_for_rotated_local_rect, transform_with_geometry,
};
pub(crate) use canvas_transform::{
    drag_rect, selection_overlay, set_selection_overlay, transform_for_command,
};
