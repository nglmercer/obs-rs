//! Interactive scene-item editing on the preview canvas.
//!
//! OBS positions a source by dragging it on the canvas rather than by typing a
//! transform, so this turns pointer deltas into
//! [`obs_rs_media::FrameTransform`] edits. Drags mutate a draft that the
//! preview renders immediately; the project command is dispatched once, when
//! the pointer is released, so a drag is one undoable edit rather than a
//! hundred and the project revision does not churn per mouse move.

use super::{ItemRect, CANVAS_SNAP_DISTANCE_DEFAULT, CANVAS_SNAP_DISTANCE_RANGE};

/// The smallest on-canvas size a drag may leave a source at.
///
/// A scene item shrunk to nothing has no handles left to grab, so resizing
/// stops here rather than letting the item become unrecoverable.
pub(super) const MINIMUM_ITEM_PIXELS: i64 = 16;
pub(super) const MAX_PAN_PIXELS: i32 = 16_384;
pub(super) const MAX_SNAP_GUIDES: usize = 64;
// Rec. ITU-R BT.1848-1 / EBU R 95 safe-area margins, represented as
// numerator / SAFE_AREA_DENOMINATOR so snapping stays deterministic and does
// not introduce floating-point geometry into pointer gestures.
pub(super) const SAFE_AREA_DENOMINATOR: i64 = 2_000;
pub(super) const ACTION_SAFE_INSET: i64 = 70; // 3.5%
pub(super) const GRAPHICS_SAFE_INSET: i64 = 100; // 5.0%
pub(super) const FOUR_BY_THREE_SAFE_X_INSET: i64 = 325; // 16.25%
pub(super) const MIN_ZOOM_PERCENT: u16 = 10;
pub(super) const MAX_ZOOM_PERCENT: u16 = 800;
pub(super) const WHEEL_ZOOM_FACTOR_MILLI: i64 = 1_250;
pub(super) const SCALE_MICROS_PER_PERCENT: i64 = 10_000;
pub(super) const SCALE_MICROS_PER_UNIT: i64 = 1_000_000;
pub(super) const RESIZE_MODIFIER_SHIFT: i32 = 1;
pub(super) const RESIZE_MODIFIER_CONTROL: i32 = 2;
pub(super) const RESIZE_MODIFIER_ALT: i32 = 4;

/// The scale a transform stores for a source that fills the canvas.
pub(super) const UNIT_SCALE_MILLI: i64 = 1_000;

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
    pub(super) enabled: bool,
    pub(super) distance: i64,
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
/// opt into free resizing. Ctrl suppresses snapping for the gesture, while
/// Alt changes a handle drag into source cropping. The policy is copied out of
/// the toolkit event at the boundary so the geometry functions remain
/// deterministic and testable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CanvasResizeModifiers {
    pub(super) preserve_aspect: bool,
    pub(super) snapping: bool,
    pub(super) crop: bool,
}

impl CanvasResizeModifiers {
    pub(super) fn from_mask(mask: i32) -> Self {
        Self {
            preserve_aspect: mask & RESIZE_MODIFIER_SHIFT == 0,
            snapping: mask & RESIZE_MODIFIER_CONTROL == 0,
            crop: mask & RESIZE_MODIFIER_ALT != 0,
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
    pub(super) snapping: SnapSettings,
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

    pub(super) fn begin_selection(self, x: i64, y: i64, additive: bool) -> Self {
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

    pub(super) fn update_selection(self, x: i64, y: i64) -> Self {
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

    pub(super) fn clear_selection(self) -> Self {
        Self {
            selection_anchor: None,
            selection_box: None,
            selection_additive: false,
            ..self
        }
    }
}
