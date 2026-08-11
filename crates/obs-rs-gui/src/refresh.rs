use std::{cell::RefCell, rc::Rc};

use obs_rs_media::{FrameTransform, FrameTransition, VideoFrame};
use obs_rs_project::{Profile, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::{Image, Model, ModelRc, SharedString, VecModel, Weak};

use crate::{
    project_store, source_filters_document, source_transform_document, LocaleOption, MainWindow,
    MixerRow, OutputRuntime, PreviewRenderer, ProfileRow, SceneRow, SourceRow,
};

thread_local! {
    /// The immutable locale picker model, shared by every refresh.
    static LOCALE_OPTIONS: ModelRc<LocaleOption> = ModelRc::new(VecModel::from(
        UiLocale::supported()
            .iter()
            .map(|locale| LocaleOption {
                code: locale.code().into(),
                label: locale.code().to_ascii_uppercase().into(),
            })
            .collect::<Vec<_>>(),
    ));
}

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
        let locale = state.borrow().locale();
        let prefix = crate::i18n::with_catalog(locale, |text| text.command_failed.clone());
        ui.set_status_message(format!("{prefix}{error}").into());
    } else {
        refresh_ui(&ui, state, renderer);
    }
}

pub(crate) fn refresh_ui(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let (revision, preview_scene, program_scene, notice) = {
        let state = state.borrow();
        let locale = state.locale();
        crate::i18n::apply_if_changed(ui, locale);
        let project = state.project_session().project();
        let profile = project.active_profile_spec();
        let profile_name = profile.map_or_else(
            || crate::i18n::with_catalog(locale, |text| text.no_profile.to_string()),
            |value| value.name().to_owned(),
        );

        ui.set_project_title(project.title().into());
        ui.set_profile_name(profile_name.into());
        ui.set_locale(locale.code().into());
        // The supported-locale list is static, so install the shared model
        // only the first time this component tree is refreshed.
        if ui.get_locale_options().row_count() == 0 {
            ui.set_locale_options(LOCALE_OPTIONS.with(Clone::clone));
        }
        ui.set_preview_scene(state.preview_scene().unwrap_or("none").into());
        ui.set_program_scene(state.program_scene().unwrap_or("none").into());
        ui.set_transition(transition_label_for_locale(locale, state.transition()).into());
        ui.set_recording(state.recording());
        ui.set_streaming(state.streaming());
        ui.set_dirty(state.is_dirty());
        ui.set_snapshot(state.accessible_snapshot().into());
        refresh_recovery_ui(ui, locale);

        let profile_rows = project
            .profiles()
            .map(|profile| ProfileRow {
                id: profile.id().as_str().into(),
                name: profile.name().into(),
            })
            .collect::<Vec<_>>();
        if !model_matches(&ui.get_profile_rows(), &profile_rows) {
            ui.set_profile_rows(ModelRc::new(VecModel::from(profile_rows)));
        }

        refresh_docks(ui, &state, profile);
        (
            state.project_session().revision(),
            state.preview_scene().map(str::to_owned),
            state.program_scene().map(str::to_owned),
            latest_notice(&state).to_owned(),
        )
    };

    // Project data is cloned only when the renderer really needs a rebuild;
    // the state borrow is therefore never held across the renderer borrow on
    // the common refresh path.
    let needs_sync = {
        let renderer = renderer.borrow();
        !renderer.is_synced(revision)
    };
    let sync_error = if needs_sync {
        let project = state.borrow().project_session().project().clone();
        renderer.borrow_mut().sync_project(&project, revision).err()
    } else {
        None
    };
    let render_error = if let Some(error) = sync_error {
        ui.set_preview_image(Image::default());
        ui.set_program_image(Image::default());
        ui.set_preview_metrics(renderer.borrow().metrics_summary().into());
        Some(format!("Preview renderer: {error}"))
    } else {
        let (_, render_error) = refresh_preview_frames(
            ui,
            renderer,
            preview_scene.as_deref(),
            program_scene.as_deref(),
        );
        render_error
    };
    ui.set_status_message(render_error.unwrap_or(notice).into());
}

