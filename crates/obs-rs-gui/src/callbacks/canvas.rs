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
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    preview::{TransformDraft, TransformDraftItem},
    settings::{CANVAS_SNAP_DISTANCE_DEFAULT, CANVAS_SNAP_DISTANCE_RANGE},
    MainWindow, PreviewSurface,
};

/// The smallest on-canvas size a drag may leave a source at.
///
/// A scene item shrunk to nothing has no handles left to grab, so resizing
/// stops here rather than letting the item become unrecoverable.
const MINIMUM_ITEM_PIXELS: i64 = 16;
const MAX_PAN_PIXELS: i32 = 16_384;
const MAX_SNAP_GUIDES: usize = 64;
// Rec. ITU-R BT.1848-1 / EBU R 95 safe-area margins, represented as
// numerator / SAFE_AREA_DENOMINATOR so snapping stays deterministic and does
// not introduce floating-point geometry into pointer gestures.
const SAFE_AREA_DENOMINATOR: i64 = 2_000;
const ACTION_SAFE_INSET: i64 = 70; // 3.5%
const GRAPHICS_SAFE_INSET: i64 = 100; // 5.0%
const FOUR_BY_THREE_SAFE_X_INSET: i64 = 325; // 16.25%
const MIN_ZOOM_PERCENT: u16 = 10;
const MAX_ZOOM_PERCENT: u16 = 800;
const WHEEL_ZOOM_FACTOR_MILLI: i64 = 1_250;
const SCALE_MICROS_PER_PERCENT: i64 = 10_000;
const SCALE_MICROS_PER_UNIT: i64 = 1_000_000;
const RESIZE_MODIFIER_SHIFT: i32 = 1;
const RESIZE_MODIFIER_CONTROL: i32 = 2;

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

    /// Returns the next bounded continuous zoom level for a wheel tick.
    ///
    /// Fit-to-window supplies its effective scale from the live viewport so
    /// the first wheel tick starts from what the user is actually seeing.
    pub(crate) fn wheel(self, direction: i32, current_scale_micros: i32) -> Self {
        if direction == 0 {
            return self;
        }
        let current_percent_milli = match self {
            Self::FitToWindow => i64::from(current_scale_micros.max(1)) / 10,
            Self::Percent(percent) => i64::from(percent) * 1_000,
        };
        let scaled = if direction > 0 {
            current_percent_milli.saturating_mul(WHEEL_ZOOM_FACTOR_MILLI) / 1_000
        } else {
            current_percent_milli
                .saturating_mul(1_000)
                .checked_div(WHEEL_ZOOM_FACTOR_MILLI)
                .unwrap_or(0)
        };
        let percent = ((scaled + 500) / 1_000)
            .clamp(i64::from(MIN_ZOOM_PERCENT), i64::from(MAX_ZOOM_PERCENT));
        Self::Percent(u16::try_from(percent).unwrap_or(MAX_ZOOM_PERCENT))
    }

    /// Moves one position through the OBS-style bounded preset list.
    pub(crate) fn stepped(self, direction: i32) -> Self {
        const PRESETS: [CanvasZoom; 5] = [
            CanvasZoom::FitToWindow,
            CanvasZoom::Percent(25),
            CanvasZoom::Percent(50),
            CanvasZoom::Percent(100),
            CanvasZoom::Percent(200),
        ];
        let current_percent = match self {
            Self::FitToWindow => 0,
            Self::Percent(percent) => i32::from(percent),
        };
        let next = match direction.cmp(&0) {
            std::cmp::Ordering::Less => PRESETS
                .iter()
                .rposition(|preset| preset.ui_value() < current_percent)
                .unwrap_or(0),
            std::cmp::Ordering::Greater => PRESETS
                .iter()
                .position(|preset| preset.ui_value() > current_percent)
                .unwrap_or(PRESETS.len() - 1),
            std::cmp::Ordering::Equal => PRESETS
                .iter()
                .position(|preset| preset.ui_value() == current_percent)
                .unwrap_or(0),
        };
        PRESETS[next]
    }

    /// Returns the fixed-point scale used by the Slint canvas mapping.
    fn scale_micros(self, fit_scale_micros: i32) -> i64 {
        match self {
            Self::FitToWindow => i64::from(fit_scale_micros.max(1)),
            Self::Percent(percent) => i64::from(percent) * SCALE_MICROS_PER_PERCENT,
        }
    }
}

/// Bounded snapping policy for the interactive canvas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SnapSettings {
    enabled: bool,
    distance: i64,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            distance: i64::from(CANVAS_SNAP_DISTANCE_DEFAULT),
        }
    }
}

/// Modifier policy captured when a resize/move gesture starts.
///
/// OBS keeps ordinary scene-item resizing aspect-preserving and uses Shift to
/// opt into free resizing. Ctrl suppresses snapping for the gesture. The
/// policy is copied out of the toolkit event at the boundary so the geometry
/// functions remain deterministic and testable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CanvasResizeModifiers {
    preserve_aspect: bool,
    snapping: bool,
}

impl CanvasResizeModifiers {
    fn from_mask(mask: i32) -> Self {
        Self {
            preserve_aspect: mask & RESIZE_MODIFIER_SHIFT == 0,
            snapping: mask & RESIZE_MODIFIER_CONTROL == 0,
        }
    }
}

/// Transform commands exposed by the OBS-style source menu.
///
/// The menu crosses the Slint boundary as a short action name, but the
/// geometry is parsed into this closed set before it can reach project state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CanvasTransformCommand {
    FitToScreen,
    StretchToScreen,
    CenterToScreen,
    CenterHorizontally,
    CenterVertically,
    AlignLeft,
    AlignRight,
    AlignTop,
    AlignBottom,
}

impl CanvasTransformCommand {
    pub(crate) fn from_action(action: &str) -> Option<Self> {
        match action {
            "fit-screen" => Some(Self::FitToScreen),
            "stretch-screen" => Some(Self::StretchToScreen),
            "center-screen" => Some(Self::CenterToScreen),
            "center-horizontally" => Some(Self::CenterHorizontally),
            "center-vertically" => Some(Self::CenterVertically),
            "align-left" => Some(Self::AlignLeft),
            "align-right" => Some(Self::AlignRight),
            "align-top" => Some(Self::AlignTop),
            "align-bottom" => Some(Self::AlignBottom),
            _ => None,
        }
    }
}

