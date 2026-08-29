#[allow(
    clippy::wildcard_imports,
    reason = "settings callback modules share the controller boundary imports"
)]
use super::*;

#[path = "settings_commit.rs"]
mod settings_commit;
#[path = "settings_helpers.rs"]
mod settings_helpers;
#[path = "settings_pages.rs"]
mod settings_pages;

#[allow(
    clippy::wildcard_imports,
    unused_imports,
    reason = "settings callback modules share the controller boundary imports"
)]
use settings_commit::*;
#[allow(
    clippy::wildcard_imports,
    reason = "settings callback modules share the controller boundary imports"
)]
use settings_helpers::*;
#[allow(
    clippy::wildcard_imports,
    reason = "settings callback modules share the controller boundary imports"
)]
use settings_pages::*;

#[allow(unused_imports)]
pub(crate) use settings_commit::{
    apply_settings_snapshot, apply_staged_audio_format, apply_staged_output_scaling,
    apply_staged_video_format, apply_video_format,
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
    canvas: Rc<CanvasController>,
    /// IDs are kept separate from the display labels shown by Slint's `ComboBox`.
    audio_device_ids: RefCell<Vec<String>>,
    /// IDs for the render devices used by desktop/system loopback.
    audio_desktop_device_ids: RefCell<Vec<String>>,
    /// IDs are kept separate from the display labels shown by Slint's `ComboBox`.
    audio_monitor_output_ids: RefCell<Vec<String>>,
    protocol_ids: RefCell<Vec<StreamProtocol>>,
    video_encoder_ids: RefCell<Vec<String>>,
    audio_encoder_ids: RefCell<Vec<String>>,
    recording_codec_ids: RefCell<Vec<VideoCodec>>,
    recording_format_ids: RefCell<Vec<RecordingFormat>>,
    recording_audio_encoder_ids: RefCell<Vec<String>>,
    recording_audio_encoder_codecs: RefCell<Vec<AudioCodec>>,
    segmented_recording_supported: bool,
    remux_supported: bool,
    /// Whether each offered video encoder runs on dedicated hardware, which is
    /// what decides whether the double-software-encode warning applies.
    video_encoder_hardware: RefCell<Vec<bool>>,
    /// The Video page's draft, kept beside the window because a half-typed
    /// resolution must not overwrite the last usable one.
    draft_video: RefCell<VideoSettings>,
    /// The system directory chooser this session found, if any.
    browse_tool: Option<&'static str>,
    /// Native output runtime identity, if this binary was built with one.
    /// Kept separately so changing the UI locale can rebuild the status text
    /// without probing `GStreamer` on the UI thread.
    production_runtime_version: Option<String>,
    production_status: ProductionOutputStatus,
}

/// The directory choosers the Browse button can drive.
///
/// OBS-RS has no native file-dialog dependency, so Browse runs whichever
/// chooser the desktop already ships. When neither is installed the button is
/// disabled and the page says why, rather than presenting a control that does
/// nothing.
const BROWSE_TOOLS: [&str; 2] = ["zenity", "kdialog"];

/// The Audio category's index in the settings window's sidebar.
const SETTINGS_CATEGORY_AUDIO: i32 = 4;

/// Returns the first directory chooser on `PATH`.
pub(super) fn detect_browse_tool() -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    BROWSE_TOOLS
        .into_iter()
        .find(|tool| std::env::split_paths(&path).any(|directory| directory.join(tool).is_file()))
}

