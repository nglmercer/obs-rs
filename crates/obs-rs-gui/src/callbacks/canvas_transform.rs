use super::{
    aspect_preserved_size, item_rect, local_item_rect, oriented_handle_points,
    rounded_canvas_coordinate, selection_overlay_for_transforms, snap_delta, visible_source_extent,
    CanvasTransformCommand, DesktopState, FrameTransform, ItemRect, MainWindow, ModelRc,
    SceneItemSpec, SelectionOverlay, SnapGuides, SnapSettings, VecModel, MINIMUM_ITEM_PIXELS,
    UNIT_SCALE_MILLI,
};

/// Applies OBS's default aspect-preserving resize around the fixed edge(s).
pub(super) fn preserve_resize_aspect(
    base: ItemRect,
    raw: ItemRect,
    handle: i32,
    aspect_width: i64,
    aspect_height: i64,
) -> ItemRect {
    if handle == 0 {
        return raw;
    }
    let (width, height) = aspect_preserved_size(raw, handle, aspect_width, aspect_height);
    let right = base.x.saturating_add(base.width);
    let bottom = base.y.saturating_add(base.height);
    let center_x = base.x.saturating_add(base.width / 2);
    let center_y = base.y.saturating_add(base.height / 2);
    match handle {
        1 => ItemRect {
            x: right.saturating_sub(width),
            y: bottom.saturating_sub(height),
            width,
            height,
        },
        2 => ItemRect {
            x: center_x.saturating_sub(width / 2),
            y: bottom.saturating_sub(height),
            width,
            height,
        },
        3 => ItemRect {
            x: base.x,
            y: bottom.saturating_sub(height),
            width,
            height,
        },
        4 => ItemRect {
            x: base.x,
            y: center_y.saturating_sub(height / 2),
            width,
            height,
        },
        5 => ItemRect {
            x: base.x,
            y: base.y,
            width,
            height,
        },
        6 => ItemRect {
            x: center_x.saturating_sub(width / 2),
            y: base.y,
            width,
            height,
        },
        7 => ItemRect {
            x: right.saturating_sub(width),
            y: base.y,
            width,
            height,
        },
        8 => ItemRect {
            x: right.saturating_sub(width),
            y: center_y.saturating_sub(height / 2),
            width,
            height,
        },
        _ => raw,
    }
}