/// Transient canvas viewport state owned by the canvas controller.
///
/// Zoom and pan are presentation state, not project data. Keeping them beside
/// the transform draft gives the UI one owner while leaving scene commands and
/// persisted documents free of widget-specific values. Keyboard nudges are
/// immediate commands, not persistent viewport state, and use the same owner
/// for selection validation before they reach the project.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CanvasState {
    zoom: CanvasZoom,
    pan: (i32, i32),
    snapping: SnapSettings,
    selection_anchor: Option<(i64, i64)>,
    selection_box: Option<ItemRect>,
    selection_additive: bool,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            zoom: CanvasZoom::FitToWindow,
            pan: (0, 0),
            snapping: SnapSettings::default(),
            selection_anchor: None,
            selection_box: None,
            selection_additive: false,
        }
    }
}

impl CanvasState {
    pub(crate) const fn zoom(self) -> CanvasZoom {
        self.zoom
    }

    pub(crate) const fn pan(self) -> (i32, i32) {
        self.pan
    }

    pub(crate) const fn with_zoom(self, zoom: CanvasZoom) -> Self {
        Self { zoom, ..self }
    }

    pub(crate) const fn with_pan(self, pan: (i32, i32)) -> Self {
        Self { pan, ..self }
    }

    pub(crate) const fn snapping(self) -> SnapSettings {
        self.snapping
    }

    pub(crate) const fn selection_box(self) -> Option<ItemRect> {
        self.selection_box
    }

    pub(crate) const fn selection_additive(self) -> bool {
        self.selection_additive
    }

    pub(crate) const fn with_snapping(self, snapping: SnapSettings) -> Self {
        Self { snapping, ..self }
    }

    /// Updates the transient snap policy from the validated settings value.
    ///
    /// The setting is stored as an unsigned canvas-pixel count, while the
    /// geometry path uses a signed distance for saturating arithmetic. Clamp at
    /// this boundary as well as in settings loading so a runtime caller cannot
    /// bypass the same resource and interaction bounds.
    pub(crate) fn with_snap_distance(self, distance: u16) -> Self {
        let distance = distance.clamp(
            *CANVAS_SNAP_DISTANCE_RANGE.start(),
            *CANVAS_SNAP_DISTANCE_RANGE.end(),
        );
        self.with_snapping(SnapSettings {
            distance: i64::from(distance),
            ..self.snapping
        })
    }

    /// Applies a bounded pointer delta in canvas pixels.
    pub(crate) fn panned(self, dx: i32, dy: i32) -> Self {
        let bounded = |current: i32, delta: i32| {
            i64::from(current)
                .saturating_add(i64::from(delta))
                .clamp(-i64::from(MAX_PAN_PIXELS), i64::from(MAX_PAN_PIXELS))
                .try_into()
                .unwrap_or_else(|_| unreachable!("pan is clamped to i32 bounds"))
        };
        self.with_pan((bounded(self.pan.0, dx), bounded(self.pan.1, dy)))
    }

    /// Changes zoom while keeping the canvas point under the wheel cursor
    /// under that same widget point. All arithmetic is fixed-point and
    /// bounded before it becomes transient viewport state.
    pub(crate) fn zoomed_at(
        self,
        direction: i32,
        anchor: (i32, i32),
        pointer: (i32, i32),
        old_view_origin: (i32, i32),
        old_scale_micros: i32,
    ) -> Self {
        let zoom = self.zoom.wheel(direction, old_scale_micros);
        let new_scale_micros = zoom.scale_micros(old_scale_micros);
        let old_scale_micros = i64::from(old_scale_micros.max(1));
        let center_offset = (
            i64::from(old_view_origin.0)
                .saturating_mul(SCALE_MICROS_PER_UNIT)
                .saturating_sub(i64::from(self.pan.0).saturating_mul(old_scale_micros)),
            i64::from(old_view_origin.1)
                .saturating_mul(SCALE_MICROS_PER_UNIT)
                .saturating_sub(i64::from(self.pan.1).saturating_mul(old_scale_micros)),
        );
        let anchored_pan = |pointer: i32, anchor: i32, center_offset: i64| {
            let numerator = i64::from(pointer)
                .saturating_mul(SCALE_MICROS_PER_UNIT)
                .saturating_sub(center_offset)
                .saturating_sub(i64::from(anchor).saturating_mul(new_scale_micros));
            let value = numerator
                .checked_div(new_scale_micros.max(1))
                .unwrap_or(0)
                .clamp(-i64::from(MAX_PAN_PIXELS), i64::from(MAX_PAN_PIXELS));
            i32::try_from(value).unwrap_or_else(|_| unreachable!("pan is bounded"))
        };
        Self {
            zoom,
            pan: (
                anchored_pan(pointer.0, anchor.0, center_offset.0),
                anchored_pan(pointer.1, anchor.1, center_offset.1),
            ),
            ..self
        }
    }

    fn begin_selection(self, x: i64, y: i64, additive: bool) -> Self {
        Self {
            selection_anchor: Some((x, y)),
            selection_box: Some(ItemRect {
                x,
                y,
                width: 0,
                height: 0,
            }),
            selection_additive: additive,
            ..self
        }
    }

    fn update_selection(self, x: i64, y: i64) -> Self {
        let Some((anchor_x, anchor_y)) = self.selection_anchor else {
            return self;
        };
        let left = anchor_x.min(x);
        let top = anchor_y.min(y);
        Self {
            selection_box: Some(ItemRect {
                x: left,
                y: top,
                width: i64::try_from(x.abs_diff(anchor_x)).unwrap_or(i64::MAX),
                height: i64::try_from(y.abs_diff(anchor_y)).unwrap_or(i64::MAX),
            }),
            ..self
        }
    }

    fn clear_selection(self) -> Self {
        Self {
            selection_anchor: None,
            selection_box: None,
            selection_additive: false,
            ..self
        }
    }
}

