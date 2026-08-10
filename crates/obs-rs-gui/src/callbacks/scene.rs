use std::{cell::RefCell, rc::Rc};

use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::ComponentHandle;

use crate::{
    apply_source_filters_and_refresh, apply_source_settings_and_refresh,
    apply_source_transform_and_refresh, dispatch_and_refresh, move_source_and_refresh,
    remove_scene_and_refresh, remove_source_and_refresh, rename_scene_and_refresh,
    toggle_source_locked_and_refresh, toggle_source_visibility_and_refresh, MainWindow,
    PreviewRenderer,
};

pub(crate) fn install_scene_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    install_scene_selection_callbacks(ui, state, renderer);
    install_source_list_callbacks(ui, state, renderer);
    install_source_property_callbacks(ui, state, renderer);
}

fn install_scene_selection_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let weak = ui.as_weak();
    let swap_state = Rc::clone(state);
    let swap_renderer = Rc::clone(renderer);
    ui.on_swap_scenes(move || {
        dispatch_and_refresh(
            &weak,
            &swap_state,
            &swap_renderer,
            UiCommand::SwapPreviewProgram,
        );
    });

    let weak = ui.as_weak();
    let preview_state = Rc::clone(state);
    let preview_renderer = Rc::clone(renderer);
    ui.on_select_preview(move |id| {
        dispatch_and_refresh(
            &weak,
            &preview_state,
            &preview_renderer,
            UiCommand::SelectPreviewScene { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let program_state = Rc::clone(state);
    let program_renderer = Rc::clone(renderer);
    ui.on_select_program(move |id| {
        dispatch_and_refresh(
            &weak,
            &program_state,
            &program_renderer,
            UiCommand::SelectProgramScene { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let remove_state = Rc::clone(state);
    let remove_renderer = Rc::clone(renderer);
    ui.on_remove_scene(move |id| {
        remove_scene_and_refresh(&weak, &remove_state, &remove_renderer, id.as_str());
    });

    let weak = ui.as_weak();
    let rename_state = Rc::clone(state);
    let rename_renderer = Rc::clone(renderer);
    ui.on_rename_scene(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        rename_scene_and_refresh(
            &ui,
            &rename_state,
            &rename_renderer,
            ui.get_scene_name().as_str(),
        );
    });

    let weak = ui.as_weak();
    let profile_state = Rc::clone(state);
    let profile_renderer = Rc::clone(renderer);
    ui.on_select_profile(move |id| {
        dispatch_and_refresh(
            &weak,
            &profile_state,
            &profile_renderer,
            UiCommand::SelectProfile { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let locale_state = Rc::clone(state);
    let locale_renderer = Rc::clone(renderer);
    ui.on_set_locale(move |code| {
        let Some(locale) = UiLocale::from_code(code.as_str()) else {
            if let Some(ui) = weak.upgrade() {
                let prefix =
                    crate::i18n::catalog(locale_state.borrow().locale()).unsupported_language;
                ui.set_status_message(format!("{prefix}{code}").into());
            }
            return;
        };
        dispatch_and_refresh(
            &weak,
            &locale_state,
            &locale_renderer,
            UiCommand::SetLocale { locale },
        );
    });
}

fn install_source_list_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let weak = ui.as_weak();
    let source_state = Rc::clone(state);
    let source_renderer = Rc::clone(renderer);
    ui.on_select_source(move |id| {
        dispatch_and_refresh(
            &weak,
            &source_state,
            &source_renderer,
            UiCommand::SelectSource { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let visibility_state = Rc::clone(state);
    let visibility_renderer = Rc::clone(renderer);
    ui.on_toggle_source_visibility(move |id| {
        toggle_source_visibility_and_refresh(
            &weak,
            &visibility_state,
            &visibility_renderer,
            id.as_str(),
        );
    });

    let weak = ui.as_weak();
    let locked_state = Rc::clone(state);
    let locked_renderer = Rc::clone(renderer);
    ui.on_toggle_source_locked(move |id| {
        toggle_source_locked_and_refresh(&weak, &locked_state, &locked_renderer, id.as_str());
    });

    let weak = ui.as_weak();
    let move_state = Rc::clone(state);
    let move_renderer = Rc::clone(renderer);
    ui.on_move_source(move |id, delta| {
        move_source_and_refresh(&weak, &move_state, &move_renderer, id.as_str(), delta);
    });

    let weak = ui.as_weak();
    let remove_state = Rc::clone(state);
    let remove_renderer = Rc::clone(renderer);
    ui.on_remove_source(move |id| {
        remove_source_and_refresh(&weak, &remove_state, &remove_renderer, id.as_str());
    });
}

fn install_source_property_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let weak = ui.as_weak();
    let settings_state = Rc::clone(state);
    let settings_renderer = Rc::clone(renderer);
    ui.on_apply_source_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let document = ui.get_source_settings().to_string();
        apply_source_settings_and_refresh(&ui, &settings_state, &settings_renderer, &document);
    });

    let weak = ui.as_weak();
    let transform_state = Rc::clone(state);
    let transform_renderer = Rc::clone(renderer);
    ui.on_apply_source_transform(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let document = ui.get_source_transform().to_string();
        apply_source_transform_and_refresh(&ui, &transform_state, &transform_renderer, &document);
    });

    let weak = ui.as_weak();
    let filters_state = Rc::clone(state);
    let filters_renderer = Rc::clone(renderer);
    ui.on_apply_source_filters(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let document = ui.get_source_filters().to_string();
        apply_source_filters_and_refresh(&ui, &filters_state, &filters_renderer, &document);
    });
}
