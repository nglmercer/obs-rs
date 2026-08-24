use std::{cell::RefCell, error::Error, path::PathBuf, rc::Rc};

use obs_rs_diagnostics::{AtomicDiagnosticFileWriter, DiagnosticBundle};
use obs_rs_project::{ProjectCommand, ProjectFileStore, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{ComponentHandle, Weak};

use super::output::scene_transition_spec;
use crate::{refresh_ui, source_settings_for_canvas, MainWindow, OutputRuntime, PreviewSurface};

const DISCARD_NEW_PROJECT: i32 = 4;
const DISCARD_EXIT: i32 = 5;
const DISCARD_SWITCH_COLLECTION: i32 = 6;
const DISCARD_IMPORT_COLLECTION: i32 = 7;
const DISCARD_LOAD_PROJECT: i32 = 8;
const DISCARD_RECOVER_PROJECT: i32 = 9;

pub(crate) fn install_project_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let weak = ui.as_weak();
    let save_state = Rc::clone(state);
    let save_surface = Rc::clone(surface);
    ui.on_save_project(move || {
        save_and_refresh(&weak, &save_state, &save_surface);
    });

    let weak = ui.as_weak();
    let save_discard_state = Rc::clone(state);
    let save_discard_surface = Rc::clone(surface);
    ui.on_save_discard(move |action| {
        save_and_resolve_discard(&weak, &save_discard_state, &save_discard_surface, action);
    });

    let weak = ui.as_weak();
    let load_state = Rc::clone(state);
    let load_surface = Rc::clone(surface);
    ui.on_load_project(move || {
        load_and_refresh(&weak, &load_state, &load_surface);
    });

    let weak = ui.as_weak();
    let recover_state = Rc::clone(state);
    let recover_surface = Rc::clone(surface);
    ui.on_recover_project(move || {
        recover_and_refresh(&weak, &recover_state, &recover_surface);
    });

    let weak = ui.as_weak();
    let diagnostics_state = Rc::clone(state);
    let diagnostics_surface = Rc::clone(surface);
    let diagnostics_output = Rc::clone(output);
    ui.on_export_diagnostics(move || {
        export_diagnostics(
            &weak,
            &diagnostics_state,
            &diagnostics_surface,
            &diagnostics_output,
        );
    });

    let weak = ui.as_weak();
    let add_state = Rc::clone(state);
    let add_surface = Rc::clone(surface);
    ui.on_add_scene(move |id, name| {
        add_scene_and_refresh(&weak, &add_state, &add_surface, id.as_str(), name.as_str());
    });

    let weak = ui.as_weak();
    let source_state = Rc::clone(state);
    let source_surface = Rc::clone(surface);
    ui.on_add_source(move |id, kind, name| {
        add_source_and_refresh(
            &weak,
            &source_state,
            &source_surface,
            id.as_str(),
            kind.as_str(),
            name.as_str(),
        );
    });
}

thread_local! {
    /// The store built for the most recent project path.
    ///
    /// Save, load, recover, and the recovery status check all ask for a store
    /// for the same path; validating the path and constructing the store once
    /// per path serves all of them.
    static PROJECT_STORE_CACHE: RefCell<Option<(String, Rc<ProjectFileStore>)>> =
        const { RefCell::new(None) };
}

/// Resolves the final and temporary paths for an atomic file write.
///
/// Shared by the project store and the diagnostics export, which apply the same
/// "must name a file" rule and the same `.tmp` sibling convention.
pub(crate) fn atomic_write_paths(
    path: &str,
    kind: &str,
) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let path = path.trim();
    if path.is_empty() {
        return Err(std::io::Error::other(format!("{kind} path is empty")).into());
    }
    let final_path = PathBuf::from(path);
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other(format!("{kind} path must name a file")))?;
    let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
    Ok((final_path, temp_path))
}

pub(crate) fn project_store(path: &str) -> Result<Rc<ProjectFileStore>, Box<dyn Error>> {
    let key = path.trim().to_owned();
    PROJECT_STORE_CACHE.with(|cache| {
        if let Some((cached_path, store)) = cache.borrow().as_ref() {
            if *cached_path == key {
                return Ok(Rc::clone(store));
            }
        }
        let (final_path, temp_path) = atomic_write_paths(&key, "project")?;
        let store = Rc::new(ProjectFileStore::new(final_path, temp_path)?);
        *cache.borrow_mut() = Some((key, Rc::clone(&store)));
        Ok(store)
    })
}

fn save_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<usize, Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        Ok(state.borrow_mut().save_project(&store)?)
    })();
    match result {
        Ok(bytes) => {
            crate::refresh::invalidate_recovery_cache();
            refresh_ui(&ui, state, surface);
            ui.set_status_message(format!("Saved {bytes} bytes to {path}").into());
        }
        Err(error) => ui.set_status_message(format!("Save failed: {error}").into()),
    }
}

