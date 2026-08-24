//! Optional `GStreamer` production output capability and pipeline contracts.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::fmt;

use obs_rs_output::OutputProfileKind;

mod capabilities;
mod destination;
mod pipeline;
mod signaling;

pub use capabilities::{
    AudioEncoderCapability, GStreamerCapabilitySnapshot, OutputCapabilitiesSnapshot,
    ProductionProtocol, ProtocolCapability, VideoEncoderCapability, VideoEncoderOption,
    VideoEncoderOptionCapabilities,
};
pub use destination::ProductionDestination;
pub use pipeline::ProductionPipelinePlan;
pub use signaling::{WebRtcSignalingSession, WebRtcSignalingState};

pub const PRODUCTION_METADATA_MAGIC: &str = "OBSRGST1";
pub const MAX_PRODUCTION_METADATA_BYTES: usize = 32 * 1_024;
pub const MAX_WEBRTC_SIGNALING_BYTES: usize = 256 * 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GStreamerError {
    RuntimeUnavailable(String),
    ProfileUnavailable(OutputProfileKind),
    InvalidEndpoint(String),
    InvalidMetadata(String),
    NativeAdapterDisabled,
    Native(String),
}

impl fmt::Display for GStreamerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeUnavailable(reason) => {
                write!(formatter, "GStreamer is unavailable: {reason}")
            }
            Self::ProfileUnavailable(profile) => {
                write!(formatter, "GStreamer profile {profile:?} is unavailable")
            }
            Self::InvalidEndpoint(reason) => write!(formatter, "invalid output endpoint: {reason}"),
            Self::InvalidMetadata(reason) => {
                write!(formatter, "invalid production metadata: {reason}")
            }
            Self::NativeAdapterDisabled => {
                formatter.write_str("native GStreamer output adapter was not compiled")
            }
            Self::Native(reason) => write!(formatter, "native GStreamer output failed: {reason}"),
        }
    }
}

impl std::error::Error for GStreamerError {}

#[cfg(feature = "native")]
mod native;

#[cfg(feature = "native")]
pub use native::{
    discover_interrupted_remux_candidates, recover_interrupted_remux_recording,
    remux_matroska_to_mp4, write_interrupted_remux_manifest, GStreamerOutputSession,
    NativeOutputState, OutputSessionTelemetry, RemuxRecovery, MAX_REMUX_MANIFEST_BYTES,
    MAX_REMUX_RECOVERY_CANDIDATES, MAX_REMUX_RECOVERY_DIRECTORY_ENTRIES,
};

#[cfg(test)]
mod tests;
