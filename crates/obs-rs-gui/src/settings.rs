//! Persistent application settings behind the OBS-style settings window.
//!
//! Everything the settings window edits is stored in one validated
//! [`obs_rs_config::Config`] document, so persistence is the same flat TOML
//! format the rest of OBS-RS uses and a malformed file degrades to defaults
//! rather than failing startup.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use obs_rs_audio::{AudioChannelLayout, AudioMonitorMode, MAX_AUDIO_SYNC_OFFSET_MILLISECONDS};
use obs_rs_config::Config;
use obs_rs_media::{ScaleFilter, VideoFormat};
use obs_rs_output::{
    AudioCodec, AudioEncoderConfig, EncoderImplementation, EncoderPreset, HlsConfig, RateControl,
    RistConfig, RtmpConfig, SecretString, SrtConfig, SrtKeyLength, SrtMode, StreamProtocol,
    StreamTarget, VideoCodec, VideoEncoderConfig, WhipConfig,
};
use obs_rs_ui::{ProjectSceneSelection, Shortcut, UiAction, UiError, UiLocale};
use slint::{Brush, Color, Model, ModelRc, VecModel};

use crate::dock_tree::{DockNode, DOCK_IDS};
use crate::settings_model::{
    metrics, FpsMode, OutputMode, RecordingQuality, UiDensity, UiStyle, VideoSettings,
    DEFAULT_FONT_SIZE, FONT_SIZE_RANGE, MAX_DIMENSION,
};
use crate::{ThemeTokens, UiMetrics};

/// File name the settings document is read from and written to.
const SETTINGS_FILE: &str = "obs-rs-settings.toml";
/// Default file names inside the per-user directory.
const PROJECT_FILE: &str = "obs-rs-project.json";
const DIAGNOSTICS_FILE: &str = "obs-rs-diagnostics.obsrdg";
const PROJECT_SCENE_SELECTIONS_KEY: &str = "project_scene_selections";
const PROJECTOR_TARGETS_KEY: &str = "layout_projector_targets";
const PROJECTOR_MONITORS_KEY: &str = "layout_projector_monitors";
const MAX_PERSISTED_PROJECT_SCENE_SELECTIONS: usize = 16;
const MAX_PERSISTED_SELECTION_KEY_BYTES: usize = 384;
const MAX_PERSISTED_PROJECTOR_TARGETS: usize = 2;
const MAX_PROJECTOR_TARGET_COMPONENT_BYTES: usize = 256;
const MAX_PERSISTED_PROJECTOR_MONITORS: usize = 5;
const MAX_PROJECTOR_MONITOR_ID_BYTES: usize = 256;

#[path = "settings_document.rs"]
mod settings_document;
#[path = "settings_layout.rs"]
mod settings_layout;
#[path = "settings_persistence.rs"]
mod settings_persistence;
#[path = "settings_theme.rs"]
mod settings_theme;
#[path = "settings_types.rs"]
mod settings_types;
#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;

#[allow(
    clippy::wildcard_imports,
    reason = "settings submodules share the validated settings namespace"
)]
use settings_document::*;
use settings_layout::{DEFAULT_PANEL_ORDER, DEFAULT_PANEL_WEIGHTS};
#[allow(
    clippy::wildcard_imports,
    reason = "settings submodules share the validated settings namespace"
)]
use settings_theme::*;

pub(crate) use settings_document::{hotkey_conflicts, shortcut_bindings, validated_hotkey};
pub(crate) use settings_layout::{
    scale_window_dimension, FloatingGeometry, LayoutSettings, ProjectorGeometry, ProjectorKind,
    ProjectorMonitor, ProjectorTarget,
};
use settings_persistence::default_recording_directory;
#[allow(unused_imports)]
pub(crate) use settings_theme::{parse_colour, ThemePreset, THEMES};
#[allow(unused_imports)]
pub(crate) use settings_types::{
    apply_default_layout, audio_monitor_mode_from_id, audio_monitor_mode_id, recording_stamp,
    settings_path, user_directory, user_file, AppSettings, RecordingFormat, SettingsLoad,
    SetupState, AUDIO_MONITOR_MODES, AUDIO_SYNC_OFFSET_DEFAULT, AUDIO_SYNC_OFFSET_RANGE,
    CANVAS_SNAP_DISTANCE_DEFAULT, CANVAS_SNAP_DISTANCE_RANGE, CHANNEL_LAYOUTS, FRAME_RATES,
    RECORDING_SPLIT_DURATION_MINUTES_DEFAULT, RECORDING_SPLIT_DURATION_MINUTES_RANGE,
    RECORDING_SPLIT_SEGMENTS_DEFAULT, RECORDING_SPLIT_SEGMENTS_RANGE,
    RECORDING_SPLIT_SIZE_MIB_DEFAULT, RECORDING_SPLIT_SIZE_MIB_RANGE,
    REPLAY_BUFFER_CAPACITY_MIB_DEFAULT, REPLAY_BUFFER_CAPACITY_MIB_RANGE,
    REPLAY_BUFFER_DURATION_DEFAULT, REPLAY_BUFFER_DURATION_RANGE, RESOLUTIONS, SAMPLE_RATES,
};
