#[allow(
    clippy::wildcard_imports,
    reason = "settings submodules share the validated settings namespace"
)]
use super::*;

/// Whether the first-run setup should be shown at startup.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SetupState {
    /// The user has not completed or explicitly skipped setup.
    #[default]
    Pending,
    /// Setup was applied successfully.
    Completed,
    /// The user chose to continue without setup.
    Skipped,
}

impl SetupState {
    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Skipped => "skipped",
        }
    }

    pub(super) fn from_id(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "completed" => Some(Self::Completed),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

/// Result of loading settings with the startup-only first-run signal.
pub(crate) struct SettingsLoad {
    pub(crate) settings: AppSettings,
    pub(crate) show_setup: bool,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RecordingFormat {
    #[default]
    Matroska,
    Mp4,
    FragmentedMp4,
    Mov,
    Flv,
    ReferencePacket,
}

impl RecordingFormat {
    pub(crate) const ALL: [Self; 6] = [
        Self::Matroska,
        Self::Mp4,
        Self::FragmentedMp4,
        Self::Mov,
        Self::Flv,
        Self::ReferencePacket,
    ];

    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::Matroska => "matroska",
            Self::Mp4 => "mp4",
            Self::FragmentedMp4 => "fragmented-mp4",
            Self::Mov => "mov",
            Self::Flv => "flv",
            Self::ReferencePacket => "obsr-packet",
        }
    }

    pub(super) fn from_id(value: &str) -> Option<Self> {
        match value {
            "matroska" | "mkv" => Some(Self::Matroska),
            "mp4" => Some(Self::Mp4),
            "fragmented-mp4" => Some(Self::FragmentedMp4),
            "mov" => Some(Self::Mov),
            "flv" => Some(Self::Flv),
            "obsr-packet" | "obsr" => Some(Self::ReferencePacket),
            _ => None,
        }
    }

    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Matroska => "mkv",
            Self::Mp4 | Self::FragmentedMp4 => "mp4",
            Self::Mov => "mov",
            Self::Flv => "flv",
            Self::ReferencePacket => "obsr",
        }
    }

    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Matroska => "Matroska (.mkv)",
            Self::Mp4 => "MPEG-4 (.mp4)",
            Self::FragmentedMp4 => "Fragmented MPEG-4 (.mp4)",
            Self::Mov => "QuickTime Movie (.mov)",
            Self::Flv => "Flash Video (.flv)",
            Self::ReferencePacket => "OBS-RS Packet (.obsr)",
        }
    }
}

/// Returns the per-user directory OBS-RS keeps its documents in.
///
/// Storing them beside the working directory meant a session launched from
/// another directory looked like it had lost every setting, so the default is
/// the XDG config directory. A file that already exists in the working
/// directory still wins, which keeps existing installs working unchanged.
/// Windows uses `%APPDATA%` and falls back to `%LOCALAPPDATA%`.
pub(crate) fn user_directory() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())?;
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    let directory = base.join("obs-rs");
    std::fs::create_dir_all(&directory).ok()?;
    Some(directory)
}

/// Returns the path the settings document lives at.
pub(crate) fn settings_path() -> PathBuf {
    PathBuf::from(user_file(SETTINGS_FILE))
}

/// Resolves `name` to the legacy working-directory file when one exists, and
/// to the per-user directory otherwise.
pub(crate) fn user_file(name: &str) -> String {
    if Path::new(name).exists() {
        return name.to_owned();
    }
    user_directory().map_or_else(
        || name.to_owned(),
        |directory| directory.join(name).to_string_lossy().into_owned(),
    )
}

/// Canvas sizes offered on the Video page, in OBS's descending order.
pub(crate) const RESOLUTIONS: [(u32, u32); 6] = [
    (1920, 1080),
    (1600, 900),
    (1280, 720),
    (1024, 576),
    (854, 480),
    (640, 360),
];

/// Frame rates offered on the Video page as `(numerator, denominator)`.
pub(crate) const FRAME_RATES: [(u32, u32); 6] = [
    (60, 1),
    (60_000, 1_001),
    (50, 1),
    (30, 1),
    (30_000, 1_001),
    (24, 1),
];

/// Sample rates offered on the Audio page.
pub(crate) const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];

