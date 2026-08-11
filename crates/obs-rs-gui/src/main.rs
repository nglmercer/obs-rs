//! Slint desktop control room for the Rust-owned OBS-RS state machine.

#![deny(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_audio::AudioFormat;
use obs_rs_ui::{DesktopState, UiCommand};
use slint::ComponentHandle;

mod callbacks;
mod fixtures;
mod i18n;
mod output;
mod preview;
mod properties;
mod refresh;
mod settings;
mod view;

#[cfg(test)]
mod tests;

pub(crate) use callbacks::{
    apply_source_filters_and_refresh, apply_source_settings_and_refresh,
    apply_source_transform_and_refresh, move_source_and_refresh, project_store,
    remove_scene_and_refresh, remove_source_and_refresh, rename_scene_and_refresh,
    source_filters_document, source_transform_document, toggle_source_locked_and_refresh,
    toggle_source_visibility_and_refresh,
};
pub(crate) use callbacks::{
    install_add_source_window, install_callbacks, install_canvas_callbacks, install_dock_callbacks,
    install_monitor_window, install_settings_window, install_source_properties_window, item_rect,
    start_preview_timer, PeerWindows,
};
pub(crate) use fixtures::{
    capture_devices, initial_project, kind_runs_in_this_session, kind_selects_monitor,
    kind_uses_portal, platform_capture_summary, source_settings,
};
pub(crate) use output::OutputRuntime;
pub(crate) use preview::{frame_to_image, PreviewRenderer};
pub(crate) use refresh::{
    dispatch_and_refresh, refresh_output_ui, refresh_preview_frames_for_view, refresh_ui,
};
pub(crate) use settings::AppSettings;
pub(crate) use view::{
    AddSourceText, AddSourceWindow, FloatingDockWindow, I18n, LocaleOption, MainWindow, MixerRow,
    MonitorRow, MonitorText, MonitorWindow, Palette, ProfileRow, PropertyRow, PropertyText,
    SceneRow, SettingsText, SettingsWindow, SourceCandidate, SourceKindRow, SourcePropertiesWindow,
    SourceRow, ThemeTokens, UiText,
};

/// Mixer channel backed by the engine's live capture input.
///
/// The engine opens one input device — the microphone or line input chosen on
/// the settings window's Audio page — so this is the channel whose fader, mute,
/// and meter correspond to real audio rather than to a placeholder.
pub(crate) const MIC_CHANNEL_ID: &str = "mic";

fn main() -> Result<(), Box<dyn Error>> {
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    if smoke {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new()?;
    // Stored settings own the file paths and the stream destination, so they
    // are loaded before anything reads them.
    let settings = AppSettings::load(&settings::settings_path());
    ui.set_new_source_kind("test_pattern".into());
    ui.set_capture_capabilities(platform_capture_summary().into());
    let project = initial_project()?;
    // Revision 0 is the "nothing observed yet" sentinel, so the first refresh
    // always syncs the renderer against the live session.
    let renderer = Rc::new(RefCell::new(PreviewRenderer::new(&project, 0)?));
    {
        // The canvas size drives the zoom readout under the preview.
        let format = renderer.borrow().format;
        ui.set_canvas_width(i32::try_from(format.width()).unwrap_or(1920));
        ui.set_canvas_height(i32::try_from(format.height()).unwrap_or(1080));
    }
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    // The stored project is reopened before anything renders, so a session
    // resumes with the scenes and sources it was left with rather than with the
    // starter fixture.
    let restored = restore_project(&state, &settings);
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
    let output = Rc::new(RefCell::new(OutputRuntime::with_audio_input(
        renderer.borrow().format,
        audio_format,
        (!settings.audio_input_id.is_empty()).then_some(settings.audio_input_id.as_str()),
    )?));

    // The mixer's live channel shows the input it captures from the first
    // frame, rather than a generic label that never matches the device list.
    let input_name = output.borrow_mut().audio_input_name();
    let _ = state
        .borrow_mut()
        .set_channel_name(MIC_CHANNEL_ID, &input_name);
    // Paths and the dock layout are pushed before the first refresh so the
    // recovery check and the docks both see the restored session.
    ui.set_project_path(settings.project_path.as_str().into());
    settings.apply_layout(&ui);
    refresh_ui(&ui, &state, &renderer);
    if let Some(message) = restored {
        ui.set_status_message(message.into());
    }
    refresh_output_ui(&ui, &output);
    install_callbacks(&ui, &state, &renderer, &output);
    // Keeps the settings window alive for the whole session; dropping the
    // controller would close it.
    // Keeps the canvas drag state and the detached docks alive for the session.
    let _canvas = install_canvas_callbacks(&ui, &state, &renderer);
    let docks = install_dock_callbacks(&ui, &state, &renderer, &output);
    let add_source_window = install_add_source_window(&ui, &state, &renderer)?;
    let monitor_window = install_monitor_window(&ui, &state, &renderer)?;
    let properties_window = install_source_properties_window(&ui, &state, &renderer)?;
    let settings_window = install_settings_window(
        &ui,
        &state,
        &renderer,
        &output,
        settings,
        &PeerWindows {
            add_source: add_source_window,
            properties: properties_window,
            monitor: monitor_window,
            docks: Rc::clone(&docks),
        },
    )?;

    if smoke {
        return Ok(());
    }

    let _preview_timer = start_preview_timer(&ui, &state, &renderer, &output);
    ui.run()?;
    // Closing the window is the ordinary way to leave OBS, so the layout and
    // the project are written back here rather than only on an explicit Save.
    if let Err(error) = settings_window.persist_session(&ui, &state) {
        eprintln!("obs-rs: could not persist the session: {error}");
    }
    Ok(())
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
        state.borrow_mut().load_project(&store)?;
        Ok(())
    });
    match result {
        Ok(()) => Some(format!("Restored project from {path}")),
        Err(error) => Some(format!("Could not restore {path}: {error}")),
    }
}
