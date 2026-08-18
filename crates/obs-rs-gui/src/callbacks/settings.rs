//! Controller for the standalone settings window.
//!
//! The window edits a draft copy of [`AppSettings`]. Nothing but Apply and OK
//! writes back, so Cancel restores the committed values — including the theme
//! and language, which are previewed live and therefore have to be undone.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use obs_rs_engine::ProductionProtocol;
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_output::{EncoderImplementation, EncoderPreset, RateControl, VideoCodec};
use obs_rs_output::{SecretString, SrtKeyLength, SrtMode, StreamProtocol};
use obs_rs_project::ProjectCommand;
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    callbacks::add_source::AddSourceController,
    callbacks::monitor::MonitorController,
    callbacks::source_filters::SourceFiltersController,
    callbacks::source_properties::SourcePropertiesController,
    callbacks::source_transform::SourceTransformController,
    refresh_ui,
    settings::{
        AppSettings, RecordingFormat, CHANNEL_LAYOUTS, FRAME_RATES, RESOLUTIONS, SAMPLE_RATES,
        THEMES,
    },
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
    filters: Rc<SourceFiltersController>,
    transform: Rc<SourceTransformController>,
    monitor: Rc<MonitorController>,
    docks: Rc<crate::callbacks::docks::DockController>,
    projectors: Rc<crate::ProjectorController>,
    /// IDs are kept separate from the display labels shown by Slint's `ComboBox`.
    audio_device_ids: RefCell<Vec<String>>,
    protocol_ids: RefCell<Vec<StreamProtocol>>,
    video_encoder_ids: RefCell<Vec<String>>,
    audio_encoder_ids: RefCell<Vec<String>>,
    recording_codec_ids: RefCell<Vec<VideoCodec>>,
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
    pub(crate) filters: Rc<SourceFiltersController>,
    pub(crate) transform: Rc<SourceTransformController>,
    pub(crate) monitor: Rc<MonitorController>,
    pub(crate) docks: Rc<crate::callbacks::docks::DockController>,
    pub(crate) projectors: Rc<crate::ProjectorController>,
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
        filters: Rc::clone(&peers.filters),
        transform: Rc::clone(&peers.transform),
        monitor: Rc::clone(&peers.monitor),
        docks: Rc::clone(&peers.docks),
        projectors: Rc::clone(&peers.projectors),
        audio_device_ids: RefCell::new(Vec::new()),
        protocol_ids: RefCell::new(Vec::new()),
        video_encoder_ids: RefCell::new(Vec::new()),
        audio_encoder_ids: RefCell::new(Vec::new()),
        recording_codec_ids: RefCell::new(Vec::new()),
    });

    populate_static_models(&controller.window);
    install_stream_protocol_switch(&controller);
    output
        .borrow_mut()
        .configure_stream(&controller.settings.borrow());
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
    window.set_recording_format_names(string_model(
        RecordingFormat::ALL
            .iter()
            .map(|format| SharedString::from(format.display_name())),
    ));
    window.set_srt_mode_names(string_model(
        ["Caller", "Listener", "Rendezvous"]
            .into_iter()
            .map(SharedString::from),
    ));
    window.set_srt_key_length_names(string_model(
        ["None", "AES-128", "AES-192", "AES-256"]
            .into_iter()
            .map(SharedString::from),
    ));
}

fn install_stream_protocol_switch(controller: &Rc<SettingsController>) {
    let callback_controller = Rc::clone(controller);
    controller.window.on_select_stream_protocol(move |index| {
        let protocol = callback_controller
            .protocol_ids
            .borrow()
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or(StreamProtocol::Reference);
        show_protocol_fields(&callback_controller.window, protocol);
    });
}