/// Channel layouts offered on the Audio page.
pub(crate) const CHANNEL_LAYOUTS: [AudioChannelLayout; 6] = [
    AudioChannelLayout::Stereo,
    AudioChannelLayout::Mono,
    AudioChannelLayout::TwoPointOne,
    AudioChannelLayout::Quad,
    AudioChannelLayout::FivePointOne,
    AudioChannelLayout::SevenPointOne,
];

/// Per-channel positive audio sync offsets accepted by the Audio page.
pub(crate) const AUDIO_SYNC_OFFSET_RANGE: std::ops::RangeInclusive<u32> =
    0..=MAX_AUDIO_SYNC_OFFSET_MILLISECONDS;

/// Default per-channel audio sync offset.
pub(crate) const AUDIO_SYNC_OFFSET_DEFAULT: u32 = 0;

/// Monitor routing choices exposed by the Audio settings page.
pub(crate) const AUDIO_MONITOR_MODES: [AudioMonitorMode; 3] = [
    AudioMonitorMode::Off,
    AudioMonitorMode::MonitorOnly,
    AudioMonitorMode::MonitorAndOutput,
];

/// Stable settings identifier for one monitor-routing choice.
pub(crate) const fn audio_monitor_mode_id(mode: AudioMonitorMode) -> &'static str {
    match mode {
        AudioMonitorMode::Off => "off",
        AudioMonitorMode::MonitorOnly => "monitor_only",
        AudioMonitorMode::MonitorAndOutput => "monitor_and_output",
    }
}

/// Parses a persisted monitor-routing choice, falling back per field when it
/// is unknown or from a future version.
pub(crate) fn audio_monitor_mode_from_id(value: &str) -> Option<AudioMonitorMode> {
    match value {
        "off" => Some(AudioMonitorMode::Off),
        "monitor_only" => Some(AudioMonitorMode::MonitorOnly),
        "monitor_and_output" => Some(AudioMonitorMode::MonitorAndOutput),
        _ => None,
    }
}

/// The smallest and largest canvas-space distance accepted by source snapping.
///
/// OBS stores this as a pixel distance in its `BasicWindow` configuration. The
/// Rust settings document keeps the same user-facing unit while bounding the
/// value before it reaches the interactive geometry path.
pub(crate) const CANVAS_SNAP_DISTANCE_RANGE: std::ops::RangeInclusive<u16> = 1..=100;

/// The default source-snapping distance used by the canvas and settings page.
pub(crate) const CANVAS_SNAP_DISTANCE_DEFAULT: u16 = 10;

/// The replay history duration accepted by the Output page, in seconds.
pub(crate) const REPLAY_BUFFER_DURATION_RANGE: std::ops::RangeInclusive<u32> = 1..=3_600;

/// The default replay history duration shown by the Output page.
pub(crate) const REPLAY_BUFFER_DURATION_DEFAULT: u32 = 20;

/// The replay history byte budget accepted by the Output page, in MiB.
pub(crate) const REPLAY_BUFFER_CAPACITY_MIB_RANGE: std::ops::RangeInclusive<u32> = 1..=256;

/// The default replay history byte budget shown by the Output page, in MiB.
pub(crate) const REPLAY_BUFFER_CAPACITY_MIB_DEFAULT: u32 = 64;

/// The split-recording target duration accepted by the Output page, in minutes.
pub(crate) const RECORDING_SPLIT_DURATION_MINUTES_RANGE: std::ops::RangeInclusive<u32> = 1..=1_440;

/// The default split-recording target duration shown by the Output page.
pub(crate) const RECORDING_SPLIT_DURATION_MINUTES_DEFAULT: u32 = 60;

/// The split-recording target size accepted by the Output page, in MiB.
pub(crate) const RECORDING_SPLIT_SIZE_MIB_RANGE: std::ops::RangeInclusive<u32> = 1..=256;

/// The default split-recording target size shown by the Output page, in MiB.
pub(crate) const RECORDING_SPLIT_SIZE_MIB_DEFAULT: u32 = 64;

/// The maximum split-recording segment count accepted by the Output page.
pub(crate) const RECORDING_SPLIT_SEGMENTS_RANGE: std::ops::RangeInclusive<u32> = 1..=1_024;

/// The default maximum split-recording segment count shown by the Output page.
pub(crate) const RECORDING_SPLIT_SEGMENTS_DEFAULT: u32 = 64;

