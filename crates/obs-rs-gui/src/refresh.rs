use std::{cell::RefCell, rc::Rc, time::Instant};

use obs_rs_media::{FrameTransition, RawVideoFrame, VideoFrame};
use obs_rs_project::{Profile, SceneItemSpec, SceneSpec};
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::{Image, Model, ModelRc, SharedString, VecModel, Weak};

use crate::preview_worker::{multiview_grid_dimensions, MAX_MULTIVIEW_SCENES};
use crate::{
    frame_to_image, project_store, selection_rect, LocaleOption, MainWindow, MixerRow,
    MultiviewScene, OutputRuntime, PreviewSurface, PreviewWorker, ProfileRow, SceneRow, SourceRow,
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
    surface: &Rc<RefCell<PreviewSurface>>,
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
        refresh_ui(&ui, state, surface);
    }
}

const MAX_SOURCE_ROW_DEPTH: usize = 64;

fn append_source_rows(
    rows: &mut Vec<SourceRow>,
    profile: &Profile,
    items: &[SceneItemSpec],
    state: &DesktopState,
    group_path: &mut Vec<String>,
) {
    let count = i32::try_from(items.len()).unwrap_or(i32::MAX);
    for (index, item) in items.iter().enumerate() {
        let target = if group_path.is_empty() {
            item.id().to_string()
        } else {
            format!("{}/{}", group_path.join("/"), item.id())
        };
        let (base_name, kind, is_group) = if let Some(source) = profile
            .source(item.source_id())
            .filter(|_| item.is_source())
        {
            (
                source.name().to_owned(),
                source.kind().as_str().to_owned(),
                false,
            )
        } else if let Some(scene) = item.scene_id().and_then(|scene_id| profile.scene(scene_id)) {
            (scene.name().to_owned(), "scene".to_owned(), false)
        } else if let Some(group) = item.group() {
            (group.name().to_owned(), "group".to_owned(), true)
        } else {
            (item.source_id().as_str().to_owned(), String::new(), false)
        };
        let name = if group_path.is_empty() {
            base_name
        } else {
            format!("{}{}", "  ".repeat(group_path.len()), base_name)
        };
        rows.push(SourceRow {
            id: target.clone().into(),
            target: target.into(),
            name: name.into(),
            kind: kind.into(),
            order: (index + 1).to_string().into(),
            count,
            nested: !group_path.is_empty(),
            group: is_group,
            selected: group_path.is_empty() && state.is_source_selected(item.id().as_str()),
            visible: item.visible(),
            locked: item.locked(),
            first: index == 0,
            last: index + 1 == items.len(),
        });
        if let Some(group) = item.group() {
            if group_path.len() < MAX_SOURCE_ROW_DEPTH {
                group_path.push(item.id().to_string());
                append_source_rows(rows, profile, group.items(), state, group_path);
                group_path.pop();
            }
        }
    }
}

pub(crate) fn refresh_ui(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
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

    // Project data is cloned only when the surface really needs a rebuild;
    // the state borrow is therefore never held across the surface borrow on
    // the common refresh path.
    let needs_sync = {
        let surface = surface.borrow();
        !surface.is_synced(revision)
    };
    let sync_error = if needs_sync {
        let project = state.borrow().project_session().project().clone();
        surface.borrow_mut().sync_project(&project, revision).err()
    } else {
        None
    };
    let render_error = if let Some(error) = sync_error {
        ui.set_preview_image(Image::default());
        ui.set_program_image(Image::default());
        ui.set_multiview_image(Image::default());
        Some(format!("Preview surface: {error}"))
    } else {
        None
    };
    ui.set_status_message(render_error.unwrap_or(notice).into());
}

type RefreshedPreviewFrames = (
    Option<VideoFrame>,
    Option<VideoFrame>,
    Option<RawVideoFrame>,
    Option<VideoFrame>,
    Option<String>,
);

