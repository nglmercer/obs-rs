use super::{
    FrameTransform, SnapSettings, ACTION_SAFE_INSET, FOUR_BY_THREE_SAFE_X_INSET,
    GRAPHICS_SAFE_INSET, MAX_SNAP_GUIDES, MINIMUM_ITEM_PIXELS, SAFE_AREA_DENOMINATOR,
    UNIT_SCALE_MILLI,
};

/// Fixed-capacity guide storage used while a pointer gesture is active.
///
/// The scene can contain more items than fit in the guide budget. The first
/// visible items are retained deterministically; skipping the rest is safer
/// than allocating on every pointer sample.
#[derive(Clone, Copy, Debug)]
pub(super) struct SnapGuides {
    pub(super) x: [i64; MAX_SNAP_GUIDES],
    pub(super) y: [i64; MAX_SNAP_GUIDES],
    pub(super) x_len: usize,
    pub(super) y_len: usize,
}

impl Default for SnapGuides {
    fn default() -> Self {
        Self {
            x: [0; MAX_SNAP_GUIDES],
            y: [0; MAX_SNAP_GUIDES],
            x_len: 0,
            y_len: 0,
        }
    }
}

impl SnapGuides {
    pub(super) fn with_canvas(canvas: (u32, u32)) -> Self {
        let mut guides = Self::default();
        guides.push_x(0);
        guides.push_x(i64::from(canvas.0) / 2);
        guides.push_x(i64::from(canvas.0));
        guides.push_y(0);
        guides.push_y(i64::from(canvas.1) / 2);
        guides.push_y(i64::from(canvas.1));
        guides.push_rect(safe_area_rect(canvas, ACTION_SAFE_INSET, ACTION_SAFE_INSET));
        guides.push_rect(safe_area_rect(
            canvas,
            GRAPHICS_SAFE_INSET,
            GRAPHICS_SAFE_INSET,
        ));
        guides.push_rect(safe_area_rect(
            canvas,
            FOUR_BY_THREE_SAFE_X_INSET,
            GRAPHICS_SAFE_INSET,
        ));
        guides
    }

    fn push_x(&mut self, value: i64) {
        if self.x_len < MAX_SNAP_GUIDES {
            self.x[self.x_len] = value;
            self.x_len += 1;
        }
    }

    fn push_y(&mut self, value: i64) {
        if self.y_len < MAX_SNAP_GUIDES {
            self.y[self.y_len] = value;
            self.y_len += 1;
        }
    }

    pub(super) fn push_rect(&mut self, rect: ItemRect) {
        self.push_x(rect.x);
        self.push_x(rect.x.saturating_add(rect.width));
        self.push_x(rect.x.saturating_add(rect.width / 2));
        self.push_y(rect.y);
        self.push_y(rect.y.saturating_add(rect.height));
        self.push_y(rect.y.saturating_add(rect.height / 2));
    }
}

/// Returns one bounded safe-area rectangle using the margins from EBU R 95.
///
/// The 4:3 rectangle has a wider horizontal inset on a 16:9 canvas while its
/// vertical inset remains the graphics-safe margin. Clamping the inset keeps
/// the helper well-defined for tiny test canvases as well as normal video
/// resolutions.
pub(super) fn safe_area_rect(canvas: (u32, u32), x_numerator: i64, y_numerator: i64) -> ItemRect {
    let width = i64::from(canvas.0);
    let height = i64::from(canvas.1);
    let rounded_inset = |extent: i64, numerator: i64| {
        let numerator = numerator.clamp(0, SAFE_AREA_DENOMINATOR);
        let rounded = i128::from(extent)
            .saturating_mul(i128::from(numerator))
            .saturating_add(i128::from(SAFE_AREA_DENOMINATOR / 2))
            / i128::from(SAFE_AREA_DENOMINATOR);
        i64::try_from(rounded)
            .unwrap_or(i64::MAX)
            .clamp(0, extent / 2)
    };
    let x = rounded_inset(width, x_numerator);
    let y = rounded_inset(height, y_numerator);
    ItemRect {
        x,
        y,
        width: width.saturating_sub(x.saturating_mul(2)).max(1),
        height: height.saturating_sub(y.saturating_mul(2)).max(1),
    }
}

