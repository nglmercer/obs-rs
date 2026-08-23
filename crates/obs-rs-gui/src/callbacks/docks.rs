//! Dock layout: tree-backed reordering, width shares, and detached windows.
//!
//! The Slint workspace consumes the tree's bounded pane projection; the
//! parallel models remain a compatibility projection for the legacy reorder
//! and splitter callbacks. Settings can therefore migrate to splits and tabs
//! without making the window another source of layout truth.

use std::{cell::RefCell, rc::Rc};

use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, Model, ModelRc, PhysicalPosition, PhysicalSize, VecModel};

use crate::{
    dock_tree::{DockAxis, DockDropZone, DockNode},
    fixtures::{desktop_bounds, screen_monitors, DesktopBounds},
    settings::FloatingGeometry,
    DockPane, DockSplitter, FloatingDockWindow, MainWindow,
};

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
    floating_geometry: RefCell<Vec<Option<FloatingGeometry>>>,
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
            window.set_selected_source_is_group(ui.get_selected_source_is_group());
            window.set_selected_source_visible(ui.get_selected_source_visible());
            window.set_selected_source_locked(ui.get_selected_source_locked());
            window.set_selected_source_first(ui.get_selected_source_first());
            window.set_selected_source_last(ui.get_selected_source_last());
            window.set_source_count(ui.get_source_count());
            window.set_can_paste(ui.get_can_paste());
            window.set_transition(ui.get_transition());
            window.set_transition_kind(ui.get_transition_kind());
            window.set_recording(ui.get_recording());
            window.set_streaming(ui.get_streaming());
            window.set_remux_recovery_supported(ui.get_remux_recovery_supported());
            window.set_remux_recovery_running(ui.get_remux_recovery_running());
            window.set_meters_paused(ui.get_meters_paused());
        }
    }

    /// Returns the tree that must be persisted with the session settings.
    pub(crate) fn tree_snapshot(&self) -> DockNode {
        self.tree.borrow().clone()
    }

    /// Replaces the tree after a settings/menu action changed the visible
    /// legacy projection outside a dock gesture.
    pub(crate) fn replace_tree(&self, tree: &DockNode, ui: &MainWindow) {
        *self.tree.borrow_mut() = tree.clone();
        ui.set_panel_order(ModelRc::new(VecModel::from(tree.leaf_order())));
        set_panes(ui, tree);
    }

    /// Closes every detached window for the Docks > Reset Layout action and
    /// discards their saved positions. A reset must not leave invisible window
    /// owners alive after the main window says the panels are docked.
    pub(crate) fn reset_floating(&self, ui: &MainWindow) {
        for slot in self.windows.borrow_mut().iter_mut() {
            if let Some(window) = slot.take() {
                let _ = window.hide();
            }
        }
        self.floating_geometry.borrow_mut().fill(None);
        for index in 0..PANEL_KINDS.len() {
            set_floating(ui, index, false);
        }
    }

    /// Captures every open detached window and returns the bounded geometry
    /// records ready for settings persistence. Closed windows retain their
    /// last captured position so a later re-detach returns to the same place.
    pub(crate) fn capture_floating_geometry(&self) -> Vec<FloatingGeometry> {
        let mut geometry = self.floating_geometry.borrow().clone();
        for (index, window) in self.windows.borrow().iter().enumerate() {
            if let Some(window) = window {
                capture_window_geometry(&mut geometry, index, window);
            }
        }
        self.floating_geometry.borrow_mut().clone_from(&geometry);
        geometry.into_iter().flatten().collect()
    }

    fn stored_geometry(&self, index: usize) -> Option<FloatingGeometry> {
        self.floating_geometry
            .borrow()
            .get(index)
            .copied()
            .flatten()
    }

    fn remember_window_geometry(&self, index: usize, window: &FloatingDockWindow) {
        let mut geometry = self.floating_geometry.borrow_mut();
        capture_window_geometry(&mut geometry, index, window);
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
#[cfg(test)]
#[allow(
    clippy::too_many_lines,
    reason = "one callback installation boundary owns all dock mutations"
)]
pub(crate) fn install_dock_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
) -> Rc<DockController> {
    install_dock_callbacks_with_layout(ui, state, None, &[])
}

