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
    install_add_source_window, install_callbacks, install_settings_window,
    install_source_properties_window, start_preview_timer,
};
pub(crate) use fixtures::{initial_project, platform_capture_summary, source_settings};
pub(crate) use output::OutputRuntime;
pub(crate) use preview::{frame_to_image, PreviewRenderer};
pub(crate) use refresh::{
    dispatch_and_refresh, refresh_output_ui, refresh_preview_frames, refresh_ui,
};
pub(crate) use settings::AppSettings;
pub(crate) use view::{
    AddSourceText, AddSourceWindow, I18n, LocaleOption, MainWindow, MixerRow, Palette, ProfileRow,
    SceneRow, SettingsText, SettingsWindow, SourceCandidate, SourceKindRow, SourcePropertiesWindow,
    SourceRow, ThemeTokens, UiText,
};

fn main() -> Result<(), Box<dyn Error>> {
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    if smoke {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new()?;
    // Stored settings own the file paths and the stream destination, so they
    // are loaded before anything reads them.
    let settings = AppSettings::load(std::path::Path::new(settings::SETTINGS_FILE));
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
    let output = Rc::new(RefCell::new(OutputRuntime::with_audio(
        renderer.borrow().format,
        audio_format,
    )?));

    refresh_ui(&ui, &state, &renderer);
    refresh_output_ui(&ui, &output);
    install_callbacks(&ui, &state, &renderer, &output);
    // Keeps the settings window alive for the whole session; dropping the
    // controller would close it.
    let add_source_window = install_add_source_window(&ui, &state, &renderer)?;
    let properties_window = install_source_properties_window(&ui, &state, &renderer)?;
    let _settings_window = install_settings_window(
        &ui,
        &state,
        &renderer,
        settings,
        &add_source_window,
        &properties_window,
    )?;

    if smoke {
        return Ok(());
    }

    let _preview_timer = start_preview_timer(&ui, &state, &renderer, &output);
    ui.run()?;
    Ok(())
}
