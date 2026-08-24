use std::{fmt, path::PathBuf};

use obs_rs_output::{OutputProfile, OutputTransport, SegmentedRecordingPolicy, StreamTarget};
use url::Url;

use super::pipeline::{srt_passphrase_valid, valid_stream_url, valid_url};
use super::GStreamerError;

/// Typed destination configuration; secret-bearing values redact in Debug.
#[derive(Clone, Eq, PartialEq)]
pub enum ProductionDestination {
    Recording(PathBuf),
    /// A Matroska recording that is finalized as an MP4 without re-encoding.
    ///
    /// The native session keeps the Matroska source hidden until the remux has
    /// successfully published the requested MP4 destination.
    RemuxRecording {
        final_path: PathBuf,
    },
    /// A bounded rolling set of native muxer segments.
    ///
    /// The native adapter retains at most the policy's segment count while a
    /// session is active. Engine/UI routing is intentionally separate from
    /// this native boundary.
    SegmentedRecording {
        base_path: PathBuf,
        policy: SegmentedRecordingPolicy,
    },
    Rtmp {
        endpoint: String,
    },
    Rtmps {
        endpoint: String,
    },
    Srt {
        endpoint: String,
        passphrase: Option<String>,
    },
    WebRtc {
        signaling_endpoint: String,
        bearer_token: Option<String>,
    },
    Hls {
        directory: PathBuf,
        segment_duration_secs: u32,
        playlist_size: u32,
        low_latency: bool,
    },
    Rist {
        host: String,
        port: u16,
        sender_buffer_ms: u32,
        shared_secret: Option<String>,
    },
}

impl fmt::Debug for ProductionDestination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recording(path) => formatter.debug_tuple("Recording").field(path).finish(),
            Self::RemuxRecording { final_path } => formatter
                .debug_struct("RemuxRecording")
                .field("final_path", final_path)
                .finish(),
            Self::SegmentedRecording { base_path, policy } => formatter
                .debug_struct("SegmentedRecording")
                .field("base_path", base_path)
                .field("policy", policy)
                .finish(),
            Self::Rtmp { .. } => formatter
                .debug_struct("Rtmp")
                .field("endpoint", &"[REDACTED]")
                .finish(),
            Self::Rtmps { .. } => formatter
                .debug_struct("Rtmps")
                .field("endpoint", &"[REDACTED]")
                .finish(),
            Self::Srt { passphrase, .. } => formatter
                .debug_struct("Srt")
                .field("endpoint", &"[REDACTED]")
                .field("passphrase", &passphrase.as_ref().map(|_| "[REDACTED]"))
                .finish(),
            Self::WebRtc { bearer_token, .. } => formatter
                .debug_struct("WebRtc")
                .field("signaling_endpoint", &"[REDACTED]")
                .field("bearer_token", &bearer_token.as_ref().map(|_| "[REDACTED]"))
                .finish(),
            Self::Hls { directory, .. } => formatter
                .debug_struct("Hls")
                .field("directory", directory)
                .finish_non_exhaustive(),
            Self::Rist { shared_secret, .. } => formatter
                .debug_struct("Rist")
                .field("endpoint", &"[REDACTED]")
                .field(
                    "shared_secret",
                    &shared_secret.as_ref().map(|_| "[REDACTED]"),
                )
                .finish(),
        }
    }
}

impl ProductionDestination {
    /// Parses an exact production streaming scheme into its profile and typed
    /// destination. Private OBSRPKT1 TCP/WebSocket endpoints are deliberately
    /// outside this production boundary.
    ///
    /// # Errors
    ///
    /// Rejects unsupported schemes and malformed protocol endpoints.
    pub fn from_stream_endpoint(endpoint: &str) -> Result<(OutputProfile, Self), GStreamerError> {
        let scheme = Url::parse(endpoint)
            .map_err(|error| GStreamerError::InvalidEndpoint(error.to_string()))?
            .scheme()
            .to_owned();
        let (profile, destination) = match scheme.as_str() {
            "rtmp" => (
                OutputProfile::rtmp_h264_aac(),
                Self::Rtmp {
                    endpoint: endpoint.to_owned(),
                },
            ),
            "rtmps" => (
                OutputProfile::rtmps_h264_aac(),
                Self::Rtmps {
                    endpoint: endpoint.to_owned(),
                },
            ),
            "srt" => (
                OutputProfile::srt_mpeg_ts_h264_aac(),
                Self::Srt {
                    endpoint: endpoint.to_owned(),
                    passphrase: None,
                },
            ),
            "rist" => {
                let url = Url::parse(endpoint)
                    .map_err(|error| GStreamerError::InvalidEndpoint(error.to_string()))?;
                (
                    OutputProfile::rist_mpeg_ts_h264_aac(),
                    Self::Rist {
                        host: url.host_str().unwrap_or_default().to_owned(),
                        port: url.port().unwrap_or(5_000),
                        sender_buffer_ms: 1_000,
                        shared_secret: None,
                    },
                )
            }
            _ => {
                return Err(GStreamerError::InvalidEndpoint(
                    "expected an srt://, rist://, rtmp://, or rtmps:// endpoint".to_owned(),
                ));
            }
        };
        destination.validate_for(profile)?;
        Ok((profile, destination))
    }