/// Renders the two stage images for one animation tick and returns the program
/// frame so an active output can consume the exact frame shown to the user.
pub(crate) fn refresh_preview_frames(
    ui: &MainWindow,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    preview_scene: Option<&str>,
    program_scene: Option<&str>,
) -> (Option<VideoFrame>, Option<String>) {
    refresh_preview_frames_for_view(ui, renderer, preview_scene, program_scene, true)
}

/// Renders the preview plus the program only when the current view or an active
/// output needs it. Single-canvas editing otherwise avoids a second full-size
/// composition and a second Slint image update on every timer tick.
pub(crate) fn refresh_preview_frames_for_view(
    ui: &MainWindow,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    preview_scene: Option<&str>,
    program_scene: Option<&str>,
    render_program: bool,
) -> (Option<VideoFrame>, Option<String>) {
    let (preview_image, preview_error, program_image, program_frame, program_error, metrics) = {
        let mut renderer = renderer.borrow_mut();
        let (preview_image, preview_frame, preview_error) =
            render_scene_image(&mut renderer, preview_scene);
        let (program_image, program_frame, program_error) = if !render_program {
            (ui.get_program_image(), None, None)
        } else if preview_scene == program_scene {
            (preview_image.clone(), preview_frame, preview_error.clone())
        } else {
            render_scene_image(&mut renderer, program_scene)
        };
        let metrics = renderer.metrics_summary();
        (
            preview_image,
            preview_error,
            program_image,
            program_frame,
            program_error,
            metrics,
        )
    };

    ui.set_preview_image(preview_image);
    ui.set_program_image(program_image);
    ui.set_preview_metrics(metrics.into());
    (program_frame, preview_error.or(program_error))
}

fn render_scene_image(
    renderer: &mut PreviewRenderer,
    scene: Option<&str>,
) -> (Image, Option<VideoFrame>, Option<String>) {
    let Some(scene) = scene else {
        return (Image::default(), None, None);
    };
    match renderer.render(scene) {
        Ok(Some(frame)) => (renderer.image_for_scene(scene, &frame), Some(frame), None),
        Ok(None) => (
            Image::default(),
            None,
            Some(format!("Scene {scene} has no frame")),
        ),
        Err(error) => (
            Image::default(),
            None,
            Some(format!("Preview renderer: {error}")),
        ),
    }
}

pub(crate) fn refresh_output_ui(ui: &MainWindow, output: &Rc<RefCell<OutputRuntime>>) {
    let output = output.borrow();
    ui.set_output_status(output.output_status().into());
    ui.set_output_metrics(output.output_metrics().into());
}

thread_local! {
    /// Last project path a recovery check ran against, with its result.
    ///
    /// The check builds a `ProjectFileStore` and validates a path; at 30 fps
    /// that repeated filesystem work every tick for an answer that only changes
    /// when the project path does.
    static RECOVERY_CACHE: RefCell<Option<(String, UiLocale, SharedString)>> =
        const { RefCell::new(None) };
}

fn refresh_recovery_ui(ui: &MainWindow, locale: UiLocale) {
    let path = ui.get_project_path().to_string();
    let status = RECOVERY_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((cached_path, cached_locale, status)) = cache.as_ref() {
            if *cached_path == path && *cached_locale == locale {
                return status.clone();
            }
        }
        let status: SharedString =
            crate::i18n::with_catalog(locale, |text| match project_store(&path) {
                Ok(store) if store.recovery_available() => text.recovery_available.clone(),
                Ok(_) => text.no_recovery.clone(),
                Err(error) => format!("Recovery check failed: {error}").into(),
            });
        *cache = Some((path.clone(), locale, status.clone()));
        status
    });
    ui.set_recovery_status(status);
}

/// Clears the cached recovery status so the next refresh re-checks the store.
pub(crate) fn invalidate_recovery_cache() {
    RECOVERY_CACHE.with(|cache| cache.borrow_mut().take());
}

