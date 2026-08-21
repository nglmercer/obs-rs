//! Dock layout: tree-backed reordering, width shares, and detached windows.
//!
//! The legacy Slint row still consumes parallel models, but every reorder or
//! splitter edit is mirrored into the bounded `DockNode` tree. Settings can
//! therefore migrate to splits and tabs without making the window another
//! source of layout truth.

use std::{cell::RefCell, rc::Rc};

use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{dock_tree::DockNode, FloatingDockWindow, MainWindow};

/// The dock kinds, in the order their IDs are numbered.
pub(crate) const PANEL_KINDS: [i32; 5] = [0, 1, 2, 3, 4];

/// Narrowest and widest share a dock may be dragged to.
///
/// Without a floor a splitter drag can push a neighbour to zero width, leaving
/// no strip to grab and no way back.
const MINIMUM_WEIGHT: f32 = 0.35;
const MAXIMUM_WEIGHT: f32 = 6.0;

/// Pixels of drag that change a dock's share by one unit of weight.
///
/// The row is laid out by stretch factors rather than pixels, so a drag is
/// converted at a fixed rate that feels like direct manipulation at the dock
/// sizes OBS-RS ships with.
const PIXELS_PER_WEIGHT: f32 = 320.0;

/// Owns the detached dock windows, one per dock kind.
pub(crate) struct DockController {
    windows: RefCell<Vec<Option<FloatingDockWindow>>>,
    tree: RefCell<DockNode>,
}

impl DockController {
    /// Repaints every open floating dock when the studio theme changes.
    pub(crate) fn set_tokens(&self, tokens: &crate::ThemeTokens) {
        for window in self.windows.borrow().iter().flatten() {
            window.global::<crate::Palette>().set_tokens(tokens.clone());
        }
    }

    /// Mirrors the studio's dock data into every open floating dock.
    ///
    /// A detached dock is the same view of the same state, so it refreshes from
    /// the same models the row uses instead of keeping its own copy.
    pub(crate) fn sync(&self, ui: &MainWindow) {
        for window in self.windows.borrow().iter().flatten() {
            window.set_locale(ui.get_locale());
            window.set_scene_rows(ui.get_scene_rows());
            window.set_source_rows(ui.get_source_rows());
            window.set_mixer_rows(ui.get_mixer_rows());
            window.set_source_scene(ui.get_source_scene());
            window.set_preview_scene(ui.get_preview_scene());
            window.set_selected_source(ui.get_selected_source());
            window.set_selected_source_is_screen(ui.get_selected_source_is_screen());
            window.set_selected_source_visible(ui.get_selected_source_visible());
            window.set_selected_source_locked(ui.get_selected_source_locked());
            window.set_selected_source_first(ui.get_selected_source_first());
            window.set_selected_source_last(ui.get_selected_source_last());
            window.set_can_paste(ui.get_can_paste());
            window.set_transition(ui.get_transition());
            window.set_recording(ui.get_recording());
            window.set_streaming(ui.get_streaming());
            window.set_meters_paused(ui.get_meters_paused());
        }
    }

    #[cfg(test)]
    pub(crate) fn is_floating(&self, kind: i32) -> bool {
        usize::try_from(kind)
            .is_ok_and(|kind| self.windows.borrow().get(kind).is_some_and(Option::is_some))
    }
}

/// Moves `panel` one place along the row, returning the new order.
///
/// Hidden and floating docks still hold their place, so a reorder is a plain
/// swap and the row does not rearrange itself when a dock is shown again.
pub(crate) fn reorder(order: &[i32], panel: i32, direction: i32) -> Option<Vec<i32>> {
    if direction == 0 {
        return None;
    }
    let index = order.iter().position(|value| *value == panel)?;
    let target = if direction < 0 {
        index.checked_sub(1)?
    } else {
        index + 1
    };
    if target >= order.len() {
        return None;
    }
    let mut order = order.to_vec();
    order.swap(index, target);
    Some(order)
}