/// Applies the newest completed background composition without waiting for one.
pub(crate) fn refresh_preview_frames_for_view(
    ui: &MainWindow,
    worker: &PreviewWorker,
) -> RefreshedPreviewFrames {
    let Some(result) = worker.try_take_latest() else {
        return (None, None, None, None, None);
    };
    match (&result.preview_scene, &result.preview_frame) {
        (_, Some(frame)) => {
            let copy_started = Instant::now();
            let image = frame_to_image(frame);
            worker.record_frame_copy(copy_started.elapsed(), frame.format().rgba_bytes());
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
            worker.record_frame_copy(copy_started.elapsed(), frame.format().rgba_bytes());
            let update_started = Instant::now();
            ui.set_program_image(image);
            worker.record_slint_update(update_started.elapsed());
        }
        (None, None) => ui.set_program_image(Image::default()),
        (Some(_), None) => {}
    }
    if let Some(frame) = result.multiview_frame.as_ref() {
        let copy_started = Instant::now();
        let image = frame_to_image(frame);
        worker.record_frame_copy(copy_started.elapsed(), frame.format().rgba_bytes());
        let update_started = Instant::now();
        ui.set_multiview_image(image);
        worker.record_slint_update(update_started.elapsed());
    } else {
        ui.set_multiview_image(Image::default());
    }
    let performance = worker.performance();
    ui.set_preview_metrics(
        format!(
            "{} · queue={} · dropped={} · render p50/p95/p99/max={}/{}/{}/{} µs · program p95={} µs · multiview p95={} µs · copy p95={} µs bytes={} · Slint p95={} µs · callback p95={} µs",
            result.metrics,
            worker.queue_depth(),
            worker.dropped_requests(),
            nanos_to_micros(performance.preview_render.percentile_nanos(50)),
            nanos_to_micros(performance.preview_render.percentile_nanos(95)),
            nanos_to_micros(performance.preview_render.percentile_nanos(99)),
            nanos_to_micros(performance.preview_render.max_nanos()),
            nanos_to_micros(performance.program_render.percentile_nanos(95)),
            nanos_to_micros(performance.multiview_render.percentile_nanos(95)),
            nanos_to_micros(performance.frame_copy.percentile_nanos(95)),
            performance.frame_copy_bytes,
            nanos_to_micros(performance.slint_update.percentile_nanos(95)),
            nanos_to_micros(performance.ui_callback.percentile_nanos(95)),
        )
        .into(),
    );
    (
        result.preview_frame,
        result.program_frame,
        result.program_output,
        result.program_output_frame,
        result.error.map(|error| format!("Preview worker: {error}")),
    )
}

const fn nanos_to_micros(nanos: u64) -> u64 {
    nanos / 1_000
}