/// Fixed-capacity guide storage used while a pointer gesture is active.
///
/// The scene can contain more items than fit in the guide budget. The first
/// visible items are retained deterministically; skipping the rest is safer
/// than allocating on every pointer sample.
#[derive(Clone, Copy, Debug)]
struct SnapGuides {
    x: [i64; MAX_SNAP_GUIDES],
    y: [i64; MAX_SNAP_GUIDES],
    x_len: usize,
    y_len: usize,
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
    fn with_canvas(canvas: (u32, u32)) -> Self {
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

    fn push_rect(&mut self, rect: ItemRect) {
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
fn safe_area_rect(canvas: (u32, u32), x_numerator: i64, y_numerator: i64) -> ItemRect {
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
fn snap_delta(values: [i64; 3], guides: &[i64], distance: i64) -> i64 {
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
fn snap_rect(rect: ItemRect, handle: i32, guides: &SnapGuides, settings: SnapSettings) -> ItemRect {
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
    fn contains(self, x: i64, y: i64) -> bool {
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CanvasPoint {
    x: i64,
    y: i64,
}

/// Returns the visible source extent after crop, in source pixels.
fn visible_source_extent(transform: FrameTransform, canvas: (u32, u32)) -> (i64, i64) {
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
fn rotated_bounds(
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
fn local_item_rect(transform: FrameTransform, canvas: (u32, u32)) -> ItemRect {
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
fn rounded_canvas_coordinate(value: f64) -> i64 {
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
fn oriented_handle_points(transform: FrameTransform, canvas: (u32, u32)) -> [CanvasPoint; 8] {
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

fn axis_handle_points(rect: ItemRect) -> [CanvasPoint; 8] {
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

fn selection_path(points: [CanvasPoint; 8]) -> String {
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

fn to_slint_coordinate(value: i64) -> i32 {
    i32::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}

fn selection_overlay_for_transforms(
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
    })
}

/// Rounds a positive fixed-point ratio without using floating point geometry.
fn rounded_ratio(value: i64, numerator: i64, denominator: i64) -> i64 {
    let product = i128::from(value.max(1)).saturating_mul(i128::from(numerator.max(1)));
    let rounded =
        product.saturating_add(i128::from(denominator.max(1) / 2)) / i128::from(denominator.max(1));
    i64::try_from(rounded).unwrap_or(i64::MAX).max(1)
}

/// Returns the aspect-preserving size selected by one OBS resize handle.
fn aspect_preserved_size(
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

/// Applies OBS's default aspect-preserving resize around the fixed edge(s).
fn preserve_resize_aspect(
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
fn transform_with_geometry(
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
fn scale_for_extent(output_extent: i64, source_extent: i64) -> u32 {
    let milli = output_extent
        .max(1)
        .saturating_mul(UNIT_SCALE_MILLI)
        .div_euclid(source_extent.max(1));
    u32::try_from(milli.clamp(1, i64::from(FrameTransform::MAX_SCALE_MILLI))).unwrap_or(1)
}

/// Translates a transform so the requested part of its visible rectangle is
/// aligned to the canvas. Translation moves a rotated rectangle as a whole,
/// so the same operation works before and after rotation.
fn align_transform(
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
fn transform_for_rect(base: FrameTransform, rect: ItemRect, canvas: (u32, u32)) -> FrameTransform {
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
fn rotate_canvas_vector(
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
fn transform_for_rotated_local_rect(
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

fn snap_rotated_resize_delta(
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
fn rotate_canvas_delta(dx: i64, dy: i64, rotation_milli_degrees: i32, inverse: bool) -> (i64, i64) {
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
fn crop_transform(
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

    /// Applies one pointer delta and returns the new viewport state.
    pub(crate) fn pan_by(&self, dx: i32, dy: i32) -> CanvasState {
        let state = self.canvas_state().panned(dx, dy);
        self.state.replace(state);
        state
    }

    /// Applies the persisted canvas snap distance to the one transient canvas
    /// policy shared by move and resize gestures.
    pub(crate) fn set_snap_distance(&self, distance: u16) -> CanvasState {
        let state = self.canvas_state().with_snap_distance(distance);
        self.state.replace(state);
        state
    }

    fn begin_selection(&self, x: i64, y: i64, additive: bool) -> CanvasState {
        let state = self.canvas_state().begin_selection(x, y, additive);
        self.state.replace(state);
        state
    }

    fn update_selection(&self, x: i64, y: i64) -> CanvasState {
        let state = self.canvas_state().update_selection(x, y);
        self.state.replace(state);
        state
    }

    fn finish_selection(&self) -> (CanvasState, Option<ItemRect>, bool) {
        let state = self.canvas_state();
        let result = (state, state.selection_box(), state.selection_additive());
        self.state.replace(state.clear_selection());
        result
    }
}

fn set_selection_box_properties(ui: &MainWindow, selection: Option<ItemRect>) {
    if let Some(selection) = selection {
        ui.set_selection_box_active(true);
        ui.set_selection_box_x(i32::try_from(selection.x).unwrap_or(0));
        ui.set_selection_box_y(i32::try_from(selection.y).unwrap_or(0));
        ui.set_selection_box_width(i32::try_from(selection.width).unwrap_or(0));
        ui.set_selection_box_height(i32::try_from(selection.height).unwrap_or(0));
    } else {
        ui.set_selection_box_active(false);
        ui.set_selection_box_x(0);
        ui.set_selection_box_y(0);
        ui.set_selection_box_width(0);
        ui.set_selection_box_height(0);
    }
}

/// Connects the UI's transient zoom controls to the one canvas state owner.
fn install_zoom_callbacks(ui: &MainWindow, controller: &Rc<CanvasController>) {
    let state = controller.canvas_state();
    ui.set_canvas_zoom(state.zoom().ui_value());
    ui.set_canvas_pan_x(state.pan().0);
    ui.set_canvas_pan_y(state.pan().1);

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

    let weak = ui.as_weak();
    let zoom_controller = Rc::clone(controller);
    ui.on_canvas_zoom_at(
        move |direction, anchor_x, anchor_y, pointer_x, pointer_y, view_x, view_y, scale| {
            let state = zoom_controller.canvas_state().zoomed_at(
                direction,
                (anchor_x, anchor_y),
                (pointer_x, pointer_y),
                (view_x, view_y),
                scale,
            );
            zoom_controller.state.replace(state);
            if let Some(ui) = weak.upgrade() {
                ui.set_canvas_zoom(state.zoom().ui_value());
                ui.set_canvas_pan_x(state.pan().0);
                ui.set_canvas_pan_y(state.pan().1);
            }
        },
    );

    let weak = ui.as_weak();
    let pan_controller = Rc::clone(controller);
    ui.on_canvas_pan_dragged(move |dx, dy| {
        let state = pan_controller.pan_by(dx, dy);
        if let Some(ui) = weak.upgrade() {
            ui.set_canvas_pan_x(state.pan().0);
            ui.set_canvas_pan_y(state.pan().1);
        }
    });
}

/// Installs the canvas selection and transform callbacks.
#[allow(
    clippy::too_many_lines,
    reason = "the callback installation keeps draft, commit, and selection lifetimes together"
)]
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
    let nudge_state = Rc::clone(state);
    let nudge_surface = Rc::clone(surface);
    ui.on_canvas_nudged(move |dx, dy| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(scene) = nudge_state.borrow().preview_scene().map(str::to_owned) else {
            return;
        };
        let (profile, transforms) = {
            let state = nudge_state.borrow();
            let profile = state
                .project_session()
                .project()
                .active_profile()
                .to_string();
            let Some(scene_spec) = state
                .project_session()
                .project()
                .active_profile_spec()
                .and_then(|profile| profile.scene(scene.as_str()))
            else {
                return;
            };
            let transforms = state
                .selected_sources()
                .filter_map(|id| scene_spec.item(id))
                .filter(|item| !item.locked())
                .map(|item| {
                    let transform = item.transform();
                    (
                        item.id().to_string(),
                        transform_with_geometry(
                            transform,
                            transform.scale_x_milli(),
                            transform.scale_y_milli(),
                            i64::from(transform.translate_x()).saturating_add(i64::from(dx)),
                            i64::from(transform.translate_y()).saturating_add(i64::from(dy)),
                        ),
                    )
                })
                .collect::<Vec<_>>();
            (profile, transforms)
        };
        if transforms.is_empty() {
            return;
        }
        crate::apply_source_transforms_to(
            &ui,
            &nudge_state,
            &nudge_surface,
            &profile,
            &scene,
            transforms,
        );
    });

    let weak = ui.as_weak();
    let pointer_state = Rc::clone(state);
    let pointer_surface = Rc::clone(surface);
    let pointer_controller = Rc::clone(&controller);
    ui.on_canvas_pointer_pressed(move |x, y, additive| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        pointer_controller.draft.borrow_mut().take();
        let canvas = canvas_size(&ui);
        // A plain click follows OBS's preview behavior: if the current
        // topmost item is already selected, walk down through the hit stack
        // so an obscured source can be reached without first moving it.
        // Ctrl-click remains an explicit toggle of the topmost hit.
        let hit = source_at(
            &pointer_state,
            canvas,
            i64::from(x),
            i64::from(y),
            !additive,
        );
        if let Some(id) = hit {
            let command = if additive {
                UiCommand::ToggleSourceSelection { id }
            } else {
                UiCommand::SelectSource { id }
            };
            crate::dispatch_and_refresh(&ui.as_weak(), &pointer_state, &pointer_surface, command);
            ui.set_canvas_pointer_mode(1);
            set_selection_box_properties(&ui, None);
        } else {
            if !additive {
                crate::dispatch_and_refresh(
                    &ui.as_weak(),
                    &pointer_state,
                    &pointer_surface,
                    UiCommand::SelectSources {
                        ids: Vec::new(),
                        additive: false,
                    },
                );
            }
            let state = pointer_controller.begin_selection(i64::from(x), i64::from(y), additive);
            ui.set_canvas_pointer_mode(2);
            set_selection_box_properties(&ui, state.selection_box());
        }
    });

    let weak = ui.as_weak();
    let selection_controller = Rc::clone(&controller);
    ui.on_canvas_selection_dragged(move |x, y| {
        let state = selection_controller.update_selection(i64::from(x), i64::from(y));
        if let Some(ui) = weak.upgrade() {
            set_selection_box_properties(&ui, state.selection_box());
        }
    });

    let weak = ui.as_weak();
    let selection_state = Rc::clone(state);
    let selection_surface = Rc::clone(surface);
    let selection_controller = Rc::clone(&controller);
    ui.on_canvas_selection_committed(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let (_, selection, additive) = selection_controller.finish_selection();
        ui.set_canvas_pointer_mode(0);
        set_selection_box_properties(&ui, None);
        let ids = selection
            .map(|selection| source_ids_in_rect(&selection_state, canvas_size(&ui), selection))
            .unwrap_or_default();
        crate::dispatch_and_refresh(
            &ui.as_weak(),
            &selection_state,
            &selection_surface,
            UiCommand::SelectSources { ids, additive },
        );
    });

    let weak = ui.as_weak();
    let drag_state = Rc::clone(state);
    let drag_controller = Rc::clone(&controller);
    ui.on_transform_dragged(move |handle, dx, dy, modifier_mask| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if selected_is_locked(&drag_state) {
            return;
        }
        let Some(scene) = drag_state.borrow().preview_scene().map(str::to_owned) else {
            return;
        };
        let canvas = canvas_size(&ui);
        let mut draft_slot = drag_controller.draft.borrow_mut();
        let needs_new_draft = draft_slot.as_ref().is_none_or(|draft| draft.scene != scene);
        if needs_new_draft {
            let items = selected_transforms(&drag_state, &scene);
            if items.is_empty() {
                return;
            }
            *draft_slot = Some(TransformDraft { scene, items });
        }
        let Some(draft) = draft_slot.as_mut() else {
            return;
        };
        let Some(group) = draft_rect(draft, canvas) else {
            return;
        };
        let modifiers = CanvasResizeModifiers::from_mask(modifier_mask);
        if (9..=16).contains(&handle) {
            if draft.items.len() != 1 {
                return;
            }
            let item = &mut draft.items[0];
            item.transform = crop_transform(
                item.transform,
                handle - 8,
                i64::from(dx),
                i64::from(dy),
                canvas,
            );
        } else if draft.items.len() == 1
            && draft.items[0].transform.is_rotated()
            && (1..=8).contains(&handle)
        {
            let snap_settings = SnapSettings {
                enabled: drag_controller.canvas_state().snapping().enabled && modifiers.snapping,
                ..drag_controller.canvas_state().snapping()
            };
            let guides = scene_snap_guides(&drag_state, &draft.scene, &draft.items, canvas);
            let base = draft.items[0].transform;
            let (adjusted_x, adjusted_y) = snap_rotated_resize_delta(
                base,
                handle,
                i64::from(dx),
                i64::from(dy),
                canvas,
                &guides,
                snap_settings,
            );
            let (axis_x, axis_y) =
                rotate_canvas_delta(adjusted_x, adjusted_y, base.rotation_milli_degrees(), true);
            let local_base = local_item_rect(base, canvas);
            let raw = drag_rect(local_base, handle, axis_x, axis_y);
            let (aspect_width, aspect_height) = visible_source_extent(base, canvas);
            let local = if modifiers.preserve_aspect {
                preserve_resize_aspect(local_base, raw, handle, aspect_width, aspect_height)
            } else {
                raw
            };
            draft.items[0].transform =
                transform_for_rotated_local_rect(base, local, handle, canvas);
        } else {
            let snap_settings = SnapSettings {
                enabled: drag_controller.canvas_state().snapping().enabled && modifiers.snapping,
                ..drag_controller.canvas_state().snapping()
            };
            let guides = scene_snap_guides(&drag_state, &draft.scene, &draft.items, canvas);
            let snapped = snap_rect(
                drag_rect(group, handle, i64::from(dx), i64::from(dy)),
                handle,
                &guides,
                snap_settings,
            );
            let (aspect_width, aspect_height) = if draft.items.len() == 1 {
                visible_source_extent(draft.items[0].transform, canvas)
            } else {
                (group.width, group.height)
            };
            let rect = if modifiers.preserve_aspect {
                preserve_resize_aspect(group, snapped, handle, aspect_width, aspect_height)
            } else {
                snapped
            };
            for item in &mut draft.items {
                let old_rect = item_rect(item.transform, canvas);
                let next_rect = if handle == 0 {
                    ItemRect {
                        x: old_rect.x.saturating_add(rect.x.saturating_sub(group.x)),
                        y: old_rect.y.saturating_add(rect.y.saturating_sub(group.y)),
                        ..old_rect
                    }
                } else {
                    map_rect_into_group(old_rect, group, rect)
                };
                item.transform = transform_for_rect(item.transform, next_rect, canvas);
            }
        }
        let overlay = draft_overlay(draft, canvas);
        drop(draft_slot);
        // The drag stays out of the project until the pointer is released: the
        // preview timer feeds this straight to the compositor, and the overlay
        // handles follow it here. A revision per mouse move would fill the undo
        // history with a hundred entries for one gesture — and, before the
        // runtime learned to update in place, restart every capture device in
        // the scene along the way.
        if let Some(overlay) = overlay {
            set_selection_overlay(&ui, Some(&overlay));
        }
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
        let profile = commit_state
            .borrow()
            .project_session()
            .project()
            .active_profile()
            .to_string();
        let transforms = draft
            .items
            .into_iter()
            .map(|item| (item.item, item.transform))
            .collect::<Vec<_>>();
        ui.set_canvas_pointer_mode(0);
        if transforms.is_empty() {
            return;
        }
        crate::apply_source_transforms_to(
            &ui,
            &commit_state,
            &commit_surface,
            &profile,
            &draft.scene,
            transforms,
        );
    });

    controller
}

/// Moves the selection overlay with the pointer during a drag.
///
/// The dock refresh derives these from the project, which the drag has not
/// reached yet, so the handles are placed from the draft instead.
/// Copies the selected scene-item transforms once, at the start of a gesture.
/// Subsequent pointer samples mutate this bounded draft in place rather than
/// rebuilding the selection from project state.
fn selected_transforms(
    state: &Rc<RefCell<DesktopState>>,
    scene_id: &str,
) -> Vec<TransformDraftItem> {
    let state = state.borrow();
    let Some(scene) = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
    else {
        return Vec::new();
    };
    state
        .selected_sources()
        .filter_map(|id| {
            scene.item(id).map(|item| TransformDraftItem {
                item: id.to_owned(),
                transform: item.transform(),
            })
        })
        .take(obs_rs_ui::MAX_CANVAS_SELECTIONS)
        .collect()
}

fn draft_rect(draft: &TransformDraft, canvas: (u32, u32)) -> Option<ItemRect> {
    draft
        .items
        .iter()
        .map(|item| item_rect(item.transform, canvas))
        .reduce(ItemRect::union)
}

fn draft_overlay(draft: &TransformDraft, canvas: (u32, u32)) -> Option<SelectionOverlay> {
    let transforms = draft
        .items
        .iter()
        .map(|item| item.transform)
        .collect::<Vec<_>>();
    selection_overlay_for_transforms(&transforms, canvas)
}

/// Maps one item rectangle from the old group bounds into the new bounds.
fn map_rect_into_group(rect: ItemRect, old_group: ItemRect, new_group: ItemRect) -> ItemRect {
    let map = |value: i64, old_start: i64, old_extent: i64, new_start: i64, new_extent: i64| {
        let offset = i128::from(value.saturating_sub(old_start));
        let scaled = offset * i128::from(new_extent) / i128::from(old_extent.max(1));
        i64::try_from(i128::from(new_start) + scaled).unwrap_or_else(|_| {
            if scaled.is_negative() {
                i64::MIN
            } else {
                i64::MAX
            }
        })
    };
    let x = map(
        rect.x,
        old_group.x,
        old_group.width,
        new_group.x,
        new_group.width,
    );
    let y = map(
        rect.y,
        old_group.y,
        old_group.height,
        new_group.y,
        new_group.height,
    );
    let right = map(
        rect.x.saturating_add(rect.width),
        old_group.x,
        old_group.width,
        new_group.x,
        new_group.width,
    );
    let bottom = map(
        rect.y.saturating_add(rect.height),
        old_group.y,
        old_group.height,
        new_group.y,
        new_group.height,
    );
    ItemRect {
        x,
        y,
        width: right.saturating_sub(x).max(MINIMUM_ITEM_PIXELS),
        height: bottom.saturating_sub(y).max(MINIMUM_ITEM_PIXELS),
    }
}

fn source_ids_in_rect(
    state: &Rc<RefCell<DesktopState>>,
    canvas: (u32, u32),
    selection: ItemRect,
) -> Vec<String> {
    let state = state.borrow();
    let Some(scene_id) = state.preview_scene() else {
        return Vec::new();
    };
    let Some(scene) = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
    else {
        return Vec::new();
    };
    scene
        .items()
        .iter()
        .filter(|item| item.visible() && item_rect(item.transform(), canvas).intersects(selection))
        .map(|item| item.id().as_str().to_owned())
        .take(obs_rs_ui::MAX_CANVAS_SELECTIONS)
        .collect()
}

/// Returns the first hit that a plain preview click may select.
///
/// OBS walks down through already-selected hits when a normal click lands on
/// a stack of overlapping sources. Ctrl-click calls the same hit test with
/// `select_below = false`, preserving the topmost-toggle behavior.
fn first_selectable_hit<'a, I>(hits: I, select_below: bool) -> Option<&'a str>
where
    I: IntoIterator<Item = (&'a str, bool)>,
{
    hits.into_iter().find_map(|(id, selected)| {
        if select_below && selected {
            None
        } else {
            Some(id)
        }
    })
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

/// Returns whether the selected source is locked against editing.
fn selected_is_locked(state: &Rc<RefCell<DesktopState>>) -> bool {
    let state = state.borrow();
    let Some(scene) = state.preview_scene() else {
        return false;
    };
    let session = state.project_session();
    session
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene))
        .is_some_and(|scene| {
            scene
                .items()
                .iter()
                .filter(|item| state.is_source_selected(item.id().as_str()))
                .any(SceneItemSpec::locked)
        })
}

/// Returns the topmost visible source covering a canvas point.
fn source_at(
    state: &Rc<RefCell<DesktopState>>,
    canvas: (u32, u32),
    x: i64,
    y: i64,
    select_below: bool,
) -> Option<String> {
    let state = state.borrow();
    let scene = state.preview_scene()?;
    let session = state.project_session();
    let scene = session.project().active_profile_spec()?.scene(scene)?;
    first_selectable_hit(
        scene.items().iter().rev().filter_map(|item| {
            if !item.visible() || !item_rect(item.transform(), canvas).contains(x, y) {
                return None;
            }
            Some((
                item.id().as_str(),
                state.is_source_selected(item.id().as_str()),
            ))
        }),
        select_below,
    )
    .map(str::to_owned)
}

/// Builds the bounded canvas and source guides for one transform gesture.
fn scene_snap_guides(
    state: &Rc<RefCell<DesktopState>>,
    scene_id: &str,
    excluded: &[TransformDraftItem],
    canvas: (u32, u32),
) -> SnapGuides {
    let mut guides = SnapGuides::with_canvas(canvas);
    let state = state.borrow();
    let Some(scene) = state
        .project_session()
        .project()
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
    else {
        return guides;
    };
    for item in scene.items() {
        if excluded
            .iter()
            .any(|selected| selected.item == item.id().as_str())
            || !item.visible()
        {
            continue;
        }
        guides.push_rect(item_rect(item.transform(), canvas));
        if guides.x_len == MAX_SNAP_GUIDES && guides.y_len == MAX_SNAP_GUIDES {
            break;
        }
    }
    guides
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
        assert_eq!(CanvasZoom::Percent(63).stepped(-1), CanvasZoom::Percent(50));
        assert_eq!(CanvasZoom::Percent(63).stepped(1), CanvasZoom::Percent(100));
    }

    #[test]
    fn wheel_zoom_is_continuous_and_bounded() {
        assert_eq!(
            CanvasZoom::FitToWindow.wheel(1, 500_000),
            CanvasZoom::Percent(63)
        );
        assert_eq!(
            CanvasZoom::Percent(63).wheel(-1, 1_000_000),
            CanvasZoom::Percent(50)
        );
        assert_eq!(
            CanvasZoom::Percent(MIN_ZOOM_PERCENT).wheel(-1, 1_000_000),
            CanvasZoom::Percent(MIN_ZOOM_PERCENT)
        );
        assert_eq!(
            CanvasZoom::Percent(MAX_ZOOM_PERCENT).wheel(1, 1_000_000),
            CanvasZoom::Percent(MAX_ZOOM_PERCENT)
        );
    }

    #[test]
    fn cursor_anchored_zoom_preserves_the_canvas_point_under_the_pointer() {
        let state = CanvasState::default()
            .with_zoom(CanvasZoom::Percent(100))
            .zoomed_at(1, (400, 250), (500, 300), (100, 50), 1_000_000);

        assert_eq!(state.zoom(), CanvasZoom::Percent(125));
        assert_eq!(state.pan(), (-80, -50));

        let new_scale = i64::from(state.zoom().ui_value()) * SCALE_MICROS_PER_PERCENT;
        let new_origin_x = 100_i64 * SCALE_MICROS_PER_UNIT + i64::from(state.pan().0) * new_scale;
        let new_origin_y = 50_i64 * SCALE_MICROS_PER_UNIT + i64::from(state.pan().1) * new_scale;
        assert_eq!(
            new_origin_x + 400_i64 * new_scale,
            500_i64 * SCALE_MICROS_PER_UNIT
        );
        assert_eq!(
            new_origin_y + 250_i64 * new_scale,
            300_i64 * SCALE_MICROS_PER_UNIT
        );
    }

    #[test]
    fn canvas_state_keeps_viewport_state_outside_the_project_transform() {
        let state = CanvasState::default()
            .with_zoom(CanvasZoom::Percent(100))
            .with_snapping(SnapSettings {
                enabled: false,
                distance: 4,
            })
            .panned(24, -12);

        assert_eq!(state.zoom().ui_value(), 100);
        assert_eq!(state.pan(), (24, -12));
        assert_eq!(state.snapping().distance, 4);

        let bounded = state.panned(i32::MAX, i32::MIN);
        assert_eq!(bounded.pan(), (MAX_PAN_PIXELS, -MAX_PAN_PIXELS));
    }

    #[test]
    fn snap_distance_is_bounded_at_the_canvas_boundary() {
        assert_eq!(
            CanvasState::default()
                .with_snap_distance(0)
                .snapping
                .distance,
            i64::from(*CANVAS_SNAP_DISTANCE_RANGE.start())
        );
        assert_eq!(
            CanvasState::default()
                .with_snap_distance(u16::MAX)
                .snapping
                .distance,
            i64::from(*CANVAS_SNAP_DISTANCE_RANGE.end())
        );

        let controller = CanvasController {
            draft: RefCell::new(None),
            state: RefCell::new(CanvasState::default()),
        };
        assert_eq!(
            controller.set_snap_distance(24).snapping.distance,
            24,
            "the controller applies the settings snapshot to its one live policy"
        );
    }

    #[test]
    fn safe_area_guides_match_ebu_r95_reference_margins() {
        let guides = SnapGuides::with_canvas(CANVAS);
        let x = &guides.x[..guides.x_len];
        let y = &guides.y[..guides.y_len];

        assert!(x.contains(&67), "3.5% action-safe left edge");
        assert!(x.contains(&1_853), "3.5% action-safe right edge");
        assert!(x.contains(&96), "5% graphics-safe left edge");
        assert!(x.contains(&1_824), "5% graphics-safe right edge");
        assert!(x.contains(&312), "16.25% 4:3-safe left edge");
        assert!(x.contains(&1_608), "16.25% 4:3-safe right edge");
        assert!(y.contains(&38), "3.5% action-safe top edge");
        assert!(y.contains(&1_042), "3.5% action-safe bottom edge");
        assert!(y.contains(&54), "5% graphics-safe top edge");
        assert!(y.contains(&1_026), "5% graphics-safe bottom edge");
    }

    #[test]
    fn snapping_aligns_moves_and_resizes_to_bounded_guides() {
        let mut guides = SnapGuides::with_canvas(CANVAS);
        guides.push_rect(ItemRect {
            x: 700,
            y: 200,
            width: 100,
            height: 100,
        });
        let settings = SnapSettings::default();

        let near_edge = snap_rect(
            ItemRect {
                x: 6,
                y: 50,
                width: 400,
                height: 300,
            },
            0,
            &guides,
            settings,
        );
        assert_eq!(near_edge.x, 0, "the left edge should snap to the canvas");

        let near_safe_area = snap_rect(
            ItemRect {
                x: 90,
                y: 100,
                width: 200,
                height: 100,
            },
            0,
            &SnapGuides::with_canvas(CANVAS),
            settings,
        );
        assert_eq!(
            near_safe_area.x, 96,
            "the left edge should snap to the graphics-safe area"
        );

        let near_other = snap_rect(
            ItemRect {
                x: 296,
                y: 50,
                width: 400,
                height: 300,
            },
            0,
            &guides,
            settings,
        );
        assert_eq!(
            near_other.x, 300,
            "the right edge should snap to another source"
        );

        let resized = snap_rect(drag_rect(rect(), 8, -96, 0), 8, &guides, settings);
        assert_eq!((resized.x, resized.width), (0, 500));

        let unchanged = snap_rect(
            ItemRect {
                x: 6,
                y: 50,
                width: 400,
                height: 300,
            },
            0,
            &guides,
            SnapSettings {
                enabled: false,
                distance: 10,
            },
        );
        assert_eq!(unchanged.x, 6);
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
    fn ordinary_resize_preserves_aspect_and_shift_allows_free_resize() {
        let base = rect();
        let raw = drag_rect(base, 5, 100, 10);
        let preserved = preserve_resize_aspect(base, raw, 5, 4, 3);
        assert_eq!(
            preserved,
            ItemRect {
                x: 100,
                y: 50,
                width: 500,
                height: 375,
            }
        );

        let raw_side = drag_rect(base, 4, 100, 0);
        let preserved_side = preserve_resize_aspect(base, raw_side, 4, 4, 3);
        assert_eq!(
            preserved_side,
            ItemRect {
                x: 100,
                y: 13,
                width: 500,
                height: 375,
            }
        );

        let free = raw;
        assert_eq!((free.width, free.height), (500, 310));
        assert_eq!(
            CanvasResizeModifiers::from_mask(0),
            CanvasResizeModifiers {
                preserve_aspect: true,
                snapping: true,
            }
        );
        assert_eq!(
            CanvasResizeModifiers::from_mask(RESIZE_MODIFIER_SHIFT),
            CanvasResizeModifiers {
                preserve_aspect: false,
                snapping: true,
            }
        );
        assert_eq!(
            CanvasResizeModifiers::from_mask(RESIZE_MODIFIER_CONTROL),
            CanvasResizeModifiers {
                preserve_aspect: true,
                snapping: false,
            }
        );
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
    fn alt_handle_crop_changes_source_edges_without_losing_the_opposite_edge() {
        let base = FrameTransform::IDENTITY;
        let left = crop_transform(base, 8, 100, 0, CANVAS);
        assert_eq!(left.crop_left(), 100);
        assert_eq!(left.crop_right(), 0);
        assert_eq!(left.translate_x(), 100);
        assert_eq!(
            item_rect(left, CANVAS).x + item_rect(left, CANVAS).width,
            1_920
        );

        let right = crop_transform(base, 4, -100, 0, CANVAS);
        assert_eq!(right.crop_left(), 0);
        assert_eq!(right.crop_right(), 100);
        assert_eq!(right.translate_x(), 0);
        assert_eq!(item_rect(right, CANVAS).width, 1_820);
    }

    #[test]
    fn rotated_alt_crop_maps_pointer_deltas_into_source_axes() {
        let base = FrameTransform::new(1_000, 1_000, 100, 50, false, false, 255)
            .expect("transform")
            .with_rotation_degrees(90)
            .expect("rotation");

        // At 90 degrees, source-local +X points down on the canvas. Moving
        // the left crop edge down therefore retains the opposite edge by
        // moving the visible rectangle down in canvas space.
        let left = crop_transform(base, 8, 0, 100, CANVAS);
        assert_eq!(left.crop_left(), 100);
        assert_eq!(left.crop_top(), 0);
        assert_eq!((left.translate_x(), left.translate_y()), (100, 150));

        // Source-local +Y points left on the canvas at the same rotation.
        let top = crop_transform(base, 2, -100, 0, CANVAS);
        assert_eq!(top.crop_left(), 0);
        assert_eq!(top.crop_top(), 100);
        assert_eq!((top.translate_x(), top.translate_y()), (0, 50));
    }

    #[test]
    fn rotation_uses_the_rotated_axis_aligned_bounds_for_selection() {
        let transform = FrameTransform::new(1_000, 1_000, 100, 50, false, false, 255)
            .expect("transform")
            .with_rotation_degrees(90)
            .expect("rotation");

        assert_eq!(
            item_rect(transform, CANVAS),
            ItemRect {
                x: 520,
                y: -370,
                width: 1_080,
                height: 1_920,
            }
        );
    }

    #[test]
    fn rotated_selection_overlay_uses_oriented_handles() {
        let transform = FrameTransform::new(500, 500, 100, 50, false, false, 255)
            .expect("transform")
            .with_rotation_degrees(90)
            .expect("rotation");
        let overlay = selection_overlay_for_transforms(&[transform], (400, 300))
            .expect("one transform should create an overlay");

        assert_eq!(
            overlay.rect,
            ItemRect {
                x: 125,
                y: 25,
                width: 150,
                height: 200
            }
        );
        assert_eq!(overlay.handle_x, [275, 275, 275, 200, 125, 125, 125, 200]);
        assert_eq!(overlay.handle_y, [25, 125, 225, 225, 225, 125, 25, 25]);
        assert_eq!(overlay.path, "M 275 25 L 275 225 L 125 225 L 125 25 Z");
    }

    #[test]
    fn rotated_resize_maps_pointer_to_local_axes_and_keeps_opposite_corner_fixed() {
        let base = FrameTransform::new(500, 500, 100, 50, false, false, 255)
            .expect("transform")
            .with_rotation_degrees(90)
            .expect("rotation");
        let canvas = (400, 300);
        let old_handles = oriented_handle_points(base, canvas);
        assert_eq!(rotate_canvas_delta(30, 0, 90_000, true), (0, -30));

        let local = drag_rect(local_item_rect(base, canvas), 1, 0, -30);
        let resized = transform_for_rotated_local_rect(base, local, 1, canvas);
        let new_handles = oriented_handle_points(resized, canvas);

        assert_eq!(resized.scale_x_milli(), 500);
        assert_eq!(resized.scale_y_milli(), 600);
        assert_eq!((resized.translate_x(), resized.translate_y()), (115, 35));
        assert_eq!(new_handles[4], old_handles[4]);
    }

    #[test]
    fn multi_selection_overlay_keeps_axis_aligned_group_handles() {
        let first = FrameTransform::new(250, 250, 20, 30, false, false, 255).expect("transform");
        let second = FrameTransform::new(250, 250, 160, 100, false, false, 255).expect("transform");
        let overlay = selection_overlay_for_transforms(&[first, second], (400, 300))
            .expect("two transforms should create an overlay");

        assert_eq!(
            overlay.rect,
            ItemRect {
                x: 20,
                y: 30,
                width: 240,
                height: 145
            }
        );
        assert_eq!(overlay.handle_x, [20, 140, 260, 260, 260, 140, 20, 20]);
        assert_eq!(overlay.handle_y, [30, 30, 30, 102, 175, 175, 175, 102]);
    }

    #[test]
    fn transform_commands_apply_screen_sizing_and_alignment() {
        let canvas = (1_280, 720);
        let base = FrameTransform::IDENTITY
            .with_crop(140, 0, 140, 0)
            .expect("crop")
            .with_rotation_degrees(0)
            .expect("rotation");

        let fit = transform_for_command(base, CanvasTransformCommand::FitToScreen, canvas);
        assert_eq!(fit.scale_x_milli(), 1_000);
        assert_eq!(fit.scale_y_milli(), 1_000);
        assert_eq!(fit.translate_x(), 140);
        assert_eq!(fit.translate_y(), 0);

        let stretch = transform_for_command(base, CanvasTransformCommand::StretchToScreen, canvas);
        assert_eq!(stretch.scale_x_milli(), 1_280);
        assert_eq!(stretch.scale_y_milli(), 1_000);
        assert_eq!(stretch.translate_x(), 0);
        assert_eq!(stretch.translate_y(), 0);

        let positioned =
            FrameTransform::new(500, 250, 100, 50, true, false, 180).expect("transform");
        let centered =
            transform_for_command(positioned, CanvasTransformCommand::CenterToScreen, canvas);
        assert_eq!(
            item_rect(centered, canvas),
            ItemRect {
                x: 320,
                y: 270,
                width: 640,
                height: 180,
            }
        );
        assert!(centered.flip_x());
        assert_eq!(centered.opacity(), 180);

        let right = transform_for_command(positioned, CanvasTransformCommand::AlignRight, canvas);
        assert_eq!(item_rect(right, canvas).x, 640);
        let bottom = transform_for_command(positioned, CanvasTransformCommand::AlignBottom, canvas);
        assert_eq!(item_rect(bottom, canvas).y, 540);
    }

    #[test]
    fn transform_command_parser_rejects_untrusted_actions() {
        assert_eq!(
            CanvasTransformCommand::from_action("fit-screen"),
            Some(CanvasTransformCommand::FitToScreen)
        );
        assert_eq!(CanvasTransformCommand::from_action("delete-project"), None);
    }

    #[test]
    fn selection_box_normalizes_reverse_drag_and_group_mapping() {
        let state = CanvasState::default().begin_selection(400, 300, true);
        let state = state.update_selection(100, 50);
        assert_eq!(
            state.selection_box(),
            Some(ItemRect {
                x: 100,
                y: 50,
                width: 300,
                height: 250,
            })
        );
        let mapped = map_rect_into_group(
            ItemRect {
                x: 200,
                y: 150,
                width: 100,
                height: 100,
            },
            ItemRect {
                x: 100,
                y: 50,
                width: 400,
                height: 300,
            },
            ItemRect {
                x: 200,
                y: 100,
                width: 800,
                height: 600,
            },
        );
        assert_eq!(
            mapped,
            ItemRect {
                x: 400,
                y: 300,
                width: 200,
                height: 200,
            }
        );
        assert!(ItemRect {
            x: 400,
            y: 300,
            width: 10,
            height: 10,
        }
        .intersects(ItemRect {
            x: 405,
            y: 305,
            width: 10,
            height: 10,
        }));
    }

    #[test]
    fn hit_testing_uses_the_item_rectangle() {
        let rect = rect();

        assert!(rect.contains(100, 50));
        assert!(rect.contains(499, 349));
        assert!(!rect.contains(99, 50));
        assert!(!rect.contains(500, 350));
    }

    #[test]
    fn plain_preview_selection_skips_selected_hits_to_reach_underneath() {
        let hits = [("top", true), ("middle", true), ("bottom", false)];
        assert_eq!(
            first_selectable_hit(hits.iter().copied(), true),
            Some("bottom")
        );
        assert_eq!(
            first_selectable_hit(hits.iter().copied(), false),
            Some("top")
        );

        let hits = [("top", false), ("underneath", true)];
        assert_eq!(
            first_selectable_hit(hits.iter().copied(), true),
            Some("top")
        );

        let hits = [("top", true)];
        assert_eq!(first_selectable_hit(hits.iter().copied(), true), None);
    }

    /// Measures the bounded group-geometry work used for every multi-select
    /// pointer sample. The report is ignored so it can be run on release
    /// builds without becoming a machine-dependent pass/fail gate.
    #[test]
    #[ignore = "timing report, not a pass/fail assertion"]
    fn multi_selection_geometry_timing_report() {
        use std::time::Instant;

        let mut items = (0..16)
            .map(|index| TransformDraftItem {
                item: format!("item_{index}"),
                transform: FrameTransform::new(
                    400 + index * 20,
                    350 + index * 15,
                    i32::try_from(index * 73).expect("translation"),
                    i32::try_from(index * 41).expect("translation"),
                    false,
                    false,
                    255,
                )
                .expect("transform"),
            })
            .collect::<Vec<_>>();
        let runs = 200;
        let started = Instant::now();
        let mut checksum = 0_i64;
        for _ in 0..runs {
            let group = items
                .iter()
                .map(|item| item_rect(item.transform, CANVAS))
                .reduce(ItemRect::union)
                .expect("group");
            let moved = drag_rect(group, 0, 12, -7);
            for item in &mut items {
                let old = item_rect(item.transform, CANVAS);
                let next = ItemRect {
                    x: old.x.saturating_add(moved.x.saturating_sub(group.x)),
                    y: old.y.saturating_add(moved.y.saturating_sub(group.y)),
                    ..old
                };
                item.transform = transform_for_rect(item.transform, next, CANVAS);
                checksum = checksum.saturating_add(i64::from(item.transform.translate_x()));
            }
        }
        println!(
            "multi-selection: items={} runs={} per_sample={:?} checksum={checksum}",
            items.len(),
            runs,
            started.elapsed() / runs
        );
    }
}
