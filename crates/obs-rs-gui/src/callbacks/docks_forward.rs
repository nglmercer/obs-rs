//! Callback forwarding for detached dock windows.
//!
//! Floating docks remain views of `MainWindow`: this module owns only the
//! adapter that forwards their events and then refreshes the shared dock tree.

use std::rc::Rc;

use super::DockController;
use crate::{FloatingDockWindow, MainWindow};
use slint::ComponentHandle;

/// Points a floating dock's callbacks at the studio window's own handlers.
///
/// Every dock action already has one implementation on the studio window, so a
/// detached dock forwards to it rather than installing a second copy that could
/// drift. After the studio has handled the action its models are current, so the
/// floating dock is re-synced from them immediately.
#[allow(
    clippy::too_many_lines,
    reason = "one forwarding boundary keeps floating and dock callback semantics identical"
)]
pub(super) fn forward_to_studio(
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
    forward!(
        on_navigate_preview_scene,
        invoke_navigate_preview_scene,
        direction
    );
    forward!(on_select_program, invoke_select_program, id);
    forward!(on_duplicate_scene, invoke_duplicate_scene, id);
    forward!(on_move_scene, invoke_move_scene, id, delta);
    forward!(on_drop_scene, invoke_drop_scene, data, target, mode);
    forward!(on_remove_scene, invoke_remove_scene, id);
    forward!(on_open_scene_projector, invoke_open_scene_projector, id);
    forward!(on_open_source_projector, invoke_open_source_projector);
    forward!(on_select_source, invoke_select_source, id);
    forward!(
        on_navigate_source_selection,
        invoke_navigate_source_selection,
        direction,
        mode
    );
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
    forward!(
        on_move_source_to_group,
        invoke_move_source_to_group,
        id,
        destination
    );
    forward!(on_drop_source, invoke_drop_source, data, target, mode);
    forward!(on_reset_source_transform, invoke_reset_source_transform, id);
    forward!(on_flip_source, invoke_flip_source, id, horizontal);
    forward!(on_transform_source, invoke_transform_source, id, action);
    forward!(on_open_source_rename, invoke_open_source_rename, id);
    forward!(on_duplicate_source, invoke_duplicate_source, id);
    forward!(on_group_sources, invoke_group_sources);
    forward!(on_ungroup_source, invoke_ungroup_source, id);
    forward!(on_copy_source, invoke_copy_source, id);
    forward!(on_paste_reference, invoke_paste_reference, target);
    forward!(on_paste_duplicate, invoke_paste_duplicate, target);
    forward!(on_remove_source, invoke_remove_source, id);
    forward!(on_remove_selected_sources, invoke_remove_selected_sources);
    forward!(on_set_mixer_gain, invoke_set_mixer_gain, id, gain);
    forward!(on_set_mixer_pan, invoke_set_mixer_pan, id, pan);
    forward!(on_toggle_mixer_mute, invoke_toggle_mixer_mute, id);
    forward!(on_toggle_meters_paused, invoke_toggle_meters_paused);
    forward!(on_cut_transition, invoke_cut_transition);
    forward!(on_take_stinger, invoke_take_stinger, duration);
    forward!(on_fade_transition, invoke_fade_transition);
    forward!(
        on_fade_transition_duration,
        invoke_fade_transition_duration,
        duration
    );
    forward!(
        on_slide_transition_direction,
        invoke_slide_transition_direction,
        duration,
        direction
    );
    forward!(
        on_swipe_transition_direction,
        invoke_swipe_transition_direction,
        duration,
        direction
    );
    forward!(
        on_swipe_transition_direction_mode,
        invoke_swipe_transition_direction_mode,
        duration,
        direction,
        swipe_in
    );
    forward!(on_fade_to_color, invoke_fade_to_color, color, duration);
    forward!(
        on_luma_transition,
        invoke_luma_transition,
        duration,
        pattern,
        invert,
        softness
    );
    forward!(
        on_set_scene_transition,
        invoke_set_scene_transition,
        kind,
        duration,
        color
    );
    forward!(
        on_set_scene_transition_direction,
        invoke_set_scene_transition_direction,
        kind,
        duration,
        color,
        direction
    );
    forward!(
        on_set_scene_transition_direction_mode,
        invoke_set_scene_transition_direction_mode,
        kind,
        duration,
        color,
        direction,
        swipe_in
    );
    forward!(
        on_set_scene_transition_luma,
        invoke_set_scene_transition_luma,
        kind,
        duration,
        pattern,
        invert,
        softness
    );
    forward!(on_clear_scene_transition, invoke_clear_scene_transition);
    forward!(on_toggle_recording, invoke_toggle_recording);
    forward!(on_toggle_streaming, invoke_toggle_streaming);
    forward!(on_recover_recording, invoke_recover_recording);
    forward!(on_open_settings_window, invoke_open_settings_window);
    forward!(
        on_open_audio_settings_window,
        invoke_open_audio_settings_window
    );

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
