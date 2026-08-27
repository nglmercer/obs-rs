//! Interactive scene-item editing on the preview canvas.

use std::{cell::Cell, cell::RefCell, rc::Rc};

use obs_rs_media::FrameTransform;
use obs_rs_project::{Profile, SceneItemSpec, SceneSpec};
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
#[path = "canvas_projection_tests.rs"]
mod projection_tests;
#[cfg(test)]
#[path = "canvas_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "canvas_transform_tests.rs"]
mod transform_tests;

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
    GRAPHICS_SAFE_INSET, MAX_SNAP_GUIDES, MINIMUM_ITEM_PIXELS, RESIZE_MODIFIER_CONTROL,
    RESIZE_MODIFIER_SHIFT, SAFE_AREA_DENOMINATOR, UNIT_SCALE_MILLI,
};
pub(crate) use canvas_model::{CanvasState, CanvasTransformCommand, CanvasZoom};
#[cfg(test)]
use canvas_model::{
    MAX_PAN_PIXELS, MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT, RESIZE_MODIFIER_ALT,
    SCALE_MICROS_PER_PERCENT, SCALE_MICROS_PER_UNIT,
};
use canvas_transform::{
    crop_transform, group_rotation_from_pointer, preserve_resize_aspect, rotate_canvas_delta,
    rotate_transform_around_point, rotation_from_pointer, snap_rotated_resize_delta,
    transform_for_rect, transform_for_rotated_local_rect, transform_with_geometry,
};
pub(crate) use canvas_transform::{
    drag_rect, selection_overlay, set_selection_overlay, transform_for_command,
};

/// One visible leaf that the editable canvas can address through the same
/// stable path as the Sources dock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CanvasItemProjection {
    pub(crate) target: String,
    /// The transform after the item's enclosing groups have been applied.
    /// Canvas hit-testing and handles operate in this profile-canvas space.
    pub(crate) transform: FrameTransform,
    /// The transform contributed by enclosing groups. It is retained so a
    /// canvas edit can be converted back to the item's local project state.
    pub(crate) parent_transform: FrameTransform,
}

/// Returns visible root items plus nested source/group/Scene-reference leaves
/// that have a path-addressable project item. Scene-reference leaves retain
/// their flattened runtime path so editors can route changes to the owning
/// scene without changing the parent scene item.
pub(crate) fn canvas_item_projections(
    state: &DesktopState,
    scene_id: &str,
    canvas: (u32, u32),
) -> Vec<CanvasItemProjection> {
    let Some(profile) = state.project_session().project().active_profile_spec() else {
        return Vec::new();
    };
    let Some(scene) = profile.scene(scene_id) else {
        return Vec::new();
    };
    let Ok(flattened) = profile.flatten_scene_items(scene_id) else {
        return Vec::new();
    };
    let mut projections = Vec::new();
    // Keep the group leaves beside their root item rather than appending all
    // nested leaves after every root. This mirrors flattening/draw order, so
    // hit-testing and snapping cannot make a child of an earlier group appear
    // above a later root source.
    for root in scene.items().iter().filter(|item| item.visible()) {
        projections.push(CanvasItemProjection {
            target: root.id().as_str().to_owned(),
            transform: root.transform(),
            parent_transform: FrameTransform::IDENTITY,
        });
        let prefix = format!("{}/", root.id());
        projections.extend(flattened.iter().filter_map(|item| {
            let target = item.item_id();
            if !target.starts_with(prefix.as_str()) {
                return None;
            }
            canvas_item_for_target(profile, scene.id().as_str(), target)?;
            let parent_transform =
                canvas_parent_transform(profile, scene.id().as_str(), target, canvas)?;
            Some(CanvasItemProjection {
                target: target.to_owned(),
                transform: item.transform(),
                parent_transform,
            })
        }));
    }
    projections.truncate(obs_rs_ui::MAX_CANVAS_SELECTIONS.saturating_mul(4));
    projections
}

