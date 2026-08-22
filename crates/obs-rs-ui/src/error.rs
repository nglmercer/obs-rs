use std::fmt;

use obs_rs_audio::AudioError;
use obs_rs_media::MediaError;
use obs_rs_project::ProjectError;

use super::types::{Shortcut, UiAction};

/// Errors from the toolkit-neutral application state.
#[derive(Debug, Eq, PartialEq)]
pub enum UiError {
    /// Project command validation failed.
    Project(ProjectError),
    /// A requested profile or scene is not in the current project.
    UnknownSelection { kind: &'static str, id: String },
    /// A shortcut key is empty or too long.
    InvalidShortcut,
    /// A shortcut is not currently bound.
    UnknownShortcut(Shortcut),
    /// A shortcut is already bound and must be explicitly replaced.
    DuplicateShortcut(Shortcut),
    /// The frontend must execute this action because it owns the external
    /// project/output boundary.
    FrontendActionRequired(UiAction),
    /// The bounded shortcut table has reached its capacity.
    TooManyShortcuts,
    /// Recording is already active.
    RecordingAlreadyActive,
    /// Recording is not active.
    RecordingNotActive,
    /// Streaming is already active.
    StreamingAlreadyActive,
    /// Streaming is not active.
    StreamingNotActive,
    /// The scene transition is invalid.
    Media(MediaError),
    /// An audio mixer operation failed.
    Audio(AudioError),
    /// A mixer channel ID is not present.
    UnknownMixerChannel(String),
    /// A mixer gain is outside the supported 0..=2000 range.
    InvalidMixerGain(u16),
    /// A mixer pan is outside the supported -1000..=1000 range.
    InvalidMixerPan(i32),
    /// The notice sequence counter overflowed.
    NoticeSequenceExhausted,
}

impl fmt::Display for UiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project(error) => error.fmt(formatter),
            Self::UnknownSelection { kind, id } => {
                write!(formatter, "unknown {kind} selection {id}")
            }
            Self::InvalidShortcut => formatter.write_str("shortcut key is empty or too long"),
            Self::UnknownShortcut(shortcut) => {
                write!(formatter, "shortcut {} is not bound", shortcut.key())
            }
            Self::DuplicateShortcut(shortcut) => {
                write!(formatter, "shortcut {} is already bound", shortcut.key())
            }
            Self::FrontendActionRequired(action) => {
                write!(
                    formatter,
                    "shortcut action {action:?} requires the frontend"
                )
            }
            Self::TooManyShortcuts => formatter.write_str("shortcut table is full"),
            Self::RecordingAlreadyActive => formatter.write_str("recording is already active"),
            Self::RecordingNotActive => formatter.write_str("recording is not active"),
            Self::StreamingAlreadyActive => formatter.write_str("streaming is already active"),
            Self::StreamingNotActive => formatter.write_str("streaming is not active"),
            Self::Media(error) => error.fmt(formatter),
            Self::Audio(error) => error.fmt(formatter),
            Self::UnknownMixerChannel(id) => write!(formatter, "mixer channel {id} does not exist"),
            Self::InvalidMixerGain(gain_milli) => {
                write!(formatter, "mixer gain {gain_milli} is outside 0..=2000")
            }
            Self::InvalidMixerPan(pan_milli) => {
                write!(formatter, "mixer pan {pan_milli} is outside -1000..=1000")
            }
            Self::NoticeSequenceExhausted => formatter.write_str("UI notice sequence is exhausted"),
        }
    }
}

impl std::error::Error for UiError {}

impl From<ProjectError> for UiError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}

impl From<AudioError> for UiError {
    fn from(error: AudioError) -> Self {
        Self::Audio(error)
    }
}
