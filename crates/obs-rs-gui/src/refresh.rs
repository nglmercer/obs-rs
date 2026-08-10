use std::{cell::RefCell, rc::Rc};

use obs_rs_media::{FrameTransform, FrameTransition};
use obs_rs_project::{Profile, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{Image, ModelRc, VecModel, Weak};

use crate::{
    project_store, scene_image, source_filters_document, source_transform_document, MainWindow,
    MixerRow, OutputRuntime, PreviewRenderer, ProfileRow, SceneRow, SourceRow,
};

pub(crate) fn dispatch_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    command: UiCommand,
) {
    let result = state.borrow_mut().dispatch(command);
    let Some(ui) = weak.upgrade() else {
        return;
    };
    if let Err(error) = result {
        ui.set_status_message(format!("Command failed: {error}").into());
    } else {
        refresh_ui(&ui, state, renderer);
    }
}

pub(crate) fn refresh_ui(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let state = state.borrow();
    let project = state.project_session().project();
    let profile_id = project.active_profile();
    let profile = project
        .profiles()
        .find(|profile| profile.id() == profile_id);
    let profile_name =
        profile.map_or_else(|| "No profile".to_owned(), |value| value.name().to_owned());

    ui.set_project_title(project.title().into());
    ui.set_profile_name(profile_name.into());
    ui.set_locale(state.locale().code().into());
    ui.set_preview_scene(state.preview_scene().unwrap_or("none").into());
    ui.set_program_scene(state.program_scene().unwrap_or("none").into());
    ui.set_transition(transition_label(state.transition()).into());
    ui.set_recording(state.recording());
    ui.set_streaming(state.streaming());
    ui.set_dirty(state.is_dirty());
    ui.set_snapshot(state.accessible_snapshot().into());
    refresh_recovery_ui(ui);

    let profile_rows = project
        .profiles()
        .map(|profile| ProfileRow {
            id: profile.id().as_str().into(),
            name: profile.name().into(),
        })
        .collect::<Vec<_>>();
    ui.set_profile_rows(ModelRc::new(VecModel::from(profile_rows)));

    let sync_error = renderer.borrow_mut().sync_project(project).err();
    let (preview_image, preview_error) = match sync_error.as_ref() {
        Some(error) => (Image::default(), Some(format!("Preview renderer: {error}"))),
        None => scene_image(renderer, state.preview_scene()),
    };
    let (program_image, program_error) = match sync_error.as_ref() {
        Some(error) => (Image::default(), Some(format!("Preview renderer: {error}"))),
        None => scene_image(renderer, state.program_scene()),
    };
    ui.set_preview_image(preview_image);
    ui.set_program_image(program_image);
    ui.set_preview_metrics(renderer.borrow().metrics_summary().into());
    let render_error = preview_error.or(program_error);
    ui.set_status_message(
        render_error
            .unwrap_or_else(|| latest_notice(&state).to_owned())
            .into(),
    );

    refresh_docks(ui, &state, profile);
}

pub(crate) fn refresh_output_ui(ui: &MainWindow, output: &Rc<RefCell<OutputRuntime>>) {
    let output = output.borrow();
    ui.set_output_status(output.output_status().into());
    ui.set_output_metrics(output.output_metrics().into());
}

fn refresh_recovery_ui(ui: &MainWindow) {
    let spanish = ui.get_locale().as_str() == "es";
    let path = ui.get_project_path().to_string();
    let status = match project_store(&path) {
        Ok(store) if store.recovery_available() => {
            if spanish {
                "Recuperación disponible: elige Recuperar para revisar el archivo temporal"
            } else {
                "Recovery available: choose Recover to review the temporary project"
            }
        }
        Ok(_) => {
            if spanish {
                "No hay archivo de recuperación"
            } else {
                "No recovery file detected"
            }
        }
        Err(error) => {
            return ui.set_recovery_status(format!("Recovery check failed: {error}").into())
        }
    };
    ui.set_recovery_status(status.into());
}