/// Installs dock callbacks from the tree stored in the session settings.
/// Legacy settings still pass `None` and are migrated from their row order and
/// weights by the same bounded constructor used by the compatibility path.
#[allow(
    clippy::too_many_lines,
    reason = "one callback installation boundary owns the complete dock gesture lifecycle"
)]
pub(crate) fn install_dock_callbacks_with_layout(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    saved_tree: Option<&DockNode>,
    saved_geometry: &[FloatingGeometry],
) -> Rc<DockController> {
    let initial_order = read_ints(&ui.get_panel_order());
    let initial_weights = read_floats(&ui.get_panel_weights());
    let tree = saved_tree
        .filter(|tree| tree.leaf_order().len() == PANEL_KINDS.len())
        .cloned()
        .or_else(|| DockNode::from_legacy(&initial_order, &initial_weights))
        .unwrap_or_else(|| {
            DockNode::from_legacy(&[1, 0, 2, 3, 4], &[1.0, 1.0, 1.85, 1.0, 1.4])
                .expect("the built-in dock layout must be valid")
        });
    let controller = Rc::new(DockController {
        windows: RefCell::new(PANEL_KINDS.map(|_| None).into_iter().collect()),
        tree: RefCell::new(tree),
        floating_geometry: RefCell::new(
            PANEL_KINDS
                .map(|panel| {
                    saved_geometry
                        .iter()
                        .find(|geometry| geometry.panel == panel)
                        .copied()
                })
                .into_iter()
                .collect(),
        ),
    });
    set_panes(ui, &controller.tree_snapshot());

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_move_panel(move |panel, direction| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let order = read_ints(&ui.get_panel_order());
        if !tree_controller.tree.borrow().is_flat_horizontal() {
            return;
        }
        if let Some(order) = reorder(&order, panel, direction) {
            let weights = read_floats(&ui.get_panel_weights());
            if let Some(tree) = DockNode::from_legacy(&order, &weights) {
                *tree_controller.tree.borrow_mut() = tree.clone();
                ui.set_panel_order(ModelRc::new(VecModel::from(tree.leaf_order())));
                set_panes(&ui, &tree);
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
        if !tree_controller.tree.borrow().is_flat_horizontal() {
            return;
        }
        let weights = resize(&weights, &order, index, delta);
        if let Some(tree) = DockNode::from_legacy(&order, &weights) {
            *tree_controller.tree.borrow_mut() = tree.clone();
            set_panes(&ui, &tree);
        }
        ui.set_panel_weights(ModelRc::new(VecModel::from(weights)));
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_select_dock_tab(move |panel| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut tree = tree_controller.tree.borrow_mut();
        if tree.activate_tab(panel) {
            set_panes(&ui, &tree);
        }
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_tab_dock_with(move |panel, target| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut tree = tree_controller.tree.borrow_mut();
        if tree.tab_dock_with(panel, target) {
            ui.set_panel_order(ModelRc::new(VecModel::from(tree.leaf_order())));
            set_panes(&ui, &tree);
        }
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_split_dock_with(move |panel, target, axis, ratio| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(ratio_milli) = u16::try_from(ratio) else {
            return;
        };
        let axis = if axis == 0 {
            DockAxis::Horizontal
        } else if axis == 1 {
            DockAxis::Vertical
        } else {
            return;
        };
        let mut tree = tree_controller.tree.borrow_mut();
        if tree.split_dock_with(panel, target, axis, ratio_milli) {
            ui.set_panel_order(ModelRc::new(VecModel::from(tree.leaf_order())));
            set_panes(&ui, &tree);
        }
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_dock_drag_start(move |panel, x, y| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !PANEL_KINDS.contains(&panel) {
            return;
        }
        ui.set_dock_dragging(true);
        ui.set_dock_drag_panel(panel);
        update_drop_preview(&ui, &tree_controller.tree_snapshot(), panel, x, y);
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_dock_drag_moved(move |panel, x, y| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_dock_dragging() && ui.get_dock_drag_panel() == panel {
            update_drop_preview(&ui, &tree_controller.tree_snapshot(), panel, x, y);
        }
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_dock_drag_end(move |panel, _x, _y| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_dock_dragging() || ui.get_dock_drag_panel() != panel {
            return;
        }
        let target = ui.get_dock_drop_target();
        let zone = dock_drop_zone(ui.get_dock_drop_zone());
        let changed = zone.and_then(|zone| {
            if target == panel {
                return None;
            }
            let mut tree = tree_controller.tree.borrow_mut();
            let changed = if tree.is_flat_horizontal()
                && matches!(zone, DockDropZone::Left | DockDropZone::Right)
            {
                reorder_flat_drop(
                    &tree,
                    panel,
                    target,
                    zone == DockDropZone::Left,
                    &read_floats(&ui.get_panel_weights()),
                )
                .is_some_and(|next| {
                    *tree = next;
                    true
                })
            } else {
                tree.drop_dock_with(panel, target, zone)
            };
            if changed {
                ui.set_panel_order(ModelRc::new(VecModel::from(tree.leaf_order())));
                set_panes(&ui, &tree);
            }
            Some(changed)
        });
        clear_drop_preview(&ui);
        if changed == Some(true) {
            ui.set_status_message("Dock layout updated".into());
        }
    });

    let weak = ui.as_weak();
    let tree_controller = Rc::clone(&controller);
    ui.on_resize_dock_splitter(move |id, delta| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        #[allow(
            clippy::cast_possible_truncation,
            reason = "splitter deltas are bounded UI movement in fixed-point milli-units"
        )]
        let delta = if delta.is_finite() {
            delta.round() as i32
        } else {
            return;
        };
        let Ok(id) = u8::try_from(id) else {
            return;
        };
        let mut tree = tree_controller.tree.borrow_mut();
        if tree.resize_splitter(id, delta) {
            set_panes(&ui, &tree);
        }
    });

    let weak = ui.as_weak();
    ui.on_toggle_meters_paused(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_meters_paused(!ui.get_meters_paused());
        }
    });

    install_float(ui, state, &controller);
    restore_floating_windows(ui, state, &controller);
    controller
}

fn set_panes(ui: &MainWindow, tree: &DockNode) {
    let panes = tree
        .pane_layout()
        .into_iter()
        .map(|pane| DockPane {
            panel_kind: pane.panel,
            x: f32::from(pane.x_milli) / 1_000.0,
            y: f32::from(pane.y_milli) / 1_000.0,
            width: f32::from(pane.width_milli) / 1_000.0,
            height: f32::from(pane.height_milli) / 1_000.0,
            tab_group: i32::from(pane.tab_group),
            tab_index: i32::from(pane.tab_index),
            tab_count: i32::from(pane.tab_count),
            tab_a: pane.tab_ids[0],
            tab_b: pane.tab_ids[1],
            tab_c: pane.tab_ids[2],
            tab_d: pane.tab_ids[3],
            tab_e: pane.tab_ids[4],
            active: pane.active,
        })
        .collect::<Vec<_>>();
    ui.set_dock_panes(ModelRc::new(VecModel::from(panes)));
    let splitters = tree
        .splitter_layout()
        .into_iter()
        .map(|splitter| DockSplitter {
            id: i32::from(splitter.id),
            boundary: f32::from(splitter.boundary_milli) / 1_000.0,
            axis: match splitter.axis {
                DockAxis::Horizontal => 0,
                DockAxis::Vertical => 1,
            },
        })
        .collect::<Vec<_>>();
    ui.set_dock_splitters(ModelRc::new(VecModel::from(splitters)));
}

fn update_drop_preview(ui: &MainWindow, tree: &DockNode, panel: i32, x: f32, y: f32) {
    let Some((target, zone)) = tree.drop_target(normalized_milli(x), normalized_milli(y)) else {
        ui.set_dock_drop_target(-1);
        ui.set_dock_drop_zone(-1);
        return;
    };
    if target == panel {
        ui.set_dock_drop_target(-1);
        ui.set_dock_drop_zone(-1);
        return;
    }
    ui.set_dock_drop_target(target);
    ui.set_dock_drop_zone(zone.ui_value());
}

fn clear_drop_preview(ui: &MainWindow) {
    ui.set_dock_dragging(false);
    ui.set_dock_drag_panel(-1);
    ui.set_dock_drop_target(-1);
    ui.set_dock_drop_zone(-1);
}

fn normalized_milli(value: f32) -> u16 {
    let value = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the normalized pointer is clamped to 0..=1 before fixed-point conversion"
    )]
    {
        (value * 1_000.0).round() as u16
    }
}