/// Returns the closest guide delta for the supplied moving edges.
pub(super) fn snap_delta(values: [i64; 3], guides: &[i64], distance: i64) -> i64 {
    let mut best_delta = 0;
    let max_distance = u64::try_from(distance).unwrap_or(0);
    let mut best_distance = max_distance.saturating_add(1);
    for value in values {
        for guide in guides {
            let delta = guide.saturating_sub(value);
            let candidate_distance = delta.unsigned_abs();
            if candidate_distance <= max_distance && candidate_distance < best_distance {
                best_delta = delta;
                best_distance = candidate_distance;
            }
        }
    }
    best_delta
}

/// Applies the active snap guides to one transient rectangle.
#[allow(
    clippy::too_many_lines,
    reason = "snap geometry keeps all handle cases in one tested operation"
)]
pub(super) fn snap_rect(
    rect: ItemRect,
    handle: i32,
    guides: &SnapGuides,
    settings: SnapSettings,
) -> ItemRect {
    if !settings.enabled || settings.distance <= 0 {
        return rect;
    }
    let (left, _top, right, _bottom) = match handle {
        1 => (true, true, false, false),
        2 => (false, true, false, false),
        3 => (false, true, true, false),
        4 => (false, false, true, false),
        5 => (false, false, true, true),
        6 => (false, false, false, true),
        7 => (true, false, false, true),
        8 => (true, false, false, false),
        0 => (false, false, false, false),
        _ => return rect,
    };
    let x_delta = if handle == 0 {
        snap_delta(
            [
                rect.x,
                rect.x.saturating_add(rect.width),
                rect.x.saturating_add(rect.width / 2),
            ],
            &guides.x[..guides.x_len],
            settings.distance,
        )
    } else if left {
        snap_delta(
            [rect.x, rect.x, rect.x],
            &guides.x[..guides.x_len],
            settings.distance,
        )
    } else if right {
        let right = rect.x.saturating_add(rect.width);
        snap_delta(
            [right, right, right],
            &guides.x[..guides.x_len],
            settings.distance,
        )
    } else {
        0
    };
    let y_delta = if handle == 0 {
        snap_delta(
            [
                rect.y,
                rect.y.saturating_add(rect.height),
                rect.y.saturating_add(rect.height / 2),
            ],
            &guides.y[..guides.y_len],
            settings.distance,
        )
    } else {
        let top = matches!(handle, 1..=3 | 8);
        let bottom = matches!(handle, 5..=7);
        if top {
            snap_delta(
                [rect.y, rect.y, rect.y],
                &guides.y[..guides.y_len],
                settings.distance,
            )
        } else if bottom {
            let bottom = rect.y.saturating_add(rect.height);
            snap_delta(
                [bottom, bottom, bottom],
                &guides.y[..guides.y_len],
                settings.distance,
            )
        } else {
            0
        }
    };
    let mut snapped = rect;
    if handle == 0 {
        snapped.x = snapped.x.saturating_add(x_delta);
        snapped.y = snapped.y.saturating_add(y_delta);
    } else {
        if left {
            snapped.x = snapped.x.saturating_add(x_delta);
            snapped.width = snapped
                .width
                .saturating_sub(x_delta)
                .max(MINIMUM_ITEM_PIXELS);
        } else if right {
            snapped.width = snapped
                .width
                .saturating_add(x_delta)
                .max(MINIMUM_ITEM_PIXELS);
        }
        if matches!(handle, 1 | 2 | 3 | 8) {
            snapped.y = snapped.y.saturating_add(y_delta);
            snapped.height = snapped
                .height
                .saturating_sub(y_delta)
                .max(MINIMUM_ITEM_PIXELS);
        } else if matches!(handle, 5..=7) {
            snapped.height = snapped
                .height
                .saturating_add(y_delta)
                .max(MINIMUM_ITEM_PIXELS);
        }
    }
    snapped
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
    pub(super) fn contains(self, x: i64, y: i64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }

    /// Returns the axis-aligned bounds covering both rectangles.
    pub(crate) fn union(self, other: Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self
            .x
            .saturating_add(self.width)
            .max(other.x.saturating_add(other.width));
        let bottom = self
            .y
            .saturating_add(self.height)
            .max(other.y.saturating_add(other.height));
        Self {
            x: left,
            y: top,
            width: right.saturating_sub(left).max(1),
            height: bottom.saturating_sub(top).max(1),
        }
    }

    /// Returns whether two rectangles overlap with positive area.
    pub(crate) fn intersects(self, other: Self) -> bool {
        self.x < other.x.saturating_add(other.width)
            && other.x < self.x.saturating_add(self.width)
            && self.y < other.y.saturating_add(other.height)
            && other.y < self.y.saturating_add(self.height)
    }
}

