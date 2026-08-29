use gstreamer as gst;
use gstreamer::prelude::*;

#[path = "native_pipeline.rs"]
mod native_pipeline;
#[path = "native_recording.rs"]
mod native_recording;
#[path = "native_remux.rs"]
mod native_remux;
#[path = "native_session.rs"]
mod native_session;
#[path = "native_stinger.rs"]
mod native_stinger;

#[cfg(test)]
#[path = "native_stinger_tests.rs"]
mod native_stinger_tests;
#[cfg(test)]
#[path = "native_tests.rs"]
mod native_tests;

use native_pipeline::{
    appsrc, configure_encoders, configure_segmented_location_callback, configure_sink,
    configure_sources, pipeline_description, video_caps, PipelineDescription,
};
use native_recording::{
    native_error, publish_recording_artifact, publish_segmented_recording,
    recover_stale_recording_artifact, recover_stale_segment_artifacts, remove_stale_recording_path,
    segmented_recording_paths, segmented_recording_pattern,
};
use native_remux::recover_stale_remux_manifest;

#[cfg(test)]
use native_remux::{remux_final_path_from_source, remux_manifest_matches, remux_manifest_path};

pub use native_remux::{
    discover_interrupted_remux_candidates, recover_interrupted_remux_recording,
    remux_matroska_to_mp4, write_interrupted_remux_manifest,
};
pub use native_session::{GStreamerOutputSession, OutputSessionTelemetry};
pub use native_stinger::{
    stinger_decode_capabilities, GStreamerStingerLoader, StingerDecodeCapabilities,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOutputState {
    Opening,
    Ready,
    Lost,
    Retrying,
    Failed,
    Closed,
}

/// Result of an explicit recovery attempt for an interrupted automatic remux.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemuxRecovery {
    /// No hidden Matroska source exists beside the requested MP4 path.
    NoCandidate,
    /// The hidden source was remuxed and the MP4 was published atomically.
    Recovered { bytes: usize },
}

/// Maximum number of remux candidates returned from one directory scan.
pub const MAX_REMUX_RECOVERY_CANDIDATES: usize = 64;
/// Maximum directory entries inspected by one remux candidate scan.
pub const MAX_REMUX_RECOVERY_DIRECTORY_ENTRIES: usize = 4_096;
/// Maximum size of one durable remux manifest.
pub const MAX_REMUX_MANIFEST_BYTES: usize = 4_096;

pub(super) const REMUX_MANIFEST_FORMAT: &str = "obs-rs-remux-manifest-v1";

const MAX_NATIVE_ERROR_DEBUG_CHARS: usize = 512;

/// Formats a native pipeline error without allowing GStreamer debug text to
/// grow the control-plane error indefinitely.
pub(super) fn native_pipeline_error(context: &str, error: &gst::message::Error) -> String {
    let mut message = format!("{context}: {}", error.error());
    if let Some(source) = error.src() {
        let source_name = source.name();
        if !source_name.is_empty() {
            message.push_str("; source=");
            message.push_str(source_name.as_str());
        }
    }
    if let Some(debug) = error.debug() {
        let debug = debug.trim();
        if !debug.is_empty() {
            message.push_str("; debug=");
            message.push_str(&bounded_native_error_debug(debug));
        }
    }
    message
}

fn bounded_native_error_debug(value: &str) -> String {
    let mut characters = value.chars();
    let bounded = characters
        .by_ref()
        .take(MAX_NATIVE_ERROR_DEBUG_CHARS)
        .collect::<String>();
    if characters.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}

#[cfg(test)]
mod tests {
    use super::{bounded_native_error_debug, MAX_NATIVE_ERROR_DEBUG_CHARS};

    #[test]
    fn native_error_debug_is_bounded() {
        let value = "x".repeat(MAX_NATIVE_ERROR_DEBUG_CHARS + 1);
        let bounded = bounded_native_error_debug(&value);

        assert_eq!(bounded.chars().count(), MAX_NATIVE_ERROR_DEBUG_CHARS + 1);
        assert!(bounded.ends_with('…'));
    }
}