/// Runs the detected chooser and returns the directory the user picked.
///
/// A cancelled dialog and a missing chooser are the same answer — `None` — so
/// the caller keeps the draft it already had.
pub(super) fn choose_directory(tool: &str, start: &str) -> Option<String> {
    let mut command = std::process::Command::new(tool);
    match tool {
        "kdialog" => command.arg("--getexistingdirectory").arg(start),
        _ => command
            .arg("--file-selection")
            .arg("--directory")
            .arg(format!("--filename={start}")),
    };
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let chosen = String::from_utf8(output.stdout).ok()?;
    let chosen = chosen.trim();
    (!chosen.is_empty()).then(|| chosen.to_owned())
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
        {
            let state = state.borrow();
            state
                .preview_scene()
                .unwrap_or_default()
                .clone_into(&mut settings.last_preview_scene);
            state
                .program_scene()
                .unwrap_or_default()
                .clone_into(&mut settings.last_program_scene);
            settings.project_scene_selections = state.project_scene_selections();
        }
        let dock_tree = self.docks.tree_snapshot();
        settings.layout.panel_order = dock_tree.leaf_order();
        settings.layout.dock_tree = dock_tree;
        settings.layout.floating_geometry = self.docks.capture_floating_geometry();
        settings.layout.projector_geometry = self.projectors.capture_geometry();
        settings.layout.projector_targets = self.projectors.capture_targets();
        settings.layout.projector_monitors = self.projectors.capture_monitors();
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
        // The dropdown labels are catalog text too, so they are rebuilt with
        // the catalog rather than staying in the language the window opened in.
        populate_static_models(&self.window);
        apply_production_status(
            &self.window,
            self.production_status,
            self.production_runtime_version.as_deref(),
        );
        self.window
            .global::<Palette>()
            .set_tokens(self.settings.borrow().tokens());
        self.push_metrics(self.settings.borrow().metrics());
    }

    /// Returns the window this controller drives.
    #[cfg(test)]
    pub(crate) const fn window(&self) -> &SettingsWindow {
        &self.window
    }

    /// Returns the committed settings document.
    pub(crate) fn committed(&self) -> AppSettings {
        self.settings.borrow().clone()
    }

    /// Renders one settings page to a PNG file at a fixed appearance.
    ///
    /// This is the visual-regression entry point: the theme, style, density,
    /// font size, and locale are pinned before the snapshot, so the only thing
    /// that can move between two runs is the layout under test. It renders
    /// through whichever backend the process was started with, which is the
    /// software renderer under `--settings-screenshot`.
    ///
    /// # Errors
    ///
    /// Returns the reason the page could not be rendered or written.
    pub(crate) fn capture_page(
        &self,
        page: &str,
        path: &Path,
        locale: UiLocale,
    ) -> Result<(), String> {
        let category = settings_category(page).ok_or_else(|| {
            format!(
                "unknown settings page {page}; expected one of {}",
                SETTINGS_PAGES.map(|(name, _)| name).join(", ")
            )
        })?;
        let pinned = AppSettings {
            theme: 0,
            style: UiStyle::Default,
            density: UiDensity::Normal,
            font_size: crate::settings_model::DEFAULT_FONT_SIZE,
            locale: locale.code().to_owned(),
            ..self.settings.borrow().clone()
        };
        self.window
            .global::<I18n>()
            .set_text(crate::i18n::catalog(pinned.ui_locale()));
        populate_static_models(&self.window);
        self.window.global::<Palette>().set_tokens(pinned.tokens());
        self.push_metrics(pinned.metrics());
        self.window.set_category(category);
        self.window.show().map_err(|error| error.to_string())?;
        let snapshot = self
            .window
            .window()
            .take_snapshot()
            .map_err(|error| error.to_string())?;
        let format = VideoFormat::new(
            snapshot.width(),
            snapshot.height(),
            FrameRate::new(60, 1).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let frame = obs_rs_media::VideoFrame::new(
            format,
            obs_rs_media::Timestamp::ZERO,
            snapshot.as_bytes().to_vec(),
        )
        .map_err(|error| error.to_string())?;
        let png = obs_rs_output::encode_png(&frame).map_err(|error| error.to_string())?;
        std::fs::write(path, png).map_err(|error| error.to_string())?;
        self.window.hide().map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Pushes one metric set onto the settings window.
    ///
    /// Density and font size are previewed live like the theme, so this is
    /// called with draft values too and Cancel puts the committed set back.
    fn push_metrics(&self, metrics: UiMetrics) {
        self.window.global::<Metrics>().set_ui(metrics);
    }
}

/// The other controllers and surfaces the settings window coordinates on a
/// theme or runtime-setting change.
pub(crate) struct PeerWindows {
    pub(crate) add_source: Rc<AddSourceController>,
    pub(crate) properties: Rc<SourcePropertiesController>,
    pub(crate) filters: Rc<SourceFiltersController>,
    pub(crate) transform: Rc<SourceTransformController>,
    pub(crate) monitor: Rc<MonitorController>,
    pub(crate) docks: Rc<crate::callbacks::docks::DockController>,
    pub(crate) projectors: Rc<crate::ProjectorController>,
    pub(crate) canvas: Rc<CanvasController>,
}

/// Creates the settings window and wires it to the studio window.
///
/// The returned controller must outlive the event loop; dropping it closes the
/// settings window.
pub(crate) fn install_settings_window(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    settings: AppSettings,
    path: PathBuf,
    peers: &PeerWindows,
) -> Result<Rc<SettingsController>, slint::PlatformError> {
    let window = SettingsWindow::new()?;
    let capabilities = output.borrow().capabilities().clone();
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
        canvas: Rc::clone(&peers.canvas),
        audio_device_ids: RefCell::new(Vec::new()),
        audio_desktop_device_ids: RefCell::new(Vec::new()),
        audio_monitor_output_ids: RefCell::new(Vec::new()),
        protocol_ids: RefCell::new(Vec::new()),
        video_encoder_ids: RefCell::new(Vec::new()),
        audio_encoder_ids: RefCell::new(Vec::new()),
        recording_codec_ids: RefCell::new(Vec::new()),
        recording_format_ids: RefCell::new(Vec::new()),
        recording_audio_encoder_ids: RefCell::new(Vec::new()),
        recording_audio_encoder_codecs: RefCell::new(Vec::new()),
        segmented_recording_supported: capabilities.supports_segmented_recording(),
        remux_supported: capabilities.supports_remux(),
        video_encoder_hardware: RefCell::new(Vec::new()),
        draft_video: RefCell::new(VideoSettings::default()),
        browse_tool: detect_browse_tool(),
        production_runtime_version: capabilities.native_runtime_version().map(str::to_owned),
        production_status: capabilities.production_status(),
    });

    // The settings document is the persisted source of truth; the canvas
    // receives one validated runtime snapshot before any pointer gesture can
    // start.
    controller
        .canvas
        .set_snap_distance(controller.settings.borrow().canvas_snap_distance);

    populate_static_models(&controller.window);
    apply_production_status(
        &controller.window,
        controller.production_status,
        controller.production_runtime_version.as_deref(),
    );
    install_stream_protocol_switch(&controller);
    output
        .borrow_mut()
        .configure_stream(&controller.settings.borrow());
    output
        .borrow_mut()
        .configure_replay(&controller.settings.borrow());
    output
        .borrow_mut()
        .configure_recording(&controller.settings.borrow());
    apply_to_studio(ui, &controller.settings.borrow());
    push_palette(ui, &controller, &controller.settings.borrow());
    controller.sync_theme(state.borrow().locale());

    install_open(ui, state, surface, output, &controller);
    install_previews(ui, state, surface, &controller);
    install_video_editing(&controller);
    install_output_page(&controller);
    install_commit(ui, state, surface, output, &controller);
    Ok(controller)
}

