//! Controller for the standalone settings window.
//!
//! The window edits a draft copy of [`AppSettings`]. Nothing but Apply and OK
//! writes back, so Cancel restores the committed values — including the theme
//! and language, which are previewed live and therefore have to be undone.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use obs_rs_engine::ProductionProtocol;
use obs_rs_media::{FrameRate, ScaleFilter, VideoFormat};
use obs_rs_output::{EncoderImplementation, EncoderPreset, RateControl, VideoCodec};
use obs_rs_output::{SecretString, SrtKeyLength, SrtMode, StreamProtocol};
use obs_rs_project::ProjectCommand;
use obs_rs_ui::{DesktopState, UiCommand, UiLocale};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    callbacks::add_source::AddSourceController,
    callbacks::canvas::CanvasController,
    callbacks::monitor::MonitorController,
    callbacks::source_filters::SourceFiltersController,
    callbacks::source_properties::SourcePropertiesController,
    callbacks::source_transform::SourceTransformController,
    refresh_ui,
    settings::{
        hotkey_conflicts, recording_stamp, shortcut_bindings, AppSettings, RecordingFormat,
        CANVAS_SNAP_DISTANCE_DEFAULT, CANVAS_SNAP_DISTANCE_RANGE, CHANNEL_LAYOUTS, FRAME_RATES,
        REPLAY_BUFFER_CAPACITY_MIB_DEFAULT, REPLAY_BUFFER_CAPACITY_MIB_RANGE,
        REPLAY_BUFFER_DURATION_DEFAULT, REPLAY_BUFFER_DURATION_RANGE, RESOLUTIONS, SAMPLE_RATES,
        THEMES,
    },
    settings_model::{
        aspect_ratio_text, parse_resolution, resolution_text, FpsMode, OutputMode,
        RecordingQuality, UiDensity, UiStyle, VideoSettings, FONT_SIZE_RANGE,
    },
    I18n, MainWindow, Metrics, OutputRuntime, Palette, PreviewSurface, SettingsWindow, UiMetrics,
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
    protocol_ids: RefCell<Vec<StreamProtocol>>,
    video_encoder_ids: RefCell<Vec<String>>,
    audio_encoder_ids: RefCell<Vec<String>>,
    recording_codec_ids: RefCell<Vec<VideoCodec>>,
    recording_audio_encoder_ids: RefCell<Vec<String>>,
    /// Whether each offered video encoder runs on dedicated hardware, which is
    /// what decides whether the double-software-encode warning applies.
    video_encoder_hardware: RefCell<Vec<bool>>,
    /// The Video page's draft, kept beside the window because a half-typed
    /// resolution must not overwrite the last usable one.
    draft_video: RefCell<VideoSettings>,
    /// The system directory chooser this session found, if any.
    browse_tool: Option<&'static str>,
}

/// The directory choosers the Browse button can drive.
///
/// OBS-RS has no native file-dialog dependency, so Browse runs whichever
/// chooser the desktop already ships. When neither is installed the button is
/// disabled and the page says why, rather than presenting a control that does
/// nothing.
const BROWSE_TOOLS: [&str; 2] = ["zenity", "kdialog"];

/// Returns the first directory chooser on `PATH`.
fn detect_browse_tool() -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    BROWSE_TOOLS
        .into_iter()
        .find(|tool| std::env::split_paths(&path).any(|directory| directory.join(tool).is_file()))
}

/// Runs the detected chooser and returns the directory the user picked.
///
/// A cancelled dialog and a missing chooser are the same answer — `None` — so
/// the caller keeps the draft it already had.
fn choose_directory(tool: &str, start: &str) -> Option<String> {
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
        protocol_ids: RefCell::new(Vec::new()),
        video_encoder_ids: RefCell::new(Vec::new()),
        audio_encoder_ids: RefCell::new(Vec::new()),
        recording_codec_ids: RefCell::new(Vec::new()),
        recording_audio_encoder_ids: RefCell::new(Vec::new()),
        video_encoder_hardware: RefCell::new(Vec::new()),
        draft_video: RefCell::new(VideoSettings::default()),
        browse_tool: detect_browse_tool(),
    });

    // The settings document is the persisted source of truth; the canvas
    // receives one validated runtime snapshot before any pointer gesture can
    // start.
    controller
        .canvas
        .set_snap_distance(controller.settings.borrow().canvas_snap_distance);

    populate_static_models(&controller.window);
    install_stream_protocol_switch(&controller);
    output
        .borrow_mut()
        .configure_stream(&controller.settings.borrow());
    output
        .borrow_mut()
        .configure_replay(&controller.settings.borrow());
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

    populate_encoder_models(window, output, controller, settings);
}

