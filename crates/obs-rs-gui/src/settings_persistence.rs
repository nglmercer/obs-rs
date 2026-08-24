use super::{
    audio_monitor_mode_from_id, audio_monitor_mode_id, bounded_text, colour_text, flag, hotkey,
    number, optional_text, parse_project_scene_selections, rtmp_from_config,
    serialize_project_scene_selections, srt_from_config, text, user_directory, AppSettings, Config,
    EncoderImplementation, FpsMode, HlsConfig, LayoutSettings, OutputMode, RecordingFormat,
    RecordingQuality, RistConfig, ScaleFilter, SecretString, SettingsLoad, SetupState,
    StreamProtocol, StreamTarget, UiDensity, UiLocale, UiStyle, VideoCodec, VideoSettings,
    WhipConfig, AUDIO_SYNC_OFFSET_RANGE, CANVAS_SNAP_DISTANCE_RANGE, CHANNEL_LAYOUTS,
    FONT_SIZE_RANGE, MAX_DIMENSION, PROJECTOR_MONITORS_KEY, PROJECTOR_TARGETS_KEY,
    PROJECT_SCENE_SELECTIONS_KEY, RECORDING_SPLIT_DURATION_MINUTES_RANGE,
    RECORDING_SPLIT_SEGMENTS_RANGE, RECORDING_SPLIT_SIZE_MIB_RANGE,
    REPLAY_BUFFER_CAPACITY_MIB_RANGE, REPLAY_BUFFER_DURATION_RANGE, SAMPLE_RATES, THEMES,
};

use std::path::{Path, PathBuf};

impl AppSettings {
    #[cfg(test)]
    pub(crate) fn stream_endpoint(&self) -> Option<String> {
        self.stream_target().endpoint()
    }

    pub(crate) fn stream_target(&self) -> StreamTarget {
        match self.stream_protocol {
            StreamProtocol::Rtmp => StreamTarget::Rtmp(self.rtmp.clone()),
            StreamProtocol::Rtmps => StreamTarget::Rtmps(self.rtmp.clone()),
            StreamProtocol::Srt => StreamTarget::Srt(self.srt.clone()),
            StreamProtocol::Whip => StreamTarget::Whip(WhipConfig {
                endpoint: self.whip_endpoint.clone(),
                bearer_token: self.whip_bearer_token.clone(),
            }),
            StreamProtocol::Hls => StreamTarget::Hls(self.hls.clone()),
            StreamProtocol::Rist => StreamTarget::Rist(self.rist.clone()),
            StreamProtocol::Reference => StreamTarget::Reference {
                address: self.reference_address.clone(),
            },
        }
    }

    /// Reads settings from `path`, falling back to defaults for anything the
    /// document does not contain or cannot express.
    #[allow(dead_code)]
    pub(crate) fn load(path: &Path) -> Self {
        Self::load_with_status(path).settings
    }

    /// Reads settings and determines whether the blocking setup wizard belongs
    /// on the first startup.
    pub(crate) fn load_with_status(path: &Path) -> SettingsLoad {
        let Ok(document) = std::fs::read_to_string(path) else {
            return SettingsLoad {
                settings: Self::default(),
                show_setup: !path.exists(),
            };
        };
        let Ok(config) = Config::parse(&document) else {
            // An existing but malformed document should not trap a user in the
            // wizard. The regular settings loader still falls back safely.
            let settings = Self {
                setup_state: SetupState::Completed,
                ..Self::default()
            };
            return SettingsLoad {
                settings,
                show_setup: false,
            };
        };
        let mut settings = Self::from_config(&config);
        // Files from before first-run setup existed are already a configured
        // installation. Only a newly created file or an explicit pending state
        // can open the wizard.
        if config.get("setup_state").is_none() {
            settings.setup_state = SetupState::Completed;
        }
        let show_setup = settings.setup_state == SetupState::Pending;
        SettingsLoad {
            settings,
            show_setup,
        }
    }