    /// Converts a semantic frontend target at the worker-owned native boundary.
    ///
    /// # Errors
    ///
    /// Rejects incomplete targets and unsupported reference transports.
    pub fn from_stream_target(
        target: &StreamTarget,
    ) -> Result<(OutputProfile, Self), GStreamerError> {
        let result = match target {
            StreamTarget::Rtmp(_) | StreamTarget::Rtmps(_) | StreamTarget::Srt(_) => {
                return Self::from_stream_endpoint(&target.endpoint().ok_or_else(|| {
                    GStreamerError::InvalidEndpoint("target endpoint is incomplete".to_owned())
                })?);
            }
            StreamTarget::Whip(config) => (
                OutputProfile::web_rtc_vp8_opus(),
                Self::WebRtc {
                    signaling_endpoint: config.endpoint.clone(),
                    bearer_token: config
                        .bearer_token
                        .as_ref()
                        .map(|secret| secret.expose_secret().to_owned()),
                },
            ),
            StreamTarget::Hls(config) => (
                OutputProfile::hls_h264_aac(),
                Self::Hls {
                    directory: config.directory.clone(),
                    segment_duration_secs: config.segment_duration_secs,
                    playlist_size: config.playlist_size,
                    low_latency: config.low_latency,
                },
            ),
            StreamTarget::Rist(config) => (
                OutputProfile::rist_mpeg_ts_h264_aac(),
                Self::Rist {
                    host: config.host.clone(),
                    port: config.port,
                    sender_buffer_ms: config.sender_buffer_ms,
                    shared_secret: config
                        .shared_secret
                        .as_ref()
                        .map(|secret| secret.expose_secret().to_owned()),
                },
            ),
            StreamTarget::Reference { .. } => {
                return Err(GStreamerError::InvalidEndpoint(
                    "reference target is not a production destination".to_owned(),
                ))
            }
        };
        result.1.validate_for(result.0)?;
        Ok(result)
    }

    /// Validates that destination and profile transport agree exactly.
    ///
    /// # Errors
    ///
    /// Rejects scheme mismatches, empty paths, control characters, and missing
    /// WebRTC signaling.
    pub fn validate_for(&self, profile: OutputProfile) -> Result<(), GStreamerError> {
        let valid = match (profile.transport(), self) {
            (OutputTransport::Matroska, Self::Recording(path)) => {
                recording_path_has_extension(path, "mkv")
            }
            (OutputTransport::Matroska, Self::RemuxRecording { final_path }) => {
                recording_path_has_extension(final_path, "mp4")
            }
            (OutputTransport::Matroska, Self::SegmentedRecording { base_path, .. }) => {
                recording_path_has_extension(base_path, "mkv")
            }
            (OutputTransport::Mp4, Self::Recording(path)) => {
                recording_path_has_extension(path, "mp4")
            }
            (OutputTransport::Mp4, Self::SegmentedRecording { base_path, .. }) => {
                recording_path_has_extension(base_path, "mp4")
            }
            (OutputTransport::Mov, Self::Recording(path)) => {
                recording_path_has_extension(path, "mov")
            }
            (OutputTransport::Mov, Self::SegmentedRecording { base_path, .. }) => {
                recording_path_has_extension(base_path, "mov")
            }
            (OutputTransport::Flv, Self::Recording(path)) => {
                recording_path_has_extension(path, "flv")
            }
            (OutputTransport::Flv, Self::SegmentedRecording { base_path, .. }) => {
                recording_path_has_extension(base_path, "flv")
            }
            (OutputTransport::Rtmp, Self::Rtmp { endpoint }) => {
                valid_stream_url(endpoint, "rtmp", true)
            }
            (OutputTransport::Rtmps, Self::Rtmps { endpoint }) => {
                valid_stream_url(endpoint, "rtmps", true)
            }
            (
                OutputTransport::SrtMpegTs,
                Self::Srt {
                    endpoint,
                    passphrase,
                },
            ) => {
                valid_stream_url(endpoint, "srt", false)
                    && srt_passphrase_valid(endpoint, passphrase.as_deref())
            }
            (
                OutputTransport::WebRtc,
                Self::WebRtc {
                    signaling_endpoint, ..
                },
            ) => {
                valid_url(signaling_endpoint, "wss://") || valid_url(signaling_endpoint, "https://")
            }
            (
                OutputTransport::Hls,
                Self::Hls {
                    directory,
                    segment_duration_secs,
                    playlist_size,
                    low_latency,
                },
            ) => {
                !directory.as_os_str().is_empty()
                    && (1..=60).contains(segment_duration_secs)
                    && (3..=1_000).contains(playlist_size)
                    && !low_latency
            }
            (
                OutputTransport::RistMpegTs,
                Self::Rist {
                    host,
                    port,
                    sender_buffer_ms,
                    shared_secret,
                },
            ) => {
                !host.trim().is_empty()
                    && *port > 0
                    && port.is_multiple_of(2)
                    && *sender_buffer_ms <= 60_000
                    && shared_secret.is_none()
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(GStreamerError::InvalidEndpoint(
                "destination does not match the selected profile".to_owned(),
            ))
        }
    }
}

fn recording_path_has_extension(path: &std::path::Path, extension: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}
