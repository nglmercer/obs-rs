//! Shared state projection for detached dock windows.
//!
//! Keeping this adapter separate prevents the dock-tree lifecycle from also
//! owning every floating-window property. The detached window remains a view
//! of `MainWindow`; it does not become a second state owner.

use crate::{FloatingDockWindow, MainWindow};

pub(super) fn sync_floating_window(window: &FloatingDockWindow, ui: &MainWindow) {
    window.set_platform_macos(ui.get_platform_macos());
    window.set_locale(ui.get_locale());
    window.set_scene_rows(ui.get_scene_rows());
    window.set_source_rows(ui.get_source_rows());
    window.set_mixer_rows(ui.get_mixer_rows());
    window.set_source_scene(ui.get_source_scene());
    window.set_preview_scene(ui.get_preview_scene());
    window.set_selected_source(ui.get_selected_source());
    window.set_selected_source_is_screen(ui.get_selected_source_is_screen());
    window.set_selected_source_is_group(ui.get_selected_source_is_group());
    window.set_selected_source_is_scene(ui.get_selected_source_is_scene());
    window.set_selected_source_is_nested(ui.get_selected_source_is_nested());
    window.set_selected_source_visible(ui.get_selected_source_visible());
    window.set_selected_source_locked(ui.get_selected_source_locked());
    window.set_selected_source_first(ui.get_selected_source_first());
    window.set_selected_source_last(ui.get_selected_source_last());
    window.set_selected_source_move_targets(ui.get_selected_source_move_targets());
    window.set_source_count(ui.get_source_count());
    window.set_can_paste(ui.get_can_paste());
    window.set_can_group_sources(ui.get_can_group_sources());
    window.set_transition(ui.get_transition());
    window.set_transition_kind(ui.get_transition_kind());
    window.set_transition_direction_index(ui.get_transition_direction_index());
    window.set_swipe_in(ui.get_swipe_in());
    window.set_luma_pattern_index(ui.get_luma_pattern_index());
    window.set_luma_invert(ui.get_luma_invert());
    window.set_luma_softness(ui.get_luma_softness());
    window.set_recording(ui.get_recording());
    window.set_streaming(ui.get_streaming());
    window.set_remux_recovery_supported(ui.get_remux_recovery_supported());
    window.set_remux_recovery_running(ui.get_remux_recovery_running());
    window.set_meters_paused(ui.get_meters_paused());
    window.set_status_message(ui.get_status_message());
    window.set_capture_capabilities(ui.get_capture_capabilities());
    window.set_preview_metrics(ui.get_preview_metrics());
    window.set_output_metrics(ui.get_output_metrics());
}