    /// Writes the settings document, creating the file when it is missing.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error when the document cannot be written.
    pub(crate) fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&temporary, self.to_config().serialize())?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    /// Reads every key the settings window owns.
    ///
    /// The document is a flat list of independent keys and this is its per-key
    /// fallback table, so splitting it would only scatter one mapping across
    /// several functions.
    #[allow(clippy::too_many_lines, reason = "one fallback arm per stored key")]
    pub(super) fn from_config(config: &Config) -> Self {
        let defaults = Self::default();
        let (hls, rist) = extended_stream_config(config, &defaults);
        Self {
            locale: config
                .get("locale")
                .filter(|value| UiLocale::from_code(value).is_some())
                .map_or_else(|| defaults.locale.clone(), str::to_owned),
            theme: config
                .get("theme")
                .and_then(|value| THEMES.iter().position(|theme| theme.key == value))
                .unwrap_or(defaults.theme),
            style: config
                .get("appearance_style")
                .and_then(UiStyle::from_id)
                .unwrap_or(defaults.style),
            font_size: config
                .get("appearance_font_size")
                .and_then(|value| value.parse::<u8>().ok())
                .filter(|size| FONT_SIZE_RANGE.contains(size))
                .unwrap_or(defaults.font_size),
            density: config
                .get("appearance_density")
                .and_then(UiDensity::from_id)
                .unwrap_or(defaults.density),
            confirm_start_stream: flag(
                config,
                "confirm_start_stream",
                defaults.confirm_start_stream,
            ),
            confirm_stop_stream: flag(config, "confirm_stop_stream", defaults.confirm_stop_stream),
            confirm_stop_recording: flag(
                config,
                "confirm_stop_recording",
                defaults.confirm_stop_recording,
            ),
            auto_record_when_streaming: flag(
                config,
                "auto_record_when_streaming",
                defaults.auto_record_when_streaming,
            ),
            canvas_snap_distance: config
                .get("canvas_snap_distance")
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|distance| CANVAS_SNAP_DISTANCE_RANGE.contains(distance))
                .unwrap_or(defaults.canvas_snap_distance),
            show_safe_areas: flag(config, "show_safe_areas", defaults.show_safe_areas),
            sample_rate: config
                .get("audio_sample_rate")
                .and_then(|value| value.parse::<u32>().ok())
                .and_then(|rate| SAMPLE_RATES.iter().position(|value| *value == rate))
                .unwrap_or(defaults.sample_rate),
            channels: config
                .get("audio_channels")
                .and_then(|value| value.parse::<u16>().ok())
                .and_then(|count| {
                    CHANNEL_LAYOUTS
                        .iter()
                        .position(|layout| layout.channels() == count)
                })
                .unwrap_or(defaults.channels),
            audio_input_sync_offset_millis: config
                .get("audio_input_sync_offset_millis")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|offset| AUDIO_SYNC_OFFSET_RANGE.contains(offset))
                .unwrap_or(defaults.audio_input_sync_offset_millis),
            desktop_audio_sync_offset_millis: config
                .get("desktop_audio_sync_offset_millis")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|offset| AUDIO_SYNC_OFFSET_RANGE.contains(offset))
                .unwrap_or(defaults.desktop_audio_sync_offset_millis),
            hotkey_swap: hotkey(config, "hotkey_swap", &defaults.hotkey_swap),
            hotkey_start_recording: hotkey(
                config,
                "hotkey_start_recording",
                &defaults.hotkey_start_recording,
            ),
            hotkey_stop_recording: hotkey(
                config,
                "hotkey_stop_recording",
                &defaults.hotkey_stop_recording,
            ),
            hotkey_start_streaming: hotkey(
                config,
                "hotkey_start_streaming",
                &defaults.hotkey_start_streaming,
            ),
            hotkey_stop_streaming: hotkey(
                config,
                "hotkey_stop_streaming",
                &defaults.hotkey_stop_streaming,
            ),
            hotkey_undo: hotkey(config, "hotkey_undo", &defaults.hotkey_undo),
            hotkey_redo: hotkey(config, "hotkey_redo", &defaults.hotkey_redo),
            hotkey_save_project: hotkey(
                config,
                "hotkey_save_project",
                &defaults.hotkey_save_project,
            ),
            hotkey_fade_transition: hotkey(
                config,
                "hotkey_fade_transition",
                &defaults.hotkey_fade_transition,
            ),
            hotkey_save_replay: hotkey(config, "hotkey_save_replay", &defaults.hotkey_save_replay),
            hotkey_start_replay: hotkey(
                config,
                "hotkey_start_replay",
                &defaults.hotkey_start_replay,
            ),
            hotkey_stop_replay: hotkey(config, "hotkey_stop_replay", &defaults.hotkey_stop_replay),
            hotkey_toggle_microphone_mute: hotkey(
                config,
                "hotkey_toggle_microphone_mute",
                &defaults.hotkey_toggle_microphone_mute,
            ),
            hotkey_toggle_desktop_mute: hotkey(
                config,
                "hotkey_toggle_desktop_mute",
                &defaults.hotkey_toggle_desktop_mute,
            ),
            preview_border_color: colour_text(
                config,
                "preview_border_color",
                &defaults.preview_border_color,
            ),
            program_border_color: colour_text(
                config,
                "program_border_color",
                &defaults.program_border_color,
            ),
            project_path: text(config, "project_path", &defaults.project_path),
            diagnostics_path: text(config, "diagnostics_path", &defaults.diagnostics_path),
            recording_path: text(config, "recording_path", &defaults.recording_path),
            recording_directory: text(config, "recording_directory", &defaults.recording_directory),
            recording_filename_without_spaces: flag(
                config,
                "recording_filename_without_spaces",
                defaults.recording_filename_without_spaces,
            ),
            recording_quality: config
                .get("recording_quality")
                .and_then(RecordingQuality::from_id)
                .unwrap_or(defaults.recording_quality),
            recording_auto_remux: flag(
                config,
                "recording_auto_remux",
                defaults.recording_auto_remux,
            ),
            recording_audio_encoder: EncoderImplementation::new(text(
                config,
                "recording_audio_encoder",
                defaults.recording_audio_encoder.id(),
            )),
            replay_buffer_duration_seconds: config
                .get("replay_buffer_duration_seconds")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|duration| REPLAY_BUFFER_DURATION_RANGE.contains(duration))
                .unwrap_or(defaults.replay_buffer_duration_seconds),
            replay_buffer_capacity_mib: config
                .get("replay_buffer_capacity_mib")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|capacity| REPLAY_BUFFER_CAPACITY_MIB_RANGE.contains(capacity))
                .unwrap_or(defaults.replay_buffer_capacity_mib),
            recording_split_enabled: flag(
                config,
                "recording_split_enabled",
                defaults.recording_split_enabled,
            ),
            recording_split_duration_minutes: config
                .get("recording_split_duration_minutes")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|duration| RECORDING_SPLIT_DURATION_MINUTES_RANGE.contains(duration))
                .unwrap_or(defaults.recording_split_duration_minutes),
            recording_split_size_mib: config
                .get("recording_split_size_mib")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|capacity| RECORDING_SPLIT_SIZE_MIB_RANGE.contains(capacity))
                .unwrap_or(defaults.recording_split_size_mib),
            recording_split_max_segments: config
                .get("recording_split_max_segments")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|segments| RECORDING_SPLIT_SEGMENTS_RANGE.contains(segments))
                .unwrap_or(defaults.recording_split_max_segments),
            output_mode: config
                .get("output_mode")
                .and_then(OutputMode::from_id)
                .unwrap_or(defaults.output_mode),
            stream_custom_encoder: flag(
                config,
                "stream_custom_encoder",
                defaults.stream_custom_encoder,
            ),
            video: video_from_config(config, defaults.video),
            recording_format: config
                .get("recording_format")
                .and_then(RecordingFormat::from_id)
                .unwrap_or(defaults.recording_format),
            recording_codec: config
                .get("recording_codec")
                .and_then(VideoCodec::from_id)
                .unwrap_or(defaults.recording_codec),
            stream_protocol: config
                .get("stream_protocol")
                .and_then(StreamProtocol::from_id)
                .unwrap_or(defaults.stream_protocol),
            rtmp: rtmp_from_config(config, &defaults.rtmp),
            srt: srt_from_config(config, &defaults.srt),
            whip_endpoint: text(config, "whip_endpoint", &defaults.whip_endpoint),
            whip_bearer_token: optional_text(config, "whip_bearer_token").map(SecretString::new),
            hls,
            rist,
            reference_address: text(config, "reference_address", &defaults.reference_address),
            audio_input_id: text(config, "audio_input_id", &defaults.audio_input_id),
            audio_monitor_output_id: text(
                config,
                "audio_monitor_output_id",
                &defaults.audio_monitor_output_id,
            ),
            microphone_monitor_mode: config
                .get("microphone_monitor_mode")
                .and_then(audio_monitor_mode_from_id)
                .unwrap_or(defaults.microphone_monitor_mode),
            desktop_audio_monitor_mode: config
                .get("desktop_audio_monitor_mode")
                .and_then(audio_monitor_mode_from_id)
                .unwrap_or(defaults.desktop_audio_monitor_mode),
            last_preview_scene: text(config, "last_preview_scene", &defaults.last_preview_scene),
            last_program_scene: text(config, "last_program_scene", &defaults.last_program_scene),
            project_scene_selections: config
                .get(PROJECT_SCENE_SELECTIONS_KEY)
                .map(parse_project_scene_selections)
                .unwrap_or_default(),
            restore_project: flag(config, "restore_project", defaults.restore_project),
            save_project_on_exit: flag(
                config,
                "save_project_on_exit",
                defaults.save_project_on_exit,
            ),
            setup_state: config
                .get("setup_state")
                .and_then(SetupState::from_id)
                .unwrap_or(defaults.setup_state),
            setup_benchmark_summary: bounded_text(
                config,
                "setup_benchmark_summary",
                &defaults.setup_benchmark_summary,
                4_096,
            ),
            layout: LayoutSettings::from_config(config),
        }
    }

    /// Writes every key the settings window owns.
    ///
    /// The inverse of [`AppSettings::from_config`], and a flat list for the
    /// same reason.
    #[allow(clippy::too_many_lines, reason = "one entry per stored key")]
    pub(super) fn to_config(&self) -> Config {
        let mut config = Config::new();
        let entries = [
            ("locale", self.locale.clone()),
            (
                "theme",
                THEMES[self.theme.min(THEMES.len() - 1)].key.to_owned(),
            ),
            (
                "confirm_start_stream",
                self.confirm_start_stream.to_string(),
            ),
            ("confirm_stop_stream", self.confirm_stop_stream.to_string()),
            (
                "confirm_stop_recording",
                self.confirm_stop_recording.to_string(),
            ),
            (
                "auto_record_when_streaming",
                self.auto_record_when_streaming.to_string(),
            ),
            (
                "canvas_snap_distance",
                self.canvas_snap_distance.to_string(),
            ),
            ("show_safe_areas", self.show_safe_areas.to_string()),
            ("audio_sample_rate", self.sample_rate_hz().to_string()),
            ("audio_channels", self.channel_count().to_string()),
            (
                "audio_input_sync_offset_millis",
                self.audio_input_sync_offset_millis.to_string(),
            ),
            (
                "desktop_audio_sync_offset_millis",
                self.desktop_audio_sync_offset_millis.to_string(),
            ),
            ("hotkey_swap", self.hotkey_swap.clone()),
            (
                "hotkey_start_recording",
                self.hotkey_start_recording.clone(),
            ),
            ("hotkey_stop_recording", self.hotkey_stop_recording.clone()),
            (
                "hotkey_start_streaming",
                self.hotkey_start_streaming.clone(),
            ),
            ("hotkey_stop_streaming", self.hotkey_stop_streaming.clone()),
            ("hotkey_undo", self.hotkey_undo.clone()),
            ("hotkey_redo", self.hotkey_redo.clone()),
            ("hotkey_save_project", self.hotkey_save_project.clone()),
            (
                "hotkey_fade_transition",
                self.hotkey_fade_transition.clone(),
            ),
            ("hotkey_save_replay", self.hotkey_save_replay.clone()),
            ("hotkey_start_replay", self.hotkey_start_replay.clone()),
            ("hotkey_stop_replay", self.hotkey_stop_replay.clone()),
            (
                "hotkey_toggle_microphone_mute",
                self.hotkey_toggle_microphone_mute.clone(),
            ),
            (
                "hotkey_toggle_desktop_mute",
                self.hotkey_toggle_desktop_mute.clone(),
            ),
            ("preview_border_color", self.preview_border_color.clone()),
            ("program_border_color", self.program_border_color.clone()),
            ("project_path", self.project_path.clone()),
            ("diagnostics_path", self.diagnostics_path.clone()),
            ("recording_path", self.recording_path.clone()),
            ("recording_directory", self.recording_directory.clone()),
            (
                "recording_filename_without_spaces",
                self.recording_filename_without_spaces.to_string(),
            ),
            ("recording_quality", self.recording_quality.id().to_owned()),
            (
                "recording_auto_remux",
                self.recording_auto_remux.to_string(),
            ),
            (
                "recording_audio_encoder",
                self.recording_audio_encoder.id().to_owned(),
            ),
            (
                "replay_buffer_duration_seconds",
                self.replay_buffer_duration_seconds.to_string(),
            ),
            (
                "replay_buffer_capacity_mib",
                self.replay_buffer_capacity_mib.to_string(),
            ),
            (
                "recording_split_enabled",
                self.recording_split_enabled.to_string(),
            ),
            (
                "recording_split_duration_minutes",
                self.recording_split_duration_minutes.to_string(),
            ),
            (
                "recording_split_size_mib",
                self.recording_split_size_mib.to_string(),
            ),
            (
                "recording_split_max_segments",
                self.recording_split_max_segments.to_string(),
            ),
            ("output_mode", self.output_mode.id().to_owned()),
            (
                "stream_custom_encoder",
                self.stream_custom_encoder.to_string(),
            ),
            ("appearance_style", self.style.id().to_owned()),
            ("appearance_font_size", self.font_size.to_string()),
            ("appearance_density", self.density.id().to_owned()),
            ("video_base_width", self.video.base_width.to_string()),
            ("video_base_height", self.video.base_height.to_string()),
            ("video_output_width", self.video.output_width.to_string()),
            ("video_output_height", self.video.output_height.to_string()),
            (
                "video_scale_filter",
                self.video.scale_filter.id().to_owned(),
            ),
            ("video_fps_mode", self.video.fps_mode.id().to_owned()),
            ("video_fps_numerator", self.video.fps_numerator.to_string()),
            (
                "video_fps_denominator",
                self.video.fps_denominator.to_string(),
            ),
            ("recording_format", self.recording_format.id().to_owned()),
            ("recording_codec", self.recording_codec.id().to_owned()),
            ("audio_input_id", self.audio_input_id.clone()),
            (
                "audio_monitor_output_id",
                self.audio_monitor_output_id.clone(),
            ),
            (
                "microphone_monitor_mode",
                audio_monitor_mode_id(self.microphone_monitor_mode).to_owned(),
            ),
            (
                "desktop_audio_monitor_mode",
                audio_monitor_mode_id(self.desktop_audio_monitor_mode).to_owned(),
            ),
            ("last_preview_scene", self.last_preview_scene.clone()),
            ("last_program_scene", self.last_program_scene.clone()),
            (
                PROJECT_SCENE_SELECTIONS_KEY,
                serialize_project_scene_selections(
                    &self.project_scene_selections,
                    Some(&self.project_path),
                ),
            ),
            ("restore_project", self.restore_project.to_string()),
            (
                "save_project_on_exit",
                self.save_project_on_exit.to_string(),
            ),
            ("setup_state", self.setup_state.id().to_owned()),
            (
                "setup_benchmark_summary",
                self.setup_benchmark_summary.chars().take(4_096).collect(),
            ),
            ("layout_panel_order", self.layout.panel_order_text()),
            ("layout_show_scenes", self.layout.show_scenes.to_string()),
            ("layout_show_sources", self.layout.show_sources.to_string()),
            ("layout_show_mixer", self.layout.show_mixer.to_string()),
            (
                "layout_show_transitions",
                self.layout.show_transitions.to_string(),
            ),
            (
                "layout_show_controls",
                self.layout.show_controls.to_string(),
            ),
            ("layout_view_mode", self.layout.view_mode.to_string()),
            ("layout_dock_height", self.layout.dock_height.to_string()),
            ("layout_panel_weights", self.layout.panel_weights_text()),
            ("layout_floating_panels", self.layout.floating_text()),
            (
                "layout_floating_geometry",
                self.layout.floating_geometry_text(),
            ),
            (
                "layout_projector_geometry",
                self.layout.projector_geometry_text(),
            ),
            (PROJECTOR_TARGETS_KEY, self.layout.projector_targets_text()),
            (
                PROJECTOR_MONITORS_KEY,
                self.layout.projector_monitors_text(),
            ),
            (
                "layout_dock_tree",
                self.layout
                    .dock_tree
                    .encode()
                    .unwrap_or_else(|| LayoutSettings::default().dock_tree.encode().unwrap()),
            ),
        ];
        for (key, value) in entries {
            // Every key here is a literal identifier and every value is bounded
            // UI text, so a rejection can only mean a programming error.
            debug_assert!(config.set(key, &value).is_ok(), "settings key {key}");
            let _ = config.set(key, &value);
        }
        self.write_stream_config(&mut config);
        config
    }

    fn write_stream_config(&self, config: &mut Config) {
        for (key, value) in self.stream_config_entries() {
            debug_assert!(config.set(key, &value).is_ok(), "settings key {key}");
            let _ = config.set(key, &value);
        }
    }

    fn stream_config_entries(&self) -> Vec<(&'static str, String)> {
        let mut entries = vec![
            ("stream_protocol", self.stream_protocol.id().to_owned()),
            ("rtmp_service", self.rtmp.service.clone()),
            ("rtmp_server", self.rtmp.server.clone()),
            (
                "rtmp_stream_key",
                self.rtmp.stream_key.expose_secret().to_owned(),
            ),
            (
                "stream_video_encoder",
                self.rtmp.video.implementation.id().to_owned(),
            ),
            (
                "stream_audio_encoder",
                self.rtmp.audio.implementation.id().to_owned(),
            ),
            (
                "stream_video_bitrate_kbps",
                self.rtmp.video.bitrate_kbps.to_string(),
            ),
            (
                "stream_audio_bitrate_kbps",
                self.rtmp.audio.bitrate_kbps.to_string(),
            ),
            (
                "stream_audio_sample_rate",
                self.rtmp.audio.sample_rate.to_string(),
            ),
            (
                "stream_audio_channels",
                self.rtmp.audio.channels.to_string(),
            ),
            (
                "stream_audio_complexity",
                self.rtmp
                    .audio
                    .complexity
                    .map_or_else(String::new, |value| value.to_string()),
            ),
            (
                "stream_rate_control",
                self.rtmp.video.rate_control.id().to_owned(),
            ),
            (
                "stream_keyframe_interval_secs",
                self.rtmp.video.keyframe_interval_secs.to_string(),
            ),
            ("stream_preset", self.rtmp.video.preset.id().to_owned()),
            (
                "stream_profile",
                self.rtmp.video.profile.clone().unwrap_or_default(),
            ),
            ("stream_b_frames", self.rtmp.video.b_frames.to_string()),
            ("stream_reconnect", self.rtmp.reconnect.to_string()),
            (
                "stream_maximum_retries",
                self.rtmp.maximum_retries.to_string(),
            ),
            (
                "stream_network_buffer_ms",
                self.rtmp.network_buffer_ms.to_string(),
            ),
            ("srt_host", self.srt.host.clone()),
            ("srt_port", self.srt.port.to_string()),
            ("srt_mode", self.srt.mode.id().to_owned()),
            ("srt_latency_ms", self.srt.latency_ms.to_string()),
            (
                "srt_passphrase",
                self.srt
                    .passphrase
                    .as_ref()
                    .map_or_else(String::new, |value| value.expose_secret().to_owned()),
            ),
            (
                "srt_pbkeylen",
                self.srt
                    .pbkeylen
                    .map_or_else(String::new, |value| value.bytes().to_string()),
            ),
            (
                "srt_stream_id",
                self.srt.stream_id.clone().unwrap_or_default(),
            ),
            (
                "srt_connect_timeout_ms",
                self.srt.connect_timeout_ms.to_string(),
            ),
            ("whip_endpoint", self.whip_endpoint.clone()),
            (
                "whip_bearer_token",
                self.whip_bearer_token
                    .as_ref()
                    .map_or_else(String::new, |value| value.expose_secret().to_owned()),
            ),
            ("reference_address", self.reference_address.clone()),
        ];
        entries.extend(extended_stream_entries(self));
        entries
    }

    /// Restores the stored dock layout into a freshly built window.
    pub(crate) fn apply_layout(&self, ui: &crate::MainWindow) {
        self.layout.apply(ui);
    }
}

