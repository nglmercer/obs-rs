pub(crate) mod add_source;
pub(crate) mod canvas;
pub(crate) mod docks;
pub(crate) mod menu;
pub(crate) mod monitor;
mod output;
mod project;
mod scene;
pub(crate) mod settings;
mod source;
pub(crate) mod source_properties;

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, Timer, TimerMode};

use crate::{
    refresh_output_ui, refresh_preview_frames_for_view, MainWindow, OutputRuntime, PreviewRenderer,
};

pub(crate) use add_source::install_add_source_window;
#[cfg(test)]
pub(crate) use add_source::{add_source_window, populate_add_source_window};
pub(crate) use canvas::{install_canvas_callbacks, item_rect};
pub(crate) use docks::install_dock_callbacks;
pub(crate) use menu::{install_menu_callbacks, ProjectorController};
pub(crate) use monitor::install_monitor_window;
pub(crate) use output::{install_mixer_callbacks, install_output_callbacks, push_program_frame};
pub(crate) use project::{install_project_callbacks, project_store, rename_scene_and_refresh};
pub(crate) use scene::install_scene_callbacks;
#[cfg(test)]
pub(crate) use settings::populate_settings_models;
pub(crate) use settings::{install_settings_window, PeerWindows};
pub(crate) use source::{
    apply_source_filters_and_refresh, apply_source_settings_and_refresh,
    apply_source_transform_and_refresh, move_source_and_refresh, remove_scene_and_refresh,
    remove_source_and_refresh, source_filters_document, source_transform_document,
    toggle_source_locked_and_refresh, toggle_source_visibility_and_refresh,
};
pub(crate) use source_properties::install_source_properties_window;

pub(crate) fn start_preview_timer(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
    projectors: &Rc<ProjectorController>,
    docks: &Rc<docks::DockController>,
) -> Timer {
    let timer = Timer::default();
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let output = Rc::clone(output);
    let projectors = Rc::clone(projectors);
    let docks = Rc::clone(docks);
    let mut last_output_ui_refresh = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let (revision, preview_scene, program_scene, output_active) = {
            let state = state.borrow();
            (
                state.project_session().revision(),
                state.preview_scene().map(str::to_owned),
                state.program_scene().map(str::to_owned),
                state.recording() || state.streaming(),
            )
        };
        // A canvas change held back while recording is applied here, at the
        // first tick after the output stopped, and before the ordinary project
        // sync so both reach the engine in one rebuild.
        if !output_active {
            match settings::apply_staged_video_format(&state, &output) {
                Some(Ok(format)) => ui.set_status_message(
                    format!(
                        "Canvas changed to {}x{} now that the output stopped",
                        format.width(),
                        format.height()
                    )
                    .into(),
                ),
                Some(Err(error)) => {
                    ui.set_status_message(format!("Staged canvas change failed: {error}").into());
                }
                None => {}
            }
        }
        if !output_active && output.borrow().needs_project_sync(revision) {
            let project = state.borrow().project_session().project().clone();
            if let Err(error) = output.borrow_mut().sync_project(project, revision) {
                ui.set_status_message(format!("Output project sync failed: {error}").into());
            }
        }
        let (preview_frame, program_frame, render_error) = refresh_preview_frames_for_view(
            &ui,
            &renderer,
            preview_scene.as_deref(),
            program_scene.as_deref(),
            // A program projector is a third consumer of the program canvas,
            // so single-canvas editing has to render it again while one is up.
            output_active || ui.get_view_mode() == 0 || projectors.wants_program(),
        );
        if let Some(error) = render_error {
            ui.set_status_message(error.into());
        }
        projectors.sync(&ui);
        if output_active {
            push_program_frame(&ui, program_frame, &output);
        } else if let Some(frame) = preview_frame.as_ref().or(program_frame.as_ref()) {
            output.borrow().monitor_audio(frame);
        }
        reconcile_output_lifecycle(&ui, &state, &output);
        // Worker counters remain live in the status bar, but formatting them
        // at 30 fps adds needless allocations and mutex traffic while editing.
        if last_output_ui_refresh.elapsed() >= Duration::from_millis(100) {
            refresh_output_ui(&ui, &output);
            refresh_input_meter(&ui, &state, &output);
            docks.sync(&ui);
            last_output_ui_refresh = Instant::now();
        }
    });
    timer
}

/// Brings the desktop's output booleans back in line with the engine's phases.
///
/// Pressing Record or Stream sets a boolean optimistically, but the engine is
/// what decides whether an output actually runs: a refused peer, a rejected
/// recording path, or a dead worker all end the output without the desktop
/// asking. Reconciling here means the controls can never keep claiming an
/// output is live after the engine has stopped it.
pub(crate) fn reconcile_output_lifecycle(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let (recording, streaming) = output.borrow().lifecycles();
    let (claims_recording, claims_streaming) = {
        let state = state.borrow();
        (state.recording(), state.streaming())
    };

    if claims_recording && recording.is_stopped() {
        let _ = state
            .borrow_mut()
            .dispatch(obs_rs_ui::UiCommand::StopRecording);
        ui.set_recording(false);
        if recording == obs_rs_engine::OutputLifecycle::Failed {
            ui.set_status_message("Recording stopped after an output failure".into());
            ui.set_active_modal(11);
        }
    }
    if claims_streaming && streaming.is_stopped() {
        let _ = state
            .borrow_mut()
            .dispatch(obs_rs_ui::UiCommand::StopStreaming);
        ui.set_streaming(false);
        if streaming == obs_rs_engine::OutputLifecycle::Failed {
            ui.set_status_message("Streaming stopped after transport failure".into());
            ui.set_active_modal(11);
        }
    }
}

/// Pushes the engine's live input peak onto the mixer's microphone channel.
///
/// The meter is refreshed from the capture backend rather than from a mix the
/// desktop performs itself, because the engine worker is what actually reads
/// the microphone. Only the mixer rows are rebuilt, so this stays off the
/// scene-graph refresh path.
fn refresh_input_meter(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    if ui.get_meters_paused() {
        return;
    }
    // A fallback generator is not the user's microphone, so its level must not
    // be shown as if the input were live.
    let peak = if output.borrow().audio_is_fallback() {
        0
    } else {
        output.borrow().input_peak_milli()
    };
    let desktop_peak = output.borrow().desktop_peak_milli();
    let mut state_guard = state.borrow_mut();
    let changed = state_guard
        .set_channel_peak_milli(crate::MIC_CHANNEL_ID, peak)
        .is_ok()
        | state_guard
            .set_channel_peak_milli("desktop", desktop_peak)
            .is_ok();
    drop(state_guard);
    if changed {
        crate::refresh::refresh_mixer_rows(ui, &state.borrow());
    }
}

pub(crate) fn install_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    install_scene_callbacks(ui, state, renderer);
    install_output_callbacks(ui, state, renderer, output);
    install_mixer_callbacks(ui, state, renderer, output);
    install_project_callbacks(ui, state, renderer, output);
}