fn refresh_docks(ui: &MainWindow, state: &DesktopState, profile: Option<&Profile>) {
    let locale = state.locale();
    let roles = SceneRoleLabels::for_locale(locale);
    let scene_rows = profile.map_or_else(Vec::new, |profile| {
        profile
            .scenes()
            .map(|scene| SceneRow {
                role: roles.role(state, scene.id().as_str()),
                id: scene.id().as_str().into(),
                name: scene.name().into(),
            })
            .collect::<Vec<_>>()
    });
    if !model_matches(&ui.get_scene_rows(), &scene_rows) {
        ui.set_scene_rows(ModelRc::new(VecModel::from(scene_rows)));
    }

    let source_scene = state.preview_scene().unwrap_or("none");
    // The selected scene was previously located three separate times: once for
    // its name, once for its source rows, and once for the selected source.
    // One keyed lookup now serves all three.
    let selected_scene = profile.and_then(|profile| profile.scene(source_scene));

    let selected_scene_name = selected_scene.map_or("", SceneSpec::name);
    if ui.get_scene_name_version().as_str() != selected_scene_name {
        ui.set_scene_name(selected_scene_name.into());
        ui.set_scene_name_version(ui.get_scene_name().clone());
    }

    let selected_source = state.selected_source().unwrap_or("none");
    let mut selected_source_spec = None;
    let source_rows = selected_scene.map_or_else(Vec::new, |scene| {
        scene
            .sources()
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let selected = source.id().as_str() == selected_source;
                if selected {
                    selected_source_spec = Some(source);
                }
                SourceRow {
                    id: source.id().as_str().into(),
                    name: source.name().into(),
                    kind: source.kind().as_str().into(),
                    order: (index + 1).to_string().into(),
                    selected,
                    visible: source.visible(),
                    locked: source.locked(),
                }
            })
            .collect::<Vec<_>>()
    });
    ui.set_source_scene(source_scene.into());
    if !model_matches(&ui.get_source_rows(), &source_rows) {
        ui.set_source_rows(ModelRc::new(VecModel::from(source_rows)));
    }

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
    if !model_matches(&ui.get_mixer_rows(), &mixer_rows) {
        ui.set_mixer_rows(ModelRc::new(VecModel::from(mixer_rows)));
    }
}

fn model_matches<T: PartialEq>(model: &ModelRc<T>, expected: &[T]) -> bool {
    model.row_count() == expected.len()
        && expected
            .iter()
            .enumerate()
            .all(|(index, expected)| model.row_data(index).as_ref() == Some(expected))
}

fn latest_notice(state: &DesktopState) -> &str {
    state
        .notices()
        .last()
        .map_or("Ready", |notice| notice.message())
}

/// The four role labels for one locale, resolved once per refresh.
struct SceneRoleLabels {
    preview_program: SharedString,
    preview: SharedString,
    program: SharedString,
}

impl SceneRoleLabels {
    /// Reads the role labels from the cached catalog a single time.
    fn for_locale(locale: UiLocale) -> Self {
        crate::i18n::with_catalog(locale, |text| Self {
            preview_program: text.preview_program_role.clone(),
            preview: text.preview_role.clone(),
            program: text.program_role.clone(),
        })
    }

    /// Returns the label for one scene without touching the catalog again.
    fn role(&self, state: &DesktopState, id: &str) -> SharedString {
        match (
            state.preview_scene() == Some(id),
            state.program_scene() == Some(id),
        ) {
            (true, true) => self.preview_program.clone(),
            (true, false) => self.preview.clone(),
            (false, true) => self.program.clone(),
            (false, false) => SharedString::new(),
        }
    }
}

pub(crate) fn transition_label_for_locale(locale: UiLocale, transition: FrameTransition) -> String {
    // Borrows the two strings it needs from the cached catalog instead of
    // materializing a whole catalog for them.
    crate::i18n::with_catalog(locale, |text| match transition {
        FrameTransition::Cut => text.cut.to_string(),
        FrameTransition::CrossFade { progress_milli } => {
            format!("{} {progress_milli}/1000", text.fade)
        }
    })
}