/// Reads the Video page's keys, falling back per key.
///
/// A document written before the canvas and the output were separate values
/// contains neither key, so both fall back to the shipped defaults rather than
/// leaving the page blank. A stored resolution outside the renderer's budget
/// is treated the same way as an unparsable one.
fn video_from_config(config: &Config, defaults: VideoSettings) -> VideoSettings {
    let dimension = |key: &str, fallback: u32| {
        config
            .get(key)
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|value| (1..=MAX_DIMENSION).contains(value))
            .unwrap_or(fallback)
    };
    let video = VideoSettings {
        base_width: dimension("video_base_width", defaults.base_width),
        base_height: dimension("video_base_height", defaults.base_height),
        output_width: dimension("video_output_width", defaults.output_width),
        output_height: dimension("video_output_height", defaults.output_height),
        scale_filter: config
            .get("video_scale_filter")
            .and_then(ScaleFilter::from_id)
            .unwrap_or(defaults.scale_filter),
        fps_mode: config
            .get("video_fps_mode")
            .and_then(FpsMode::from_id)
            .unwrap_or(defaults.fps_mode),
        fps_numerator: number(config, "video_fps_numerator", defaults.fps_numerator),
        fps_denominator: number(config, "video_fps_denominator", defaults.fps_denominator),
    };
    // A pair that cannot become a format would break the renderer on the next
    // sync, so the whole video block falls back rather than half of it.
    if video.base_format().is_err() || video.output_format().is_err() {
        return defaults;
    }
    video
}

