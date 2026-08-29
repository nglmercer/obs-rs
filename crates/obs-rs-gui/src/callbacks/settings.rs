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

use obs_rs_audio::AudioFormat;
use obs_rs_engine::{ProductionOutputStatus, ProductionProtocol};
use obs_rs_media::{FrameRate, ScaleFilter, VideoFormat};
use obs_rs_output::{
    AudioCodec, EncoderImplementation, EncoderPreset, OutputProfileKind, RateControl,
    StreamingServicePreset, VideoCodec, RTMP_SERVICE_PRESETS,
};
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
        AUDIO_MONITOR_MODES, AUDIO_SYNC_OFFSET_RANGE, CANVAS_SNAP_DISTANCE_DEFAULT,
        CANVAS_SNAP_DISTANCE_RANGE, CHANNEL_LAYOUTS, FRAME_RATES,
        RECORDING_SPLIT_DURATION_MINUTES_DEFAULT, RECORDING_SPLIT_DURATION_MINUTES_RANGE,
        RECORDING_SPLIT_SEGMENTS_DEFAULT, RECORDING_SPLIT_SEGMENTS_RANGE,
        RECORDING_SPLIT_SIZE_MIB_DEFAULT, RECORDING_SPLIT_SIZE_MIB_RANGE,
        REPLAY_BUFFER_CAPACITY_MIB_DEFAULT, REPLAY_BUFFER_CAPACITY_MIB_RANGE,
        REPLAY_BUFFER_DURATION_DEFAULT, REPLAY_BUFFER_DURATION_RANGE, RESOLUTIONS, SAMPLE_RATES,
        THEMES,
    },
    settings_model::{
        aspect_ratio_text, parse_resolution, resolution_text, FpsMode, OutputMode,
        RecordingQuality, UiDensity, UiStyle, VideoSettings, FONT_SIZE_RANGE,
    },
    I18n, MainWindow, Metrics, OutputRuntime, Palette, PreviewSurface, SettingsText,
    SettingsWindow, UiMetrics,
};

#[path = "settings_controller.rs"]
mod settings_controller;

pub(crate) use settings_controller::{
    apply_settings_snapshot, apply_staged_audio_format, apply_staged_output_scaling,
    apply_staged_video_format, install_settings_window, PeerWindows, SettingsController,
};

#[cfg(test)]
pub(crate) use settings_controller::{apply_video_format, populate_settings_models};