fn save_and_resolve_discard(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    action: i32,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<usize, Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        Ok(state.borrow_mut().save_project(&store)?)
    })();
    match result {
        Ok(bytes) => {
            crate::refresh::invalidate_recovery_cache();
            refresh_ui(&ui, state, surface);
            ui.set_pending_discard(0);
            continue_after_discard_save(&ui, action);
            if action != 5 {
                ui.set_status_message(format!("Saved {bytes} bytes to {path}").into());
            }
        }
        Err(error) => ui.set_status_message(format!("Save failed: {error}").into()),
    }
}

fn continue_after_discard_save(ui: &MainWindow, action: i32) {
    match action {
        DISCARD_NEW_PROJECT => ui.invoke_new_project(),
        DISCARD_EXIT => {
            let _ = slint::quit_event_loop();
        }
        DISCARD_SWITCH_COLLECTION => ui.invoke_select_collection(ui.get_pending_collection()),
        DISCARD_IMPORT_COLLECTION => {
            ui.set_collection_transfer_path("".into());
            ui.set_project_dialog_mode(2);
            ui.set_active_modal(1);
        }
        DISCARD_LOAD_PROJECT => ui.invoke_load_project(),
        DISCARD_RECOVER_PROJECT => ui.invoke_recover_project(),
        _ => {}
    }
}

fn load_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<(), Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        state.borrow_mut().load_project_for_key(&store, &path)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            crate::refresh::invalidate_recovery_cache();
            refresh_ui(&ui, state, surface);
            ui.set_status_message(format!("Loaded project from {path}").into());
        }
        Err(error) => ui.set_status_message(format!("Load failed: {error}").into()),
    }
}

fn recover_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<bool, Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        Ok(state.borrow_mut().recover_project_for_key(&store, &path)?)
    })();
    match result {
        Ok(true) => {
            crate::refresh::invalidate_recovery_cache();
            refresh_ui(&ui, state, surface);
            ui.set_status_message(
                format!("Recovered interrupted project for {path}; save to publish it").into(),
            );
        }
        Ok(false) => ui.set_status_message("No recoverable project was found".into()),
        Err(error) => ui.set_status_message(format!("Recovery failed: {error}").into()),
    }
}

fn export_diagnostics(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_diagnostics_path().to_string();
    let result: Result<usize, Box<dyn Error>> = (|| {
        let (final_path, temp_path) = atomic_write_paths(&path, "diagnostics")?;
        let state = state.borrow();
        // The engine runs on the preview worker, so its counters are read from
        // the snapshot that worker publishes rather than from a second runtime
        // this window would otherwise have to keep alive.
        let diagnostics = surface.borrow().diagnostics();
        let (metrics, usage, limits) = (diagnostics.metrics, diagnostics.usage, diagnostics.limits);
        let mut bundle = DiagnosticBundle::new();
        bundle.insert_text("project", &state.project_document())?;
        bundle.insert_text("ui", &state.accessible_snapshot())?;
        bundle.insert_text("setup", &ui.get_setup_benchmark_summary())?;
        bundle.insert_text(
            "runtime",
            &format!(
                "render_calls={} source_requests={} source_frames={} empty_sources={} failed_sources={} contract_violations={} transformed={} filtered={} blends={} usage_plugins={} usage_source_kinds={} usage_scenes={} usage_sources={} usage_filters={} limit_plugins={} limit_source_kinds={} limit_scenes={} limit_sources={} limit_filters_per_source={}",
                metrics.render_calls(),
                metrics.source_requests(),
                metrics.source_frames(),
                metrics.empty_sources(),
                metrics.failed_sources(),
                metrics.contract_violations(),
                metrics.transformed_frames(),
                metrics.filtered_frames(),
                metrics.blended_layers(),
                usage.plugins(),
                usage.source_kinds(),
                usage.scenes(),
                usage.sources(),
                usage.filters(),
                limits.max_plugins(),
                limits.max_source_kinds(),
                limits.max_scenes(),
                limits.max_sources(),
                limits.max_filters_per_source()
            ),
        )?;
        // A source that is failing is the first thing anyone reading a bundle
        // wants to know, so it gets its own entry instead of being inferred
        // from a counter.
        bundle.insert_text(
            "source-failures",
            &if diagnostics.failures.is_empty() {
                "none".to_owned()
            } else {
                diagnostics.failures.join("\n")
            },
        )?;
        bundle.insert_text(
            "filter-diagnostics",
            &if diagnostics.filter_diagnostics.is_empty() {
                "none".to_owned()
            } else {
                diagnostics.filter_diagnostics.join("\n")
            },
        )?;
        bundle.insert_text("output", &output.borrow_mut().diagnostics_document())?;
        let mut writer = AtomicDiagnosticFileWriter::new(final_path, temp_path)?;
        Ok(writer.finalize(&bundle)?)
    })();
    match result {
        Ok(bytes) => ui.set_status_message(format!("Diagnostics exported: {bytes} bytes").into()),
        Err(error) => ui.set_status_message(format!("Diagnostics failed: {error}").into()),
    }
}