/// Returns the directory new recordings are written into by default.
///
/// `XDG_VIDEOS_DIR` is not read from `user-dirs.dirs` here — that file is a
/// shell fragment, not a config document — so the home directory's `Videos`
/// folder is used when it already exists and the per-user directory otherwise.
pub(super) fn default_recording_directory() -> String {
    let videos = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Videos"))
        .filter(|path| path.is_dir());
    videos.or_else(user_directory).map_or_else(
        || ".".to_owned(),
        |path| path.to_string_lossy().into_owned(),
    )
}

fn extended_stream_config(config: &Config, defaults: &AppSettings) -> (HlsConfig, RistConfig) {
    let hls = HlsConfig {
        directory: PathBuf::from(text(
            config,
            "hls_directory",
            defaults.hls.directory.to_string_lossy().as_ref(),
        )),
        segment_duration_secs: number(
            config,
            "hls_segment_duration_secs",
            defaults.hls.segment_duration_secs,
        ),
        playlist_size: number(config, "hls_playlist_size", defaults.hls.playlist_size),
        low_latency: flag(config, "hls_low_latency", defaults.hls.low_latency),
    };
    let rist = RistConfig {
        host: text(config, "rist_host", &defaults.rist.host),
        port: number(config, "rist_port", defaults.rist.port),
        sender_buffer_ms: number(
            config,
            "rist_sender_buffer_ms",
            defaults.rist.sender_buffer_ms,
        ),
        shared_secret: optional_text(config, "rist_shared_secret").map(SecretString::new),
    };
    (hls, rist)
}

fn extended_stream_entries(settings: &AppSettings) -> Vec<(&'static str, String)> {
    vec![
        (
            "hls_directory",
            settings.hls.directory.to_string_lossy().into_owned(),
        ),
        (
            "hls_segment_duration_secs",
            settings.hls.segment_duration_secs.to_string(),
        ),
        ("hls_playlist_size", settings.hls.playlist_size.to_string()),
        ("hls_low_latency", settings.hls.low_latency.to_string()),
        ("rist_host", settings.rist.host.clone()),
        ("rist_port", settings.rist.port.to_string()),
        (
            "rist_sender_buffer_ms",
            settings.rist.sender_buffer_ms.to_string(),
        ),
        (
            "rist_shared_secret",
            settings
                .rist
                .shared_secret
                .as_ref()
                .map_or_else(String::new, |value| value.expose_secret().to_owned()),
        ),
    ]
}
