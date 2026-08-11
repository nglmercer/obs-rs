//! Controller for the standalone settings window.
//!
//! The window edits a draft copy of [`AppSettings`]. Nothing but Apply and OK
//! writes back, so Cancel restores the committed values — including the theme
//! and language, which are previewed live and therefore have to be undone.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_project::ProjectCommand;
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    callbacks::add_source::AddSourceController,
    callbacks::monitor::MonitorController,
    callbacks::source_properties::SourcePropertiesController,
    refresh_ui,
    settings::{AppSettings, CHANNEL_LAYOUTS, FRAME_RATES, RESOLUTIONS, SAMPLE_RATES, THEMES},
    I18n, MainWindow, OutputRuntime, Palette, PreviewRenderer, SettingsWindow,
};

/// Owns the settings window and the committed settings document.
pub(crate) struct SettingsController {
    window: SettingsWindow,
    settings: Rc<RefCell<AppSettings>>,
    path: PathBuf,
    /// Repainted alongside this window so a theme change reaches every surface.
    add_source: Rc<AddSourceController>,
    properties: Rc<SourcePropertiesController>,
    monitor: Rc<MonitorController>,
    docks: Rc<crate::callbacks::docks::DockController>,
    /// IDs are kept separate from the display labels shown by Slint's `ComboBox`.
    audio_device_ids: RefCell<Vec<String>>,
}

