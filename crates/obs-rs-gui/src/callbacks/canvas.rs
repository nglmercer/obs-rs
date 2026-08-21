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
const MAX_PAN_PIXELS: i32 = 16_384;
const MAX_SNAP_GUIDES: usize = 64;

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
            distance: 10,
        }
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
    snapping: SnapSettings,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            zoom: CanvasZoom::FitToWindow,
            pan: (0, 0),
            snapping: SnapSettings::default(),
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

    #[allow(
        dead_code,
        reason = "the future snap-distance control will update this transient policy"
    )]
    pub(crate) const fn with_snapping(self, snapping: SnapSettings) -> Self {
        Self { snapping, ..self }
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
pub(crate) fn item_rect(transform: FrameTransform, canvas: (u32, u32)) -> ItemRect {
    let (source_width, source_height) = visible_source_extent(transform, canvas);
    let width = (source_width * i64::from(transform.scale_x_milli()) / UNIT_SCALE_MILLI).max(1);
    let height = (source_height * i64::from(transform.scale_y_milli()) / UNIT_SCALE_MILLI).max(1);
    if transform.is_rotated() {
        rotated_bounds(transform, width, height)
    } else {
        ItemRect {
            x: i64::from(transform.translate_x()),
            y: i64::from(transform.translate_y()),
            width,
            height,
        }
    }
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

/// Applies an Alt-drag on one of the eight transform handles as a source
/// crop. The handle number is the ordinary 1-8 clockwise handle number.
fn crop_transform(
    base: FrameTransform,
    handle: i32,
    dx: i64,
    dy: i64,
    canvas: (u32, u32),
) -> FrameTransform {
    // Axis-aligned crop handles are unambiguous before rotation. The dialog
    // remains the explicit editing path for a rotated item until a rotated
    // handle overlay is added.
    if base.is_rotated() {
        return base;
    }
    let scale_source_delta = |delta: i64, scale: u32| {
        delta
            .saturating_mul(UNIT_SCALE_MILLI)
            .div_euclid(i64::from(scale.max(1)))
    };
    let mut left = i64::from(base.crop_left());
    let mut top = i64::from(base.crop_top());
    let mut right = i64::from(base.crop_right());
    let mut bottom = i64::from(base.crop_bottom());
    let delta_x = scale_source_delta(dx, base.scale_x_milli());
    let delta_y = scale_source_delta(dy, base.scale_y_milli());
    let max_left = (i64::from(canvas.0) - right - 1).max(0);
    let max_right = (i64::from(canvas.0) - left - 1).max(0);
    let max_top = (i64::from(canvas.1) - bottom - 1).max(0);
    let max_bottom = (i64::from(canvas.1) - top - 1).max(0);
    let mut translate_x = i64::from(base.translate_x());
    let mut translate_y = i64::from(base.translate_y());
    if matches!(handle, 1 | 7 | 8) {
        let next = (left + delta_x).clamp(0, max_left);
        let actual = next - left;
        left = next;
        translate_x = translate_x.saturating_add(
            actual
                .saturating_mul(i64::from(base.scale_x_milli()))
                .div_euclid(UNIT_SCALE_MILLI),
        );
    }
    if matches!(handle, 1..=3) {
        let next = (top + delta_y).clamp(0, max_top);
        let actual = next - top;
        top = next;
        translate_y = translate_y.saturating_add(
            actual
                .saturating_mul(i64::from(base.scale_y_milli()))
                .div_euclid(UNIT_SCALE_MILLI),
        );
    }
    if matches!(handle, 3..=5) {
        right = (right - delta_x).clamp(0, max_right);
    }
    if matches!(handle, 5..=7) {
        bottom = (bottom - delta_y).clamp(0, max_bottom);
    }
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
        let transform = if (9..=16).contains(&handle) {
            crop_transform(base, handle - 8, i64::from(dx), i64::from(dy), canvas)
        } else {
            let rect = drag_rect(
                item_rect(base, canvas),
                handle,
                i64::from(dx),
                i64::from(dy),
            );
            let rect = snap_rect(
                rect,
                handle,
                &scene_snap_guides(&drag_state, &scene, &item, canvas),
                drag_controller.canvas_state().snapping(),
            );
            transform_for_rect(base, rect, canvas)
        };
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

/// Builds the bounded canvas and source guides for one transform gesture.
fn scene_snap_guides(
    state: &Rc<RefCell<DesktopState>>,
    scene_id: &str,
    item_id: &str,
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
        if item.id().as_str() == item_id || !item.visible() {
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
    fn hit_testing_uses_the_item_rectangle() {
        let rect = rect();

        assert!(rect.contains(100, 50));
        assert!(rect.contains(499, 349));
        assert!(!rect.contains(99, 50));
        assert!(!rect.contains(500, 350));
    }
}
