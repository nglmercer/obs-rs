use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_project::{ProjectCommand, SceneItemDuplicateMode, SceneItemSpec};
use obs_rs_ui::{DesktopState, UiCommand, UiLocale, MAX_CANVAS_SELECTIONS};
use slint::ComponentHandle;

use crate::{
    apply_scene_properties_and_refresh, apply_source_name_and_refresh,
    apply_source_settings_and_refresh, dispatch_and_refresh, duplicate_scene_and_refresh,
    duplicate_source_and_refresh, flip_source_and_refresh, move_source_and_refresh,
    move_source_to_and_refresh, move_source_to_group_and_refresh, refresh_ui,
    remove_scene_and_refresh, remove_source_and_refresh, reset_source_transform_and_refresh,
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
    let move_state = Rc::clone(state);
    let move_surface = Rc::clone(surface);
    ui.on_move_scene(move |id, delta| {
        move_scene_and_refresh(&weak, &move_state, &move_surface, id.as_str(), delta);
    });

    let weak = ui.as_weak();
    let rename_state = Rc::clone(state);
    let rename_surface = Rc::clone(surface);
    ui.on_rename_scene(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        apply_scene_properties_and_refresh(
            &ui,
            &rename_state,
            &rename_surface,
            ui.get_scene_name().as_str(),
            ui.get_scene_transition_index(),
            ui.get_scene_transition_duration().as_str(),
            ui.get_scene_transition_color().as_str(),
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

fn unique_group_identity(
    state: &DesktopState,
    scene_id: &str,
    parent_path: &[String],
) -> Result<(String, String), Box<dyn Error>> {
    let project = state.project_session().project();
    let scene = project
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .ok_or_else(|| std::io::Error::other("preview scene is missing"))?;
    let parent_items = if parent_path.is_empty() {
        scene.items()
    } else {
        let parent_target = parent_path.join("/");
        crate::callbacks::item_for_target(scene, parent_target.as_str())
            .and_then(SceneItemSpec::group)
            .map(obs_rs_project::GroupSpec::items)
            .ok_or_else(|| std::io::Error::other("grouping parent is missing"))?
    };
    for ordinal in 1..=MAX_CANVAS_SELECTIONS.saturating_add(1) {
        let id = if ordinal == 1 {
            "group".to_owned()
        } else {
            format!("group_{ordinal}")
        };
        if !parent_items.iter().any(|item| item.id().as_str() == id) {
            return Ok((id, format!("Group {ordinal}")));
        }
    }
    Err(std::io::Error::other("no bounded group identifier is available").into())
}

fn group_child_targets(
    state: &DesktopState,
    scene_id: &str,
    group_target: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let project = state.project_session().project();
    let group = project
        .active_profile_spec()
        .and_then(|profile| profile.scene(scene_id))
        .and_then(|scene| crate::callbacks::item_for_target(scene, group_target))
        .and_then(SceneItemSpec::group)
        .ok_or_else(|| std::io::Error::other("source is not a group"))?;
    let parent_path = crate::callbacks::source_parent_path(group_target)
        .ok_or_else(|| std::io::Error::other("group target path is invalid"))?;
    Ok(group
        .items()
        .iter()
        .take(MAX_CANVAS_SELECTIONS)
        .map(|item| {
            if parent_path.is_empty() {
                item.id().to_string()
            } else {
                format!("{}/{}", parent_path.join("/"), item.id())
            }
        })
        .collect())
}

fn move_scene_and_refresh(
    weak: &slint::Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    scene_id: &str,
    delta: i32,
) {
    let result = (|| {
        let state = state.borrow();
        let profile = state.project_session().project().active_profile_spec()?;
        let scenes = profile.scenes().collect::<Vec<_>>();
        let current_index = scenes
            .iter()
            .position(|scene| scene.id().as_str() == scene_id)?;
        let magnitude = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
        let target_index = if delta < 0 {
            current_index.saturating_sub(magnitude)
        } else {
            current_index.saturating_add(magnitude)
        };
        if target_index >= scenes.len() || target_index == current_index {
            return None;
        }
        Some((profile.id().to_string(), target_index))
    })();
    let Some((profile, target_index)) = result else {
        return;
    };
    dispatch_and_refresh(
        weak,
        state,
        surface,
        UiCommand::Project(ProjectCommand::MoveScene {
            profile,
            scene: scene_id.to_owned(),
            target_index,
        }),
    );
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
    let select_all_state = Rc::clone(state);
    let select_all_surface = Rc::clone(surface);
    ui.on_select_all_sources(move || {
        let ids = {
            let state = select_all_state.borrow();
            let Some(scene_id) = state.preview_scene() else {
                return;
            };
            let Some(scene) = state
                .project_session()
                .project()
                .active_profile_spec()
                .and_then(|profile| profile.scene(scene_id))
            else {
                return;
            };
            let Some(profile) = state.project_session().project().active_profile_spec() else {
                return;
            };
            let mut rows = Vec::new();
            crate::refresh::append_source_rows(
                &mut rows,
                profile,
                scene.items(),
                &state,
                &mut Vec::new(),
            );
            rows.into_iter()
                .map(|row| row.target.to_string())
                .take(obs_rs_ui::MAX_CANVAS_SELECTIONS)
                .collect::<Vec<_>>()
        };
        dispatch_and_refresh(
            &weak,
            &select_all_state,
            &select_all_surface,
            UiCommand::SelectSources {
                ids,
                additive: false,
            },
        );
    });

    let weak = ui.as_weak();
    let group_state = Rc::clone(state);
    let group_surface = Rc::clone(surface);
    ui.on_group_sources(move || {
        let result: Result<String, Box<dyn Error>> = (|| {
            let (profile, scene, items, group_target, group) = {
                let state = group_state.borrow();
                let project = state.project_session().project();
                let scene = state
                    .preview_scene()
                    .ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
                let profile = project.active_profile().to_string();
                let items = state
                    .selected_sources()
                    .take(MAX_CANVAS_SELECTIONS)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let parent_path =
                    crate::callbacks::common_source_parent(items.iter().map(String::as_str))
                        .ok_or_else(|| {
                            std::io::Error::other("selected sources must share one group parent")
                        })?;
                let (group_id, group_name) = unique_group_identity(&state, scene, &parent_path)?;
                let group = SceneItemSpec::for_group(&group_id, &group_name)?;
                let group_target = if parent_path.is_empty() {
                    group_id
                } else {
                    format!("{}/{}", parent_path.join("/"), group_id)
                };
                (profile, scene.to_owned(), items, group_target, group)
            };
            group_state.borrow_mut().dispatch(UiCommand::Project(
                ProjectCommand::GroupSceneItems {
                    profile,
                    scene,
                    items,
                    group,
                },
            ))?;
            group_state.borrow_mut().dispatch(UiCommand::SelectSource {
                id: group_target.clone(),
            })?;
            Ok(group_target)
        })();
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(_) => refresh_ui(&ui, &group_state, &group_surface),
            Err(error) => ui.set_status_message(format!("Group sources failed: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let ungroup_state = Rc::clone(state);
    let ungroup_surface = Rc::clone(surface);
    ui.on_ungroup_source(move |id| {
        let result: Result<Vec<String>, Box<dyn Error>> = (|| {
            let (profile, scene, child_ids) = {
                let state = ungroup_state.borrow();
                let project = state.project_session().project();
                let scene = state
                    .preview_scene()
                    .ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
                let child_ids = group_child_targets(&state, scene, id.as_str())?;
                (
                    project.active_profile().to_string(),
                    scene.to_owned(),
                    child_ids,
                )
            };
            ungroup_state.borrow_mut().dispatch(UiCommand::Project(
                ProjectCommand::UngroupSceneItem {
                    profile,
                    scene,
                    group: id.to_string(),
                },
            ))?;
            if !child_ids.is_empty() {
                ungroup_state
                    .borrow_mut()
                    .dispatch(UiCommand::SelectSources {
                        ids: child_ids.clone(),
                        additive: false,
                    })?;
            }
            Ok(child_ids)
        })();
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match result {
            Ok(_) => refresh_ui(&ui, &ungroup_state, &ungroup_surface),
            Err(error) => ui.set_status_message(format!("Ungroup failed: {error}").into()),
        }
    });

    let weak = ui.as_weak();
    let navigation_state = Rc::clone(state);
    let navigation_surface = Rc::clone(surface);
    ui.on_navigate_source_selection(move |direction, mode| {
        navigate_source_selection_and_refresh(
            &weak,
            &navigation_state,
            &navigation_surface,
            direction,
            mode,
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
    let move_group_state = Rc::clone(state);
    let move_group_surface = Rc::clone(surface);
    ui.on_move_source_to_group(move |id, destination| {
        move_source_to_group_and_refresh(
            &weak,
            &move_group_state,
            &move_group_surface,
            id.as_str(),
            destination.as_str(),
        );
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
            project.active_profile_spec().and_then(|profile| {
                state.preview_scene().and_then(|scene_id| {
                    profile.scene(scene_id).and_then(|scene| {
                        crate::callbacks::item_for_target(scene, id.as_str()).and_then(|item| {
                            item.group()
                                .map(|group| group.name().to_owned())
                                .or_else(|| {
                                    if item.is_source() {
                                        profile
                                            .source(item.source_id())
                                            .map(|source| source.name().to_owned())
                                    } else {
                                        None
                                    }
                                })
                        })
                    })
                })
            })
        };
        match source_name {
            Some(source_name) => {
                dispatch_and_refresh(
                    &ui.as_weak(),
                    &rename_state,
                    &rename_surface,
                    UiCommand::SelectSource { id: id.to_string() },
                );
                ui.set_source_rename_target(id);
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
        let target = ui.get_source_rename_target().to_string();
        apply_source_name_and_refresh(&ui, &apply_name_state, &apply_name_surface, &target, &name);
    });

    let weak = ui.as_weak();
    let remove_state = Rc::clone(state);
    let remove_surface = Rc::clone(surface);
    ui.on_remove_source(move |id| {
        remove_source_and_refresh(&weak, &remove_state, &remove_surface, id.as_str());
    });
}

/// Resolves one keyboard navigation request against the active scene's
/// depth-first visible source-row order. The row target is the same bounded
/// outer-to-inner path used by dock clicks and context-menu actions.
fn navigate_source_selection_and_refresh(
    weak: &slint::Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    direction: i32,
    mode: i32,
) {
    let target = {
        let state = state.borrow();
        let Some(scene_id) = state.preview_scene() else {
            return;
        };
        let Some(scene) = state
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene(scene_id))
        else {
            return;
        };
        let Some(profile) = state.project_session().project().active_profile_spec() else {
            return;
        };
        let mut rows = Vec::new();
        crate::refresh::append_source_rows(
            &mut rows,
            profile,
            scene.items(),
            &state,
            &mut Vec::new(),
        );
        let current = state
            .selected_source()
            .and_then(|selected| rows.iter().position(|row| row.target.as_str() == selected));
        let Some(index) = source_navigation_index(current, rows.len(), direction) else {
            return;
        };
        if current == Some(index) {
            return;
        }
        rows.get(index).map(|row| row.target.to_string())
    };
    let Some(target) = target else {
        return;
    };
    let command = match mode {
        0 => UiCommand::SelectSource { id: target },
        1 => UiCommand::SelectSources {
            ids: vec![target],
            additive: true,
        },
        2 => UiCommand::ToggleSourceSelection { id: target },
        _ => return,
    };
    dispatch_and_refresh(weak, state, surface, command);
}

/// Returns the next top-level source index for a bounded list-navigation
/// request. Edges do not wrap, matching the native source list's behavior.
fn source_navigation_index(current: Option<usize>, count: usize, direction: i32) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match direction {
        -2 => Some(0),
        -1 => current
            .filter(|&index| index < count)
            .map_or(Some(count - 1), |index| index.checked_sub(1)),
        1 => current
            .filter(|&index| index < count)
            .map_or(Some(0), |index| (index + 1 < count).then_some(index + 1)),
        2 => Some(count - 1),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::source_navigation_index;

    #[test]
    fn source_navigation_is_bounded_and_non_wrapping() {
        assert_eq!(source_navigation_index(None, 3, 1), Some(0));
        assert_eq!(source_navigation_index(None, 3, -1), Some(2));
        assert_eq!(source_navigation_index(Some(1), 3, -1), Some(0));
        assert_eq!(source_navigation_index(Some(1), 3, 1), Some(2));
        assert_eq!(source_navigation_index(Some(0), 3, -1), None);
        assert_eq!(source_navigation_index(Some(2), 3, 1), None);
        assert_eq!(source_navigation_index(Some(1), 3, -2), Some(0));
        assert_eq!(source_navigation_index(Some(1), 3, 2), Some(2));
        assert_eq!(source_navigation_index(Some(9), 3, 1), Some(0));
        assert_eq!(source_navigation_index(None, 0, 1), None);
        assert_eq!(source_navigation_index(Some(1), 3, 99), None);
    }
}