/// Rebuilds a transform after changing only its geometry.
pub(super) fn transform_with_geometry(
    base: FrameTransform,
    scale_x: u32,
    scale_y: u32,
    translate_x: i64,
    translate_y: i64,
) -> FrameTransform {
    FrameTransform::new(
        scale_x,
        scale_y,
        i32::try_from(translate_x).unwrap_or_else(|_| {
            if translate_x.is_negative() {
                i32::MIN
            } else {
                i32::MAX
            }
        }),
        i32::try_from(translate_y).unwrap_or_else(|_| {
            if translate_y.is_negative() {
                i32::MIN
            } else {
                i32::MAX
            }
        }),
        base.flip_x(),
        base.flip_y(),
        base.opacity(),
    )
    .and_then(|transform| transform.with_rotation_milli_degrees(base.rotation_milli_degrees()))
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

/// Returns a scale in fixed-point thousandths for one source extent.
pub(super) fn scale_for_extent(output_extent: i64, source_extent: i64) -> u32 {
    let milli = output_extent
        .max(1)
        .saturating_mul(UNIT_SCALE_MILLI)
        .div_euclid(source_extent.max(1));
    u32::try_from(milli.clamp(1, i64::from(FrameTransform::MAX_SCALE_MILLI))).unwrap_or(1)
}

/// Translates a transform so the requested part of its visible rectangle is
/// aligned to the canvas. Translation moves a rotated rectangle as a whole,
/// so the same operation works before and after rotation.
pub(super) fn align_transform(
    base: FrameTransform,
    canvas: (u32, u32),
    horizontal: Option<i64>,
    vertical: Option<i64>,
) -> FrameTransform {
    let rect = item_rect(base, canvas);
    let right = rect.x.saturating_add(rect.width);
    let bottom = rect.y.saturating_add(rect.height);
    let target_x = horizontal.unwrap_or(rect.x);
    let target_y = vertical.unwrap_or(rect.y);
    let delta_x = match horizontal {
        Some(0) => target_x.saturating_sub(rect.x),
        Some(1) => i64::from(canvas.0).saturating_sub(right),
        Some(2) => i64::from(canvas.0) / 2 - rect.x.saturating_add(rect.width / 2),
        _ => 0,
    };
    let delta_y = match vertical {
        Some(0) => target_y.saturating_sub(rect.y),
        Some(1) => i64::from(canvas.1).saturating_sub(bottom),
        Some(2) => i64::from(canvas.1) / 2 - rect.y.saturating_add(rect.height / 2),
        _ => 0,
    };
    transform_with_geometry(
        base,
        base.scale_x_milli(),
        base.scale_y_milli(),
        i64::from(base.translate_x()).saturating_add(delta_x),
        i64::from(base.translate_y()).saturating_add(delta_y),
    )
}

/// Applies one OBS-style Transform submenu command to a scene-item transform.
pub(crate) fn transform_for_command(
    base: FrameTransform,
    command: CanvasTransformCommand,
    canvas: (u32, u32),
) -> FrameTransform {
    match command {
        CanvasTransformCommand::FitToScreen | CanvasTransformCommand::StretchToScreen => {
            let (source_width, source_height) = visible_source_extent(base, canvas);
            let scale_x = scale_for_extent(i64::from(canvas.0), source_width);
            let scale_y = scale_for_extent(i64::from(canvas.1), source_height);
            let (scale_x, scale_y) = match command {
                CanvasTransformCommand::FitToScreen => {
                    let scale = scale_x.min(scale_y);
                    (scale, scale)
                }
                CanvasTransformCommand::StretchToScreen => (scale_x, scale_y),
                _ => unreachable!("the outer match limits this arm to screen sizing"),
            };
            let resized = transform_with_geometry(base, scale_x, scale_y, 0, 0);
            align_transform(resized, canvas, Some(2), Some(2))
        }
        CanvasTransformCommand::CenterToScreen => align_transform(base, canvas, Some(2), Some(2)),
        CanvasTransformCommand::CenterHorizontally => align_transform(base, canvas, Some(2), None),
        CanvasTransformCommand::CenterVertically => align_transform(base, canvas, None, Some(2)),
        CanvasTransformCommand::AlignLeft => align_transform(base, canvas, Some(0), None),
        CanvasTransformCommand::AlignRight => align_transform(base, canvas, Some(1), None),
        CanvasTransformCommand::AlignTop => align_transform(base, canvas, None, Some(0)),
        CanvasTransformCommand::AlignBottom => align_transform(base, canvas, None, Some(1)),
    }
}

/// Returns the single or multi-item geometry used by the transform overlay.
pub(crate) fn selection_overlay(
    state: &DesktopState,
    canvas: (u32, u32),
) -> Option<SelectionOverlay> {
    let scene_id = state.preview_scene()?;
    let scene = state
        .project_session()
        .project()
        .active_profile_spec()?
        .scene(scene_id)?;
    let transforms = scene
        .items()
        .iter()
        .filter(|item| state.is_source_selected(item.id().as_str()))
        .take(obs_rs_ui::MAX_CANVAS_SELECTIONS)
        .map(SceneItemSpec::transform)
        .collect::<Vec<_>>();
    selection_overlay_for_transforms(&transforms, canvas)
}

/// Applies one Rust-owned overlay projection to the generated Slint window.
///
/// The model lengths are always exactly eight when a selection exists and
/// zeroed otherwise. This keeps the declarative handle loop safe even while a
/// selection is being cleared by a click or project command.
pub(crate) fn set_selection_overlay(ui: &MainWindow, overlay: Option<&SelectionOverlay>) {
    let (active, rect, handle_x, handle_y, path) = overlay.map_or_else(
        || {
            (
                false,
                ItemRect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
                [0; 8],
                [0; 8],
                String::new(),
            )
        },
        |overlay| {
            (
                true,
                overlay.rect,
                overlay.handle_x,
                overlay.handle_y,
                overlay.path.clone(),
            )
        },
    );
    ui.set_item_active(active);
    ui.set_item_x(i32::try_from(rect.x).unwrap_or(0));
    ui.set_item_y(i32::try_from(rect.y).unwrap_or(0));
    ui.set_item_width(i32::try_from(rect.width).unwrap_or(0));
    ui.set_item_height(i32::try_from(rect.height).unwrap_or(0));
    ui.set_item_handle_x(ModelRc::new(VecModel::from(handle_x.to_vec())));
    ui.set_item_handle_y(ModelRc::new(VecModel::from(handle_y.to_vec())));
    ui.set_item_selection_path(path.into());
}

/// Rebuilds a transform from an edited rectangle, keeping flips, opacity,
/// crop, and rotation.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "rotated bounds are scaled through floating-point ratios and clamped before storage"
)]
pub(super) fn transform_for_rect(
    base: FrameTransform,
    rect: ItemRect,
    canvas: (u32, u32),
) -> FrameTransform {
    let (source_width, source_height) = visible_source_extent(base, canvas);
    let scale = |extent: i64, source_extent: i64| {
        let source_extent = source_extent.max(1);
        let milli = extent.saturating_mul(UNIT_SCALE_MILLI) / source_extent;
        u32::try_from(milli.clamp(1, i64::from(FrameTransform::MAX_SCALE_MILLI))).unwrap_or(1)
    };
    let (scale_x, scale_y, translate_x, translate_y) = if base.is_rotated() {
        let old_bounds = item_rect(base, canvas);
        let width_ratio = rect.width.max(1) as f64 / old_bounds.width.max(1) as f64;
        let height_ratio = rect.height.max(1) as f64 / old_bounds.height.max(1) as f64;
        let scale_x = (f64::from(base.scale_x_milli()) * width_ratio)
            .round()
            .clamp(1.0, f64::from(FrameTransform::MAX_SCALE_MILLI)) as i64;
        let scale_y = (f64::from(base.scale_y_milli()) * height_ratio)
            .round()
            .clamp(1.0, f64::from(FrameTransform::MAX_SCALE_MILLI)) as i64;
        let width = source_width * scale_x / UNIT_SCALE_MILLI;
        let height = source_height * scale_y / UNIT_SCALE_MILLI;
        (
            u32::try_from(scale_x).unwrap_or(FrameTransform::MAX_SCALE_MILLI),
            u32::try_from(scale_y).unwrap_or(FrameTransform::MAX_SCALE_MILLI),
            rect.x.saturating_add(rect.width / 2) - width / 2,
            rect.y.saturating_add(rect.height / 2) - height / 2,
        )
    } else {
        (
            scale(rect.width, source_width),
            scale(rect.height, source_height),
            rect.x,
            rect.y,
        )
    };
    let translate = |value: i64| i32::try_from(value).unwrap_or(0);
    FrameTransform::new(
        scale_x,
        scale_y,
        translate(translate_x),
        translate(translate_y),
        base.flip_x(),
        base.flip_y(),
        base.opacity(),
    )
    .and_then(|transform| transform.with_rotation_milli_degrees(base.rotation_milli_degrees()))
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

/// Rotates a floating-point vector with the same clockwise matrix used by the
/// media renderer. The inverse form maps a screen-space pointer delta into the
/// selected item's local resize axes.
#[allow(
    clippy::cast_precision_loss,
    reason = "the pointer/transform matrix intentionally uses f64 for sub-pixel rotation geometry"
)]
pub(super) fn rotate_canvas_vector(
    dx: f64,
    dy: f64,
    rotation_milli_degrees: i32,
    inverse: bool,
) -> (f64, f64) {
    let angle = f64::from(rotation_milli_degrees) / 180_000.0 * std::f64::consts::PI;
    let (sin, cos) = angle.sin_cos();
    if inverse {
        (cos * dx + sin * dy, -sin * dx + cos * dy)
    } else {
        (cos * dx - sin * dy, sin * dx + cos * dy)
    }
}

