mod output;
mod project;
mod scene;
mod settings;
mod source;

use std::{cell::RefCell, rc::Rc, time::Duration};

use obs_rs_ui::DesktopState;
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::{
    refresh_output_ui, refresh_preview_frames, MainWindow, OutputRuntime, PreviewRenderer,
};

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
