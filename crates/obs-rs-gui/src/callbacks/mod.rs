mod output;
mod project;
mod scene;
mod source;

use std::{cell::RefCell, rc::Rc, time::Duration};

use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, Timer, TimerMode};

use crate::{
    refresh_output_ui, refresh_preview_frames, MainWindow, OutputRuntime, PreviewRenderer,
};

pub(crate) use output::{install_mixer_callbacks, install_output_callbacks, push_program_frame};
pub(crate) use project::{install_project_callbacks, project_store, rename_scene_and_refresh};
pub(crate) use scene::install_scene_callbacks;
pub(crate) use source::{
    apply_source_filters_and_refresh, apply_source_settings_and_refresh,
    apply_source_transform_and_refresh, move_source_and_refresh, remove_scene_and_refresh,
    remove_source_and_refresh, source_filters_document, source_transform_document,
    toggle_source_locked_and_refresh, toggle_source_visibility_and_refresh,
};

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
    timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let (preview_scene, program_scene, output_active) = {
            let state = state.borrow();
            (
                state.preview_scene().map(str::to_owned),
                state.program_scene().map(str::to_owned),
                state.recording() || state.streaming(),
            )
        };
        let (program_frame, render_error) = refresh_preview_frames(
            &ui,
            &renderer,
            preview_scene.as_deref(),
            program_scene.as_deref(),
        );
        if let Some(error) = render_error {
            ui.set_status_message(error.into());
        }
        if output_active {
            push_program_frame(&ui, program_frame, &output);
            refresh_output_ui(&ui, &output);
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
    install_mixer_callbacks(ui, state, renderer);
    install_project_callbacks(ui, state, renderer);
}