fn populate_stream_models(
    window: &SettingsWindow,
    output: &OutputRuntime,
    controller: &SettingsController,
    settings: &AppSettings,
) {
    let mut protocol_ids = Vec::new();
    let mut protocol_names = Vec::new();
    for capability in output
        .capabilities()
        .protocols()
        .iter()
        .filter(|capability| capability.available())
    {
        let protocol = match capability.protocol() {
            ProductionProtocol::Rtmp => StreamProtocol::Rtmp,
            ProductionProtocol::Rtmps => StreamProtocol::Rtmps,
            ProductionProtocol::Srt => StreamProtocol::Srt,
            ProductionProtocol::WebRtc => StreamProtocol::Whip,
            ProductionProtocol::Hls => StreamProtocol::Hls,
            ProductionProtocol::Rist => StreamProtocol::Rist,
            ProductionProtocol::Reference => StreamProtocol::Reference,
            ProductionProtocol::Matroska => continue,
        };
        protocol_ids.push(protocol);
        protocol_names.push(SharedString::from(capability.protocol().display_name()));
    }
    let selected = protocol_ids
        .iter()
        .position(|protocol| *protocol == settings.stream_protocol)
        .unwrap_or_else(|| {
            protocol_ids
                .iter()
                .position(|protocol| *protocol == StreamProtocol::Reference)
                .unwrap_or(0)
        });
    let selected_protocol = protocol_ids
        .get(selected)
        .copied()
        .unwrap_or(StreamProtocol::Reference);
    *controller.protocol_ids.borrow_mut() = protocol_ids;
    window.set_protocol_names(ModelRc::new(VecModel::from(protocol_names)));
    window.set_protocol_index(i32::try_from(selected).unwrap_or(0));
    show_protocol_fields(window, selected_protocol);

    let video = output
        .capabilities()
        .video_encoders()
        .iter()
        .filter(|encoder| encoder.codec() == VideoCodec::H264)
        .collect::<Vec<_>>();
    *controller.video_encoder_ids.borrow_mut() = video
        .iter()
        .map(|encoder| encoder.id().to_owned())
        .collect();
    window.set_video_encoder_names(string_model(video.iter().map(|encoder| {
        let suffix = if encoder.hardware() {
            "hardware"
        } else {
            "software"
        };
        SharedString::from(format!("{} ({suffix})", encoder.display_name()))
    })));
    window.set_video_encoder_index(index_of(
        &controller.video_encoder_ids.borrow(),
        &settings.rtmp.video.implementation.id().to_owned(),
    ));

    let recording_codecs = output.capabilities().recording_codecs().to_vec();
    window.set_recording_codec_names(string_model(recording_codecs.iter().map(|codec| {
        SharedString::from(match codec {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC / H.265",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp8 => "VP8",
            VideoCodec::ReferenceRle => "Reference RLE",
        })
    })));
    window.set_recording_codec_index(index_of(&recording_codecs, &settings.recording_codec));
    *controller.recording_codec_ids.borrow_mut() = recording_codecs;

    let audio = output.capabilities().audio_encoders();
    *controller.audio_encoder_ids.borrow_mut() = audio
        .iter()
        .map(|encoder| encoder.id().to_owned())
        .collect();
    window.set_audio_encoder_names(string_model(
        audio
            .iter()
            .map(|encoder| SharedString::from(encoder.display_name())),
    ));
    window.set_audio_encoder_index(index_of(
        &controller.audio_encoder_ids.borrow(),
        &settings.rtmp.audio.implementation.id().to_owned(),
    ));
}