/// Fills the encoder pickers the Output page offers.
///
/// Only implementations discovered at runtime are listed, so a machine without
/// a hardware encoder never shows one it cannot use.
fn populate_encoder_models(
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
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let surface = Rc::clone(surface);
    let output = Rc::clone(output);
    let controller = Rc::clone(controller);
    ui.on_open_settings_window(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        load_draft(&state, &surface, &output, &controller);
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
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let settings = controller.settings.borrow();
    let window = &controller.window;
    window.set_dirty(false);
    window.set_language_index(locale_index(state.borrow().locale()));
    window.set_theme_index(i32::try_from(settings.theme).unwrap_or(0));
    window.set_style_index(index_of(&UiStyle::ALL, &settings.style));
    window.set_density_index(index_of(&UiDensity::ALL, &settings.density));
    window.set_font_size(f32::from(settings.font_size));
    window.set_confirm_start_stream(settings.confirm_start_stream);
    window.set_confirm_stop_stream(settings.confirm_stop_stream);
    window.set_confirm_stop_recording(settings.confirm_stop_recording);
    window.set_auto_record_when_streaming(settings.auto_record_when_streaming);
    window.set_snap_distance(i32::from(settings.canvas_snap_distance));
    window.set_show_safe_areas(settings.show_safe_areas);
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
    window.set_encoder_preset_index(index_of(
        &[
            EncoderPreset::Speed,
            EncoderPreset::Balanced,
            EncoderPreset::Quality,
        ],
        &settings.rtmp.video.preset,
    ));
    window.set_output_mode_index(index_of(&OutputMode::ALL, &settings.output_mode));
    window.set_output_simple_mode(settings.output_mode == OutputMode::Simple);
    window.set_custom_encoder_settings(settings.stream_custom_encoder);
    window.set_stream_encoder_profile(settings.rtmp.video.profile.as_deref().unwrap_or("").into());
    window.set_stream_b_frames(i32::from(settings.rtmp.video.b_frames));
    window.set_stream_reconnect(settings.rtmp.reconnect);
    window.set_stream_maximum_retries(
        i32::try_from(settings.rtmp.maximum_retries).unwrap_or(i32::MAX),
    );
    window.set_stream_network_buffer(
        i32::try_from(settings.rtmp.network_buffer_ms).unwrap_or(i32::MAX),
    );
    load_connection_draft(window, &settings);
    window.set_whip_endpoint(settings.whip_endpoint.as_str().into());
    window.set_reference_address(settings.reference_address.as_str().into());
    window.set_recording_directory(settings.recording_directory.as_str().into());
    window.set_recording_filename_without_spaces(settings.recording_filename_without_spaces);
    window.set_recording_quality_index(index_of(
        &RecordingQuality::ALL,
        &settings.recording_quality,
    ));
    window.set_recording_format_index(index_of(&RecordingFormat::ALL, &settings.recording_format));
    window.set_replay_buffer_duration(
        i32::try_from(settings.replay_buffer_duration_seconds)
            .unwrap_or(i32::try_from(REPLAY_BUFFER_DURATION_DEFAULT).unwrap_or(20)),
    );
    window.set_replay_buffer_capacity(
        i32::try_from(settings.replay_buffer_capacity_mib)
            .unwrap_or(i32::try_from(REPLAY_BUFFER_CAPACITY_MIB_DEFAULT).unwrap_or(64)),
    );
    window.set_browse_enabled(controller.browse_tool.is_some());
    window.set_browse_hint(
        window
            .global::<I18n>()
            .get_text()
            .settings_ui
            .browse_unavailable_hint,
    );
    window.set_project_path(settings.project_path.as_str().into());
    window.set_diagnostics_path(settings.diagnostics_path.as_str().into());
    window.set_restore_project(settings.restore_project);
    window.set_save_project_on_exit(settings.save_project_on_exit);
    window.set_hotkey_swap(settings.hotkey_swap.as_str().into());
    window.set_hotkey_start_recording(settings.hotkey_start_recording.as_str().into());
    window.set_hotkey_stop_recording(settings.hotkey_stop_recording.as_str().into());
    window.set_hotkey_start_streaming(settings.hotkey_start_streaming.as_str().into());
    window.set_hotkey_stop_streaming(settings.hotkey_stop_streaming.as_str().into());
    window.set_hotkey_undo(settings.hotkey_undo.as_str().into());
    window.set_hotkey_redo(settings.hotkey_redo.as_str().into());
    window.set_hotkey_save_project(settings.hotkey_save_project.as_str().into());
    window.set_hotkey_fade_transition(settings.hotkey_fade_transition.as_str().into());
    window.set_hotkey_save_replay(settings.hotkey_save_replay.as_str().into());
    window.set_hotkeys_conflict(hotkey_conflicts(&settings).join(", ").into());
    window.set_preview_border_color(settings.preview_border_color.as_str().into());
    window.set_program_border_color(settings.program_border_color.as_str().into());
    window.set_preview_swatch(settings.tokens().preview_border);
    window.set_program_swatch(settings.tokens().program_border);

    let audio_format = state.borrow().audio_format();
    window.set_sample_rate_index(index_of(&SAMPLE_RATES, &audio_format.sample_rate()));
    window.set_channel_index(index_of(&CHANNEL_LAYOUTS, &audio_format.channels()));
    populate_audio_devices(window, state, output, controller, &settings);

    load_video_draft(surface, controller, &settings);
}

/// Copies the Video page's values into the draft.
///
/// The canvas the session is actually rendering at wins over the stored one:
/// the project owns the canvas, and a settings document that has drifted from
/// it must not silently resize the renderer on the next Apply.
fn load_video_draft(
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<SettingsController>,
    settings: &AppSettings,
) {
    let window = &controller.window;
    let video_format = surface.borrow().format;
    let video = VideoSettings {
        base_width: video_format.width(),
        base_height: video_format.height(),
        fps_numerator: video_format.frame_rate().numerator(),
        fps_denominator: video_format.frame_rate().denominator(),
        ..settings.video
    };
    *controller.draft_video.borrow_mut() = video;
    window.set_base_resolution(resolution_text(video.base_width, video.base_height).into());
    window.set_output_resolution(resolution_text(video.output_width, video.output_height).into());
    window.set_base_resolution_valid(true);
    window.set_output_resolution_valid(true);
    window.set_scale_filter_index(index_of(&ScaleFilter::ALL, &video.scale_filter));
    window.set_fps_mode_index(index_of(&FpsMode::ALL, &video.fps_mode));
    window.set_fps_numerator(i32::try_from(video.fps_numerator).unwrap_or(60));
    window.set_fps_denominator(i32::try_from(video.fps_denominator).unwrap_or(1));
    window.set_frame_rate_index(index_of(
        &FRAME_RATES,
        &(video.fps_numerator, video.fps_denominator),
    ));
    show_fps_fields(window, video.fps_mode);
    refresh_video_page(controller);
    refresh_recording_page(controller, settings.recording_quality);
}

/// Copies the protocol-specific connection fields into the draft.
///
/// These belong to the Stream page's destination, not to encoding, so they are
/// loaded together and kept out of the page-wide draft loader.
fn load_connection_draft(window: &SettingsWindow, settings: &AppSettings) {
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
    load_extended_stream_draft(window, settings);
}

/// Theme and language change the moment they are picked, because judging either
/// from a dropdown label alone is not realistic.
fn install_previews(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    controller: &Rc<SettingsController>,
) {
    let weak = ui.as_weak();
    let theme_controller = Rc::clone(controller);
    controller.window.on_preview_theme(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        preview_appearance(&ui, &theme_controller);
        let _ = index;
    });

    let weak = ui.as_weak();
    let style_controller = Rc::clone(controller);
    controller.window.on_preview_style(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        preview_appearance(&ui, &style_controller);
        let _ = index;
    });

    // Density and font size change geometry rather than colour, so they push
    // the metric set instead of the palette. Both are previewed for the same
    // reason the theme is: a name in a dropdown does not tell you what the
    // window will look like.
    let density_controller = Rc::clone(controller);
    controller.window.on_preview_density(move |index| {
        density_controller.push_metrics(draft_metrics(&density_controller));
        let _ = index;
    });

    let font_controller = Rc::clone(controller);
    controller.window.on_preview_font_size(move |value| {
        font_controller.push_metrics(draft_metrics(&font_controller));
        let _ = value;
    });

    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let surface = Rc::clone(surface);
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
            refresh_ui(&ui, &state, &surface);
            language_controller.sync_theme(locale);
        }
    });
}