/// Resolves a flattened canvas target through groups and scene references.
/// The Sources dock currently exposes group paths only; the canvas may also
/// select a visible leaf below a scene source using the runtime's stable path.
pub(crate) fn canvas_item_for_target<'a>(
    profile: &'a Profile,
    scene_id: &str,
    target: &str,
) -> Option<&'a SceneItemSpec> {
    let parts = target.split('/').collect::<Vec<_>>();
    let scene = profile.scene(scene_id)?;
    canvas_item_for_parts(profile, scene.items(), &parts)
}

fn canvas_item_for_parts<'a>(
    profile: &'a Profile,
    items: &'a [SceneItemSpec],
    parts: &[&str],
) -> Option<&'a SceneItemSpec> {
    let (part, rest) = parts.split_first()?;
    let item = items.iter().find(|item| item.id().as_str() == *part)?;
    if rest.is_empty() {
        return Some(item);
    }
    if let Some(group) = item.group() {
        return canvas_item_for_parts(profile, group.items(), rest);
    }
    let child_scene = item.scene_id()?;
    let scene = profile.scene(child_scene)?;
    canvas_item_for_parts(profile, scene.items(), rest)
}

/// Returns the effective parent transform crossed by a flattened canvas leaf.
/// Both group and scene-reference boundaries use the same axis-aligned
/// composition rule as project flattening.
pub(crate) fn canvas_parent_transform(
    profile: &Profile,
    scene_id: &str,
    target: &str,
    canvas: (u32, u32),
) -> Option<FrameTransform> {
    let mut items = profile.scene(scene_id)?.items();
    let mut parts = target.split('/').collect::<Vec<_>>();
    parts.pop()?;
    let mut parent = FrameTransform::IDENTITY;
    for part in parts {
        let item = items.iter().find(|item| item.id().as_str() == part)?;
        parent = item
            .transform()
            .compose_axis_aligned(parent, canvas.0, canvas.1)
            .ok()?;
        if let Some(group) = item.group() {
            items = group.items();
        } else {
            let child_scene = item.scene_id()?;
            items = profile.scene(child_scene)?.items();
        }
    }
    Some(parent)
}

/// Returns the effective transform contributed by a leaf item's enclosing
/// groups. The order matches project flattening: the innermost group is
/// composed with the already accumulated outer transform.
#[cfg(test)]
pub(crate) fn group_parent_transform(
    scene: &SceneSpec,
    target: &str,
    canvas: (u32, u32),
) -> Option<FrameTransform> {
    let mut parts = target.split('/').collect::<Vec<_>>();
    parts.pop()?;
    if parts.is_empty() {
        return Some(FrameTransform::IDENTITY);
    }
    let mut items = scene.items();
    let mut parent = FrameTransform::IDENTITY;
    for group_id in parts {
        let group_item = items.iter().find(|item| item.id().as_str() == group_id)?;
        let group = group_item.group()?;
        parent = group_item
            .transform()
            .compose_axis_aligned(parent, canvas.0, canvas.1)
            .ok()?;
        items = group.items();
    }
    Some(parent)
}