/// Everything the settings window persists between sessions.
///
/// The independent booleans mirror the independent checkboxes on the General
/// page; grouping them into a flags type would only add a translation layer.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AppSettings {
    pub(crate) locale: String,
    pub(crate) theme: usize,
    pub(crate) style: UiStyle,
    pub(crate) font_size: u8,
    pub(crate) density: UiDensity,
    pub(crate) confirm_start_stream: bool,
    pub(crate) confirm_stop_stream: bool,
    pub(crate) confirm_stop_recording: bool,
    pub(crate) auto_record_when_streaming: bool,
    /// Canvas pixels within which a moving or resizing item snaps to a guide.
    pub(crate) canvas_snap_distance: u16,
    /// Draw the OBS-style action, graphics, and 4:3 safe-area guides in preview.
    pub(crate) show_safe_areas: bool,
    pub(crate) sample_rate: usize,
    pub(crate) channels: usize,
    pub(crate) hotkey_swap: String,
    pub(crate) hotkey_previous_scene: String,
    pub(crate) hotkey_next_scene: String,
    pub(crate) hotkey_start_recording: String,
    pub(crate) hotkey_stop_recording: String,
    pub(crate) hotkey_start_streaming: String,
    pub(crate) hotkey_stop_streaming: String,
    pub(crate) hotkey_undo: String,
    pub(crate) hotkey_redo: String,
    pub(crate) hotkey_save_project: String,
    pub(crate) hotkey_cut_transition: String,
    pub(crate) hotkey_fade_transition: String,
    pub(crate) hotkey_save_replay: String,
    pub(crate) hotkey_start_replay: String,
    pub(crate) hotkey_stop_replay: String,
    pub(crate) hotkey_toggle_microphone_mute: String,
    pub(crate) hotkey_toggle_desktop_mute: String,
    pub(crate) hotkey_push_to_talk_microphone: String,
    pub(crate) hotkey_push_to_mute_microphone: String,
    pub(crate) hotkey_toggle_studio_mode: String,
    pub(crate) hotkey_toggle_selected_source_visibility: String,
    pub(crate) hotkey_toggle_selected_source_lock: String,
    pub(crate) hotkey_toggle_selected_source_projector: String,
    pub(crate) hotkey_toggle_preview_scene_projector: String,
    pub(crate) preview_border_color: String,
    pub(crate) program_border_color: String,
    pub(crate) project_path: String,
    pub(crate) diagnostics_path: String,
    pub(crate) recording_path: String,
    /// Directory new recordings are written into, as the Output page shows it.
    pub(crate) recording_directory: String,
    /// Generate recording file names with `-` instead of spaces.
    pub(crate) recording_filename_without_spaces: bool,
    pub(crate) recording_quality: RecordingQuality,
    pub(crate) recording_format: RecordingFormat,
    /// Publish Matroska recordings as MP4 after the native bounded remux step.
    pub(crate) recording_auto_remux: bool,
    pub(crate) recording_codec: VideoCodec,
    pub(crate) recording_audio_encoder: EncoderImplementation,
    /// Maximum wall-clock history retained by the replay buffer, in seconds.
    pub(crate) replay_buffer_duration_seconds: u32,
    /// Maximum encoded replay history retained, in mebibytes.
    pub(crate) replay_buffer_capacity_mib: u32,
    /// Publish bounded numbered segments instead of one recording file when
    /// the effective format has a supported split-muxer boundary.
    pub(crate) recording_split_enabled: bool,
    /// Target wall-clock duration for one split segment, in minutes.
    pub(crate) recording_split_duration_minutes: u32,
    /// Target encoded size for one split segment, in mebibytes.
    pub(crate) recording_split_size_mib: u32,
    /// Maximum number of numbered split segments kept in one recording.
    pub(crate) recording_split_max_segments: u32,
    pub(crate) output_mode: OutputMode,
    /// Show the detailed encoder controls inside Simple output mode.
    pub(crate) stream_custom_encoder: bool,
    pub(crate) video: VideoSettings,
    pub(crate) stream_protocol: StreamProtocol,
    pub(crate) rtmp: RtmpConfig,
    pub(crate) srt: SrtConfig,
    pub(crate) whip_endpoint: String,
    pub(crate) whip_bearer_token: Option<SecretString>,
    pub(crate) hls: HlsConfig,
    pub(crate) rist: RistConfig,
    pub(crate) reference_address: String,
    /// Provider-stable platform input ID; empty selects the provider-declared
    /// default input and keeps the deterministic fallback as a safe last
    /// resort.
    pub(crate) audio_input_id: String,
    /// Provider-stable render-device ID used for desktop/system loopback;
    /// empty selects the provider's default output route.
    pub(crate) desktop_audio_id: String,
    /// Provider-stable local monitor-output ID; empty disables local playback.
    pub(crate) audio_monitor_output_id: String,
    /// Monitor destination policy for the microphone channel.
    pub(crate) microphone_monitor_mode: AudioMonitorMode,
    /// Monitor destination policy for the desktop-audio channel.
    pub(crate) desktop_audio_monitor_mode: AudioMonitorMode,
    /// Positive, sample-quantized delay applied to the microphone channel.
    pub(crate) audio_input_sync_offset_millis: u32,
    /// Positive, sample-quantized delay applied to the desktop-audio channel.
    pub(crate) desktop_audio_sync_offset_millis: u32,
    /// Last Preview scene selected in the desktop session; empty means use the
    /// first scene after a project is restored.
    pub(crate) last_preview_scene: String,
    /// Last Program scene selected in the desktop session; empty means use the
    /// first scene after a project is restored.
    pub(crate) last_program_scene: String,
    /// Bounded per-document Preview/Program choices restored across sessions.
    pub(crate) project_scene_selections: Vec<ProjectSceneSelection>,
    /// Reopen the project file from [`AppSettings::project_path`] at startup.
    pub(crate) restore_project: bool,
    /// Write the project back to the same file when the window closes.
    pub(crate) save_project_on_exit: bool,
    /// Controls the blocking setup wizard shown for a new installation.
    pub(crate) setup_state: SetupState,
    /// Bounded summary of the last local setup benchmark for diagnostics.
    pub(crate) setup_benchmark_summary: String,
    /// The dock layout the window was last left in.
    pub(crate) layout: LayoutSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            locale: "en".to_owned(),
            theme: 0,
            style: UiStyle::default(),
            font_size: DEFAULT_FONT_SIZE,
            density: UiDensity::default(),
            confirm_start_stream: false,
            confirm_stop_stream: true,
            confirm_stop_recording: true,
            auto_record_when_streaming: false,
            canvas_snap_distance: CANVAS_SNAP_DISTANCE_DEFAULT,
            show_safe_areas: false,
            // 48 kHz stereo, matching the mixer the desktop state starts with.
            sample_rate: 1,
            channels: 0,
            hotkey_swap: "Space".to_owned(),
            hotkey_previous_scene: "F6".to_owned(),
            hotkey_next_scene: "F7".to_owned(),
            hotkey_start_recording: "Ctrl+R".to_owned(),
            hotkey_stop_recording: "Ctrl+Shift+R".to_owned(),
            hotkey_start_streaming: "Ctrl+B".to_owned(),
            hotkey_stop_streaming: "Ctrl+Shift+B".to_owned(),
            hotkey_undo: "Ctrl+Z".to_owned(),
            hotkey_redo: "Ctrl+Y".to_owned(),
            hotkey_save_project: "Ctrl+S".to_owned(),
            hotkey_cut_transition: String::new(),
            hotkey_fade_transition: "Ctrl+Shift+F".to_owned(),
            hotkey_save_replay: "F8".to_owned(),
            hotkey_start_replay: "Ctrl+Shift+F8".to_owned(),
            hotkey_stop_replay: "Ctrl+Alt+F8".to_owned(),
            hotkey_toggle_microphone_mute: String::new(),
            hotkey_toggle_desktop_mute: String::new(),
            hotkey_push_to_talk_microphone: String::new(),
            hotkey_push_to_mute_microphone: String::new(),
            // Keep this opt-in until the user chooses a key that fits their
            // existing desktop/window-manager bindings.
            hotkey_toggle_studio_mode: String::new(),
            hotkey_toggle_selected_source_visibility: String::new(),
            hotkey_toggle_selected_source_lock: String::new(),
            hotkey_toggle_selected_source_projector: String::new(),
            hotkey_toggle_preview_scene_projector: String::new(),
            preview_border_color: "#60A5FA".to_owned(),
            program_border_color: "#F87171".to_owned(),
            project_path: user_file(PROJECT_FILE),
            diagnostics_path: user_file(DIAGNOSTICS_FILE),
            recording_path: user_file("obs-rs-recording.mkv"),
            recording_directory: default_recording_directory(),
            recording_filename_without_spaces: false,
            recording_quality: RecordingQuality::default(),
            recording_format: RecordingFormat::Matroska,
            recording_auto_remux: false,
            recording_codec: VideoCodec::H264,
            recording_audio_encoder: EncoderImplementation::default(),
            replay_buffer_duration_seconds: REPLAY_BUFFER_DURATION_DEFAULT,
            replay_buffer_capacity_mib: REPLAY_BUFFER_CAPACITY_MIB_DEFAULT,
            recording_split_enabled: false,
            recording_split_duration_minutes: RECORDING_SPLIT_DURATION_MINUTES_DEFAULT,
            recording_split_size_mib: RECORDING_SPLIT_SIZE_MIB_DEFAULT,
            recording_split_max_segments: RECORDING_SPLIT_SEGMENTS_DEFAULT,
            output_mode: OutputMode::default(),
            stream_custom_encoder: false,
            video: VideoSettings::default(),
            stream_protocol: StreamProtocol::Rtmp,
            rtmp: RtmpConfig::default(),
            srt: SrtConfig::default(),
            whip_endpoint: "https://127.0.0.1/whip".to_owned(),
            whip_bearer_token: None,
            hls: HlsConfig::default(),
            rist: RistConfig::default(),
            reference_address: "127.0.0.1:9000".to_owned(),
            audio_input_id: String::new(),
            desktop_audio_id: String::new(),
            audio_monitor_output_id: String::new(),
            microphone_monitor_mode: AudioMonitorMode::Off,
            desktop_audio_monitor_mode: AudioMonitorMode::Off,
            audio_input_sync_offset_millis: AUDIO_SYNC_OFFSET_DEFAULT,
            desktop_audio_sync_offset_millis: AUDIO_SYNC_OFFSET_DEFAULT,
            last_preview_scene: String::new(),
            last_program_scene: String::new(),
            project_scene_selections: Vec::new(),
            restore_project: true,
            save_project_on_exit: true,
            setup_state: SetupState::Pending,
            setup_benchmark_summary: String::new(),
            layout: LayoutSettings::default(),
        }
    }
}

