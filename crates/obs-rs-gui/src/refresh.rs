use std::{cell::RefCell, rc::Rc, time::Instant};

use obs_rs_media::{FrameTransform, FrameTransition, RawVideoFrame, VideoFrame};
use obs_rs_project::{Profile, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::{Image, Model, ModelRc, SharedString, VecModel, Weak};

use crate::{
    frame_to_image, project_store, source_filters_document, source_transform_document,
    LocaleOption, MainWindow, MixerRow, OutputRuntime, PreviewRenderer, PreviewWorker, ProfileRow,
    SceneRow, SourceRow,
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
    let (revision, notice) = {
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

        let active_profile = project.active_profile().as_str();
        let profile_rows = project
            .profiles()
            .map(|profile| ProfileRow {
                id: profile.id().as_str().into(),
                name: profile.name().into(),
                active: profile.id().as_str() == active_profile,
            })
            .collect::<Vec<_>>();
        if !model_matches(&ui.get_profile_rows(), &profile_rows) {
            ui.set_profile_rows(ModelRc::new(VecModel::from(profile_rows)));
        }

        // The Edit menu greys out an entry it cannot honour, so the history
        // depth has to reach the window on every refresh, not only after an
        // edit that happens to come through this path.
        ui.set_can_undo(state.can_undo());
        ui.set_can_redo(state.can_redo());

        refresh_docks(ui, &state, profile);
        (
            state.project_session().revision(),
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
        None
    };
    ui.set_status_message(render_error.unwrap_or(notice).into());
}

/// Applies the newest completed background composition without waiting for one.
pub(crate) fn refresh_preview_frames_for_view(
    ui: &MainWindow,
    worker: &PreviewWorker,
) -> (
    Option<VideoFrame>,
    Option<VideoFrame>,
    Option<RawVideoFrame>,
    Option<String>,
) {
    let Some(result) = worker.try_take_latest() else {
        return (None, None, None, None);
    };
    match (&result.preview_scene, &result.preview_frame) {
        (_, Some(frame)) => {
            let copy_started = Instant::now();
            let image = frame_to_image(frame);
            worker.record_frame_copy(copy_started.elapsed());
            let update_started = Instant::now();
            ui.set_preview_image(image);
            worker.record_slint_update(update_started.elapsed());
        }
        (None, None) => ui.set_preview_image(Image::default()),
        (Some(_), None) => {}
    }
    match (&result.program_scene, &result.program_frame) {
        (_, Some(frame)) => {
            let copy_started = Instant::now();
            let image = frame_to_image(frame);
            worker.record_frame_copy(copy_started.elapsed());
            let update_started = Instant::now();
            ui.set_program_image(image);
            worker.record_slint_update(update_started.elapsed());
        }
        (None, None) => ui.set_program_image(Image::default()),
        (Some(_), None) => {}
    }
    let performance = worker.performance();
    ui.set_preview_metrics(
        format!(
            "{} · queue={} · dropped={} · render p50/p95/p99/max={}/{}/{}/{} µs · program p95={} µs · copy p95={} µs · Slint p95={} µs · callback p95={} µs",
            result.metrics,
            worker.queue_depth(),
            worker.dropped_requests(),
            nanos_to_micros(performance.preview_render.percentile_nanos(50)),
            nanos_to_micros(performance.preview_render.percentile_nanos(95)),
            nanos_to_micros(performance.preview_render.percentile_nanos(99)),
            nanos_to_micros(performance.preview_render.max_nanos()),
            nanos_to_micros(performance.program_render.percentile_nanos(95)),
            nanos_to_micros(performance.frame_copy.percentile_nanos(95)),
            nanos_to_micros(performance.slint_update.percentile_nanos(95)),
            nanos_to_micros(performance.ui_callback.percentile_nanos(95)),
        )
        .into(),
    );
    (
        result.preview_frame,
        result.program_frame,
        result.program_output,
        result.error.map(|error| format!("Preview worker: {error}")),
    )
}

const fn nanos_to_micros(nanos: u64) -> u64 {
    nanos / 1_000
}

pub(crate) fn refresh_output_ui(ui: &MainWindow, output: &Rc<RefCell<OutputRuntime>>) {
    let output = output.borrow();
    ui.set_output_status(output.output_status().into());
    ui.set_output_metrics(output.output_metrics().into());
    ui.set_recording_elapsed(output.recording_elapsed().into());
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
    // The canvas overlay needs the item's rectangle in canvas pixels, which is
    // derived from the same transform the compositor uses.
    let canvas = (
        u32::try_from(ui.get_canvas_width()).unwrap_or(1_920).max(1),
        u32::try_from(ui.get_canvas_height())
            .unwrap_or(1_080)
            .max(1),
    );
    let rect = selected_source_spec.map(|source| crate::item_rect(source.transform(), canvas));
    ui.set_item_active(rect.is_some());
    ui.set_item_locked(selected_source_spec.is_some_and(SourceSpec::locked));
    if let Some(rect) = rect {
        ui.set_item_x(i32::try_from(rect.x).unwrap_or(0));
        ui.set_item_y(i32::try_from(rect.y).unwrap_or(0));
        ui.set_item_width(i32::try_from(rect.width).unwrap_or(0));
        ui.set_item_height(i32::try_from(rect.height).unwrap_or(0));
    }
    // Only a display-backed source offers the picker, so the docks derive the
    // affordance from the selected row instead of guessing from the name.
    ui.set_selected_source_is_screen(
        selected_source_spec
            .is_some_and(|source| crate::kind_selects_monitor(source.kind().as_str())),
    );

    refresh_mixer_rows(ui, state);
}

/// Rebuilds the mixer dock rows from the desktop state.
///
/// The live input meter refreshes far more often than the scene graph, so it
/// updates these rows on their own rather than running a whole dock refresh.
pub(crate) fn refresh_mixer_rows(ui: &MainWindow, state: &DesktopState) {
    let mixer_rows = state
        .mixer_channels()
        .map(|channel| MixerRow {
            id: channel.id().into(),
            name: channel.name().into(),
            gain: f32::from(channel.gain_milli()) / 1_000.0,
            peak_db: peak_db(channel.peak_milli()),
            muted: channel.muted(),
        })
        .collect::<Vec<_>>();
    if !model_matches(&ui.get_mixer_rows(), &mixer_rows) {
        ui.set_mixer_rows(ModelRc::new(VecModel::from(mixer_rows)));
    }
}

/// Converts a bounded linear peak to the conventional -60..0 dBFS meter.
pub(crate) fn peak_db(peak_milli: u16) -> f32 {
    if peak_milli == 0 {
        -60.0
    } else {
        (20.0 * (f32::from(peak_milli) / 1_000.0).log10()).clamp(-60.0, 0.0)
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