fn dock_drop_zone(value: i32) -> Option<DockDropZone> {
    match value {
        0 => Some(DockDropZone::Tab),
        1 => Some(DockDropZone::Left),
        2 => Some(DockDropZone::Right),
        3 => Some(DockDropZone::Top),
        4 => Some(DockDropZone::Bottom),
        _ => None,
    }
}

/// Keeps the old horizontal row's direct reorder semantics when a drag lands
/// on the left or right edge of a neighbour. Non-flat trees use the same drop
/// zone to create a real nested split instead.
fn reorder_flat_drop(
    tree: &DockNode,
    panel: i32,
    target: i32,
    before: bool,
    weights: &[f32],
) -> Option<DockNode> {
    let mut order = tree.leaf_order();
    let panel_index = order.iter().position(|id| *id == panel)?;
    order.remove(panel_index);
    let target_index = order.iter().position(|id| *id == target)?;
    let insertion = if before {
        target_index
    } else {
        target_index.saturating_add(1)
    };
    order.insert(insertion.min(order.len()), panel);
    DockNode::from_legacy(&order, weights)
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
        match float(&ui, &state, &controller, kind, current_desktop_bounds()) {
            Ok(window) => {
                if let Some(slot) = controller.windows.borrow_mut().get_mut(index) {
                    *slot = Some(window);
                }
                set_floating(&ui, index, true);
                controller.sync(&ui);
            }
            Err(error) => ui.set_status_message(format!("Floating dock: {error}").into()),
        }
    });
}