fn refresh_docks(ui: &MainWindow, state: &DesktopState, profile: Option<&Profile>) {
    let scene_rows = profile.map_or_else(Vec::new, |profile| {
        profile
            .scenes()
            .map(|scene| {
                let id = scene.id().to_string();
                let role = scene_role(state, &id);
                SceneRow {
                    id: id.into(),
                    name: scene.name().into(),
                    role: role.into(),
                }
            })
            .collect::<Vec<_>>()
    });
    ui.set_scene_rows(ModelRc::new(VecModel::from(scene_rows)));

    let source_scene = state.preview_scene().unwrap_or("none");
    let selected_scene_name = profile
        .and_then(|profile| {
            profile
                .scenes()
                .find(|scene| scene.id().as_str() == source_scene)
        })
        .map_or_else(String::new, |scene| scene.name().to_owned());
    if ui.get_scene_name_version().as_str() != selected_scene_name {
        ui.set_scene_name(selected_scene_name.into());
        ui.set_scene_name_version(ui.get_scene_name().clone());
    }
    let selected_source = state.selected_source().unwrap_or("none");
    let source_rows = profile
        .and_then(|profile| {
            profile
                .scenes()
                .find(|scene| scene.id().as_str() == source_scene)
        })
        .map_or_else(Vec::new, |scene| {
            scene
                .sources()
                .iter()
                .enumerate()
                .map(|(index, source)| SourceRow {
                    id: source.id().as_str().into(),
                    name: source.name().into(),
                    kind: source.kind().as_str().into(),
                    order: (index + 1).to_string().into(),
                    selected: source.id().as_str() == selected_source,
                    visible: source.visible(),
                    locked: source.locked(),
                })
                .collect::<Vec<_>>()
        });
    ui.set_source_scene(source_scene.into());
    ui.set_source_rows(ModelRc::new(VecModel::from(source_rows)));
    let selected_source_spec = profile
        .and_then(|profile| {
            profile
                .scenes()
                .find(|scene| scene.id().as_str() == source_scene)
        })
        .and_then(|scene| {
            scene
                .sources()
                .iter()
                .find(|source| source.id().as_str() == selected_source)
        });
    let selected_settings =
        selected_source_spec.map_or_else(String::new, |source| source.settings().serialize());
    if ui.get_source_settings_version().as_str() != selected_settings {
        ui.set_source_settings(selected_settings.into());
        ui.set_source_settings_version(ui.get_source_settings().clone());
    }
    let selected_transform =
        selected_source_spec.map_or(FrameTransform::IDENTITY, SourceSpec::transform);
    let transform_document = source_transform_document(selected_transform);
    let filters_document = selected_source_spec.map_or_else(String::new, |source| {
        source_filters_document(source.filters())
    });
    let properties_version = format!("{transform_document}\u{1f}{filters_document}");
    if ui.get_source_properties_version().as_str() != properties_version {
        ui.set_source_transform(transform_document.into());
        ui.set_source_filters(filters_document.into());
        ui.set_source_properties_version(properties_version.into());
    }
    ui.set_selected_source(selected_source.into());

    let mixer_rows = state
        .mixer_channels()
        .map(|channel| MixerRow {
            id: channel.id().into(),
            name: channel.name().into(),
            gain: f32::from(channel.gain_milli()) / 1_000.0,
            peak: f32::from(channel.peak_milli()) / 1_000.0,
            muted: channel.muted(),
        })
        .collect::<Vec<_>>();
    ui.set_mixer_rows(ModelRc::new(VecModel::from(mixer_rows)));
}

fn latest_notice(state: &DesktopState) -> &str {
    state
        .notices()
        .last()
        .map_or("Ready", |notice| notice.message())
}

fn scene_role(state: &DesktopState, id: &str) -> &'static str {
    match (
        state.preview_scene() == Some(id),
        state.program_scene() == Some(id),
    ) {
        (true, true) => "Preview / Program",
        (true, false) => "Preview",
        (false, true) => "Program",
        (false, false) => "",
    }
}

pub(crate) fn transition_label(transition: FrameTransition) -> String {
    match transition {
        FrameTransition::Cut => "Cut".to_owned(),
        FrameTransition::CrossFade { progress_milli } => {
            format!("Fade {progress_milli}/1000")
        }
    }
}
