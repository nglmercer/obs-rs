//! Slint desktop control room for the Rust-owned OBS-RS state machine.

// `deny`, not `forbid`, and deliberately so: this binary includes Slint's
// generated UI module, which carries its own `#[allow(unsafe_code)]`
// attributes. `forbid` cannot be lifted by an inner `allow`, so it fails the
// build outright. `deny` gives the same "no hand-written unsafe" guarantee for
// this crate's own code while letting the generated module compile. Every other
// crate in the workspace uses `forbid`.
#![deny(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_audio::AudioFormat;
use obs_rs_ui::{DesktopState, UiCommand};
use slint::{CloseRequestResponse, ComponentHandle};

mod callbacks;
mod dock_tree;
mod filter_properties;
mod fixtures;
mod i18n;
mod output;
mod preview;
mod preview_benchmark;
mod preview_worker;
mod properties;
mod refresh;
mod settings;
mod settings_model;
mod view;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use callbacks::install_dock_callbacks;
#[cfg(test)]
pub(crate) use callbacks::selected_target;
pub(crate) use callbacks::{
    apply_source_name_and_refresh, apply_source_settings_and_refresh, apply_source_settings_to,
    apply_source_transform_to, apply_source_transforms_to, duplicate_scene_and_refresh,
    duplicate_source_and_refresh, flip_source_and_refresh, item_for_target,
    move_source_and_refresh, move_source_to_and_refresh, move_source_to_group_and_refresh,
    project_store, remove_scene_and_refresh, remove_source_and_refresh, rename_scene_and_refresh,
    reset_source_transform_and_refresh, scene_item_target, source_target, source_target_is_locked,
    source_transform_document, target_settings_document, toggle_source_locked_and_refresh,
    toggle_source_visibility_and_refresh, transform_source_and_refresh, SceneItemTarget,
    SourceTarget,
};
pub(crate) use callbacks::{
    install_add_source_window, install_callbacks, install_canvas_callbacks,
    install_dock_callbacks_with_layout, install_menu_callbacks, install_monitor_window,
    install_settings_window, install_setup_window, install_source_filters_window,
    install_source_properties_window, install_source_transform_window, selection_overlay,
    set_selection_overlay, start_preview_timer, PeerWindows, ProjectorController,
};
#[cfg(test)]
pub(crate) use fixtures::source_settings;
pub(crate) use fixtures::{
    capture_devices, initial_project, kind_runs_in_this_session, kind_selects_monitor,
    kind_uses_portal, platform_capture_summary, source_settings_for_canvas,
};
pub(crate) use output::OutputRuntime;
pub(crate) use preview::{frame_to_image, PreviewRenderer, PreviewSurface};
pub(crate) use preview_benchmark::run_gui_setup_benchmark;
pub(crate) use preview_worker::PreviewWorker;
pub(crate) use refresh::{
    dispatch_and_refresh, refresh_output_ui, refresh_preview_frames_for_view, refresh_ui,
};
pub(crate) use settings::AppSettings;
pub(crate) use view::{
    AddSourceText, AddSourceWindow, DockPane, DockSplitter, FloatingDockWindow, I18n, LocaleOption,
    MainWindow, Metrics, MixerRow, MonitorRow, MonitorText, MonitorWindow, MoveTarget,
    MultiviewScene, Palette, ProfileRow, ProjectorWindow, PropertyRow, PropertyText, SceneRow,
    SettingsText, SettingsWindow, SetupWindow, SourceCandidate, SourceFilterRow,
    SourceFiltersWindow, SourceKindRow, SourcePropertiesWindow, SourceRow, SourceTransformWindow,
    ThemeTokens, UiMetrics, UiText,
};

/// Mixer channel backed by the engine's live capture input.
///
/// The engine opens one input device — the microphone or line input chosen on
/// the settings window's Audio page — so this is the channel whose fader, mute,
/// and meter correspond to real audio rather than to a placeholder.
pub(crate) const MIC_CHANNEL_ID: &str = "mic";