const fn fixed_handle_signs(handle: i32) -> Option<(i64, i64)> {
    match handle {
        1 => Some((1, 1)),
        2 => Some((0, 1)),
        3 => Some((-1, 1)),
        4 => Some((-1, 0)),
        5 => Some((-1, -1)),
        6 => Some((0, -1)),
        7 => Some((1, -1)),
        8 => Some((1, 0)),
        _ => None,
    }
}

/// Rebuilds a rotated transform from its local-space rectangle while keeping
/// the opposite visual edge/corner fixed in canvas space.
#[allow(
    clippy::cast_precision_loss,
    reason = "the fixed opposite handle is solved through the same f64 rotation matrix as the renderer"
)]
pub(super) fn transform_for_rotated_local_rect(
    base: FrameTransform,
    local: ItemRect,
    handle: i32,
    canvas: (u32, u32),
) -> FrameTransform {
    let Some((fixed_sign_x, fixed_sign_y)) = fixed_handle_signs(handle) else {
        return base;
    };
    let old = local_item_rect(base, canvas);
    let new_width = local.width.max(MINIMUM_ITEM_PIXELS);
    let new_height = local.height.max(MINIMUM_ITEM_PIXELS);
    let (source_width, source_height) = visible_source_extent(base, canvas);
    let scale_x = scale_for_extent(new_width, source_width);
    let scale_y = scale_for_extent(new_height, source_height);
    let actual_width = source_width * i64::from(scale_x) / UNIT_SCALE_MILLI;
    let actual_height = source_height * i64::from(scale_y) / UNIT_SCALE_MILLI;
    let old_center = (
        old.x as f64 + old.width as f64 / 2.0,
        old.y as f64 + old.height as f64 / 2.0,
    );
    let old_fixed_offset = (
        fixed_sign_x as f64 * old.width as f64 / 2.0,
        fixed_sign_y as f64 * old.height as f64 / 2.0,
    );
    let fixed_point = {
        let (offset_x, offset_y) = rotate_canvas_vector(
            old_fixed_offset.0,
            old_fixed_offset.1,
            base.rotation_milli_degrees(),
            false,
        );
        (old_center.0 + offset_x, old_center.1 + offset_y)
    };
    let new_fixed_offset = (
        fixed_sign_x as f64 * actual_width as f64 / 2.0,
        fixed_sign_y as f64 * actual_height as f64 / 2.0,
    );
    let (offset_x, offset_y) = rotate_canvas_vector(
        new_fixed_offset.0,
        new_fixed_offset.1,
        base.rotation_milli_degrees(),
        false,
    );
    let new_center = (fixed_point.0 - offset_x, fixed_point.1 - offset_y);
    let translate_x = rounded_canvas_coordinate(new_center.0 - actual_width as f64 / 2.0);
    let translate_y = rounded_canvas_coordinate(new_center.1 - actual_height as f64 / 2.0);
    transform_with_geometry(base, scale_x, scale_y, translate_x, translate_y)
}