pub(crate) fn refresh_output_ui(ui: &MainWindow, output: &Rc<RefCell<OutputRuntime>>) {
    let output = output.borrow();
    let status = output.output_status();
    let metrics = output.output_metrics();
    let multiview = output.multiview_telemetry();
    ui.set_output_status(status.clone().into());
    ui.set_output_metrics(metrics.into());
    ui.set_multiview_status(status.into());
    ui.set_multiview_metrics(multiview.metrics.into());
    ui.set_multiview_audio_db(peak_db(multiview.audio_peak_milli));
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

#[allow(
    clippy::too_many_lines,
    reason = "the dock models are refreshed together to keep their selection state consistent"
)]
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
    let multiview_scenes = profile.map_or_else(Vec::new, |profile| {
        let scenes = profile
            .scenes()
            .take(MAX_MULTIVIEW_SCENES)
            .collect::<Vec<_>>();
        let (columns, rows) = multiview_grid_dimensions(scenes.len().max(1));
        let columns_f = f32::from(u16::try_from(columns).unwrap_or(u16::MAX));
        let rows_f = f32::from(u16::try_from(rows).unwrap_or(u16::MAX));
        let tile_width = 1.0_f32 / columns_f;
        let tile_height = 1.0_f32 / rows_f;
        scenes
            .into_iter()
            .enumerate()
            .map(|(index, scene)| {
                let preview = state.preview_scene() == Some(scene.id().as_str());
                let program = state.program_scene() == Some(scene.id().as_str());
                MultiviewScene {
                    id: scene.id().as_str().into(),
                    name: scene.name().into(),
                    role: roles.role(state, scene.id().as_str()),
                    preview,
                    program,
                    x: f32::from(u16::try_from(index % columns).unwrap_or(u16::MAX)) * tile_width,
                    y: f32::from(u16::try_from(index / columns).unwrap_or(u16::MAX)) * tile_height,
                    width: tile_width,
                    height: tile_height,
                    selected: preview || program,
                }
            })
            .collect::<Vec<_>>()
    });
    if !model_matches(&ui.get_multiview_scenes(), &multiview_scenes) {
        ui.set_multiview_scenes(ModelRc::new(VecModel::from(multiview_scenes)));
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

    // The dock selection is a scene-item ID. Source configuration is resolved
    // through the profile registry so two rows can point at the same source.
    let selected_source = state.selected_source().unwrap_or("none");
    let mut selected_item = None;
    let mut selected_source_spec = None;
    if let Some(scene) = selected_scene {
        for item in scene.items() {
            if state.is_source_selected(item.id().as_str()) {
                selected_item = Some(item);
                selected_source_spec = profile.and_then(|profile| {
                    item.is_source()
                        .then(|| profile.source(item.source_id()))
                        .flatten()
                });
            }
        }
    }
    let source_rows = selected_scene.map_or_else(Vec::new, |scene| {
        let Some(profile) = profile else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        append_source_rows(&mut rows, profile, scene.items(), state, &mut Vec::new());
        rows
    });
    ui.set_source_scene(source_scene.into());
    ui.set_source_count(selected_scene.map_or(0, |scene| {
        i32::try_from(scene.items().len()).unwrap_or(i32::MAX)
    }));
    if !model_matches(&ui.get_source_rows(), &source_rows) {
        ui.set_source_rows(ModelRc::new(VecModel::from(source_rows)));
    }
    let selected_source_index = selected_scene.and_then(|scene| {
        scene
            .items()
            .iter()
            .position(|item| item.id().as_str() == selected_source)
    });
    ui.set_selected_source_visible(selected_item.is_some_and(SceneItemSpec::visible));
    ui.set_selected_source_locked(selected_item.is_some_and(SceneItemSpec::locked));
    ui.set_selected_source_first(selected_source_index == Some(0));
    ui.set_selected_source_last(
        selected_source_index.is_some_and(|index| {
            selected_scene.is_some_and(|scene| index + 1 == scene.items().len())
        }),
    );
    ui.set_can_paste(state.can_paste_source());

    let selected_settings =
        selected_source_spec.map_or_else(String::new, |source| source.settings().serialize());
    if ui.get_source_settings_version().as_str() != selected_settings {
        ui.set_source_settings(selected_settings.into());
        ui.set_source_settings_version(ui.get_source_settings().clone());
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
    let rect = selection_rect(state, canvas);
    ui.set_item_active(rect.is_some());
    ui.set_item_locked(selected_scene.is_some_and(|scene| {
        scene
            .items()
            .iter()
            .filter(|item| state.is_source_selected(item.id().as_str()))
            .any(SceneItemSpec::locked)
    }));
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
    ui.set_selected_source_is_group(selected_item.is_some_and(SceneItemSpec::is_group));

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
            pan: f32::from(i16::try_from(channel.pan_milli()).unwrap_or(0)) / 1_000.0,
            peak_db: peak_db(channel.peak_milli()),
            peak_hold_db: peak_db(channel.peak_hold_milli()),
            muted: channel.muted(),
            clipped: channel.clipped(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_project::{ProjectCommand, SceneItemSpec};

    #[test]
    fn source_rows_expose_nested_group_targets_and_relative_order() {
        let mut project = crate::initial_project().expect("initial project");
        let mut group = SceneItemSpec::for_group("overlay-group", "Overlay group").expect("group");
        group
            .group_mut()
            .expect("group target")
            .add_item(SceneItemSpec::for_source("background").expect("group child"))
            .expect("group child attach");
        project
            .apply(ProjectCommand::AddSceneItem {
                profile: "live".to_owned(),
                scene: "preview".to_owned(),
                item: group,
            })
            .expect("add group");
        let state = DesktopState::new(project);
        let profile = state
            .project_session()
            .project()
            .active_profile_spec()
            .expect("profile");
        let scene = profile.scene("preview").expect("preview scene");
        let mut rows = Vec::new();
        append_source_rows(&mut rows, profile, scene.items(), &state, &mut Vec::new());

        let group_row = rows
            .iter()
            .find(|row| row.target == "overlay-group")
            .expect("group row");
        assert!(group_row.group);
        assert!(!group_row.nested);
        assert_eq!(group_row.count, 2);

        let child_row = rows
            .iter()
            .find(|row| row.target == "overlay-group/background")
            .expect("group child row");
        assert!(!child_row.group);
        assert!(child_row.nested);
        assert_eq!(child_row.count, 1);
        assert_eq!(child_row.order.as_str(), "1");
    }
}