/// Geometry for the one bounded transform overlay shown on the preview.
///
/// The selection box remains an axis-aligned [`ItemRect`] for hit testing and
/// group selection. A single rotated item also publishes its oriented handle
/// points so the visual overlay follows the same transform matrix as the
/// compositor. The fixed eight-element arrays keep this presentation boundary
/// bounded and make it impossible for a scene to grow UI overlay state without
/// a corresponding cap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SelectionOverlay {
    pub(crate) rect: ItemRect,
    pub(crate) handle_x: [i32; 8],
    pub(crate) handle_y: [i32; 8],
    pub(crate) path: String,
    pub(crate) rotation_handle: Option<(i32, i32)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CanvasPoint {
    pub(super) x: i64,
    pub(super) y: i64,
}

/// Returns the visible source extent after crop, in source pixels.
pub(super) fn visible_source_extent(transform: FrameTransform, canvas: (u32, u32)) -> (i64, i64) {
    (
        (i64::from(canvas.0)
            - i64::from(transform.crop_left())
            - i64::from(transform.crop_right()))
        .max(1),
        (i64::from(canvas.1)
            - i64::from(transform.crop_top())
            - i64::from(transform.crop_bottom()))
        .max(1),
    )
}

/// Returns the axis-aligned bounds of a rotated rectangle.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "canvas geometry is deliberately rounded out to cover the rotated pixel bounds"
)]
pub(super) fn rotated_bounds(
    transform: FrameTransform,
    unrotated_width: i64,
    unrotated_height: i64,
) -> ItemRect {
    let angle = f64::from(transform.rotation_milli_degrees()) / 180_000.0 * std::f64::consts::PI;
    let (sin, cos) = angle.sin_cos();
    let width = (unrotated_width as f64 * cos.abs() + unrotated_height as f64 * sin.abs() - 1e-9)
        .ceil()
        .max(1.0) as i64;
    let height = (unrotated_width as f64 * sin.abs() + unrotated_height as f64 * cos.abs() - 1e-9)
        .ceil()
        .max(1.0) as i64;
    let center_x = f64::from(transform.translate_x()) + unrotated_width as f64 / 2.0;
    let center_y = f64::from(transform.translate_y()) + unrotated_height as f64 / 2.0;
    ItemRect {
        x: (center_x - width as f64 / 2.0).floor() as i64,
        y: (center_y - height as f64 / 2.0).floor() as i64,
        width,
        height,
    }
}

/// Returns where a transform places a source of `canvas` size on the canvas.
///
/// Sources render at canvas size, so the transform's scale is exactly the
/// item's size relative to the canvas and its translation is the top-left
/// corner of the unrotated visible source. Rotation expands this to the
/// axis-aligned bounds used by hit testing and the selection overlay.
pub(super) fn local_item_rect(transform: FrameTransform, canvas: (u32, u32)) -> ItemRect {
    let (source_width, source_height) = visible_source_extent(transform, canvas);
    ItemRect {
        x: i64::from(transform.translate_x()),
        y: i64::from(transform.translate_y()),
        width: (source_width * i64::from(transform.scale_x_milli()) / UNIT_SCALE_MILLI).max(1),
        height: (source_height * i64::from(transform.scale_y_milli()) / UNIT_SCALE_MILLI).max(1),
    }
}

