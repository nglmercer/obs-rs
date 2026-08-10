//! Slint desktop control room for the Rust-owned OBS-RS state machine.

#![deny(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{cell::RefCell, error::Error, rc::Rc};

use obs_rs_ui::DesktopState;
use slint::ComponentHandle;

mod callbacks;
mod fixtures;
mod i18n;
mod output;
mod preview;
mod refresh;
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
pub(crate) use callbacks::{install_callbacks, start_preview_timer};
pub(crate) use fixtures::{initial_project, platform_capture_summary, source_settings};
pub(crate) use output::OutputRuntime;
pub(crate) use preview::{frame_to_image, PreviewRenderer};
pub(crate) use refresh::{
    dispatch_and_refresh, refresh_output_ui, refresh_preview_frames, refresh_ui,
};
pub(crate) use view::{
    I18n, LocaleOption, MainWindow, MixerRow, ProfileRow, SceneRow, SourceRow, UiText,
};

fn main() -> Result<(), Box<dyn Error>> {
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    if smoke {
        i_slint_backend_testing::init_no_event_loop();
    }
    let ui = MainWindow::new()?;
    ui.set_project_path("obs-rs-project.txt".into());
    ui.set_diagnostics_path("obs-rs-diagnostics.obsrdg".into());
    ui.set_recording_path("obs-rs-recording.y4m".into());
    ui.set_streaming_address("127.0.0.1:9000".into());
    ui.set_new_source_kind("test_pattern".into());
    ui.set_capture_capabilities(platform_capture_summary().into());
    let project = initial_project()?;
    let renderer = Rc::new(RefCell::new(PreviewRenderer::new(&project)?));
    {
        // The canvas size drives the zoom readout under the preview.
        let format = renderer.borrow().format;
        ui.set_canvas_width(i32::try_from(format.width()).unwrap_or(1920));
        ui.set_canvas_height(i32::try_from(format.height()).unwrap_or(1080));
    }
    let state = Rc::new(RefCell::new(DesktopState::new(project)));
    let output = Rc::new(RefCell::new(OutputRuntime::new(renderer.borrow().format)));

    refresh_ui(&ui, &state, &renderer);
    refresh_output_ui(&ui, &output);
    install_callbacks(&ui, &state, &renderer, &output);

    if smoke {
        return Ok(());
    }

    let _preview_timer = start_preview_timer(&ui, &state, &renderer, &output);
    ui.run()?;
    Ok(())
}