/// Restores the layout OBS-RS ships with, discarding the session's arrangement.
///
/// This is the Docks menu's reset: it writes the defaults straight onto the
/// window, and the ordinary shutdown capture then persists them, so a reset
/// survives the session without a second save path.
pub(crate) fn apply_default_layout(ui: &crate::MainWindow) {
    LayoutSettings::default().apply(ui);
}

impl LayoutSettings {
    pub(super) fn apply(&self, ui: &crate::MainWindow) {
        let layout = self;
        ui.set_panel_order(ModelRc::new(VecModel::from(layout.dock_tree.leaf_order())));
        ui.set_show_scenes(layout.show_scenes);
        ui.set_show_sources(layout.show_sources);
        ui.set_show_mixer(layout.show_mixer);
        ui.set_show_transitions(layout.show_transitions);
        ui.set_show_controls(layout.show_controls);
        ui.set_show_stats(layout.show_stats);
        ui.set_panel_weights(ModelRc::new(VecModel::from(layout.panel_weights.clone())));
        // Docks are restored to the row; reopening their windows is left to the
        // user, so a session never starts with windows they cannot see.
        let floating = (0..DEFAULT_PANEL_ORDER.len())
            .map(|kind| {
                i32::try_from(kind).is_ok_and(|kind| layout.floating_panels.contains(&kind))
            })
            .collect::<Vec<_>>();
        ui.set_panel_floating(ModelRc::new(VecModel::from(floating)));
        ui.set_view_mode(layout.view_mode);
        #[allow(
            clippy::cast_precision_loss,
            reason = "dock heights are bounded to 1200 logical pixels"
        )]
        ui.set_dock_height(layout.dock_height as f32);
    }
}