impl SettingsController {
    /// Writes the dock layout and, when enabled, the project back to disk.
    ///
    /// This runs when the studio window closes. Both writes are attempted even
    /// if the first fails, so one unwritable path cannot silently discard the
    /// other half of the session.
    ///
    /// # Errors
    ///
    /// Returns the combined description of whatever could not be written.
    pub(crate) fn persist_session(
        &self,
        ui: &MainWindow,
        state: &Rc<RefCell<DesktopState>>,
    ) -> Result<(), String> {
        let mut settings = self.settings.borrow().clone();
        settings.capture_layout(ui);
        let mut failures = Vec::new();
        if let Err(error) = settings.save(&self.path) {
            failures.push(format!("settings file: {error}"));
        }
        *self.settings.borrow_mut() = settings.clone();
        if settings.save_project_on_exit {
            let result = crate::project_store(&settings.project_path)
                .and_then(|store| Ok(state.borrow_mut().save_project(&store)?));
            if let Err(error) = result {
                failures.push(format!("project file: {error}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    /// Keeps the settings window's catalog and palette in step with the studio.
    pub(crate) fn sync_theme(&self, locale: UiLocale) {
        self.window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(locale));
        self.window
            .global::<Palette>()
            .set_tokens(self.settings.borrow().tokens());
    }
}

/// The other top-level windows the settings window repaints on a theme change.
pub(crate) struct PeerWindows {
    pub(crate) add_source: Rc<AddSourceController>,
    pub(crate) properties: Rc<SourcePropertiesController>,
    pub(crate) monitor: Rc<MonitorController>,
    pub(crate) docks: Rc<crate::callbacks::docks::DockController>,
}

/// Creates the settings window and wires it to the studio window.
///
/// The returned controller must outlive the event loop; dropping it closes the
/// settings window.
pub(crate) fn install_settings_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
    settings: AppSettings,
    peers: &PeerWindows,
) -> Result<Rc<SettingsController>, slint::PlatformError> {
    let window = SettingsWindow::new()?;
    let path = crate::settings::settings_path();
    let controller = Rc::new(SettingsController {
        window,
        settings: Rc::new(RefCell::new(settings)),
        path,
        add_source: Rc::clone(&peers.add_source),
        properties: Rc::clone(&peers.properties),
        monitor: Rc::clone(&peers.monitor),
        docks: Rc::clone(&peers.docks),
        audio_device_ids: RefCell::new(Vec::new()),
    });

    populate_static_models(&controller.window);
    apply_to_studio(ui, &controller.settings.borrow());
    push_palette(ui, &controller, &controller.settings.borrow());
    controller.sync_theme(state.borrow().locale());

    install_open(ui, state, renderer, output, &controller);
    install_previews(ui, state, renderer, &controller);
    install_commit(ui, state, renderer, output, &controller);
    Ok(controller)
}

/// Fills the dropdown models so a test can render every page.
#[cfg(test)]
pub(crate) fn populate_settings_models(window: &SettingsWindow) {
    populate_static_models(window);
}

/// Model contents that never change while the application runs.
fn populate_static_models(window: &SettingsWindow) {
    let text = window.global::<I18n>().get_text().settings_ui;
    window.set_language_names(string_model(
        UiLocale::supported()
            .iter()
            .map(|locale| language_label(*locale)),
    ));
    window.set_theme_names(string_model(
        [
            text.theme_dark,
            text.theme_darker,
            text.theme_midnight,
            text.theme_slate,
        ]
        .into_iter(),
    ));
    window.set_sample_rate_names(string_model(
        SAMPLE_RATES
            .iter()
            .map(|rate| SharedString::from(format!("{rate} Hz"))),
    ));
    window.set_channel_names(string_model(
        [text.channels_stereo, text.channels_mono].into_iter(),
    ));
    window.set_resolution_names(string_model(
        RESOLUTIONS
            .iter()
            .map(|(width, height)| SharedString::from(format!("{width}x{height}"))),
    ));
    window.set_frame_rate_names(string_model(
        FRAME_RATES
            .iter()
            .map(|rate| SharedString::from(frame_rate_label(*rate))),
    ));
}

fn install_open(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let output = Rc::clone(output);
    let controller = Rc::clone(controller);
    ui.on_open_settings_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        load_draft(&state, &renderer, &output, &controller);
        controller.sync_theme(state.borrow().locale());
        if let Err(error) = controller.window.show() {
            ui.set_status_message(format!("Settings window: {error}").into());
        }
    });
}

/// Copies committed settings plus live project state into the window's draft.
fn load_draft(
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let settings = controller.settings.borrow();
    let window = &controller.window;
    window.set_dirty(false);
    window.set_language_index(locale_index(state.borrow().locale()));
    window.set_theme_index(i32::try_from(settings.theme).unwrap_or(0));
    window.set_confirm_start_stream(settings.confirm_start_stream);
    window.set_confirm_stop_stream(settings.confirm_stop_stream);
    window.set_confirm_stop_recording(settings.confirm_stop_recording);
    window.set_auto_record_when_streaming(settings.auto_record_when_streaming);
    window.set_streaming_address(settings.streaming_address.as_str().into());
    window.set_recording_path(settings.recording_path.as_str().into());
    window.set_project_path(settings.project_path.as_str().into());
    window.set_diagnostics_path(settings.diagnostics_path.as_str().into());
    window.set_restore_project(settings.restore_project);
    window.set_save_project_on_exit(settings.save_project_on_exit);
    window.set_hotkey_swap(settings.hotkey_swap.as_str().into());
    window.set_hotkey_start_recording(settings.hotkey_start_recording.as_str().into());
    window.set_hotkey_stop_recording(settings.hotkey_stop_recording.as_str().into());
    window.set_hotkey_start_streaming(settings.hotkey_start_streaming.as_str().into());
    window.set_hotkey_stop_streaming(settings.hotkey_stop_streaming.as_str().into());
    window.set_preview_border_color(settings.preview_border_color.as_str().into());
    window.set_program_border_color(settings.program_border_color.as_str().into());
    window.set_preview_swatch(settings.tokens().preview_border);
    window.set_program_swatch(settings.tokens().program_border);

    let audio_format = state.borrow().audio_format();
    window.set_sample_rate_index(index_of(&SAMPLE_RATES, &audio_format.sample_rate()));
    window.set_channel_index(index_of(&CHANNEL_LAYOUTS, &audio_format.channels()));
    let audio_devices = output.borrow_mut().audio_input_devices();
    let mut device_ids = vec![String::new()];
    let mut device_names = vec![crate::i18n::with_catalog(state.borrow().locale(), |text| {
        text.settings_ui.audio_input_auto.clone()
    })];
    device_ids.extend(audio_devices.iter().map(|(id, _)| id.clone()));
    device_names.extend(
        audio_devices
            .iter()
            .map(|(_, name)| SharedString::from(name.as_str())),
    );
    let selected_device_index = settings
        .audio_input_id
        .is_empty()
        .then_some(0)
        .or_else(|| {
            device_ids
                .iter()
                .position(|id| id == &settings.audio_input_id)
        })
        .unwrap_or(0);
    controller.audio_device_ids.replace(device_ids);
    window.set_audio_device_names(string_model(device_names.into_iter()));
    window.set_audio_device_index(i32::try_from(selected_device_index).unwrap_or(0));
    window.set_devices_summary(output.borrow_mut().audio_devices_summary().into());

    let video_format = renderer.borrow().format;
    let resolution = (video_format.width(), video_format.height());
    window.set_base_resolution_index(index_of(&RESOLUTIONS, &resolution));
    window.set_output_resolution(format!("{}x{}", resolution.0, resolution.1).into());
    window.set_frame_rate_index(index_of(
        &FRAME_RATES,
        &(
            video_format.frame_rate().numerator(),
            video_format.frame_rate().denominator(),
        ),
    ));
    window.set_recording_format(
        format!(
            "OBSRPKT1 A/V · {}x{} @ {}",
            resolution.0,
            resolution.1,
            frame_rate_label((
                video_format.frame_rate().numerator(),
                video_format.frame_rate().denominator()
            ))
        )
        .into(),
    );
}

/// Theme and language change the moment they are picked, because judging either
/// from a dropdown label alone is not realistic.
fn install_previews(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    controller: &Rc<SettingsController>,
) {
    let weak = ui.as_weak();
    let theme_controller = Rc::clone(controller);
    controller.window.on_preview_theme(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let theme = usize::try_from(index).unwrap_or(0).min(THEMES.len() - 1);
        let tokens = theme_controller.settings.borrow().tokens_for_theme(theme);
        push_palette_tokens(&ui, &theme_controller, &tokens);
    });

    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let language_controller = Rc::clone(controller);
    controller.window.on_preview_language(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Some(locale) = UiLocale::supported()
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
        else {
            return;
        };
        if state
            .borrow_mut()
            .dispatch(UiCommand::SetLocale { locale })
            .is_ok()
        {
            refresh_ui(&ui, &state, &renderer);
            language_controller.sync_theme(locale);
        }
    });
}