/// Returns a dock to the row and closes its window.
fn redock(ui: &MainWindow, controller: &Rc<DockController>, index: usize) {
    let window = controller
        .windows
        .borrow_mut()
        .get_mut(index)
        .and_then(Option::take);
    if let Some(window) = window {
        controller.remember_window_geometry(index, &window);
        let _ = window.hide();
    }
    set_floating(ui, index, false);
}

/// Reopens detached docks after the main window has installed all forwarding
/// callbacks. A failure is visible in the status line and the dock is put back
/// into the row instead of leaving a hidden boolean-only layout entry behind.
fn restore_floating_windows(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<DockController>,
) {
    let floating = read_bools(&ui.get_panel_floating());
    let bounds = current_desktop_bounds();
    for (index, is_floating) in floating.into_iter().enumerate() {
        if !is_floating {
            continue;
        }
        let Ok(kind) = i32::try_from(index) else {
            continue;
        };
        match float(ui, state, controller, kind, bounds) {
            Ok(window) => {
                if let Some(slot) = controller.windows.borrow_mut().get_mut(index) {
                    *slot = Some(window);
                }
                controller.sync(ui);
            }
            Err(error) => {
                set_floating(ui, index, false);
                ui.set_status_message(format!("Restoring floating dock: {error}").into());
            }
        }
    }
}