impl AppSettings {
    /// Downgrades persisted production selections when the current binary has
    /// no usable native output backend.
    ///
    /// The reference packet path is deliberately portable, so a package built
    /// without the optional GStreamer runtime can still record and stream. A
    /// stale `.mkv`/RTMP selection must not be left in the live studio: it
    /// would make the controls look configured while the first output action
    /// could only fail at the engine boundary.
    pub(crate) fn adapt_to_output_capabilities(&mut self, production_supported: bool) {
        if production_supported {
            return;
        }

        self.stream_protocol = StreamProtocol::Reference;
        self.recording_format = RecordingFormat::ReferencePacket;
        self.recording_auto_remux = false;
        if Path::new(&self.recording_path)
            .extension()
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("obsr"))
        {
            self.recording_path = PathBuf::from(&self.recording_path)
                .with_extension(RecordingFormat::ReferencePacket.extension())
                .to_string_lossy()
                .into_owned();
        }
    }

    /// Reads the window's current dock layout back into this document.
    pub(crate) fn capture_layout(&mut self, ui: &crate::MainWindow) {
        let order = read_model(&ui.get_panel_order());
        // A window that somehow lost a dock keeps the stored order rather than
        // persisting a layout that could never be restored.
        if LayoutSettings::parse_panel_order(
            &order
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .is_some()
        {
            self.layout.panel_order = order;
        }
        self.layout.show_scenes = ui.get_show_scenes();
        self.layout.show_sources = ui.get_show_sources();
        self.layout.show_mixer = ui.get_show_mixer();
        self.layout.show_transitions = ui.get_show_transitions();
        self.layout.show_controls = ui.get_show_controls();
        self.layout.show_stats = ui.get_show_stats();
        let weights = read_model(&ui.get_panel_weights());
        if weights.len() == DEFAULT_PANEL_WEIGHTS.len() {
            self.layout.panel_weights = weights;
        }
        self.layout.dock_tree =
            DockNode::from_legacy(&self.layout.panel_order, &self.layout.panel_weights)
                .unwrap_or_else(|| LayoutSettings::default().dock_tree);
        self.layout.floating_panels = read_model(&ui.get_panel_floating())
            .into_iter()
            .enumerate()
            .filter(|(_, floating)| *floating)
            .filter_map(|(kind, _)| i32::try_from(kind).ok())
            .collect();
        self.layout.view_mode = ui.get_view_mode().clamp(0, 2);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the height is clamped into the persisted range first"
        )]
        {
            self.layout.dock_height = ui.get_dock_height().clamp(120.0, 1_200.0) as u32;
        }
    }

    /// Returns the stored locale, falling back to English for an unknown code.
    pub(crate) fn ui_locale(&self) -> UiLocale {
        UiLocale::from_code(&self.locale).unwrap_or(UiLocale::English)
    }

    /// Returns the recording format the selected quality actually writes.
    ///
    /// The lossless preset is the reference RLE pipeline, which only the
    /// OBS-RS packet container carries, so choosing it changes the format
    /// rather than silently producing a lossy file.
    pub(crate) const fn effective_recording_format(&self) -> RecordingFormat {
        if self.recording_quality.is_lossless() {
            RecordingFormat::ReferencePacket
        } else {
            self.recording_format
        }
    }

    /// Returns whether the next generated path is the final MP4 published by
    /// the bounded native remux boundary.
    pub(crate) fn recording_uses_auto_remux(&self) -> bool {
        self.recording_auto_remux
            && !self.recording_split_enabled
            && self.effective_recording_format() == RecordingFormat::Matroska
    }

    /// Returns the video codec the selected quality actually encodes with.
    pub(crate) const fn effective_recording_codec(&self) -> VideoCodec {
        if self.recording_quality.is_lossless() {
            VideoCodec::ReferenceRle
        } else {
            self.recording_codec
        }
    }

    /// Returns the file the next recording is written to.
    ///
    /// OBS names recordings from the clock, so two recordings in one session
    /// never collide and the file says when it was made. The extension always
    /// comes from the effective format, so the container and the name agree.
    pub(crate) fn recording_file_path(&self, stamp: &str) -> String {
        let name = if self.recording_filename_without_spaces {
            stamp.replace(' ', "-")
        } else {
            stamp.to_owned()
        };
        let mut path = PathBuf::from(self.recording_directory.trim());
        let extension = if self.recording_uses_auto_remux() {
            "mp4"
        } else {
            self.effective_recording_format().extension()
        };
        path.push(format!("{name}.{extension}"));
        path.to_string_lossy().into_owned()
    }

    /// Builds the encoder configuration the recording quality asks for.
    ///
    /// `format` is the encoded output geometry, not the canvas: a quality
    /// preset is a bitrate target, and the same target means something
    /// different at 720p than at 1080p.
    pub(crate) fn recording_video_encoder(&self, format: VideoFormat) -> VideoEncoderConfig {
        let codec = self.effective_recording_codec();
        let mut encoder = VideoEncoderConfig {
            codec,
            implementation: EncoderImplementation::default(),
            profile: (codec == VideoCodec::H264).then(|| "high".to_owned()),
            ..self.rtmp.video.clone()
        };
        if let Some(bitrate) = self.recording_quality.video_bitrate_kbps(format) {
            encoder.bitrate_kbps = bitrate;
        }
        encoder
    }

    /// Builds the audio encoder configuration recordings use.
    pub(crate) fn recording_audio_encoder_config(&self) -> AudioEncoderConfig {
        AudioEncoderConfig {
            codec: AudioCodec::Aac,
            implementation: self.recording_audio_encoder.clone(),
            ..self.rtmp.audio.clone()
        }
    }

    /// Returns the selected sample rate in hertz.
    pub(crate) fn sample_rate_hz(&self) -> u32 {
        SAMPLE_RATES[self.sample_rate.min(SAMPLE_RATES.len() - 1)]
    }

    /// Returns the selected channel count.
    pub(crate) fn channel_count(&self) -> u16 {
        CHANNEL_LAYOUTS[self.channels.min(CHANNEL_LAYOUTS.len() - 1)].channels()
    }

    /// Builds the complete token set for the selected theme and style, with
    /// the accessibility colour overrides applied on top.
    pub(crate) fn tokens(&self) -> ThemeTokens {
        self.tokens_for(self.theme, self.style)
    }

    /// Returns the geometry every settings page lays out against.
    pub(crate) fn metrics(&self) -> UiMetrics {
        metrics(self.density, self.font_size)
    }

    /// Returns the geometry for a density and font size that have not been
    /// committed yet, which is what makes the Appearance page previewable.
    pub(crate) fn metrics_for(density: UiDensity, font_size: u8) -> UiMetrics {
        metrics(density, font_size)
    }

    /// Builds a palette preview for `theme` and `style` while retaining this
    /// settings value's accessibility colour overrides.
    pub(crate) fn tokens_for(&self, theme: usize, style: UiStyle) -> ThemeTokens {
        let preset = styled(&THEMES[theme.min(THEMES.len() - 1)], style);
        ThemeTokens {
            window_bg: brush(preset.window_bg),
            panel_bg: brush(preset.panel_bg),
            header_bg: brush(preset.header_bg),
            header_active_bg: brush(preset.header_active_bg),
            border: brush(preset.border),
            border_strong: brush(preset.border_strong),
            row_bg: brush(preset.row_bg),
            row_selected_bg: brush(preset.row_selected_bg),
            control_bg: brush(preset.control_bg),
            text: brush(preset.text),
            text_strong: brush(preset.text_strong),
            text_muted: brush(preset.text_muted),
            accent: brush(preset.accent),
            canvas_bg: brush(preset.canvas_bg),
            preview_border: parse_colour(&self.preview_border_color)
                .map_or_else(|| brush([0x60, 0xA5, 0xFA]), Brush::SolidColor),
            program_border: parse_colour(&self.program_border_color)
                .map_or_else(|| brush([0xF8, 0x71, 0x71]), Brush::SolidColor),
            meter: brush([0x22, 0xC5, 0x5E]),
            meter_muted: brush([0x7F, 0x1D, 0x1D]),
            warning: brush([0xFB, 0xBF, 0x24]),
        }
    }
}

/// Formats `now` as `YYYY-MM-DD HH-MM-SS` in UTC.
///
/// Recording file names are derived from the clock so two recordings in one
/// session cannot collide. UTC is deliberate: a local-time name would jump
/// backwards across a daylight-saving change and produce a name that already
/// exists.
pub(crate) fn recording_stamp(now: std::time::SystemTime) -> String {
    let seconds = now
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rest / 3_600, (rest % 3_600) / 60, rest % 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}-{minute:02}-{second:02}")
}

/// Converts days since the Unix epoch into a civil `(year, month, day)`.
///
/// This is Howard Hinnant's `civil_from_days`, which is exact for every date
/// the proleptic Gregorian calendar defines and needs no lookup tables.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // Shift the epoch to 0000-03-01 so leap days land at the end of the era.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}