/// Fills the dropdown models so a test can render every page.
#[cfg(test)]
pub(crate) fn populate_settings_models(window: &SettingsWindow) {
    populate_static_models(window);
}

/// Model contents that never change while the application runs.
pub(super) fn populate_static_models(window: &SettingsWindow) {
    let text = window.global::<I18n>().get_text().settings_ui;
    populate_channel_names(window, &text);
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
    window.set_style_names(string_model(
        [text.style_default, text.style_flat, text.style_contrast].into_iter(),
    ));
    window.set_density_names(string_model(
        [
            text.density_classic,
            text.density_compact,
            text.density_normal,
            text.density_comfortable,
        ]
        .into_iter(),
    ));
    window.set_font_size_minimum(f32::from(*FONT_SIZE_RANGE.start()));
    window.set_font_size_maximum(f32::from(*FONT_SIZE_RANGE.end()));
    window.set_output_mode_names(string_model(
        [text.mode_simple, text.mode_advanced].into_iter(),
    ));
    window.set_encoder_preset_names(string_model(
        [text.preset_speed, text.preset_balanced, text.preset_quality].into_iter(),
    ));
    window.set_rtmp_service_names(string_model(
        RTMP_SERVICE_PRESETS
            .iter()
            .map(|preset| SharedString::from(preset.display_name())),
    ));
    window.set_rtmp_server_names(string_model(std::iter::once(SharedString::from("Primary"))));
    window.set_rtmp_server_index(0);
    window.set_recording_quality_names(string_model(
        [
            text.quality_stream,
            text.quality_high,
            text.quality_indistinguishable,
            text.quality_lossless,
        ]
        .into_iter(),
    ));
    window.set_scale_filter_names(string_model(
        [
            text.filter_bilinear,
            text.filter_bicubic,
            text.filter_lanczos,
        ]
        .into_iter(),
    ));
    window.set_fps_mode_names(string_model(
        [
            text.fps_mode_common,
            text.fps_mode_integer,
            text.fps_mode_fractional,
        ]
        .into_iter(),
    ));
    window.set_sample_rate_names(string_model(
        SAMPLE_RATES
            .iter()
            .map(|rate| SharedString::from(format!("{rate} Hz"))),
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

pub(super) fn populate_channel_names(window: &SettingsWindow, text: &SettingsText) {
    window.set_channel_names(string_model(
        [
            text.channels_stereo.clone(),
            text.channels_mono.clone(),
            text.channels_two_point_one.clone(),
            text.channels_quad.clone(),
            text.channels_five_point_one.clone(),
            text.channels_seven_point_one.clone(),
        ]
        .into_iter(),
    ));
}

pub(super) fn install_stream_protocol_switch(controller: &Rc<SettingsController>) {
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

    let callback_controller = Rc::clone(controller);
    controller.window.on_select_stream_service(move |index| {
        let Some(preset) = RTMP_SERVICE_PRESETS.get(usize::try_from(index).unwrap_or(0)) else {
            return;
        };
        if let Some(protocol_index) = callback_controller
            .protocol_ids
            .borrow()
            .iter()
            .position(|protocol| *protocol == preset.protocol())
        {
            callback_controller
                .window
                .set_protocol_index(i32::try_from(protocol_index).unwrap_or(0));
            show_protocol_fields(&callback_controller.window, preset.protocol());
        }
        populate_stream_server_model(
            &callback_controller.window,
            *preset,
            preset.default_server(),
        );
        callback_controller
            .window
            .set_rtmp_server(preset.default_server().into());
    });

    let callback_controller = Rc::clone(controller);
    controller.window.on_select_stream_server(move |index| {
        let service_index =
            usize::try_from(callback_controller.window.get_rtmp_service_index()).unwrap_or(0);
        let Some(preset) = RTMP_SERVICE_PRESETS.get(service_index).copied() else {
            return;
        };
        let Some(server) = stream_server_for_index(preset, index) else {
            return;
        };
        callback_controller.window.set_rtmp_server(server.into());
    });
}

pub(super) fn populate_stream_server_model(
    window: &SettingsWindow,
    preset: StreamingServicePreset,
    current_server: impl AsRef<str>,
) {
    let current_server = current_server.as_ref();
    let names = std::iter::once(SharedString::from("Primary"))
        .chain(
            preset
                .additional_servers()
                .iter()
                .map(|server| SharedString::from(server.display_name())),
        )
        .collect::<Vec<_>>();
    let selected = stream_server_index(preset, current_server);
    window.set_rtmp_server_names(string_model(names.into_iter()));
    window.set_rtmp_server_index(selected);
}

pub(super) fn stream_server_index(preset: StreamingServicePreset, current_server: &str) -> i32 {
    if preset.default_server() == current_server {
        return 0;
    }
    preset
        .additional_servers()
        .iter()
        .position(|server| server.server() == current_server)
        .and_then(|index| i32::try_from(index + 1).ok())
        .unwrap_or(0)
}

pub(super) fn stream_server_for_index(
    preset: StreamingServicePreset,
    index: i32,
) -> Option<&'static str> {
    let index = usize::try_from(index).ok()?;
    if index == 0 {
        return Some(preset.default_server());
    }
    preset
        .additional_servers()
        .get(index - 1)
        .map(|server| server.server())
}

pub(super) fn populate_stream_models(
    window: &SettingsWindow,
    output: &OutputRuntime,
    controller: &SettingsController,
    settings: &AppSettings,
) {
    apply_production_status(
        window,
        output.capabilities().production_status(),
        output.capabilities().native_runtime_version(),
    );
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

    populate_encoder_models(window, output, controller, settings);
}

/// Keeps the Output page honest about the difference between the portable
/// reference path and a binary that can produce ordinary encoded media.
fn apply_production_status(
    window: &SettingsWindow,
    production_status: ProductionOutputStatus,
    runtime_version: Option<&str>,
) {
    let text = window.global::<I18n>().get_text().settings_ui;
    let status = match production_status {
        ProductionOutputStatus::NativeAdapterNotCompiled => {
            text.production_backend_not_compiled.to_string()
        }
        ProductionOutputStatus::RuntimeUnavailable => {
            text.production_backend_runtime_unavailable.to_string()
        }
        ProductionOutputStatus::NoUsableProfile => text.production_backend_no_profile.to_string(),
        ProductionOutputStatus::Ready => runtime_version.map_or_else(
            || text.production_backend_no_profile.to_string(),
            |version| format!("{}{}", text.production_backend_ready, version),
        ),
    };
    window.set_production_status(status.into());
}

/// Fills the encoder pickers the Output page offers.
///
/// Only implementations discovered at runtime are listed, so a machine without
/// a hardware encoder never shows one it cannot use.
pub(super) fn populate_encoder_models(
    window: &SettingsWindow,
    output: &OutputRuntime,
    controller: &SettingsController,
    settings: &AppSettings,
) {
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
    *controller.video_encoder_hardware.borrow_mut() =
        video.iter().map(|encoder| encoder.hardware()).collect();
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

    let recording_profiles = output.capabilities().recording_formats();
    let recording_formats = RecordingFormat::ALL
        .into_iter()
        .filter(|format| recording_format_available(*format, recording_profiles))
        .collect::<Vec<_>>();
    window.set_recording_format_names(string_model(
        recording_formats
            .iter()
            .map(|format| SharedString::from(format.display_name())),
    ));
    window.set_recording_format_index(index_of(&recording_formats, &settings.recording_format));
    *controller.recording_format_ids.borrow_mut() = recording_formats;

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

    // Recordings pick their audio encoder independently of the stream, which
    // is what the reference Output page offers, so the list is built twice
    // rather than shared by index.
    *controller.recording_audio_encoder_ids.borrow_mut() = audio
        .iter()
        .map(|encoder| encoder.id().to_owned())
        .collect();
    *controller.recording_audio_encoder_codecs.borrow_mut() = audio
        .iter()
        .map(obs_rs_engine::AudioEncoderCapability::codec)
        .collect();
    window.set_recording_audio_encoder_names(string_model(
        audio
            .iter()
            .map(|encoder| SharedString::from(encoder.display_name())),
    ));
    window.set_recording_audio_encoder_index(index_of(
        &controller.recording_audio_encoder_ids.borrow(),
        &settings.recording_audio_encoder.id().to_owned(),
    ));
}

pub(super) fn recording_format_available(
    format: RecordingFormat,
    profiles: &[OutputProfileKind],
) -> bool {
    match format {
        RecordingFormat::ReferencePacket => true,
        RecordingFormat::Matroska => profiles.iter().any(|profile| {
            matches!(
                profile,
                OutputProfileKind::MatroskaH264Aac
                    | OutputProfileKind::MatroskaHevcAac
                    | OutputProfileKind::MatroskaAv1Aac
            )
        }),
        RecordingFormat::Mp4 => profiles.contains(&OutputProfileKind::Mp4H264Aac),
        RecordingFormat::FragmentedMp4 => {
            profiles.contains(&OutputProfileKind::FragmentedMp4H264Aac)
        }
        RecordingFormat::Mov => profiles.contains(&OutputProfileKind::MovH264Aac),
        RecordingFormat::Flv => profiles.contains(&OutputProfileKind::FlvH264Aac),
    }
}

pub(super) fn show_protocol_fields(window: &SettingsWindow, protocol: StreamProtocol) {
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

pub(super) fn install_open(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    fn show_settings_window(
        ui: &MainWindow,
        state: &Rc<RefCell<DesktopState>>,
        surface: &Rc<RefCell<PreviewSurface>>,
        output: &Rc<RefCell<OutputRuntime>>,
        controller: &Rc<SettingsController>,
        category: Option<i32>,
    ) {
        load_draft(state, surface, output, controller);
        if let Some(category) = category {
            controller.window.set_category(category);
        }
        controller.sync_theme(state.borrow().locale());
        match controller.window.show() {
            Ok(()) => controller.window.invoke_focus_keyboard_boundary(),
            Err(error) => ui.set_status_message(format!("Settings window: {error}").into()),
        }
    }

    let weak = ui.as_weak();
    let settings_state = Rc::clone(state);
    let settings_surface = Rc::clone(surface);
    let settings_output = Rc::clone(output);
    let settings_controller = Rc::clone(controller);
    let audio_state = Rc::clone(state);
    let audio_surface = Rc::clone(surface);
    let audio_output = Rc::clone(output);
    let audio_controller = Rc::clone(controller);
    ui.on_open_settings_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        show_settings_window(
            &ui,
            &settings_state,
            &settings_surface,
            &settings_output,
            &settings_controller,
            None,
        );
    });

    let weak = ui.as_weak();
    ui.on_open_audio_settings_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        show_settings_window(
            &ui,
            &audio_state,
            &audio_surface,
            &audio_output,
            &audio_controller,
            Some(SETTINGS_CATEGORY_AUDIO),
        );
    });
}

