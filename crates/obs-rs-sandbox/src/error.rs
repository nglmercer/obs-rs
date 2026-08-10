use std::fmt;

use obs_rs_capture::CaptureError;
use obs_rs_media::MediaError;
use obs_rs_plugin_api::PluginError;

/// Errors raised while validating or running a sandbox extension.
#[derive(Debug, Eq, PartialEq)]
pub enum SandboxError {
    /// The manifest exceeds the bounded parser input.
    ManifestTooLarge,
    /// The manifest is malformed or violates an extension limit.
    InvalidManifest { reason: String },
    /// The executable path is empty or otherwise invalid for a direct process
    /// launch.
    InvalidCommand { reason: String },
    /// A command argument exceeds the configured count or byte bound.
    InvalidArguments { reason: String },
    /// The plugin API version is incompatible.
    Plugin(PluginError),
    /// A child process or frame stream failed.
    Capture(CaptureError),
    /// A media value failed validation.
    Media(MediaError),
}

impl fmt::Display for SandboxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifestTooLarge => formatter.write_str("sandbox manifest is too large"),
            Self::InvalidManifest { reason } => {
                write!(formatter, "invalid sandbox manifest: {reason}")
            }
            Self::InvalidCommand { reason } => {
                write!(formatter, "invalid sandbox command: {reason}")
            }
            Self::InvalidArguments { reason } => {
                write!(formatter, "invalid sandbox arguments: {reason}")
            }
            Self::Plugin(error) => error.fmt(formatter),
            Self::Capture(error) => error.fmt(formatter),
            Self::Media(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SandboxError {}

impl From<CaptureError> for SandboxError {
    fn from(error: CaptureError) -> Self {
        Self::Capture(error)
    }
}

impl From<MediaError> for SandboxError {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

impl From<PluginError> for SandboxError {
    fn from(error: PluginError) -> Self {
        Self::Plugin(error)
    }
}