fn add_scene_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    id: &str,
    name: &str,
) {
    let profile = state
        .borrow()
        .project_session()
        .project()
        .active_profile()
        .to_string();
    let result: Result<(), Box<dyn Error>> = (|| {
        let scene = SceneSpec::new(id, name)?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddScene {
                profile,
                scene,
            }))?;
        // OBS makes a newly created scene the current preview scene. Keep the
        // selection as a UI-state transition next to the project mutation so
        // the project model does not gain a second active-scene field.
        state
            .borrow_mut()
            .dispatch(UiCommand::SelectPreviewScene { id: id.to_owned() })?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => {
            refresh_ui(&ui, state, surface);
            ui.set_new_scene_id("".into());
            ui.set_new_scene_name("".into());
        }
        Err(error) => ui.set_status_message(format!("Add scene failed: {error}").into()),
    }
}

pub(crate) fn apply_scene_properties_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    name: &str,
    transition_index: i32,
    duration: &str,
    color: &str,
) {
    let result: Result<(), Box<dyn Error>> = (|| {
        let profile = state
            .borrow()
            .project_session()
            .project()
            .active_profile()
            .to_string();
        let scene = state
            .borrow()
            .preview_scene()
            .map(str::to_owned)
            .ok_or_else(|| std::io::Error::other("no preview scene is selected"))?;
        let transition = match transition_index {
            0 => None,
            1 => {
                Some(scene_transition_spec("cut", duration, color).map_err(std::io::Error::other)?)
            }
            2 => Some(
                scene_transition_spec("cross_fade", duration, color)
                    .map_err(std::io::Error::other)?,
            ),
            3 => Some(
                scene_transition_spec("fade_to_color", duration, color)
                    .map_err(std::io::Error::other)?,
            ),
            _ => {
                return Err(std::io::Error::other("Scene transition selection is invalid").into());
            }
        };
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneProperties {
                profile,
                scene,
                name: name.to_owned(),
                transition,
            }))?;
        Ok(())
    })();
    match result {
        Ok(()) => refresh_ui(ui, state, surface),
        Err(error) => ui.set_status_message(format!("Scene properties failed: {error}").into()),
    }
}

pub(crate) fn duplicate_scene_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    scene_id: &str,
) {
    let profile = state
        .borrow()
        .project_session()
        .project()
        .active_profile()
        .to_string();
    let result: Result<(), Box<dyn Error>> = (|| {
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::DuplicateScene {
                profile,
                scene: scene_id.to_owned(),
            }))?;
        // Profile::add_scene appends the validated duplicate to the persistent
        // order. Read that result back from the project instead of rebuilding
        // the command's identifier-suffix policy in the GUI.
        let duplicate = state
            .borrow()
            .project_session()
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scenes().last())
            .map(|scene| scene.id().to_string())
            .ok_or_else(|| std::io::Error::other("duplicated scene is unavailable"))?;
        state
            .borrow_mut()
            .dispatch(UiCommand::SelectPreviewScene { id: duplicate })?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => refresh_ui(&ui, state, surface),
        Err(error) => ui.set_status_message(format!("Duplicate scene failed: {error}").into()),
    }
}

pub(crate) fn add_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    id: &str,
    kind: &str,
    name: &str,
) {
    let (profile, scene, canvas_width, canvas_height) = {
        let state = state.borrow();
        let profile = state
            .project_session()
            .project()
            .active_profile()
            .to_string();
        let scene = state
            .preview_scene()
            .map(str::to_owned)
            .ok_or_else(|| std::io::Error::other("no preview scene is selected"));
        let (canvas_width, canvas_height) = state
            .project_session()
            .project()
            .active_profile_spec()
            .map_or((1_280, 720), |profile| {
                let format = profile.video_format();
                (format.width(), format.height())
            });
        (profile, scene, canvas_width, canvas_height)
    };
    let result: Result<(), Box<dyn Error>> = (|| {
        let scene = scene?;
        let source = SourceSpec::new(
            id,
            kind,
            name,
            source_settings_for_canvas(kind, canvas_width, canvas_height)?,
        )?;
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::AddSource {
                profile,
                scene,
                source,
            }))?;
        state
            .borrow_mut()
            .dispatch(UiCommand::SelectSource { id: id.to_owned() })?;
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => {
            refresh_ui(&ui, state, surface);
            ui.set_new_source_id("".into());
            ui.set_new_source_name("".into());
        }
        Err(error) => ui.set_status_message(format!("Add source failed: {error}").into()),
    }
}