/// Returns the input ID the picker currently shows, committed or not.
///
/// The list index is the only place a not-yet-applied selection lives, so a
/// rebuild has to read it back rather than fall back to the stored document.
pub(super) fn draft_audio_input_id(controller: &SettingsController) -> String {
    controller
        .audio_device_ids
        .borrow()
        .get(usize::try_from(controller.window.get_audio_device_index()).unwrap_or(0))
        .cloned()
        .unwrap_or_default()
}

/// Returns the monitor-output ID currently shown by the draft picker.
pub(super) fn draft_audio_monitor_output_id(controller: &SettingsController) -> String {
    controller
        .audio_monitor_output_ids
        .borrow()
        .get(usize::try_from(controller.window.get_audio_monitor_output_index()).unwrap_or(0))
        .cloned()
        .unwrap_or_default()
}

/// Returns the desktop-loopback render-device ID currently shown by the draft
/// picker.
pub(super) fn draft_desktop_audio_id(controller: &SettingsController) -> String {
    controller
        .audio_desktop_device_ids
        .borrow()
        .get(usize::try_from(controller.window.get_desktop_audio_device_index()).unwrap_or(0))
        .cloned()
        .unwrap_or_default()
}

pub(super) fn monitor_mode_index(mode: obs_rs_audio::AudioMonitorMode) -> i32 {
    AUDIO_MONITOR_MODES
        .iter()
        .position(|candidate| *candidate == mode)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

pub(super) fn draft_monitor_mode(index: i32) -> obs_rs_audio::AudioMonitorMode {
    AUDIO_MONITOR_MODES
        .get(usize::try_from(index).unwrap_or(0))
        .copied()
        .unwrap_or_default()
}

pub(super) fn monitor_mode_names(locale: UiLocale) -> Vec<SharedString> {
    crate::i18n::with_catalog(locale, |text| {
        vec![
            text.settings_ui.monitor_off.clone(),
            text.settings_ui.monitor_only.clone(),
            text.settings_ui.monitor_and_output.clone(),
        ]
    })
}

/// Rebuilds the audio-input picker from a fresh platform-device snapshot.
///
/// The stored selection drives the list rather than the other way round: a
/// device that has been unplugged is still offered, marked unavailable, so
/// applying settings while it is missing cannot quietly reset the choice to
/// "automatic". That is also what makes an explicit refresh useful — the same
/// list rebuilt after a hot-plug simply promotes the entry back to available.
#[allow(
    clippy::too_many_lines,
    reason = "the settings page updates the three related audio pickers atomically"
)]
pub(super) fn populate_audio_devices(
    window: &SettingsWindow,
    state: &Rc<RefCell<DesktopState>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &SettingsController,
    settings: &AppSettings,
) {
    let locale = state.borrow().locale();
    let (entries, output_entries) = {
        let mut output = output.borrow_mut();
        (
            output.audio_input_entries(&settings.audio_input_id),
            output.audio_output_entries(&settings.audio_monitor_output_id),
        )
    };
    let desktop_output_entries = {
        let mut output = output.borrow_mut();
        output.audio_output_entries(&settings.desktop_audio_id)
    };
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

    let selected_desktop_index = if settings.desktop_audio_id.is_empty() {
        0
    } else {
        desktop_output_entries
            .iter()
            .position(|entry| entry.id == settings.desktop_audio_id)
            .map_or(0, |index| index.saturating_add(1))
    };
    let mut desktop_device_ids = vec![String::new()];
    let mut desktop_device_names = vec![crate::i18n::with_catalog(locale, |text| {
        text.settings_ui.desktop_audio_auto.clone()
    })];
    for entry in &desktop_output_entries {
        desktop_device_ids.push(entry.id.clone());
        desktop_device_names.push(if entry.available {
            SharedString::from(entry.name.as_str())
        } else {
            SharedString::from(format!(
                "{} {}",
                entry.name,
                crate::i18n::with_catalog(locale, |text| {
                    text.settings_ui.audio_input_missing.clone()
                })
            ))
        });
    }
    controller
        .audio_desktop_device_ids
        .replace(desktop_device_ids);
    window.set_desktop_audio_device_names(string_model(desktop_device_names.into_iter()));
    window.set_desktop_audio_device_index(i32::try_from(selected_desktop_index).unwrap_or(0));
    window.set_desktop_audio_missing(desktop_output_entries.iter().any(|entry| !entry.available));

    let selected_monitor_index = if settings.audio_monitor_output_id.is_empty() {
        0
    } else {
        output_entries
            .iter()
            .position(|entry| entry.id == settings.audio_monitor_output_id)
            .map_or(0, |index| index.saturating_add(1))
    };
    let mut monitor_output_ids = vec![String::new()];
    let mut monitor_output_names = vec![crate::i18n::with_catalog(locale, |text| {
        text.settings_ui.audio_monitor_output_disabled.clone()
    })];
    for entry in &output_entries {
        monitor_output_ids.push(entry.id.clone());
        monitor_output_names.push(if entry.available {
            SharedString::from(entry.name.as_str())
        } else {
            SharedString::from(format!(
                "{} {}",
                entry.name,
                crate::i18n::with_catalog(locale, |text| {
                    text.settings_ui.audio_input_missing.clone()
                })
            ))
        });
    }
    controller
        .audio_monitor_output_ids
        .replace(monitor_output_ids);
    window.set_audio_monitor_output_names(string_model(monitor_output_names.into_iter()));
    window.set_audio_monitor_output_index(i32::try_from(selected_monitor_index).unwrap_or(0));
    window.set_audio_monitor_output_missing(output_entries.iter().any(|entry| !entry.available));
    window.set_microphone_monitor_mode_names(string_model(monitor_mode_names(locale).into_iter()));
    window.set_microphone_monitor_mode_index(monitor_mode_index(settings.microphone_monitor_mode));
    window.set_desktop_monitor_mode_names(string_model(monitor_mode_names(locale).into_iter()));
    window.set_desktop_monitor_mode_index(monitor_mode_index(settings.desktop_audio_monitor_mode));
    window.set_devices_summary(output.borrow_mut().audio_devices_summary().into());
    window.set_audio_input_missing(entries.iter().any(|entry| !entry.available));
}