/// Applies a splitter drag between the docks either side of `index`.
///
/// The two neighbours trade share, so the row's total width is unchanged and
/// the docks beyond the splitter do not move.
pub(crate) fn resize(weights: &[f32], order: &[i32], index: usize, delta_pixels: i32) -> Vec<f32> {
    let mut weights = weights.to_vec();
    let (Some(left), Some(right)) = (
        index
            .checked_sub(1)
            .and_then(|index| order.get(index))
            .and_then(|kind| usize::try_from(*kind).ok()),
        order
            .get(index)
            .and_then(|kind| usize::try_from(*kind).ok()),
    ) else {
        return weights;
    };
    let (Some(left_weight), Some(right_weight)) =
        (weights.get(left).copied(), weights.get(right).copied())
    else {
        return weights;
    };
    #[allow(
        clippy::cast_precision_loss,
        reason = "a drag is a few hundred pixels, far inside f32's exact range"
    )]
    let delta = delta_pixels as f32 / PIXELS_PER_WEIGHT;
    // Clamping the delta rather than the results keeps the pair's total fixed,
    // so the rest of the row never shifts.
    let delta = delta
        .min(right_weight - MINIMUM_WEIGHT)
        .max(MINIMUM_WEIGHT - left_weight)
        .min(MAXIMUM_WEIGHT - left_weight)
        .max(right_weight - MAXIMUM_WEIGHT);
    weights[left] = left_weight + delta;
    weights[right] = right_weight - delta;
    weights
}

/// Installs the dock reorder, resize, and detach callbacks.
///
/// A detached dock forwards every action to the studio window, so this needs
/// only the studio state the window titles are localized from — not the
/// surface or the output runtime.
pub(crate) fn install_dock_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
) -> Rc<DockController> {
    let initial_order = read_ints(&ui.get_panel_order());
    let initial_weights = read_floats(&ui.get_panel_weights());
    let tree = DockNode::from_legacy(&initial_order, &initial_weights).unwrap_or_else(|| {
        DockNode::from_legacy(&[1, 0, 2, 3, 4], &[1.0, 1.0, 1.85, 1.0, 1.4])
            .expect("the built-in dock layout must be valid")
    });
    let controller = Rc::new(DockController {
        windows: RefCell::new(PANEL_KINDS.map(|_| None).into_iter().collect()),
        tree: RefCell::new(tree),
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_move_panel(move |panel, direction| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let order = read_ints(&ui.get_panel_order());
        if let Some(order) = reorder(&order, panel, direction) {
            let weights = read_floats(&ui.get_panel_weights());
            if let Some(tree) = DockNode::from_legacy(&order, &weights) {
                *tree_controller.tree.borrow_mut() = tree.clone();
                ui.set_panel_order(ModelRc::new(VecModel::from(tree.leaf_order())));
            }
        }
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_resize_panel(move |index, delta| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let weights = read_floats(&ui.get_panel_weights());
        let order = read_ints(&ui.get_panel_order());
        let weights = resize(&weights, &order, index, delta);
        if let Some(tree) = DockNode::from_legacy(&order, &weights) {
            *tree_controller.tree.borrow_mut() = tree;
        }
        ui.set_panel_weights(ModelRc::new(VecModel::from(weights)));
    });

    let weak = ui.as_weak();
    ui.on_toggle_meters_paused(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_meters_paused(!ui.get_meters_paused());
        }
    });

    install_float(ui, state, &controller);
    controller
}

fn install_float(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<DockController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let controller = Rc::clone(controller);
    ui.on_float_panel(move |kind| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(index) = usize::try_from(kind) else {
            return;
        };
        let open = controller
            .windows
            .borrow()
            .get(index)
            .is_some_and(Option::is_some);
        if open {
            redock(&ui, &controller, index);
            return;
        }
        match float(&ui, &state, &controller, kind) {
            Ok(window) => {
                controller.windows.borrow_mut()[index] = Some(window);
                set_floating(&ui, index, true);
                controller.sync(&ui);
            }
            Err(error) => ui.set_status_message(format!("Floating dock: {error}").into()),
        }
    });
}