/// Builds and shows the window for one dock kind.
fn float(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    controller: &Rc<DockController>,
    kind: i32,
    bounds: Option<DesktopBounds>,
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

    if let Some(geometry) = controller.stored_geometry(redock_index) {
        restore_window_geometry(&window, geometry, bounds);
    }
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
    forward!(on_move_scene, invoke_move_scene, id, delta);
    forward!(on_remove_scene, invoke_remove_scene, id);
    forward!(on_open_scene_projector, invoke_open_scene_projector, id);
    forward!(on_select_source, invoke_select_source, id);
    forward!(on_open_properties, invoke_open_source_properties_for, id);
    forward!(on_open_filters, invoke_open_source_filters_for, id);
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
    forward!(on_paste_reference, invoke_paste_reference, target);
    forward!(on_paste_duplicate, invoke_paste_duplicate, target);
    forward!(on_remove_source, invoke_remove_source, id);
    forward!(on_set_mixer_gain, invoke_set_mixer_gain, id, gain);
    forward!(on_set_mixer_pan, invoke_set_mixer_pan, id, pan);
    forward!(on_toggle_mixer_mute, invoke_toggle_mixer_mute, id);
    forward!(on_toggle_meters_paused, invoke_toggle_meters_paused);
    forward!(on_cut_transition, invoke_cut_transition);
    forward!(on_fade_transition, invoke_fade_transition);
    forward!(
        on_fade_transition_duration,
        invoke_fade_transition_duration,
        duration
    );
    forward!(on_fade_to_color, invoke_fade_to_color, color, duration);
    forward!(
        on_set_scene_transition,
        invoke_set_scene_transition,
        kind,
        duration,
        color
    );
    forward!(on_clear_scene_transition, invoke_clear_scene_transition);
    forward!(on_toggle_recording, invoke_toggle_recording);
    forward!(on_toggle_streaming, invoke_toggle_streaming);
    forward!(on_recover_recording, invoke_recover_recording);
    forward!(on_open_settings_window, invoke_open_settings_window);

    let weak = ui.as_weak();
    window.on_set_view_mode(move |mode| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // -1 toggles studio mode, matching the studio window's own handling;
        // explicit mode 2 selects the bounded multiview surface.
        let current = ui.get_view_mode();
        ui.set_view_mode(if mode == -1 {
            i32::from(current == 0)
        } else {
            mode.clamp(0, 2)
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

fn read_bools(model: &ModelRc<bool>) -> Vec<bool> {
    (0..model.row_count())
        .filter_map(|row| model.row_data(row))
        .collect()
}

/// Stores the backend's physical desktop geometry and scale factor. The
/// backend returns physical coordinates even when the generated Slint window
/// properties are expressed in logical pixels, which keeps multi-monitor
/// positions unambiguous.
fn capture_window_geometry(
    geometry: &mut [Option<FloatingGeometry>],
    index: usize,
    window: &FloatingDockWindow,
) {
    let Ok(panel) = i32::try_from(index) else {
        return;
    };
    let position = window.window().position();
    let size = window.window().size();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the scale factor is finite and stored as bounded thousandths"
    )]
    let scale_milli = (window.window().scale_factor().max(0.5) * 1_000.0).round() as u32;
    if let Some(entry) = FloatingGeometry::new(
        panel,
        position.x,
        position.y,
        size.width,
        size.height,
        scale_milli,
    ) {
        if let Some(slot) = geometry.get_mut(index) {
            *slot = Some(entry);
        }
    }
}

/// Restores a saved physical position and scales dimensions to the current
/// display DPI. Position stays in desktop coordinates: if a second monitor
/// changed DPI, the window remains on that monitor rather than jumping to a
/// different global location, subject only to the known-desktop visibility
/// clamp.
fn restore_window_geometry(
    window: &FloatingDockWindow,
    geometry: FloatingGeometry,
    bounds: Option<DesktopBounds>,
) {
    let current_scale = window.window().scale_factor().max(0.5);
    #[allow(
        clippy::cast_precision_loss,
        reason = "the stored scale is bounded thousandths and f32 is sufficient for DPI"
    )]
    let saved_scale = (geometry.scale_milli as f32 / 1_000.0).max(0.5);
    let ratio = (current_scale / saved_scale).clamp(0.5, 2.0);
    let width = scale_dimension(geometry.width, ratio, 240, 8_192);
    let height = scale_dimension(geometry.height, ratio, 160, 8_192);
    let (x, y) = bounds.map_or((geometry.x, geometry.y), |bounds| {
        clamp_window_position(geometry.x, geometry.y, width, height, bounds)
    });
    window.window().set_position(PhysicalPosition::new(x, y));
    window.window().set_size(PhysicalSize::new(width, height));
}