fn install_commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let weak = ui.as_weak();
    let apply_state = Rc::clone(state);
    let apply_renderer = Rc::clone(renderer);
    let apply_output = Rc::clone(output);
    let apply_controller = Rc::clone(controller);
    controller.window.on_apply_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        commit(
            &ui,
            &apply_state,
            &apply_renderer,
            &apply_output,
            &apply_controller,
        );
    });

    let weak = ui.as_weak();
    let accept_state = Rc::clone(state);
    let accept_renderer = Rc::clone(renderer);
    let accept_output = Rc::clone(output);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        commit(
            &ui,
            &accept_state,
            &accept_renderer,
            &accept_output,
            &accept_controller,
        );
        let _ = accept_controller.window.hide();
    });

    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let renderer = Rc::clone(renderer);
    let cancel_controller = Rc::clone(controller);
    controller.window.on_cancel_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        // Theme and language were previewed live, so Cancel has to put the
        // committed document back rather than merely closing the window.
        let committed = cancel_controller.settings.borrow().clone();
        push_palette(&ui, &cancel_controller, &committed);
        let locale = committed.ui_locale();
        if state.borrow().locale() != locale
            && state
                .borrow_mut()
                .dispatch(UiCommand::SetLocale { locale })
                .is_ok()
        {
            refresh_ui(&ui, &state, &renderer);
        }
        cancel_controller.sync_theme(locale);
        cancel_controller.window.set_dirty(false);
        let _ = cancel_controller.window.hide();
    });
}

