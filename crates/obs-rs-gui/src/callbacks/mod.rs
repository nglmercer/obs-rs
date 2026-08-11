pub(crate) mod add_source;
mod output;
mod project;
mod scene;
mod settings;
mod source;
pub(crate) mod source_properties;

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::{
    refresh_output_ui, refresh_preview_frames_for_view, MainWindow, OutputRuntime, PreviewRenderer,
};

pub(crate) use add_source::install_add_source_window;
#[cfg(test)]
pub(crate) use add_source::{add_source_window, populate_add_source_window};
pub(crate) use output::{install_mixer_callbacks, install_output_callbacks, push_program_frame};
pub(crate) use project::{install_project_callbacks, project_store, rename_scene_and_refresh};
pub(crate) use scene::install_scene_callbacks;
pub(crate) use settings::install_settings_window;
#[cfg(test)]
pub(crate) use settings::populate_settings_models;
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
) -> Timer {
    let timer = Timer::default();
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let output = Rc::clone(output);
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
        if !output_active && output.borrow().needs_project_sync(revision) {
            let project = state.borrow().project_session().project().clone();
            if let Err(error) = output.borrow_mut().sync_project(project, revision) {
                ui.set_status_message(format!("Output project sync failed: {error}").into());
            }
        }
        let (program_frame, render_error) = refresh_preview_frames_for_view(
            &ui,
            &renderer,
            preview_scene.as_deref(),
            program_scene.as_deref(),
            output_active || ui.get_view_mode() == 0,
        );
        if let Some(error) = render_error {
            ui.set_status_message(error.into());
        }
        if output_active {
            push_program_frame(&ui, program_frame, &output);
        }
        if state.borrow().streaming() && output.borrow().stream_failed() {
            let _ = state
                .borrow_mut()
                .dispatch(obs_rs_ui::UiCommand::StopStreaming);
            ui.set_streaming(false);
            ui.set_status_message("Streaming stopped after transport failure".into());
        }
        // Worker counters remain live in the status bar, but formatting them
        // at 30 fps adds needless allocations and mutex traffic while editing.
        if last_output_ui_refresh.elapsed() >= Duration::from_millis(100) {
            refresh_output_ui(&ui, &output);
            last_output_ui_refresh = Instant::now();
        }
    });
    timer
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
    install_panel_callbacks(ui);
}

fn install_panel_callbacks(ui: &MainWindow) {
    let weak = ui.as_weak();
    ui.on_move_panel(move |panel, direction| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !(0..=4).contains(&panel) || direction == 0 {
            return;
        }
        let model = ui.get_panel_order();
        let mut order = (0..model.row_count())
            .filter_map(|index| model.row_data(index))
            .collect::<Vec<_>>();
        let Some(index) = order.iter().position(|value| *value == panel) else {
            return;
        };
        let target = if direction < 0 {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let Some(target) = target.filter(|target| *target < order.len()) else {
            return;
        };
        order.swap(index, target);
        ui.set_panel_order(ModelRc::new(VecModel::from(order)));
    });
}