/// Returns a dock to the row and closes its window.
fn redock(ui: &MainWindow, controller: &Rc<DockController>, index: usize) {
    if let Some(window) = controller.windows.borrow_mut()[index].take() {
        let _ = window.hide();
    }
    set_floating(ui, index, false);
}

/// Builds and shows the window for one dock kind.
fn float(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<DockController>,
    kind: i32,
) -> Result<FloatingDockWindow, slint::PlatformError> {
    let window = FloatingDockWindow::new()?;
    window
        .global::<crate::I18n>()
        .set_text(crate::i18n::catalog(state.borrow().locale()));
    window
        .global::<crate::Palette>()
        .set_tokens(ui.global::<crate::Palette>().get_tokens());
    window.set_panel_kind(kind);
    window.set_dock_title(dock_title(state, kind));

    forward_to_studio(&window, ui, controller);

    let weak = ui.as_weak();
    let redock_controller = Rc::clone(controller);
    let redock_index = usize::try_from(kind).unwrap_or(0);
    window.on_redock_panel(move |_| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        redock(&ui, &redock_controller, redock_index);
    });

    window.show()?;
    Ok(window)
}

/// Points a floating dock's callbacks at the studio window's own handlers.
///
/// Every dock action already has one implementation on the studio window, so a
/// detached dock forwards to it rather than installing a second copy that could
/// drift. After the studio has handled the action its models are current, so the
/// floating dock is re-synced from them immediately.
fn forward_to_studio(
    window: &FloatingDockWindow,
    ui: &MainWindow,
    controller: &Rc<DockController>,
) {
    /// Forwards one callback and refreshes the floating docks afterwards.
    macro_rules! forward {
        ($setter:ident, $invoke:ident $(, $argument:ident)*) => {{
            let weak = ui.as_weak();
            let controller = Rc::clone(controller);
            window.$setter(move |$($argument),*| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                ui.$invoke($($argument),*);
                controller.sync(&ui);
            });
        }};
    }

    forward!(on_select_preview, invoke_select_preview, id);
    forward!(on_select_program, invoke_select_program, id);
    forward!(on_duplicate_scene, invoke_duplicate_scene, id);
    forward!(on_remove_scene, invoke_remove_scene, id);
    forward!(on_select_source, invoke_select_source, id);
    forward!(
        on_toggle_source_visibility,
        invoke_toggle_source_visibility,
        id
    );
    forward!(on_toggle_source_locked, invoke_toggle_source_locked, id);
    forward!(on_move_source, invoke_move_source, id, delta);
    forward!(on_move_source_to, invoke_move_source_to, id, index);
    forward!(on_reset_source_transform, invoke_reset_source_transform, id);
    forward!(on_flip_source, invoke_flip_source, id, horizontal);
    forward!(on_transform_source, invoke_transform_source, id, action);
    forward!(on_open_source_rename, invoke_open_source_rename, id);
    forward!(on_duplicate_source, invoke_duplicate_source, id);
    forward!(on_copy_source, invoke_copy_source, id);
    forward!(on_paste_reference, invoke_paste_reference);
    forward!(on_paste_duplicate, invoke_paste_duplicate);
    forward!(on_remove_source, invoke_remove_source, id);
    forward!(on_set_mixer_gain, invoke_set_mixer_gain, id, gain);
    forward!(on_toggle_mixer_mute, invoke_toggle_mixer_mute, id);
    forward!(on_toggle_meters_paused, invoke_toggle_meters_paused);
    forward!(on_cut_transition, invoke_cut_transition);
    forward!(on_fade_transition, invoke_fade_transition);
    forward!(on_toggle_recording, invoke_toggle_recording);
    forward!(on_toggle_streaming, invoke_toggle_streaming);
    forward!(on_open_settings_window, invoke_open_settings_window);

    let weak = ui.as_weak();
    window.on_set_view_mode(move |mode| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // -1 toggles studio mode, matching the studio window's own handling.
        let current = ui.get_view_mode();
        ui.set_view_mode(if mode == -1 {
            i32::from(current == 0)
        } else {
            mode
        });
    });

    let weak = ui.as_weak();
    window.on_open_modal(move |modal| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Only the modals that are their own windows can be raised from a
        // detached dock; the in-window dialogs belong to the studio window.
        match modal {
            3 => ui.invoke_open_add_source_window(),
            6 => ui.invoke_open_source_properties_window(),
            13 => ui.invoke_open_source_filters_window(),
            14 => ui.invoke_open_source_transform_window(),
            8 => ui.invoke_open_monitor_window(),
            _ => {}
        }
    });
}