fn commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let window = &controller.window;
    let mut settings = controller.settings.borrow().clone();
    settings.theme = usize::try_from(window.get_theme_index())
        .unwrap_or(0)
        .min(THEMES.len() - 1);
    settings.confirm_start_stream = window.get_confirm_start_stream();
    settings.confirm_stop_stream = window.get_confirm_stop_stream();
    settings.confirm_stop_recording = window.get_confirm_stop_recording();
    settings.auto_record_when_streaming = window.get_auto_record_when_streaming();
    settings.sample_rate = usize::try_from(window.get_sample_rate_index())
        .unwrap_or(0)
        .min(SAMPLE_RATES.len() - 1);
    settings.channels = usize::try_from(window.get_channel_index())
        .unwrap_or(0)
        .min(CHANNEL_LAYOUTS.len() - 1);
    settings.hotkey_swap = window.get_hotkey_swap().to_string();
    settings.hotkey_start_recording = window.get_hotkey_start_recording().to_string();
    settings.hotkey_stop_recording = window.get_hotkey_stop_recording().to_string();
    settings.hotkey_start_streaming = window.get_hotkey_start_streaming().to_string();
    settings.hotkey_stop_streaming = window.get_hotkey_stop_streaming().to_string();
    settings.preview_border_color = window.get_preview_border_color().to_string();
    settings.program_border_color = window.get_program_border_color().to_string();
    settings.project_path = window.get_project_path().to_string();
    settings.diagnostics_path = window.get_diagnostics_path().to_string();
    settings.recording_path = window.get_recording_path().to_string();
    settings.streaming_address = window.get_streaming_address().to_string();
    settings.restore_project = window.get_restore_project();
    settings.save_project_on_exit = window.get_save_project_on_exit();
    if let Some(device_id) = controller
        .audio_device_ids
        .borrow()
        .get(usize::try_from(window.get_audio_device_index()).unwrap_or(0))
    {
        device_id.clone_into(&mut settings.audio_input_id);
    } else {
        settings.audio_input_id.clear();
    }
    if let Some(locale) = UiLocale::supported()
        .get(usize::try_from(window.get_language_index()).unwrap_or(0))
        .copied()
    {
        locale.code().clone_into(&mut settings.locale);
    }

    let mut notes = Vec::new();

    // Audio: rebuild the mixer only when the format actually differs.
    if let Err(error) = state.borrow_mut().dispatch(UiCommand::SetAudioFormat {
        sample_rate: settings.sample_rate_hz(),
        channels: settings.channel_count(),
    }) {
        notes.push(format!("audio: {error}"));
    }

    if let Err(error) = output.borrow_mut().set_audio_input_id(
        (!settings.audio_input_id.is_empty()).then_some(settings.audio_input_id.as_str()),
    ) {
        notes.push(format!("audio input: {error}"));
    }
    // The mixer row names the device it is capturing, so the fader and meter
    // are visibly tied to the input the user just chose.
    let input_name = output.borrow_mut().audio_input_name();
    if let Err(error) = state
        .borrow_mut()
        .set_channel_name(crate::MIC_CHANNEL_ID, &input_name)
    {
        notes.push(format!("mixer label: {error}"));
    }

    // Video: write the canvas back to the active profile so it persists with
    // the project and the renderer rebuilds on the next sync.
    if let Some(format) = video_format_from(window) {
        let profile = state
            .borrow()
            .project_session()
            .project()
            .active_profile()
            .to_string();
        if let Err(error) =
            state
                .borrow_mut()
                .dispatch(UiCommand::Project(ProjectCommand::SetProfileVideoFormat {
                    profile,
                    format,
                }))
        {
            notes.push(format!("video: {error}"));
        }
    }

    if let Err(error) = settings.save(&controller.path) {
        notes.push(format!("settings file: {error}"));
    }

    *controller.settings.borrow_mut() = settings.clone();
    apply_to_studio(ui, &settings);
    push_palette(ui, controller, &settings);
    window.set_preview_swatch(settings.tokens().preview_border);
    window.set_program_swatch(settings.tokens().program_border);
    window.set_dirty(false);
    refresh_ui(ui, state, renderer);
    ui.set_canvas_width(i32::try_from(renderer.borrow().format.width()).unwrap_or(1920));
    ui.set_canvas_height(i32::try_from(renderer.borrow().format.height()).unwrap_or(1080));

    if !notes.is_empty() {
        ui.set_status_message(
            format!("Settings applied with warnings: {}", notes.join("; ")).into(),
        );
    }
}