/// Mixer channel backed by the playback monitor the engine records.
///
/// A machine with no readable monitor keeps this channel silent rather than
/// hiding it: its fader and mute still apply to the mix, and the row says what
/// it is capturing so a silent meter is never mistaken for a broken one.
pub(crate) const DESKTOP_CHANNEL_ID: &str = "desktop";

#[allow(
    clippy::too_many_lines,
    reason = "startup wires the complete desktop lifecycle in one boundary"
)]
fn main() -> Result<(), Box<dyn Error>> {
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    // `--settings-screenshot <page> [file]` renders one settings page through
    // the software renderer at a pinned appearance and exits. It is the visual
    // regression entry point: two runs of the same page differ only where the
    // layout differs.
    let screenshot = screenshot_request();
    if screenshot.is_some() {
        // The snapshot has to come from a renderer that can read its own
        // output back, and from a clock that does not advance between runs.
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                ..Default::default()
            },
        )))
        .map_err(|error| format!("screenshot backend: {error}"))?;
    } else if smoke {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new()?;
    // Stored settings own the file paths and the stream destination, so they
    // are loaded before anything reads them.
    // A screenshot run must not depend on whatever this machine's settings
    // file happens to contain, so it starts from the shipped defaults.
    let settings_load = if screenshot.is_some() {
        settings::SettingsLoad {
            settings: AppSettings::default(),
            show_setup: false,
        }
    } else {
        settings::AppSettings::load_with_status(&settings::settings_path())
    };
    let show_setup = settings_load.show_setup;
    let settings = settings_load.settings;
    ui.set_new_source_kind("test_pattern".into());
    ui.set_capture_capabilities(platform_capture_summary().into());
    let project = initial_project()?;
    // Revision 0 is the "nothing observed yet" sentinel, so the first refresh
    // always syncs the surface against the live session.
    let surface = Rc::new(RefCell::new(PreviewSurface::new(&project, 0)?));
    {
        // The canvas size drives the zoom readout under the preview.
        let format = surface.borrow().format;
        ui.set_canvas_width(i32::try_from(format.width()).unwrap_or(1920));
        ui.set_canvas_height(i32::try_from(format.height()).unwrap_or(1080));
    }
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    state
        .borrow_mut()
        .set_project_selection_key(settings.project_path.as_str());
    let shortcut_status = match settings::shortcut_bindings(&settings) {
        Ok(bindings) => state
            .borrow_mut()
            .replace_shortcuts(&bindings)
            .err()
            .map(|error| format!("Hotkeys disabled: {error}")),
        Err(error) => Some(format!("Hotkeys disabled: {error}")),
    };
    // The stored project is reopened before anything renders, so a session
    // resumes with the scenes and sources it was left with rather than with the
    // starter fixture.
    let restored = restore_project(&state, &settings);
    if restored_project_loaded(restored.as_deref()) {
        state
            .borrow_mut()
            .restore_project_selections(&settings.project_scene_selections);
        state
            .borrow_mut()
            .restore_project_selection_for_current_key();
    }
    let (preview_project, preview_revision) = {
        let state = state.borrow();
        (
            state.project_session().project().clone(),
            state.project_session().revision(),
        )
    };
    // The worker owns the only live runtime in the process, so it is the only
    // thing that opens a camera or a screen-cast session.
    let preview_worker = Rc::new(PreviewWorker::spawn(
        preview_project,
        preview_revision,
        &surface.borrow().diagnostics_handle(),
    )?);
    let audio_format = AudioFormat::new(settings.sample_rate_hz(), settings.channel_count())?;
    state
        .borrow_mut()
        .dispatch(UiCommand::SetLocale {
            locale: settings.ui_locale(),
        })
        .map_err(|error| format!("stored language: {error}"))?;
    state
        .borrow_mut()
        .dispatch(UiCommand::SetAudioFormat {
            sample_rate: settings.sample_rate_hz(),
            channels: settings.channel_count(),
        })
        .map_err(|error| format!("stored audio format: {error}"))?;
    let output = Rc::new(RefCell::new(OutputRuntime::with_audio_settings(
        surface.borrow().format,
        audio_format,
        (!settings.audio_input_id.is_empty()).then_some(settings.audio_input_id.as_str()),
        settings.audio_input_sync_offset_millis,
        settings.desktop_audio_sync_offset_millis,
        (!settings.audio_monitor_output_id.is_empty())
            .then_some(settings.audio_monitor_output_id.as_str()),
        settings.microphone_monitor_mode,
        settings.desktop_audio_monitor_mode,
    )?));

    // The mixer's live channel shows the input it captures from the first
    // frame, rather than a generic label that never matches the device list.
    let input_name = output.borrow_mut().audio_input_name();
    let desktop_name = output
        .borrow()
        .desktop_audio_name()
        .unwrap_or_else(|| "Desktop Audio (silent)".to_owned());
    {
        let mut state = state.borrow_mut();
        let _ = state.set_channel_name(MIC_CHANNEL_ID, &input_name);
        let _ = state.set_channel_name(DESKTOP_CHANNEL_ID, &desktop_name);
    }
    // Paths and the dock layout are pushed before the first refresh so the
    // recovery check and the docks both see the restored session.
    ui.set_project_path(settings.project_path.as_str().into());
    ui.set_setup_benchmark_summary(settings.setup_benchmark_summary.as_str().into());
    settings.apply_layout(&ui);
    // Build the pane projection before the first refresh so the initial frame
    // and headless snapshots render the same tree as the running window.
    let docks = install_dock_callbacks_with_layout(
        &ui,
        &state,
        Some(&settings.layout.dock_tree),
        &settings.layout.floating_geometry,
    );
    refresh_ui(&ui, &state, &surface);
    if let Some(message) = restored {
        ui.set_status_message(message.into());
    }
    if let Some(message) = shortcut_status {
        ui.set_status_message(message.into());
    }
    refresh_output_ui(&ui, &output);
    install_callbacks(&ui, &state, &surface, &output);
    // Keeps the canvas drag state and the detached docks alive for the session.
    let canvas = install_canvas_callbacks(&ui, &state, &surface);
    // Menu-bar actions and the projector windows they open.
    let projectors = install_menu_callbacks(&ui, &state, &surface, &docks);
    projectors.restore_geometry(&settings.layout.projector_geometry);
    projectors.restore_targets(&settings.layout.projector_targets);
    projectors.restore_monitors(&settings.layout.projector_monitors);
    if !smoke && screenshot.is_none() {
        projectors.reopen_persisted(&ui, &state);
    }
    // Keeps the settings window alive for the whole session; dropping the
    // controller would close it.
    let add_source_window = install_add_source_window(&ui, &state, &surface)?;
    let monitor_window = install_monitor_window(&ui, &state, &surface)?;
    let properties_window = install_source_properties_window(&ui, &state, &surface)?;
    let filters_window = install_source_filters_window(&ui, &state, &surface)?;
    let transform_window = install_source_transform_window(&ui, &state, &surface)?;
    let startup_recording_path = settings.recording_path.clone();
    let settings_window = install_settings_window(
        &ui,
        &state,
        &surface,
        &output,
        settings,
        settings::settings_path(),
        &PeerWindows {
            add_source: add_source_window,
            properties: properties_window,
            filters: filters_window,
            transform: transform_window,
            monitor: monitor_window,
            docks: Rc::clone(&docks),
            projectors: Rc::clone(&projectors),
            canvas: Rc::clone(&canvas),
        },
    )?;
    let setup_window = install_setup_window(&ui, &state, &surface, &output, &settings_window)?;

    if let Some((page, path, locale)) = screenshot {
        // Opening the window through its own callback is what fills the draft,
        // so the screenshot shows the same values a user would see.
        ui.invoke_open_settings_window();
        settings_window.capture_page(&page, std::path::Path::new(&path), locale)?;
        println!("obs-rs: wrote {path}");
        return Ok(());
    }

    if smoke {
        return Ok(());
    }

    // A previous automatic-remux session may have left a marked Matroska
    // source behind. Scan once after the normal callbacks are installed so
    // the bounded result can reach the existing chooser on the first timer
    // tick; recovery remains an operator choice rather than a silent startup
    // write.
    if !show_setup && output.borrow().remux_recovery_supported() {
        match output
            .borrow_mut()
            .request_startup_remux_discovery(&startup_recording_path)
        {
            Ok(()) => {
                ui.set_status_message(
                    "Checking for interrupted automatic-remux recordings…".into(),
                );
            }
            Err(error) => {
                ui.set_status_message(format!("Startup recovery scan failed: {error}").into());
            }
        }
    }

    if show_setup {
        setup_window.open();
    }

    // The window-manager close request must use the same discard guard as the
    // File -> Exit action. Without this boundary, a dirty project could be
    // discarded by closing the native window even though the menu path asks
    // for confirmation.
    let close_state = Rc::clone(&state);
    let close_ui = ui.as_weak();
    ui.window().on_close_requested(move || {
        let dirty = close_state.borrow().is_dirty();
        if dirty {
            if let Some(ui) = close_ui.upgrade() {
                ui.set_pending_discard(5);
            }
        }
        close_request_response(dirty)
    });

    let _preview_timer = start_preview_timer(
        &ui,
        &state,
        &preview_worker,
        &surface,
        &output,
        &projectors,
        &docks,
        &canvas,
    );
    ui.run()?;
    // Closing the window is the ordinary way to leave OBS, so the layout and
    // the project are written back here rather than only on an explicit Save.
    if let Err(error) = settings_window.persist_session(&ui, &state) {
        eprintln!("obs-rs: could not persist the session: {error}");
    }
    Ok(())
}