/// Returns the localized title for a floating dock's window.
fn dock_title(state: &Rc<RefCell<DesktopState>>, kind: i32) -> slint::SharedString {
    crate::i18n::with_catalog(state.borrow().locale(), |text| match kind {
        0 => text.scenes_title.clone(),
        1 => text.sources_title.clone(),
        2 => text.mixer_title.clone(),
        3 => text.transition_title.clone(),
        _ => text.controls_title.clone(),
    })
}

/// Marks one dock kind as detached or docked in the studio window.
fn set_floating(ui: &MainWindow, index: usize, floating: bool) {
    let mut flags = (0..ui.get_panel_floating().row_count())
        .filter_map(|row| ui.get_panel_floating().row_data(row))
        .collect::<Vec<_>>();
    if flags.len() < PANEL_KINDS.len() {
        flags.resize(PANEL_KINDS.len(), false);
    }
    if let Some(flag) = flags.get_mut(index) {
        *flag = floating;
    }
    ui.set_panel_floating(ModelRc::new(VecModel::from(flags)));
}

fn read_ints(model: &ModelRc<i32>) -> Vec<i32> {
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

fn read_floats(model: &ModelRc<f32>) -> Vec<f32> {
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: [i32; 5] = [1, 0, 2, 3, 4];

    #[test]
    fn a_dock_swaps_with_its_neighbour() {
        assert_eq!(reorder(&ORDER, 0, -1).expect("left"), [0, 1, 2, 3, 4]);
        assert_eq!(reorder(&ORDER, 0, 1).expect("right"), [1, 2, 0, 3, 4]);
    }

    #[test]
    fn a_dock_at_the_end_of_the_row_stays_put() {
        assert!(reorder(&ORDER, 1, -1).is_none(), "already leftmost");
        assert!(reorder(&ORDER, 4, 1).is_none(), "already rightmost");
        assert!(reorder(&ORDER, 2, 0).is_none(), "no direction");
        assert!(reorder(&ORDER, 9, 1).is_none(), "unknown dock");
    }

    #[test]
    fn a_splitter_drag_trades_width_between_its_neighbours() {
        let weights = [1.0, 1.0, 1.85, 1.0, 1.4];

        // Index 2 of the order sits between dock 0 and dock 2.
        let resized = resize(&weights, &ORDER, 2, 320);

        assert!((resized[0] - 2.0).abs() < 1e-5, "left dock grew");
        assert!((resized[2] - 0.85).abs() < 1e-5, "right dock shrank");
        let total: f32 = resized.iter().sum();
        assert!(
            (total - weights.iter().sum::<f32>()).abs() < 1e-5,
            "the row's total width must not change"
        );
    }

    #[test]
    fn a_dock_cannot_be_collapsed_out_of_reach() {
        let weights = [1.0, 1.0, 1.85, 1.0, 1.4];

        let resized = resize(&weights, &ORDER, 2, 10_000);

        assert!(resized[2] >= MINIMUM_WEIGHT - 1e-5);
        assert!(resized[0] <= MAXIMUM_WEIGHT + 1e-5);
    }

    #[test]
    fn a_drag_at_the_row_edge_changes_nothing() {
        let weights = [1.0, 1.0, 1.85, 1.0, 1.4];

        // There is no splitter before the first dock.
        assert_eq!(resize(&weights, &ORDER, 0, 200), weights);
    }
}
