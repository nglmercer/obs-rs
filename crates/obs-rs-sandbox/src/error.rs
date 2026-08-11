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
    /// A signed bundle is malformed or violates a structural limit.
    InvalidBundle { reason: String },
    /// A bundle exceeds the aggregate byte limit.
    BundleTooLarge { bytes: usize },
    /// A payload path is absolute, traversing, or otherwise unsafe.
    UnsafeBundlePath,
    /// No trusted key matches the manifest's signing key ID.
    UnknownSigningKey,
    /// The Ed25519 signature did not verify.
    InvalidBundleSignature,
    /// A payload differs from its signed SHA-256 metadata.
    PayloadHashMismatch,
    /// The bundle target differs from the host target.
    BundleTargetMismatch { expected: String, actual: String },
    /// The running application is older than the bundle's minimum version.
    BundleVersionIncompatible { required: String, actual: String },
    /// The plugin subprocess API is incompatible with this host.
    BundleApiIncompatible {
        required_major: u16,
        required_minor: u16,
    },
    /// Requested capabilities exceed the user's/host's grant.
    BundlePermissionDenied,
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
            Self::InvalidBundle { reason } => write!(formatter, "invalid plugin bundle: {reason}"),
            Self::BundleTooLarge { bytes } => {
                write!(formatter, "plugin bundle is too large: {bytes} bytes")
            }
            Self::UnsafeBundlePath => formatter.write_str("plugin bundle contains an unsafe path"),
            Self::UnknownSigningKey => {
                formatter.write_str("plugin bundle signing key is not trusted")
            }
            Self::InvalidBundleSignature => {
                formatter.write_str("plugin bundle signature is invalid")
            }
            Self::PayloadHashMismatch => {
                formatter.write_str("plugin bundle payload hash does not match")
            }
            Self::BundleTargetMismatch { expected, actual } => write!(
                formatter,
                "plugin bundle target {actual} does not match {expected}"
            ),
            Self::BundleVersionIncompatible { required, actual } => write!(
                formatter,
                "plugin bundle needs application {required}, current version is {actual}"
            ),
            Self::BundleApiIncompatible {
                required_major,
                required_minor,
            } => write!(
                formatter,
                "plugin bundle needs API {required_major}.{required_minor}"
            ),
            Self::BundlePermissionDenied => {
                formatter.write_str("plugin bundle requests capabilities that were not granted")
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