/// Converts an effective canvas transform back to the local transform of a
/// path-addressed group or Scene-reference child. Root items need no
/// conversion. A transformed boundary accepts a leaf crop exactly and
/// accepts leaf rotation only under a uniform, unmirrored parent scale.
/// Parent crop/rotation and a rotated leaf under non-uniform or mirrored
/// ancestry return `None` instead of being silently approximated.
pub(crate) fn local_transform_for_canvas_item(
    profile: &Profile,
    scene: &SceneSpec,
    target: &str,
    effective: FrameTransform,
) -> Option<FrameTransform> {
    let parent = canvas_parent_transform(
        profile,
        scene.id().as_str(),
        target,
        (
            profile.video_format().width(),
            profile.video_format().height(),
        ),
    )?;
    if parent == FrameTransform::IDENTITY {
        return Some(effective);
    }
    if parent.is_cropped() || parent.is_rotated() {
        return None;
    }
    if effective.is_rotated()
        && (parent.scale_x_milli() != parent.scale_y_milli() || parent.flip_x() || parent.flip_y())
    {
        return None;
    }
    let local = canvas_item_for_target(profile, scene.id().as_str(), target)?;
    let width = i64::from(profile.video_format().width());
    let height = i64::from(profile.video_format().height());
    let scale_x = i64::from(effective.scale_x_milli())
        .saturating_mul(1_000)
        .checked_div(i64::from(parent.scale_x_milli()))?;
    let scale_y = i64::from(effective.scale_y_milli())
        .saturating_mul(1_000)
        .checked_div(i64::from(parent.scale_y_milli()))?;
    let scale_x =
        u32::try_from(scale_x.clamp(1, i64::from(FrameTransform::MAX_SCALE_MILLI))).ok()?;
    let scale_y =
        u32::try_from(scale_y.clamp(1, i64::from(FrameTransform::MAX_SCALE_MILLI))).ok()?;
    let visible_width =
        width.checked_sub(i64::from(effective.crop_left()) + i64::from(effective.crop_right()))?;
    let visible_height =
        height.checked_sub(i64::from(effective.crop_top()) + i64::from(effective.crop_bottom()))?;
    if visible_width <= 0 || visible_height <= 0 {
        return None;
    }
    let translate_x = inverse_nested_translation(
        effective.translate_x(),
        parent.translate_x(),
        parent.scale_x_milli(),
        scale_x,
        width,
        visible_width,
        parent.flip_x(),
    )?;
    let translate_y = inverse_nested_translation(
        effective.translate_y(),
        parent.translate_y(),
        parent.scale_y_milli(),
        scale_y,
        height,
        visible_height,
        parent.flip_y(),
    )?;
    let candidate = FrameTransform::new(
        scale_x,
        scale_y,
        translate_x,
        translate_y,
        effective.flip_x() != parent.flip_x(),
        effective.flip_y() != parent.flip_y(),
        local.transform().opacity(),
    )
    .ok()?
    .with_rotation_milli_degrees(effective.rotation_milli_degrees())
    .ok()?
    .with_crop(
        effective.crop_left(),
        effective.crop_top(),
        effective.crop_right(),
        effective.crop_bottom(),
    )
    .ok()?;
    Some(candidate)
}

fn inverse_nested_translation(
    effective_translation: i32,
    parent_translation: i32,
    parent_scale: u32,
    local_scale: u32,
    canvas_dimension: i64,
    visible_dimension: i64,
    parent_flipped: bool,
) -> Option<i32> {
    let origin = i64::from(effective_translation)
        .saturating_sub(i64::from(parent_translation))
        .saturating_mul(1_000)
        .checked_div(i64::from(parent_scale))?;
    let extent = visible_dimension
        .saturating_mul(i64::from(local_scale))
        .checked_div(1_000)?;
    let local_translation = if parent_flipped {
        canvas_dimension
            .saturating_sub(extent)
            .saturating_sub(origin)
    } else {
        origin
    };
    i32::try_from(local_translation).ok()
}

/// Returns whether a canvas target or any of its enclosing groups is locked.
#[cfg(test)]
pub(crate) fn canvas_target_is_locked(scene: &SceneSpec, target: &str) -> bool {
    let mut items = scene.items();
    let parts = target.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        let Some(item) = items.iter().find(|item| item.id().as_str() == *part) else {
            return true;
        };
        if item.locked() {
            return true;
        }
        if index + 1 < parts.len() {
            let Some(group) = item.group() else {
                return true;
            };
            items = group.items();
        }
    }
    false
}

/// Returns whether a flattened canvas target or any group/scene-reference
/// ancestor is locked.
pub(crate) fn canvas_target_is_locked_in_profile(
    profile: &Profile,
    scene_id: &str,
    target: &str,
) -> bool {
    let mut items = match profile.scene(scene_id) {
        Some(scene) => scene.items(),
        None => return true,
    };
    let parts = target.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        let Some(item) = items.iter().find(|item| item.id().as_str() == *part) else {
            return true;
        };
        if item.locked() {
            return true;
        }
        if index + 1 == parts.len() {
            return false;
        }
        if let Some(group) = item.group() {
            items = group.items();
        } else if let Some(child_scene) = item.scene_id() {
            let Some(scene) = profile.scene(child_scene) else {
                return true;
            };
            items = scene.items();
        } else {
            return true;
        }
    }
    true
}