/// Keeps a restored dock at least partially visible in the known virtual
/// desktop. The position remains in physical coordinates, so negative offsets
/// for a monitor left or above the primary display are preserved.
const MIN_VISIBLE_DOCK_PIXELS: i32 = 48;

fn clamp_window_position(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    bounds: DesktopBounds,
) -> (i32, i32) {
    let width = i32::try_from(width).unwrap_or(i32::MAX);
    let height = i32::try_from(height).unwrap_or(i32::MAX);
    let min_x = bounds
        .left
        .saturating_sub(width.saturating_sub(MIN_VISIBLE_DOCK_PIXELS));
    let max_x = bounds.right.saturating_sub(MIN_VISIBLE_DOCK_PIXELS);
    let min_y = bounds
        .top
        .saturating_sub(height.saturating_sub(MIN_VISIBLE_DOCK_PIXELS));
    let max_y = bounds.bottom.saturating_sub(MIN_VISIBLE_DOCK_PIXELS);
    let x = if width >= bounds.width {
        bounds.left
    } else {
        clamp_position(x, min_x, max_x, bounds.left)
    };
    let y = if height >= bounds.height {
        bounds.top
    } else {
        clamp_position(y, min_y, max_y, bounds.top)
    };
    (x, y)
}

fn clamp_position(value: i32, minimum: i32, maximum: i32, oversized_fallback: i32) -> i32 {
    if minimum <= maximum {
        value.clamp(minimum, maximum)
    } else {
        // A dock wider/taller than the whole desktop cannot fit between both
        // visibility constraints; anchoring its origin at the desktop edge
        // leaves the largest useful portion on-screen.
        oversized_fallback
    }
}

fn current_desktop_bounds() -> Option<DesktopBounds> {
    let monitors = screen_monitors();
    (!monitors.is_empty()).then(|| desktop_bounds(&monitors))
}

fn scale_dimension(value: u32, ratio: f32, minimum: u32, maximum: u32) -> u32 {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "dimensions are bounded by FloatingGeometry before scaling"
    )]
    let scaled = (value as f32 * ratio).round() as u32;
    scaled.clamp(minimum, maximum)
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

    fn bounds() -> DesktopBounds {
        desktop_bounds(&[
            crate::fixtures::MonitorChoice {
                id: "DP-1".to_owned(),
                name: "DP-1".to_owned(),
                x: 0,
                y: 0,
                width: 1_920,
                height: 1_080,
                primary: true,
            },
            crate::fixtures::MonitorChoice {
                id: "HDMI-1".to_owned(),
                name: "HDMI-1".to_owned(),
                x: -1_280,
                y: 120,
                width: 1_280,
                height: 1_024,
                primary: false,
            },
        ])
    }

    #[test]
    fn restored_dock_position_keeps_a_title_bar_visible() {
        assert_eq!(
            clamp_window_position(5_000, 5_000, 720, 520, bounds()),
            (1_872, 1_096)
        );
        assert_eq!(
            clamp_window_position(-5_000, -5_000, 720, 520, bounds()),
            (-1_952, -472)
        );
    }

    #[test]
    fn restored_dock_preserves_negative_secondary_monitor_offsets() {
        assert_eq!(
            clamp_window_position(-1_920, 84, 720, 520, bounds()),
            (-1_920, 84)
        );
    }

    #[test]
    fn oversized_dock_anchors_to_the_desktop_edge() {
        assert_eq!(
            clamp_window_position(500, 500, 10_000, 10_000, bounds()),
            (-1_280, 0)
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