fn close_request_response(dirty: bool) -> CloseRequestResponse {
    if dirty {
        CloseRequestResponse::KeepWindowShown
    } else {
        CloseRequestResponse::HideWindow
    }
}

/// Parses `--settings-screenshot <page> [file] [locale]`.
///
/// The file defaults to `obs-rs-settings-<page>.png` in the working directory
/// and the locale to English, so the common case is one argument.
fn screenshot_request() -> Option<(String, String, obs_rs_ui::UiLocale)> {
    let mut arguments = std::env::args().skip_while(|argument| argument != "--settings-screenshot");
    arguments.next()?;
    let page = arguments.next()?;
    let mut rest = arguments.take_while(|value| !value.starts_with("--"));
    let path = rest
        .next()
        .unwrap_or_else(|| format!("obs-rs-settings-{page}.png"));
    // The locale is part of the fixture: Spanish labels are materially wider,
    // so a page is worth capturing in both.
    let locale = rest
        .next()
        .and_then(|code| obs_rs_ui::UiLocale::from_code(&code))
        .unwrap_or(obs_rs_ui::UiLocale::English);
    Some((page, path, locale))
}

fn restored_project_loaded(message: Option<&str>) -> bool {
    message.is_some_and(|message| message.starts_with("Restored project from "))
}

/// Reopens the stored project file, returning the message to show for it.
///
/// A missing file is the first-run case and is silent; a corrupt one keeps the
/// starter project and reports why, which is safer than starting empty.
fn restore_project(state: &Rc<RefCell<DesktopState>>, settings: &AppSettings) -> Option<String> {
    if !settings.restore_project {
        return None;
    }
    let path = settings.project_path.trim();
    if path.is_empty() || !std::path::Path::new(path).exists() {
        return None;
    }
    let result = project_store(path).and_then(|store| {
        state.borrow_mut().load_project_for_key(&store, path)?;
        let preview = (!settings.last_preview_scene.is_empty())
            .then_some(settings.last_preview_scene.as_str());
        let program = (!settings.last_program_scene.is_empty())
            .then_some(settings.last_program_scene.as_str());
        state.borrow_mut().restore_scene_selection(preview, program);
        Ok(())
    });
    match result {
        Ok(()) => Some(format!("Restored project from {path}")),
        Err(error) => Some(format!("Could not restore {path}: {error}")),
    }
}