fn show_protocol_fields(window: &SettingsWindow, protocol: StreamProtocol) {
    window.set_stream_show_rtmp(matches!(
        protocol,
        StreamProtocol::Rtmp | StreamProtocol::Rtmps
    ));
    window.set_stream_show_srt(protocol == StreamProtocol::Srt);
    window.set_stream_show_whip(protocol == StreamProtocol::Whip);
    window.set_stream_show_hls(protocol == StreamProtocol::Hls);
    window.set_stream_show_rist(protocol == StreamProtocol::Rist);
    window.set_stream_show_reference(protocol == StreamProtocol::Reference);
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

/// Returns the input ID the picker currently shows, committed or not.
///
/// The list index is the only place a not-yet-applied selection lives, so a
/// rebuild has to read it back rather than fall back to the stored document.
fn draft_audio_input_id(controller: &SettingsController) -> String {
    controller
        .audio_device_ids
        .borrow()
        .get(usize::try_from(controller.window.get_audio_device_index()).unwrap_or(0))
        .cloned()
        .unwrap_or_default()
}

/// Rebuilds the audio-input picker from a fresh view of the `PipeWire` graph.
///
/// The stored selection drives the list rather than the other way round: a
/// device that has been unplugged is still offered, marked unavailable, so
/// applying settings while it is missing cannot quietly reset the choice to
/// "automatic". That is also what makes an explicit refresh useful — the same
/// list rebuilt after a hot-plug simply promotes the entry back to available.
fn populate_audio_devices(
    window: &SettingsWindow,
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &SettingsController,
    settings: &AppSettings,
) {
    let locale = state.borrow().locale();
    let entries = output
        .borrow_mut()
        .audio_input_entries(&settings.audio_input_id);
    let unavailable_suffix =
        crate::i18n::with_catalog(locale, |text| text.settings_ui.audio_input_missing.clone());

    let mut device_ids = vec![String::new()];
    let mut device_names = vec![crate::i18n::with_catalog(locale, |text| {
        text.settings_ui.audio_input_auto.clone()
    })];
    for entry in &entries {
        device_ids.push(entry.id.clone());
        device_names.push(if entry.available {
            SharedString::from(entry.name.as_str())
        } else {
            SharedString::from(format!("{} {unavailable_suffix}", entry.name))
        });
    }

    let selected_device_index = if settings.audio_input_id.is_empty() {
        0
    } else {
        device_ids
            .iter()
            .position(|id| id == &settings.audio_input_id)
            .unwrap_or(0)
    };
    controller.audio_device_ids.replace(device_ids);
    window.set_audio_device_names(string_model(device_names.into_iter()));
    window.set_audio_device_index(i32::try_from(selected_device_index).unwrap_or(0));
    window.set_devices_summary(output.borrow_mut().audio_devices_summary().into());
    window.set_audio_input_missing(entries.iter().any(|entry| !entry.available));
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
    populate_stream_models(window, &output.borrow(), controller, &settings);
    window.set_rtmp_service(settings.rtmp.service.as_str().into());
    window.set_rtmp_server(settings.rtmp.server.as_str().into());
    window.set_rtmp_stream_key(settings.rtmp.stream_key.expose_secret().into());
    window.set_stream_video_bitrate(
        i32::try_from(settings.rtmp.video.bitrate_kbps).unwrap_or(i32::MAX),
    );
    window.set_stream_audio_bitrate(
        i32::try_from(settings.rtmp.audio.bitrate_kbps).unwrap_or(i32::MAX),
    );
    window.set_stream_rate_control(settings.rtmp.video.rate_control.id().to_uppercase().into());
    window.set_stream_keyframe_interval(
        i32::try_from(settings.rtmp.video.keyframe_interval_secs).unwrap_or(i32::MAX),
    );
    window.set_stream_encoder_preset(settings.rtmp.video.preset.id().into());
    window.set_stream_encoder_profile(settings.rtmp.video.profile.as_deref().unwrap_or("").into());
    window.set_stream_b_frames(i32::from(settings.rtmp.video.b_frames));
    window.set_stream_reconnect(settings.rtmp.reconnect);
    window.set_stream_maximum_retries(
        i32::try_from(settings.rtmp.maximum_retries).unwrap_or(i32::MAX),
    );
    window.set_stream_network_buffer(
        i32::try_from(settings.rtmp.network_buffer_ms).unwrap_or(i32::MAX),
    );
    window.set_srt_mode_index(match settings.srt.mode {
        SrtMode::Caller => 0,
        SrtMode::Listener => 1,
        SrtMode::Rendezvous => 2,
    });
    window.set_srt_host(settings.srt.host.as_str().into());
    window.set_srt_port(i32::from(settings.srt.port));
    window.set_srt_latency(i32::try_from(settings.srt.latency_ms).unwrap_or(i32::MAX));
    window.set_srt_passphrase(
        settings
            .srt
            .passphrase
            .as_ref()
            .map_or("", SecretString::expose_secret)
            .into(),
    );
    window.set_srt_key_length_index(match settings.srt.pbkeylen {
        None => 0,
        Some(SrtKeyLength::Bits128) => 1,
        Some(SrtKeyLength::Bits192) => 2,
        Some(SrtKeyLength::Bits256) => 3,
    });
    window.set_srt_stream_id(settings.srt.stream_id.as_deref().unwrap_or("").into());
    window.set_srt_timeout(i32::try_from(settings.srt.connect_timeout_ms).unwrap_or(i32::MAX));
    load_extended_stream_draft(window, &settings);
    window.set_whip_endpoint(settings.whip_endpoint.as_str().into());
    window.set_reference_address(settings.reference_address.as_str().into());
    window.set_recording_path(settings.recording_path.as_str().into());
    window.set_recording_format_index(index_of(&RecordingFormat::ALL, &settings.recording_format));
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
    populate_audio_devices(window, state, output, controller, &settings);

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

    // Re-running discovery is the hot-plug path: a device connected after the
    // window opened is only visible once `pw-dump` is asked again, and the
    // draft selection is preserved across the rebuild.
    let refresh_state = Rc::clone(state);
    let refresh_output = Rc::clone(output);
    let refresh_controller = Rc::clone(controller);
    controller.window.on_refresh_audio_devices(move || {
        refresh_output.borrow_mut().refresh_audio_devices();
        // The picker's current choice, not the committed one, is what the user
        // is in the middle of making, so it is what survives the rebuild.
        let draft = draft_audio_input_id(&refresh_controller);
        let settings = AppSettings {
            audio_input_id: draft,
            ..refresh_controller.settings.borrow().clone()
        };
        populate_audio_devices(
            &refresh_controller.window,
            &refresh_state,
            &refresh_output,
            &refresh_controller,
            &settings,
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

/// Applies a canvas change to the active profile and rebuilds the engine.
///
/// The two halves have to agree: the project is the record of what the canvas
/// is, and the engine is what encodes at that size. If the engine cannot be
/// rebuilt the project change is rolled back, because leaving a project that
/// claims one resolution while the engine encodes another produces a recording
/// whose geometry matches neither.
///
/// # Errors
///
/// Returns the reason the change could not be applied, after the rollback.
pub(crate) fn apply_video_format(
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
    format: VideoFormat,
) -> Result<(), String> {
    let (profile, previous) = {
        let state = state.borrow();
        let project = state.project_session().project();
        (
            project.active_profile().to_string(),
            project
                .active_profile_spec()
                .map(obs_rs_project::Profile::video_format),
        )
    };
    if previous == Some(format) {
        return Ok(());
    }
    state
        .borrow_mut()
        .dispatch(UiCommand::Project(ProjectCommand::SetProfileVideoFormat {
            profile: profile.clone(),
            format,
        }))
        .map_err(|error| error.to_string())?;

    let (project, revision) = {
        let state = state.borrow();
        (
            state.project_session().project().clone(),
            state.project_session().revision(),
        )
    };
    let Err(error) = output.borrow_mut().sync_project(project, revision) else {
        return Ok(());
    };

    let reason = error.to_string();
    // The rollback is itself a project command, so it is undoable and the
    // engine picks it up on the next idle sync exactly like any other edit.
    if let Some(previous) = previous {
        let rolled_back = state.borrow_mut().dispatch(UiCommand::Project(
            ProjectCommand::SetProfileVideoFormat {
                profile,
                format: previous,
            },
        ));
        if let Err(rollback) = rolled_back {
            return Err(format!("{reason}; rollback also failed: {rollback}"));
        }
        return Err(format!("{reason}; the previous canvas was restored"));
    }
    Err(reason)
}

/// Applies a canvas change that was staged while an output was running.
///
/// Called from the idle boundary in the preview timer, which is the first
/// moment the encoders can safely be rebuilt.
pub(crate) fn apply_staged_video_format(
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
) -> Option<Result<VideoFormat, String>> {
    let format = output.borrow_mut().take_staged_video_format()?;
    Some(apply_video_format(state, output, format).map(|()| format))
}

/// Reads every editable field out of the window into a settings document.
///
/// Committing is two distinct jobs — collecting the draft and acting on it —
/// and separating them keeps the "what did the user type" logic away from the
/// "what does that change in the running session" logic.
fn read_draft(controller: &SettingsController) -> AppSettings {
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
    read_recording_draft(controller, &mut settings);
    settings.stream_protocol = controller
        .protocol_ids
        .borrow()
        .get(usize::try_from(window.get_protocol_index()).unwrap_or(0))
        .copied()
        .unwrap_or(StreamProtocol::Reference);
    settings.rtmp.service = window.get_rtmp_service().to_string();
    settings.rtmp.server = window.get_rtmp_server().to_string();
    settings.rtmp.stream_key = SecretString::new(window.get_rtmp_stream_key().to_string());
    if let Some(encoder) = controller
        .video_encoder_ids
        .borrow()
        .get(usize::try_from(window.get_video_encoder_index()).unwrap_or(0))
    {
        settings.rtmp.video.implementation = EncoderImplementation::new(encoder.clone());
    }
    if let Some(encoder) = controller
        .audio_encoder_ids
        .borrow()
        .get(usize::try_from(window.get_audio_encoder_index()).unwrap_or(0))
    {
        settings.rtmp.audio.implementation = EncoderImplementation::new(encoder.clone());
    }
    settings.rtmp.video.bitrate_kbps = unsigned(window.get_stream_video_bitrate());
    settings.rtmp.audio.bitrate_kbps = unsigned(window.get_stream_audio_bitrate());
    settings.rtmp.video.rate_control =
        RateControl::from_id(&window.get_stream_rate_control()).unwrap_or(RateControl::Cbr);
    settings.rtmp.video.keyframe_interval_secs = unsigned(window.get_stream_keyframe_interval());
    settings.rtmp.video.preset = EncoderPreset::from_id(&window.get_stream_encoder_preset())
        .unwrap_or(EncoderPreset::Balanced);
    settings.rtmp.video.profile = nonempty(window.get_stream_encoder_profile().to_string());
    settings.rtmp.video.b_frames = u8::try_from(window.get_stream_b_frames()).unwrap_or(0);
    settings.rtmp.reconnect = window.get_stream_reconnect();
    settings.rtmp.maximum_retries = unsigned(window.get_stream_maximum_retries());
    settings.rtmp.network_buffer_ms = unsigned(window.get_stream_network_buffer());
    settings.srt.mode = match window.get_srt_mode_index() {
        1 => SrtMode::Listener,
        2 => SrtMode::Rendezvous,
        _ => SrtMode::Caller,
    };
    settings.srt.host = window.get_srt_host().to_string();
    settings.srt.port = u16::try_from(window.get_srt_port()).unwrap_or(9_000);
    settings.srt.latency_ms = unsigned(window.get_srt_latency());
    settings.srt.passphrase =
        nonempty(window.get_srt_passphrase().to_string()).map(SecretString::new);
    settings.srt.pbkeylen = match window.get_srt_key_length_index() {
        1 => Some(SrtKeyLength::Bits128),
        2 => Some(SrtKeyLength::Bits192),
        3 => Some(SrtKeyLength::Bits256),
        _ => None,
    };
    settings.srt.stream_id = nonempty(window.get_srt_stream_id().to_string());
    settings.srt.connect_timeout_ms = unsigned(window.get_srt_timeout());
    settings.whip_endpoint = window.get_whip_endpoint().to_string();
    read_extended_stream_draft(window, &mut settings);
    settings.reference_address = window.get_reference_address().to_string();
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
    settings
}

fn load_extended_stream_draft(window: &SettingsWindow, settings: &AppSettings) {
    window.set_whip_bearer_token(
        settings
            .whip_bearer_token
            .as_ref()
            .map_or("", SecretString::expose_secret)
            .into(),
    );
    window.set_hls_directory(settings.hls.directory.to_string_lossy().as_ref().into());
    window.set_hls_segment_duration(
        i32::try_from(settings.hls.segment_duration_secs).unwrap_or(i32::MAX),
    );
    window.set_hls_playlist_size(i32::try_from(settings.hls.playlist_size).unwrap_or(i32::MAX));
    window.set_hls_low_latency(settings.hls.low_latency);
    window.set_rist_host(settings.rist.host.as_str().into());
    window.set_rist_port(i32::from(settings.rist.port));
    window.set_rist_buffer(i32::try_from(settings.rist.sender_buffer_ms).unwrap_or(i32::MAX));
    window.set_rist_shared_secret(
        settings
            .rist
            .shared_secret
            .as_ref()
            .map_or("", SecretString::expose_secret)
            .into(),
    );
}

fn read_extended_stream_draft(window: &SettingsWindow, settings: &mut AppSettings) {
    settings.whip_bearer_token =
        nonempty(window.get_whip_bearer_token().to_string()).map(SecretString::new);
    settings.hls.directory = PathBuf::from(window.get_hls_directory().to_string());
    settings.hls.segment_duration_secs = unsigned(window.get_hls_segment_duration());
    settings.hls.playlist_size = unsigned(window.get_hls_playlist_size());
    settings.hls.low_latency = window.get_hls_low_latency();
    settings.rist.host = window.get_rist_host().to_string();
    settings.rist.port = u16::try_from(window.get_rist_port()).unwrap_or(0);
    settings.rist.sender_buffer_ms = unsigned(window.get_rist_buffer());
    settings.rist.shared_secret =
        nonempty(window.get_rist_shared_secret().to_string()).map(SecretString::new);
}

fn read_recording_draft(controller: &SettingsController, settings: &mut AppSettings) {
    let window = &controller.window;
    settings.recording_format = RecordingFormat::ALL
        .get(usize::try_from(window.get_recording_format_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    settings.recording_path =
        recording_path_for_format(&window.get_recording_path(), settings.recording_format);
    if let Some(codec) = controller
        .recording_codec_ids
        .borrow()
        .get(usize::try_from(window.get_recording_codec_index()).unwrap_or(0))
        .copied()
    {
        settings.recording_codec = codec;
    }
}

fn commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    renderer: &Rc<RefCell<PreviewRenderer>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let window = &controller.window;
    let settings = read_draft(controller);

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

    output.borrow_mut().configure_stream(&settings);
    // A selection the graph cannot resolve is kept rather than reset, so the
    // user has to be told the engine is on the fallback in the meantime;
    // silently recording from a different source would be worse.
    if !output.borrow_mut().audio_input_available() {
        notes.push(format!(
            "audio input {} is not connected; the fallback is capturing until it returns",
            settings.audio_input_id
        ));
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
        let output_active = {
            let state = state.borrow();
            state.recording() || state.streaming()
        };
        if output_active {
            // Changing the canvas mid-output would rebuild the encoders under a
            // container whose frames are already committed at the old geometry,
            // so the change is held rather than applied or thrown away.
            output.borrow_mut().stage_video_format(format);
            notes.push(format!(
                "video: {}x{} is staged and applies when the output stops",
                format.width(),
                format.height()
            ));
        } else if let Err(error) = apply_video_format(state, output, format) {
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
    ui.set_streaming_address(stream_display_label(settings).into());
}

fn unsigned(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn stream_display_label(settings: &AppSettings) -> String {
    match settings.stream_protocol {
        StreamProtocol::Rtmp => format!("RTMP · {}", settings.rtmp.server),
        StreamProtocol::Rtmps => format!("RTMPS · {}", settings.rtmp.server),
        StreamProtocol::Srt => format!("SRT · {}:{}", settings.srt.host, settings.srt.port),
        StreamProtocol::Whip => format!("WHIP · {}", settings.whip_endpoint),
        StreamProtocol::Hls => format!("HLS · {}", settings.hls.directory.display()),
        StreamProtocol::Rist => format!("RIST · {}:{}", settings.rist.host, settings.rist.port),
        StreamProtocol::Reference => format!("Reference · {}", settings.reference_address),
    }
}

fn recording_path_for_format(path: &str, format: RecordingFormat) -> String {
    let mut path = PathBuf::from(path.trim());
    path.set_extension(format.extension());
    path.to_string_lossy().into_owned()
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
    controller.filters.set_tokens(tokens.clone());
    controller.transform.set_tokens(tokens.clone());
    controller.monitor.set_tokens(tokens.clone());
    controller.docks.set_tokens(tokens);
    controller.projectors.set_tokens(tokens);
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