pub(crate) fn item_rect(transform: FrameTransform, canvas: (u32, u32)) -> ItemRect {
    let local = local_item_rect(transform, canvas);
    if transform.is_rotated() {
        rotated_bounds(transform, local.width, local.height)
    } else {
        local
    }
}

/// Rounds a floating-point canvas coordinate into the bounded integer space
/// consumed by the Slint overlay.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "the transform matrix is evaluated in f64 and clamped before it crosses the UI boundary"
)]
pub(super) fn rounded_canvas_coordinate(value: f64) -> i64 {
    value.round().clamp(i64::MIN as f64, i64::MAX as f64) as i64
}

/// Returns the eight clockwise transform-handle points for one item.
///
/// The order matches the existing callback IDs: top-left, top, top-right,
/// right, bottom-right, bottom, bottom-left, left. Translation is the
/// unrotated visible-source origin and rotation is around the visible item's
/// centre, matching the media renderer.
#[allow(
    clippy::cast_precision_loss,
    reason = "canvas dimensions are converted to f64 for the same rotation matrix used by the media renderer"
)]
pub(super) fn oriented_handle_points(
    transform: FrameTransform,
    canvas: (u32, u32),
) -> [CanvasPoint; 8] {
    let (source_width, source_height) = visible_source_extent(transform, canvas);
    let width = (source_width * i64::from(transform.scale_x_milli()) / UNIT_SCALE_MILLI).max(1);
    let height = (source_height * i64::from(transform.scale_y_milli()) / UNIT_SCALE_MILLI).max(1);
    let center_x = f64::from(transform.translate_x()) + width as f64 / 2.0;
    let center_y = f64::from(transform.translate_y()) + height as f64 / 2.0;
    let angle = f64::from(transform.rotation_milli_degrees()) / 180_000.0 * std::f64::consts::PI;
    let (sin, cos) = angle.sin_cos();
    let half_width = width as f64 / 2.0;
    let half_height = height as f64 / 2.0;
    let local_points = [
        (-half_width, -half_height),
        (0.0, -half_height),
        (half_width, -half_height),
        (half_width, 0.0),
        (half_width, half_height),
        (0.0, half_height),
        (-half_width, half_height),
        (-half_width, 0.0),
    ];

    local_points.map(|(local_x, local_y)| CanvasPoint {
        x: rounded_canvas_coordinate(center_x + cos * local_x - sin * local_y),
        y: rounded_canvas_coordinate(center_y + sin * local_x + cos * local_y),
    })
}

pub(super) fn axis_handle_points(rect: ItemRect) -> [CanvasPoint; 8] {
    let right = rect.x.saturating_add(rect.width);
    let bottom = rect.y.saturating_add(rect.height);
    let middle_x = rect.x.saturating_add(rect.width / 2);
    let middle_y = rect.y.saturating_add(rect.height / 2);
    [
        CanvasPoint {
            x: rect.x,
            y: rect.y,
        },
        CanvasPoint {
            x: middle_x,
            y: rect.y,
        },
        CanvasPoint {
            x: right,
            y: rect.y,
        },
        CanvasPoint {
            x: right,
            y: middle_y,
        },
        CanvasPoint {
            x: right,
            y: bottom,
        },
        CanvasPoint {
            x: middle_x,
            y: bottom,
        },
        CanvasPoint {
            x: rect.x,
            y: bottom,
        },
        CanvasPoint {
            x: rect.x,
            y: middle_y,
        },
    ]
}

pub(super) fn selection_path(points: [CanvasPoint; 8]) -> String {
    let corners = [points[0], points[2], points[4], points[6]];
    format!(
        "M {} {} L {} {} L {} {} L {} {} Z",
        corners[0].x,
        corners[0].y,
        corners[1].x,
        corners[1].y,
        corners[2].x,
        corners[2].y,
        corners[3].x,
        corners[3].y,
    )
}

