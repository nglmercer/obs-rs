use std::{cell::RefCell, rc::Rc};

use obs_rs_project::SceneItemDuplicateMode;
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::ComponentHandle;

use crate::{
    apply_source_name_and_refresh, apply_source_settings_and_refresh, dispatch_and_refresh,
    duplicate_scene_and_refresh, duplicate_source_and_refresh, flip_source_and_refresh,
    move_source_and_refresh, move_source_to_and_refresh, remove_scene_and_refresh,
    remove_source_and_refresh, rename_scene_and_refresh, reset_source_transform_and_refresh,
    toggle_source_locked_and_refresh, toggle_source_visibility_and_refresh,
    transform_source_and_refresh, MainWindow, PreviewSurface,
};

pub(crate) fn install_scene_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    install_scene_selection_callbacks(ui, state, surface);
    install_source_list_callbacks(ui, state, surface);
    install_source_property_callbacks(ui, state, surface);
}

fn install_scene_selection_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let swap_state = Rc::clone(state);
    let swap_surface = Rc::clone(surface);
    ui.on_swap_scenes(move || {
        dispatch_and_refresh(
            &weak,
            &swap_state,
            &swap_surface,
            UiCommand::SwapPreviewProgram,
        );
    });

    let weak = ui.as_weak();
    let preview_state = Rc::clone(state);
    let preview_surface = Rc::clone(surface);
    ui.on_select_preview(move |id| {
        dispatch_and_refresh(
            &weak,
            &preview_state,
            &preview_surface,
            UiCommand::SelectPreviewScene { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let program_state = Rc::clone(state);
    let program_surface = Rc::clone(surface);
    ui.on_select_program(move |id| {
        dispatch_and_refresh(
            &weak,
            &program_state,
            &program_surface,
            UiCommand::SelectProgramScene { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let remove_state = Rc::clone(state);
    let remove_surface = Rc::clone(surface);
    ui.on_remove_scene(move |id| {
        remove_scene_and_refresh(&weak, &remove_state, &remove_surface, id.as_str());
    });

    let weak = ui.as_weak();
    let duplicate_state = Rc::clone(state);
    let duplicate_surface = Rc::clone(surface);
    ui.on_duplicate_scene(move |id| {
        duplicate_scene_and_refresh(&weak, &duplicate_state, &duplicate_surface, id.as_str());
    });

    let weak = ui.as_weak();
    let rename_state = Rc::clone(state);
    let rename_surface = Rc::clone(surface);
    ui.on_rename_scene(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        rename_scene_and_refresh(
            &ui,
            &rename_state,
            &rename_surface,
            ui.get_scene_name().as_str(),
        );
    });

    let weak = ui.as_weak();
    let profile_state = Rc::clone(state);
    let profile_surface = Rc::clone(surface);
    ui.on_select_profile(move |id| {
        dispatch_and_refresh(
            &weak,
            &profile_state,
            &profile_surface,
            UiCommand::SelectProfile { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let locale_state = Rc::clone(state);
    let locale_surface = Rc::clone(surface);
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
            &locale_surface,
            UiCommand::SetLocale { locale },
        );
    });
}

#[allow(
    clippy::too_many_lines,
    reason = "one UI surface owns all source-list actions and their refresh paths"
)]
fn install_source_list_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let source_state = Rc::clone(state);
    let source_surface = Rc::clone(surface);
    ui.on_select_source(move |id| {
        dispatch_and_refresh(
            &weak,
            &source_state,
            &source_surface,
            UiCommand::SelectSource { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let visibility_state = Rc::clone(state);
    let visibility_surface = Rc::clone(surface);
    ui.on_toggle_source_visibility(move |id| {
        toggle_source_visibility_and_refresh(
            &weak,
            &visibility_state,
            &visibility_surface,
            id.as_str(),
        );
    });

    let weak = ui.as_weak();
    let locked_state = Rc::clone(state);
    let locked_surface = Rc::clone(surface);
    ui.on_toggle_source_locked(move |id| {
        toggle_source_locked_and_refresh(&weak, &locked_state, &locked_surface, id.as_str());
    });

    let weak = ui.as_weak();
    let move_state = Rc::clone(state);
    let move_surface = Rc::clone(surface);
    ui.on_move_source(move |id, delta| {
        move_source_and_refresh(&weak, &move_state, &move_surface, id.as_str(), delta);
    });

    let weak = ui.as_weak();
    let move_to_state = Rc::clone(state);
    let move_to_surface = Rc::clone(surface);
    ui.on_move_source_to(move |id, index| {
        move_source_to_and_refresh(&weak, &move_to_state, &move_to_surface, id.as_str(), index);
    });

    let weak = ui.as_weak();
    let reset_state = Rc::clone(state);
    let reset_surface = Rc::clone(surface);
    ui.on_reset_source_transform(move |id| {
        reset_source_transform_and_refresh(&weak, &reset_state, &reset_surface, id.as_str());
    });

    let weak = ui.as_weak();
    let flip_state = Rc::clone(state);
    let flip_surface = Rc::clone(surface);
    ui.on_flip_source(move |id, horizontal| {
        flip_source_and_refresh(&weak, &flip_state, &flip_surface, id.as_str(), horizontal);
    });

    let weak = ui.as_weak();
    let transform_state = Rc::clone(state);
    let transform_surface = Rc::clone(surface);
    ui.on_transform_source(move |id, action| {
        transform_source_and_refresh(
            &weak,
            &transform_state,
            &transform_surface,
            id.as_str(),
            action.as_str(),
        );
    });

    let weak = ui.as_weak();
    let duplicate_source_state = Rc::clone(state);
    let duplicate_source_surface = Rc::clone(surface);
    ui.on_duplicate_source(move |id| {
        duplicate_source_and_refresh(
            &weak,
            &duplicate_source_state,
            &duplicate_source_surface,
            id.as_str(),
        );
    });

    let weak = ui.as_weak();
    let copy_state = Rc::clone(state);
    let copy_surface = Rc::clone(surface);
    ui.on_copy_source(move |id| {
        dispatch_and_refresh(
            &weak,
            &copy_state,
            &copy_surface,
            UiCommand::CopySource { id: id.to_string() },
        );
    });

    let weak = ui.as_weak();
    let paste_reference_state = Rc::clone(state);
    let paste_reference_surface = Rc::clone(surface);
    ui.on_paste_reference(move |target| {
        dispatch_and_refresh(
            &weak,
            &paste_reference_state,
            &paste_reference_surface,
            UiCommand::PasteSource {
                mode: SceneItemDuplicateMode::Reference,
                target: target.to_string(),
            },
        );
    });

    let weak = ui.as_weak();
    let paste_duplicate_state = Rc::clone(state);
    let paste_duplicate_surface = Rc::clone(surface);
    ui.on_paste_duplicate(move |target| {
        dispatch_and_refresh(
            &weak,
            &paste_duplicate_state,
            &paste_duplicate_surface,
            UiCommand::PasteSource {
                mode: SceneItemDuplicateMode::DuplicateSource,
                target: target.to_string(),
            },
        );
    });

    let weak = ui.as_weak();
    let rename_state = Rc::clone(state);
    let rename_surface = Rc::clone(surface);
    ui.on_open_source_rename(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let source_name = {
            let state = rename_state.borrow();
            let project = state.project_session().project();
            let profile = project.active_profile_spec();
            let item = state
                .preview_scene()
                .and_then(|scene_id| profile.and_then(|profile| profile.scene(scene_id)))
                .and_then(|scene| scene.item(id.as_str()));
            item.and_then(|item| profile.and_then(|profile| profile.source(item.source_id())))
                .map(|source| source.name().to_owned())
        };
        match source_name {
            Some(source_name) => {
                dispatch_and_refresh(
                    &ui.as_weak(),
                    &rename_state,
                    &rename_surface,
                    UiCommand::SelectSource { id: id.to_string() },
                );
                ui.set_source_name_draft(source_name.into());
                ui.set_active_modal(12);
            }
            None => ui.set_status_message("Source is not in the preview scene".into()),
        }
    });

    let weak = ui.as_weak();
    let apply_name_state = Rc::clone(state);
    let apply_name_surface = Rc::clone(surface);
    ui.on_apply_source_name(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let name = ui.get_source_name_draft().to_string();
        apply_source_name_and_refresh(&ui, &apply_name_state, &apply_name_surface, &name);
    });

    let weak = ui.as_weak();
    let remove_state = Rc::clone(state);
    let remove_surface = Rc::clone(surface);
    ui.on_remove_source(move |id| {
        remove_source_and_refresh(&weak, &remove_state, &remove_surface, id.as_str());
    });
}

fn install_source_property_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let weak = ui.as_weak();
    let settings_state = Rc::clone(state);
    let settings_surface = Rc::clone(surface);
    ui.on_apply_source_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let document = ui.get_source_settings().to_string();
        apply_source_settings_and_refresh(&ui, &settings_state, &settings_surface, &document);
    });
}
