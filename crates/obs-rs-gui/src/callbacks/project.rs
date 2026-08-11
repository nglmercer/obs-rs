use std::{cell::RefCell, error::Error, path::PathBuf, rc::Rc};

use obs_rs_diagnostics::{AtomicDiagnosticFileWriter, DiagnosticBundle};
use obs_rs_project::{ProjectCommand, ProjectFileStore, SceneSpec, SourceSpec};
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{ComponentHandle, Weak};

use crate::{refresh_ui, source_settings, MainWindow, OutputRuntime, PreviewRenderer};

pub(crate) fn install_project_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let weak = ui.as_weak();
    let save_state = Rc::clone(state);
    let save_renderer = Rc::clone(renderer);
    ui.on_save_project(move || {
        save_and_refresh(&weak, &save_state, &save_renderer);
    });

    let weak = ui.as_weak();
    let load_state = Rc::clone(state);
    let load_renderer = Rc::clone(renderer);
    ui.on_load_project(move || {
        load_and_refresh(&weak, &load_state, &load_renderer);
    });

    let weak = ui.as_weak();
    let recover_state = Rc::clone(state);
    let recover_renderer = Rc::clone(renderer);
    ui.on_recover_project(move || {
        recover_and_refresh(&weak, &recover_state, &recover_renderer);
    });

    let weak = ui.as_weak();
    let diagnostics_state = Rc::clone(state);
    let diagnostics_renderer = Rc::clone(renderer);
    let diagnostics_output = Rc::clone(output);
    ui.on_export_diagnostics(move || {
        export_diagnostics(
            &weak,
            &diagnostics_state,
            &diagnostics_renderer,
            &diagnostics_output,
        );
    });

    let weak = ui.as_weak();
    let add_state = Rc::clone(state);
    let add_renderer = Rc::clone(renderer);
    ui.on_add_scene(move |id, name| {
        add_scene_and_refresh(&weak, &add_state, &add_renderer, id.as_str(), name.as_str());
    });

    let weak = ui.as_weak();
    let source_state = Rc::clone(state);
    let source_renderer = Rc::clone(renderer);
    ui.on_add_source(move |id, kind, name| {
        add_source_and_refresh(
            &weak,
            &source_state,
            &source_renderer,
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
    renderer: &Rc<RefCell<PreviewRenderer>>,
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
            refresh_ui(&ui, state, renderer);
            ui.set_status_message(format!("Saved {bytes} bytes to {path}").into());
        }
        Err(error) => ui.set_status_message(format!("Save failed: {error}").into()),
    }
}

fn load_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<(), Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        state.borrow_mut().load_project(&store)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            crate::refresh::invalidate_recovery_cache();
            refresh_ui(&ui, state, renderer);
            ui.set_status_message(format!("Loaded project from {path}").into());
        }
        Err(error) => ui.set_status_message(format!("Load failed: {error}").into()),
    }
}

fn recover_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_project_path().to_string();
    let result: Result<bool, Box<dyn Error>> = (|| {
        let store = project_store(&path)?;
        Ok(state.borrow_mut().recover_project(&store)?)
    })();
    match result {
        Ok(true) => {
            crate::refresh::invalidate_recovery_cache();
            refresh_ui(&ui, state, renderer);
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
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
) {
    let Some(ui) = weak.upgrade() else {
        return;
    };
    let path = ui.get_diagnostics_path().to_string();
    let result: Result<usize, Box<dyn Error>> = (|| {
        let (final_path, temp_path) = atomic_write_paths(&path, "diagnostics")?;
        let state = state.borrow();
        // One borrow of the renderer for all three snapshots.
        let (metrics, usage, limits) = {
            let renderer = renderer.borrow();
            (
                renderer.runtime.compositor_metrics(),
                renderer.runtime.usage(),
                renderer.runtime.limits(),
            )
        };
        let mut bundle = DiagnosticBundle::new();
        bundle.insert_text("project", &state.project_document())?;
        bundle.insert_text("ui", &state.accessible_snapshot())?;
        bundle.insert_text(
            "runtime",
            &format!(
                "render_calls={} source_requests={} source_frames={} empty_sources={} transformed={} filtered={} blends={} usage_plugins={} usage_source_kinds={} usage_scenes={} usage_sources={} usage_filters={} limit_plugins={} limit_source_kinds={} limit_scenes={} limit_sources={} limit_filters_per_item={}",
                metrics.render_calls(),
                metrics.source_requests(),
                metrics.source_frames(),
                metrics.empty_sources(),
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
                limits.max_filters_per_item()
            ),
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
    renderer: &Rc<RefCell<PreviewRenderer>>,
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
        Ok(())
    })();
    let Some(ui) = weak.upgrade() else {
        return;
    };
    match result {
        Ok(()) => {
            refresh_ui(&ui, state, renderer);
            ui.set_new_scene_id("".into());
            ui.set_new_scene_name("".into());
        }
        Err(error) => ui.set_status_message(format!("Add scene failed: {error}").into()),
    }
}

pub(crate) fn rename_scene_and_refresh(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    name: &str,
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
        state
            .borrow_mut()
            .dispatch(UiCommand::Project(ProjectCommand::SetSceneName {
                profile,
                scene,
                name: name.to_owned(),
            }))?;
        Ok(())
    })();
    match result {
        Ok(()) => refresh_ui(ui, state, renderer),
        Err(error) => ui.set_status_message(format!("Rename scene failed: {error}").into()),
    }
}

pub(crate) fn add_source_and_refresh(
    weak: &Weak<MainWindow>,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    id: &str,
    kind: &str,
    name: &str,
) {
    let (profile, scene) = {
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
        (profile, scene)
    };
    let result: Result<(), Box<dyn Error>> = (|| {
        let scene = scene?;
        let source = SourceSpec::new(id, kind, name, source_settings(kind)?)?;
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
            refresh_ui(&ui, state, renderer);
            ui.set_new_source_id("".into());
            ui.set_new_source_name("".into());
        }
        Err(error) => ui.set_status_message(format!("Add source failed: {error}").into()),
    }
}