pub(super) fn snap_rotated_resize_delta(
    base: FrameTransform,
    handle: i32,
    dx: i64,
    dy: i64,
    canvas: (u32, u32),
    guides: &SnapGuides,
    settings: SnapSettings,
) -> (i64, i64) {
    if !settings.enabled || settings.distance <= 0 || !(1..=8).contains(&handle) {
        return (dx, dy);
    }
    let index = usize::try_from(handle - 1).unwrap_or(0);
    let point = oriented_handle_points(base, canvas)[index];
    let target_x = point.x.saturating_add(dx);
    let target_y = point.y.saturating_add(dy);
    (
        dx.saturating_add(snap_delta(
            [target_x; 3],
            &guides.x[..guides.x_len],
            settings.distance,
        )),
        dy.saturating_add(snap_delta(
            [target_y; 3],
            &guides.y[..guides.y_len],
            settings.distance,
        )),
    )
}

/// Rotates a canvas-space delta into or out of the source-local frame.
///
/// The media transform uses the same clockwise matrix as the renderer. Crop
/// handles need the inverse mapping for pointer movement, while the retained
/// opposite edge needs the forward mapping when its local translation changes.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "pointer deltas are bounded canvas coordinates and the rotation matrix is intentionally evaluated in f64"
)]
pub(super) fn rotate_canvas_delta(
    dx: i64,
    dy: i64,
    rotation_milli_degrees: i32,
    inverse: bool,
) -> (i64, i64) {
    let angle = f64::from(rotation_milli_degrees) / 180_000.0 * std::f64::consts::PI;
    let (sin, cos) = angle.sin_cos();
    let (x, y) = if inverse {
        (
            cos * dx as f64 + sin * dy as f64,
            -sin * dx as f64 + cos * dy as f64,
        )
    } else {
        (
            cos * dx as f64 - sin * dy as f64,
            sin * dx as f64 + cos * dy as f64,
        )
    };
    (x.round() as i64, y.round() as i64)
}