/// Repaints every window with the theme and style the Appearance page shows.
fn preview_appearance(ui: &MainWindow, controller: &Rc<SettingsController>) {
    let window = &controller.window;
    let theme = usize::try_from(window.get_theme_index())
        .unwrap_or(0)
        .min(THEMES.len() - 1);
    let style = draft_style(window);
    let tokens = controller.settings.borrow().tokens_for(theme, style);
    push_palette_tokens(ui, controller, &tokens);
}

/// Returns the style the Appearance page currently shows.
fn draft_style(window: &SettingsWindow) -> UiStyle {
    UiStyle::ALL
        .get(usize::try_from(window.get_style_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default()
}

/// Returns the density the Appearance page currently shows.
fn draft_density(window: &SettingsWindow) -> UiDensity {
    UiDensity::ALL
        .get(usize::try_from(window.get_density_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default()
}

/// Returns the font size the Appearance page currently shows.
fn draft_font_size(window: &SettingsWindow) -> u8 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the slider is bounded to the font size range before the cast"
    )]
    let value = window
        .get_font_size()
        .clamp(
            f32::from(*FONT_SIZE_RANGE.start()),
            f32::from(*FONT_SIZE_RANGE.end()),
        )
        .round() as u8;
    value
}

/// Builds the metric set for the appearance the window currently shows.
fn draft_metrics(controller: &Rc<SettingsController>) -> UiMetrics {
    AppSettings::metrics_for(
        draft_density(&controller.window),
        draft_font_size(&controller.window),
    )
}

/// Wires the Video page's editable resolutions and frame-rate modes.
///
/// The typed value is validated on every keystroke and only a usable pair is
/// written to the draft, so a field left half-typed cannot be committed and
/// cannot overwrite the resolution the session is already rendering at.
fn install_video_editing(controller: &Rc<SettingsController>) {
    let base_controller = Rc::clone(controller);
    controller.window.on_edit_base_resolution(move |value| {
        let parsed = parse_resolution(&value);
        base_controller
            .window
            .set_base_resolution_valid(parsed.is_some());
        if let Some((width, height)) = parsed {
            let mut video = base_controller.draft_video.borrow_mut();
            video.base_width = width;
            video.base_height = height;
        }
        refresh_video_page(&base_controller);
    });

    let output_controller = Rc::clone(controller);
    controller.window.on_edit_output_resolution(move |value| {
        let parsed = parse_resolution(&value);
        output_controller
            .window
            .set_output_resolution_valid(parsed.is_some());
        if let Some((width, height)) = parsed {
            let mut video = output_controller.draft_video.borrow_mut();
            video.output_width = width;
            video.output_height = height;
        }
        refresh_video_page(&output_controller);
    });

    let mode_controller = Rc::clone(controller);
    controller.window.on_select_fps_mode(move |index| {
        let mode = FpsMode::ALL
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or_default();
        mode_controller.draft_video.borrow_mut().fps_mode = mode;
        show_fps_fields(&mode_controller.window, mode);
        // Switching to an explicit mode starts from the rate that is already
        // selected, so the frame rate never jumps because the presentation of
        // it changed.
        let (numerator, denominator) = {
            let video = mode_controller.draft_video.borrow();
            (video.fps_numerator, video.fps_denominator)
        };
        mode_controller
            .window
            .set_fps_numerator(i32::try_from(numerator).unwrap_or(60));
        mode_controller
            .window
            .set_fps_denominator(i32::try_from(denominator).unwrap_or(1));
    });

    let rate_controller = Rc::clone(controller);
    controller.window.on_select_frame_rate(move |index| {
        let Some((numerator, denominator)) = FRAME_RATES
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
        else {
            return;
        };
        let mut video = rate_controller.draft_video.borrow_mut();
        video.fps_numerator = numerator;
        video.fps_denominator = denominator;
    });
}

/// Shows the frame-rate control the selected FPS mode uses.
fn show_fps_fields(window: &SettingsWindow, mode: FpsMode) {
    window.set_fps_common(mode == FpsMode::Common);
    window.set_fps_integer(mode == FpsMode::Integer);
    window.set_fps_fractional(mode == FpsMode::Fractional);
}

/// Recomputes the aspect ratios and the downscale state from the draft.
fn refresh_video_page(controller: &Rc<SettingsController>) {
    let video = *controller.draft_video.borrow();
    let window = &controller.window;
    window.set_base_aspect_ratio(aspect_ratio_text(video.base_width, video.base_height).into());
    window
        .set_output_aspect_ratio(aspect_ratio_text(video.output_width, video.output_height).into());
    window.set_downscale_active(!video.is_unscaled());
    // The picker points at the entry the value matches; a custom size leaves
    // it where it was rather than jumping to an unrelated resolution.
    if let Some(index) = suggestion_index(video.base_width, video.base_height) {
        window.set_base_suggestion_index(index);
    }
    if let Some(index) = suggestion_index(video.output_width, video.output_height) {
        window.set_output_suggestion_index(index);
    }
}

/// Returns the offered resolution matching `width` and `height`, if any.
fn suggestion_index(width: u32, height: u32) -> Option<i32> {
    RESOLUTIONS
        .iter()
        .position(|entry| *entry == (width, height))
        .and_then(|index| i32::try_from(index).ok())
}

/// Wires the Output page's mode switch, quality preset, and Browse button.
fn install_output_page(controller: &Rc<SettingsController>) {
    let mode_controller = Rc::clone(controller);
    controller.window.on_select_output_mode(move |index| {
        let mode = OutputMode::ALL
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or_default();
        mode_controller
            .window
            .set_output_simple_mode(mode == OutputMode::Simple);
    });

    let quality_controller = Rc::clone(controller);
    controller.window.on_select_recording_quality(move |index| {
        let quality = RecordingQuality::ALL
            .get(usize::try_from(index).unwrap_or(0))
            .copied()
            .unwrap_or_default();
        refresh_recording_page(&quality_controller, quality);
    });

    let browse_controller = Rc::clone(controller);
    controller.window.on_browse_recording_directory(move || {
        let Some(tool) = browse_controller.browse_tool else {
            return;
        };
        let start = browse_controller
            .window
            .get_recording_directory()
            .to_string();
        let Some(directory) = choose_directory(tool, &start) else {
            return;
        };
        browse_controller
            .window
            .set_recording_directory(directory.as_str().into());
        browse_controller.window.set_dirty(true);
        refresh_recording_page(
            &browse_controller,
            draft_recording_quality(&browse_controller.window),
        );
    });
}

/// Returns the recording quality the Output page currently shows.
fn draft_recording_quality(window: &SettingsWindow) -> RecordingQuality {
    RecordingQuality::ALL
        .get(usize::try_from(window.get_recording_quality_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default()
}

/// Recomputes everything the recording group derives from its own controls.
fn refresh_recording_page(controller: &Rc<SettingsController>, quality: RecordingQuality) {
    let window = &controller.window;
    window.set_recording_format_locked(quality.is_lossless());
    let format = if quality.is_lossless() {
        RecordingFormat::ReferencePacket
    } else {
        RecordingFormat::ALL
            .get(usize::try_from(window.get_recording_format_index()).unwrap_or(0))
            .copied()
            .unwrap_or_default()
    };
    let settings = AppSettings {
        recording_directory: window.get_recording_directory().to_string(),
        recording_filename_without_spaces: window.get_recording_filename_without_spaces(),
        recording_quality: quality,
        recording_format: format,
        ..controller.settings.borrow().clone()
    };
    // Only the path is pushed; the sentence around it is composed in the page
    // so it follows the catalog the window is currently showing.
    window.set_recording_file_preview(
        settings
            .recording_file_path(&recording_stamp(std::time::SystemTime::now()))
            .into(),
    );
    // Two software encodes of every frame is the one combination worth warning
    // about, so the warning depends on the encoder actually selected.
    let software_stream = controller
        .video_encoder_hardware
        .borrow()
        .get(usize::try_from(window.get_video_encoder_index()).unwrap_or(0))
        .is_some_and(|hardware| !hardware);
    window.set_software_encoding_warning(
        software_stream && quality != RecordingQuality::SameAsStream,
    );
}

fn install_commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let weak = ui.as_weak();
    let apply_state = Rc::clone(state);
    let apply_surface = Rc::clone(surface);
    let apply_output = Rc::clone(output);
    let apply_controller = Rc::clone(controller);
    controller.window.on_apply_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        commit(
            &ui,
            &apply_state,
            &apply_surface,
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
    let accept_surface = Rc::clone(surface);
    let accept_output = Rc::clone(output);
    let accept_controller = Rc::clone(controller);
    controller.window.on_accept_settings(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        commit(
            &ui,
            &accept_state,
            &accept_surface,
            &accept_output,
            &accept_controller,
        );
        let _ = accept_controller.window.hide();
    });

    let weak = ui.as_weak();
    let state = Rc::clone(state);
    let surface = Rc::clone(surface);
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
            refresh_ui(&ui, &state, &surface);
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

/// The settings pages by name, in sidebar order.
///
/// The screenshot harness names pages rather than numbering them, so a
/// reordered sidebar cannot silently change which page a golden image holds.
pub(crate) const SETTINGS_PAGES: [(&str, i32); 9] = [
    ("general", 0),
    ("appearance", 1),
    ("stream", 2),
    ("output", 3),
    ("audio", 4),
    ("video", 5),
    ("hotkeys", 6),
    ("accessibility", 7),
    ("advanced", 8),
];

/// Returns the sidebar index for a page name.
pub(crate) fn settings_category(page: &str) -> Option<i32> {
    SETTINGS_PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, index)| *index)
}

/// The Video category's index in the settings window's sidebar.
///
/// Validation failures switch to the page carrying the invalid field, so the
/// user is looking at the row that stopped the commit.
const SETTINGS_CATEGORY_VIDEO: i32 = 5;

/// The Hotkeys category's index in the settings window's sidebar.
const SETTINGS_CATEGORY_HOTKEYS: i32 = 6;

/// Applies an output-scaling change that was staged while an output was
/// running.
///
/// Called from the same idle boundary as the staged canvas change, which is
/// the first moment the encoders can safely be rebuilt.
pub(crate) fn apply_staged_output_scaling(
    output: &Rc<RefCell<OutputRuntime>>,
) -> Option<Result<(u32, u32), String>> {
    let (width, height, filter) = output.borrow_mut().take_staged_output_scaling()?;
    Some(
        output
            .borrow_mut()
            .set_output_scaling(width, height, filter)
            .map(|()| (width, height))
            .map_err(|error| error.to_string()),
    )
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
    settings.style = draft_style(window);
    settings.density = draft_density(window);
    settings.font_size = draft_font_size(window);
    settings.video = read_video_draft(controller);
    settings.confirm_start_stream = window.get_confirm_start_stream();
    settings.confirm_stop_stream = window.get_confirm_stop_stream();
    settings.confirm_stop_recording = window.get_confirm_stop_recording();
    settings.auto_record_when_streaming = window.get_auto_record_when_streaming();
    read_canvas_draft(window, &mut settings);
    settings.sample_rate = usize::try_from(window.get_sample_rate_index())
        .unwrap_or(0)
        .min(SAMPLE_RATES.len() - 1);
    settings.channels = usize::try_from(window.get_channel_index())
        .unwrap_or(0)
        .min(CHANNEL_LAYOUTS.len() - 1);
    read_hotkey_draft(window, &mut settings);
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
    settings.rtmp.video.preset = [
        EncoderPreset::Speed,
        EncoderPreset::Balanced,
        EncoderPreset::Quality,
    ]
    .get(usize::try_from(window.get_encoder_preset_index()).unwrap_or(1))
    .copied()
    .unwrap_or(EncoderPreset::Balanced);
    settings.output_mode = OutputMode::ALL
        .get(usize::try_from(window.get_output_mode_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    settings.stream_custom_encoder = window.get_custom_encoder_settings();
    settings.rtmp.video.profile = nonempty(window.get_stream_encoder_profile().to_string());
    settings.rtmp.video.b_frames = u8::try_from(window.get_stream_b_frames()).unwrap_or(0);
    settings.rtmp.reconnect = window.get_stream_reconnect();
    settings.rtmp.maximum_retries = unsigned(window.get_stream_maximum_retries());
    settings.rtmp.network_buffer_ms = unsigned(window.get_stream_network_buffer());
    read_connection_draft(window, &mut settings);
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

/// Reads the hotkey fields through the shared typed parser before Apply can
/// publish them to the persistent settings document.
fn read_hotkey_draft(window: &SettingsWindow, settings: &mut AppSettings) {
    settings.hotkey_swap =
        crate::settings::validated_hotkey(window.get_hotkey_swap().as_str(), &settings.hotkey_swap);
    settings.hotkey_start_recording = crate::settings::validated_hotkey(
        window.get_hotkey_start_recording().as_str(),
        &settings.hotkey_start_recording,
    );
    settings.hotkey_stop_recording = crate::settings::validated_hotkey(
        window.get_hotkey_stop_recording().as_str(),
        &settings.hotkey_stop_recording,
    );
    settings.hotkey_start_streaming = crate::settings::validated_hotkey(
        window.get_hotkey_start_streaming().as_str(),
        &settings.hotkey_start_streaming,
    );
    settings.hotkey_stop_streaming = crate::settings::validated_hotkey(
        window.get_hotkey_stop_streaming().as_str(),
        &settings.hotkey_stop_streaming,
    );
    settings.hotkey_undo =
        crate::settings::validated_hotkey(window.get_hotkey_undo().as_str(), &settings.hotkey_undo);
    settings.hotkey_redo =
        crate::settings::validated_hotkey(window.get_hotkey_redo().as_str(), &settings.hotkey_redo);
    settings.hotkey_save_project = crate::settings::validated_hotkey(
        window.get_hotkey_save_project().as_str(),
        &settings.hotkey_save_project,
    );
    settings.hotkey_fade_transition = crate::settings::validated_hotkey(
        window.get_hotkey_fade_transition().as_str(),
        &settings.hotkey_fade_transition,
    );
    settings.hotkey_save_replay = crate::settings::validated_hotkey(
        window.get_hotkey_save_replay().as_str(),
        &settings.hotkey_save_replay,
    );
}

/// Reads the canvas-only settings as one validated presentation policy.
fn read_canvas_draft(window: &SettingsWindow, settings: &mut AppSettings) {
    settings.canvas_snap_distance = u16::try_from(window.get_snap_distance())
        .unwrap_or(CANVAS_SNAP_DISTANCE_DEFAULT)
        .clamp(
            *CANVAS_SNAP_DISTANCE_RANGE.start(),
            *CANVAS_SNAP_DISTANCE_RANGE.end(),
        );
    settings.show_safe_areas = window.get_show_safe_areas();
}

/// Reads the protocol-specific connection fields out of the draft.
fn read_connection_draft(window: &SettingsWindow, settings: &mut AppSettings) {
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
    read_extended_stream_draft(window, settings);
    settings.reference_address = window.get_reference_address().to_string();
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
    settings.recording_quality = draft_recording_quality(window);
    settings.recording_format = RecordingFormat::ALL
        .get(usize::try_from(window.get_recording_format_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    settings.recording_directory = window.get_recording_directory().to_string();
    settings.recording_filename_without_spaces = window.get_recording_filename_without_spaces();
    settings.replay_buffer_duration_seconds = u32::try_from(window.get_replay_buffer_duration())
        .ok()
        .filter(|duration| REPLAY_BUFFER_DURATION_RANGE.contains(duration))
        .unwrap_or(REPLAY_BUFFER_DURATION_DEFAULT);
    settings.replay_buffer_capacity_mib = u32::try_from(window.get_replay_buffer_capacity())
        .ok()
        .filter(|capacity| REPLAY_BUFFER_CAPACITY_MIB_RANGE.contains(capacity))
        .unwrap_or(REPLAY_BUFFER_CAPACITY_MIB_DEFAULT);
    // The concrete file is derived here rather than typed, so the name, the
    // extension, and the container always agree. The studio's own start-
    // recording dialog can still edit the path afterwards.
    settings.recording_path =
        settings.recording_file_path(&recording_stamp(std::time::SystemTime::now()));
    if let Some(codec) = controller
        .recording_codec_ids
        .borrow()
        .get(usize::try_from(window.get_recording_codec_index()).unwrap_or(0))
        .copied()
    {
        settings.recording_codec = codec;
    }
    if let Some(encoder) = controller
        .recording_audio_encoder_ids
        .borrow()
        .get(usize::try_from(window.get_recording_audio_encoder_index()).unwrap_or(0))
    {
        settings.recording_audio_encoder = EncoderImplementation::new(encoder.clone());
    }
}

/// Reads the Video page's draft, taking the frame rate from whichever control
/// the selected FPS mode shows.
fn read_video_draft(controller: &SettingsController) -> VideoSettings {
    let window = &controller.window;
    let mut video = *controller.draft_video.borrow();
    video.scale_filter = ScaleFilter::ALL
        .get(usize::try_from(window.get_scale_filter_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    video.fps_mode = FpsMode::ALL
        .get(usize::try_from(window.get_fps_mode_index()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    match video.fps_mode {
        FpsMode::Common => {}
        FpsMode::Integer => {
            video.fps_numerator = unsigned(window.get_fps_numerator()).max(1);
            video.fps_denominator = 1;
        }
        FpsMode::Fractional => {
            video.fps_numerator = unsigned(window.get_fps_numerator()).max(1);
            video.fps_denominator = unsigned(window.get_fps_denominator()).max(1);
        }
    }
    video
}

fn commit(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
) {
    let window = &controller.window;
    // Validation runs before anything is written: a rejected field leaves the
    // window open with the offending row marked and commits nothing at all,
    // rather than persisting the settings that happened to be readable.
    if !window.get_base_resolution_valid() || !window.get_output_resolution_valid() {
        window.set_category(SETTINGS_CATEGORY_VIDEO);
        ui.set_status_message(
            crate::i18n::with_catalog(controller.settings.borrow().ui_locale(), |text| {
                text.settings_ui.resolution_invalid.clone()
            })
            .to_string()
            .into(),
        );
        return;
    }
    let settings = read_draft(controller);
    let conflicts = hotkey_conflicts(&settings);
    if !conflicts.is_empty() {
        let conflict_list = conflicts.join(", ");
        window.set_category(SETTINGS_CATEGORY_HOTKEYS);
        window.set_hotkeys_conflict(conflict_list.as_str().into());
        ui.set_status_message(
            crate::i18n::with_catalog(controller.settings.borrow().ui_locale(), |text| {
                format!("{}: {conflict_list}", text.settings_ui.hotkeys_conflict)
            })
            .into(),
        );
        return;
    }
    window.set_hotkeys_conflict("".into());
    apply_settings_snapshot(ui, state, surface, output, controller, &settings);
}

/// Applies a validated settings snapshot from either the settings draft or the
/// first-run setup wizard.
///
/// Setup deliberately comes through the same path as the ordinary settings
/// window so audio routing, output staging, project video geometry, palette,
/// and persistence cannot drift into two subtly different implementations.
pub(crate) fn apply_settings_snapshot(
    ui: &MainWindow,
    state: &Rc<RefCell<DesktopState>>,
    surface: &Rc<RefCell<PreviewSurface>>,
    output: &Rc<RefCell<OutputRuntime>>,
    controller: &Rc<SettingsController>,
    settings: &AppSettings,
) {
    let window = &controller.window;

    // Publish the complete typed shortcut table first. This keeps the running
    // event path and the persisted settings atomic: a malformed or conflicting
    // map cannot leave the UI displaying one binding set while the state owns
    // another.
    let bindings = match shortcut_bindings(settings) {
        Ok(bindings) => bindings,
        Err(error) => {
            ui.set_status_message(format!("Hotkeys could not be applied: {error}").into());
            window.set_category(SETTINGS_CATEGORY_HOTKEYS);
            return;
        }
    };
    if let Err(error) = state.borrow_mut().replace_shortcuts(&bindings) {
        ui.set_status_message(format!("Hotkeys could not be applied: {error}").into());
        window.set_category(SETTINGS_CATEGORY_HOTKEYS);
        return;
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

    output.borrow_mut().configure_stream(settings);
    output.borrow_mut().configure_replay(settings);
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
    // the project and the surface rebuilds on the next sync, and configure the
    // encoders for the scaled output geometry beside it. Both are canvas-class
    // changes, so both are staged together while an output is running.
    let output_active = {
        let state = state.borrow();
        state.recording() || state.streaming()
    };
    if let Some(format) = video_format_from(settings.video) {
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
    if output_active {
        output.borrow_mut().stage_output_scaling(
            settings.video.output_width,
            settings.video.output_height,
            settings.video.scale_filter,
        );
        notes.push(format!(
            "output scaling: {}x{} is staged and applies when the output stops",
            settings.video.output_width, settings.video.output_height
        ));
    } else if let Err(error) = output.borrow_mut().set_output_scaling(
        settings.video.output_width,
        settings.video.output_height,
        settings.video.scale_filter,
    ) {
        notes.push(format!("output scaling: {error}"));
    }

    if let Err(error) = settings.save(&controller.path) {
        notes.push(format!("settings file: {error}"));
    }

    *controller.settings.borrow_mut() = settings.clone();
    controller
        .canvas
        .set_snap_distance(settings.canvas_snap_distance);
    apply_to_studio(ui, settings);
    push_palette(ui, controller, settings);
    // The previewed geometry becomes the committed geometry, so a later Cancel
    // restores what was applied rather than what the window opened with.
    controller.push_metrics(settings.metrics());
    window.set_preview_swatch(settings.tokens().preview_border);
    window.set_program_swatch(settings.tokens().program_border);
    window.set_dirty(false);
    refresh_ui(ui, state, surface);
    ui.set_canvas_width(i32::try_from(surface.borrow().format.width()).unwrap_or(1920));
    ui.set_canvas_height(i32::try_from(surface.borrow().format.height()).unwrap_or(1080));

    if !notes.is_empty() {
        ui.set_status_message(
            format!("Settings applied with warnings: {}", notes.join("; ")).into(),
        );
    }
}

/// Returns the canvas format the Video page asks the renderer for.
fn video_format_from(video: VideoSettings) -> Option<VideoFormat> {
    let rate = FrameRate::new(video.fps_numerator, video.fps_denominator).ok()?;
    VideoFormat::new(video.base_width, video.base_height, rate).ok()
}

/// Pushes the values the studio window reads directly.
fn apply_to_studio(ui: &MainWindow, settings: &AppSettings) {
    ui.set_hotkey_undo(settings.hotkey_undo.as_str().into());
    ui.set_hotkey_redo(settings.hotkey_redo.as_str().into());
    ui.set_hotkey_save_project(settings.hotkey_save_project.as_str().into());
    ui.set_hotkey_fade_transition(settings.hotkey_fade_transition.as_str().into());
    ui.set_hotkey_save_replay(settings.hotkey_save_replay.as_str().into());
    ui.set_confirm_start_stream(settings.confirm_start_stream);
    ui.set_confirm_stop_stream(settings.confirm_stop_stream);
    ui.set_confirm_stop_recording(settings.confirm_stop_recording);
    ui.set_auto_record_when_streaming(settings.auto_record_when_streaming);
    ui.set_show_safe_areas(settings.show_safe_areas);
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