/// Returns the canvas-space centre of the rotation handle above one item.
///
/// The handle follows the oriented top edge for a single item and the top
/// edge of the axis-aligned group bounds for a multi-selection. Rust owns the
/// corresponding pivot for the gesture; Slint only presents this point.
#[allow(
    clippy::cast_precision_loss,
    reason = "the handle offset follows the same floating-point rotation geometry as the overlay"
)]
pub(super) fn rotation_handle_point(points: [CanvasPoint; 8]) -> Option<(i32, i32)> {
    const ROTATION_HANDLE_DISTANCE: f64 = 32.0;
    let top = points[1];
    let bottom = points[5];
    let direction_x = top.x.saturating_sub(bottom.x) as f64;
    let direction_y = top.y.saturating_sub(bottom.y) as f64;
    let length = direction_x.hypot(direction_y);
    if length <= f64::EPSILON {
        return None;
    }
    Some((
        to_slint_coordinate(rounded_canvas_coordinate(
            top.x as f64 + direction_x / length * ROTATION_HANDLE_DISTANCE,
        )),
        to_slint_coordinate(rounded_canvas_coordinate(
            top.y as f64 + direction_y / length * ROTATION_HANDLE_DISTANCE,
        )),
    ))
}

pub(super) fn to_slint_coordinate(value: i64) -> i32 {
    i32::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}

pub(super) fn selection_overlay_for_transforms(
    transforms: &[FrameTransform],
    canvas: (u32, u32),
) -> Option<SelectionOverlay> {
    let rect = transforms
        .iter()
        .copied()
        .map(|transform| item_rect(transform, canvas))
        .reduce(ItemRect::union)?;
    let points = if transforms.len() == 1 {
        oriented_handle_points(transforms[0], canvas)
    } else {
        axis_handle_points(rect)
    };
    Some(SelectionOverlay {
        rect,
        handle_x: points.map(|point| to_slint_coordinate(point.x)),
        handle_y: points.map(|point| to_slint_coordinate(point.y)),
        path: selection_path(points),
        rotation_handle: rotation_handle_point(points),
    })
}

/// Rounds a positive fixed-point ratio without using floating point geometry.
pub(super) fn rounded_ratio(value: i64, numerator: i64, denominator: i64) -> i64 {
    let product = i128::from(value.max(1)).saturating_mul(i128::from(numerator.max(1)));
    let rounded =
        product.saturating_add(i128::from(denominator.max(1) / 2)) / i128::from(denominator.max(1));
    i64::try_from(rounded).unwrap_or(i64::MAX).max(1)
}

/// Returns the aspect-preserving size selected by one OBS resize handle.
pub(super) fn aspect_preserved_size(
    raw: ItemRect,
    handle: i32,
    aspect_width: i64,
    aspect_height: i64,
) -> (i64, i64) {
    let aspect_width = aspect_width.max(1);
    let aspect_height = aspect_height.max(1);
    let raw_width = raw.width.max(MINIMUM_ITEM_PIXELS);
    let raw_height = raw.height.max(MINIMUM_ITEM_PIXELS);
    let (mut width, mut height) = match handle {
        2 | 6 => (
            rounded_ratio(raw_height, aspect_width, aspect_height),
            raw_height,
        ),
        4 | 8 => (
            raw_width,
            rounded_ratio(raw_width, aspect_height, aspect_width),
        ),
        _ => {
            // For a corner, preserve the dimension that is currently the
            // stronger part of the drag, matching OBS's ClampAspect rule.
            if i128::from(raw_width) * i128::from(aspect_height)
                < i128::from(raw_height) * i128::from(aspect_width)
            {
                (
                    rounded_ratio(raw_height, aspect_width, aspect_height),
                    raw_height,
                )
            } else {
                (
                    raw_width,
                    rounded_ratio(raw_width, aspect_height, aspect_width),
                )
            }
        }
    };
    if width < MINIMUM_ITEM_PIXELS {
        width = MINIMUM_ITEM_PIXELS;
        height = rounded_ratio(width, aspect_height, aspect_width);
    }
    if height < MINIMUM_ITEM_PIXELS {
        height = MINIMUM_ITEM_PIXELS;
        width = rounded_ratio(height, aspect_width, aspect_height);
    }
    (
        width.max(MINIMUM_ITEM_PIXELS),
        height.max(MINIMUM_ITEM_PIXELS),
    )
}