fn video_format_from(window: &SettingsWindow) -> Option<VideoFormat> {
    let (width, height) =
        *RESOLUTIONS.get(usize::try_from(window.get_base_resolution_index()).ok()?)?;
    let (numerator, denominator) =
        *FRAME_RATES.get(usize::try_from(window.get_frame_rate_index()).ok()?)?;
    let rate = FrameRate::new(numerator, denominator).ok()?;
    VideoFormat::new(width, height, rate).ok()
}

/// Pushes the values the studio window reads directly.
fn apply_to_studio(ui: &MainWindow, settings: &AppSettings) {
    ui.set_hotkey_swap(settings.hotkey_swap.as_str().into());
    ui.set_hotkey_start_recording(settings.hotkey_start_recording.as_str().into());
    ui.set_hotkey_stop_recording(settings.hotkey_stop_recording.as_str().into());
    ui.set_hotkey_start_streaming(settings.hotkey_start_streaming.as_str().into());
    ui.set_hotkey_stop_streaming(settings.hotkey_stop_streaming.as_str().into());
    ui.set_confirm_start_stream(settings.confirm_start_stream);
    ui.set_confirm_stop_stream(settings.confirm_stop_stream);
    ui.set_confirm_stop_recording(settings.confirm_stop_recording);
    ui.set_auto_record_when_streaming(settings.auto_record_when_streaming);
    ui.set_project_path(settings.project_path.as_str().into());
    ui.set_diagnostics_path(settings.diagnostics_path.as_str().into());
    ui.set_recording_path(settings.recording_path.as_str().into());
    ui.set_streaming_address(settings.streaming_address.as_str().into());
}

/// Globals are per component tree, so every window is painted explicitly.
fn push_palette(ui: &MainWindow, controller: &SettingsController, settings: &AppSettings) {
    push_palette_tokens(ui, controller, &settings.tokens());
}

fn push_palette_tokens(
    ui: &MainWindow,
    controller: &SettingsController,
    tokens: &crate::ThemeTokens,
) {
    ui.global::<Palette>().set_tokens(tokens.clone());
    controller
        .window
        .global::<Palette>()
        .set_tokens(tokens.clone());
    controller.add_source.set_tokens(tokens.clone());
    controller.properties.set_tokens(tokens.clone());
    controller.monitor.set_tokens(tokens.clone());
    controller.docks.set_tokens(tokens);
}

fn string_model(values: impl Iterator<Item = SharedString>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(values.collect::<Vec<_>>()))
}

fn language_label(locale: UiLocale) -> SharedString {
    match locale {
        UiLocale::English => "English".into(),
        UiLocale::Spanish => "Español".into(),
    }
}

fn locale_index(locale: UiLocale) -> i32 {
    i32::try_from(
        UiLocale::supported()
            .iter()
            .position(|value| *value == locale)
            .unwrap_or(0),
    )
    .unwrap_or(0)
}

fn index_of<T: PartialEq>(values: &[T], needle: &T) -> i32 {
    i32::try_from(values.iter().position(|value| value == needle).unwrap_or(0)).unwrap_or(0)
}

fn frame_rate_label((numerator, denominator): (u32, u32)) -> String {
    if denominator == 1 {
        format!("{numerator}")
    } else {
        // 60000/1001 reads as 59.94 the way OBS presents NTSC rates.
        let value = f64::from(numerator) / f64::from(denominator);
        format!("{value:.2}")
    }
}