/// Applies an Alt-drag on one of the eight transform handles as a source
/// crop. The handle number is the ordinary 1-8 clockwise handle number.
pub(super) fn crop_transform(
    base: FrameTransform,
    handle: i32,
    dx: i64,
    dy: i64,
    canvas: (u32, u32),
) -> FrameTransform {
    let scale_source_delta = |delta: i64, scale: u32| {
        delta
            .saturating_mul(UNIT_SCALE_MILLI)
            .div_euclid(i64::from(scale.max(1)))
    };
    let mut left = i64::from(base.crop_left());
    let mut top = i64::from(base.crop_top());
    let mut right = i64::from(base.crop_right());
    let mut bottom = i64::from(base.crop_bottom());
    let (pointer_local_x, pointer_local_y) = if base.is_rotated() {
        rotate_canvas_delta(dx, dy, base.rotation_milli_degrees(), true)
    } else {
        (dx, dy)
    };
    let delta_x = scale_source_delta(pointer_local_x, base.scale_x_milli());
    let delta_y = scale_source_delta(pointer_local_y, base.scale_y_milli());
    let max_left = (i64::from(canvas.0) - right - 1).max(0);
    let max_right = (i64::from(canvas.0) - left - 1).max(0);
    let max_top = (i64::from(canvas.1) - bottom - 1).max(0);
    let max_bottom = (i64::from(canvas.1) - top - 1).max(0);
    let mut local_translate_x = 0;
    let mut local_translate_y = 0;
    if matches!(handle, 1 | 7 | 8) {
        let next = (left + delta_x).clamp(0, max_left);
        let actual = next - left;
        left = next;
        local_translate_x = actual
            .saturating_mul(i64::from(base.scale_x_milli()))
            .div_euclid(UNIT_SCALE_MILLI);
    }
    if matches!(handle, 1..=3) {
        let next = (top + delta_y).clamp(0, max_top);
        let actual = next - top;
        top = next;
        local_translate_y = actual
            .saturating_mul(i64::from(base.scale_y_milli()))
            .div_euclid(UNIT_SCALE_MILLI);
    }
    if matches!(handle, 3..=5) {
        right = (right - delta_x).clamp(0, max_right);
    }
    if matches!(handle, 5..=7) {
        bottom = (bottom - delta_y).clamp(0, max_bottom);
    }
    let (translation_delta_x, translation_delta_y) = if base.is_rotated() {
        rotate_canvas_delta(
            local_translate_x,
            local_translate_y,
            base.rotation_milli_degrees(),
            false,
        )
    } else {
        (local_translate_x, local_translate_y)
    };
    let translate_x = i64::from(base.translate_x()).saturating_add(translation_delta_x);
    let translate_y = i64::from(base.translate_y()).saturating_add(translation_delta_y);
    let Ok(transform) = FrameTransform::new(
        base.scale_x_milli(),
        base.scale_y_milli(),
        i32::try_from(translate_x).unwrap_or(0),
        i32::try_from(translate_y).unwrap_or(0),
        base.flip_x(),
        base.flip_y(),
        base.opacity(),
    ) else {
        return base;
    };
    transform
        .with_rotation_milli_degrees(base.rotation_milli_degrees())
        .and_then(|transform| {
            transform.with_crop(
                u32::try_from(left).unwrap_or(u32::MAX),
                u32::try_from(top).unwrap_or(u32::MAX),
                u32::try_from(right).unwrap_or(u32::MAX),
                u32::try_from(bottom).unwrap_or(u32::MAX),
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
