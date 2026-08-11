//! Optional `GStreamer` production output capability and pipeline contracts.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{collections::BTreeMap, fmt, path::PathBuf, process::Command};

use obs_rs_output::{OutputCapabilities, OutputProfile, OutputProfileKind, OutputTransport};
use url::Url;

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
                formatter.write_str("native GStreamer adapter was not compiled")
            }
            Self::Native(reason) => write!(formatter, "native GStreamer output failed: {reason}"),
        }
    }
}

impl std::error::Error for GStreamerError {}

/// Approved plugin selection, including hardware/software encoder choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GStreamerCapabilitySnapshot {
    runtime_version: Option<String>,
    selected_elements: BTreeMap<&'static str, String>,
    output: OutputCapabilities,
}

impl GStreamerCapabilitySnapshot {
    /// Probes only explicitly approved elements. Unapproved runtime plugins are
    /// ignored even when installed.
    #[must_use]
    pub fn probe() -> Self {
        if !cfg!(feature = "native") {
            return Self {
                runtime_version: None,
                selected_elements: BTreeMap::new(),
                output: OutputCapabilities::reference_only(),
            };
        }
        let runtime_version = Command::new("gst-inspect-1.0")
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
        let Some(runtime_version) = runtime_version else {
            return Self {
                runtime_version: None,
                selected_elements: BTreeMap::new(),
                output: OutputCapabilities::reference_only(),
            };
        };

        let mut selected = BTreeMap::new();
        let h264 = first_available(&["vah264enc", "vaapih264enc", "nvh264enc", "openh264enc"]);
        let aac = first_available(&["avenc_aac"]);
        let vp8 = first_available(&["vp8enc"]);
        let opus = first_available(&["opusenc"]);
        if let Some(value) = &h264 {
            selected.insert("h264", value.clone());
        }
        if let Some(value) = &aac {
            selected.insert("aac", value.clone());
        }
        if let Some(value) = &vp8 {
            selected.insert("vp8", value.clone());
        }
        if let Some(value) = &opus {
            selected.insert("opus", value.clone());
        }

        let mut profiles = Vec::new();
        if h264.is_some() && aac.is_some() && element_available("matroskamux") {
            profiles.push(OutputProfileKind::MatroskaH264Aac);
        }
        if h264.is_some()
            && aac.is_some()
            && element_available("flvmux")
            && element_available("rtmpsink")
        {
            profiles.push(OutputProfileKind::RtmpH264Aac);
            profiles.push(OutputProfileKind::RtmpsH264Aac);
        }
        if h264.is_some()
            && aac.is_some()
            && element_available("mpegtsmux")
            && element_available("srtsink")
        {
            profiles.push(OutputProfileKind::SrtMpegTsH264Aac);
        }
        if vp8.is_some() && opus.is_some() && element_available("webrtcbin") {
            profiles.push(OutputProfileKind::WebRtcVp8Opus);
        }
        let hardware_h264 = h264
            .as_deref()
            .is_some_and(|encoder| matches!(encoder, "vah264enc" | "vaapih264enc" | "nvh264enc"));
        Self {
            runtime_version: Some(runtime_version),
            selected_elements: selected,
            output: OutputCapabilities::approved(profiles, hardware_h264),
        }
    }

    #[must_use]
    pub fn runtime_version(&self) -> Option<&str> {
        self.runtime_version.as_deref()
    }

    #[must_use]
    pub const fn output_capabilities(&self) -> &OutputCapabilities {
        &self.output
    }

    #[must_use]
    pub fn selected_element(&self, role: &str) -> Option<&str> {
        self.selected_elements.get(role).map(String::as_str)
    }
}

/// Typed destination configuration; secret-bearing values redact in Debug.
#[derive(Clone, Eq, PartialEq)]
pub enum ProductionDestination {
    Recording(PathBuf),
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
    },
}

impl fmt::Debug for ProductionDestination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recording(path) => formatter.debug_tuple("Recording").field(path).finish(),
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
            Self::WebRtc { .. } => formatter
                .debug_struct("WebRtc")
                .field("signaling_endpoint", &"[REDACTED]")
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
            _ => {
                return Err(GStreamerError::InvalidEndpoint(
                    "expected an srt://, rtmp://, or rtmps:// endpoint".to_owned(),
                ));
            }
        };
        destination.validate_for(profile)?;
        Ok((profile, destination))
    }

    /// Validates that destination and profile transport agree exactly.
    ///
    /// # Errors
    ///
    /// Rejects scheme mismatches, empty paths, control characters, and missing
    /// WebRTC signaling.
    pub fn validate_for(&self, profile: OutputProfile) -> Result<(), GStreamerError> {
        let valid = match (profile.transport(), self) {
            (OutputTransport::Matroska, Self::Recording(path)) => !path.as_os_str().is_empty(),
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
            (OutputTransport::WebRtc, Self::WebRtc { signaling_endpoint }) => {
                valid_url(signaling_endpoint, "wss://") || valid_url(signaling_endpoint, "https://")
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

/// Application-driven WebRTC signaling lifecycle. Network exchange remains in
/// the application; the media adapter never logs or owns signaling credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebRtcSignalingState {
    AwaitingLocalDescription,
    AwaitingRemoteDescription,
    Connecting,
    Connected,
    Retrying,
    Failed,
    Closed,
}

/// Bounded state machine joining an application signaling channel to WebRTC.
pub struct WebRtcSignalingSession {
    state: WebRtcSignalingState,
    retries: u32,
}

impl WebRtcSignalingSession {
    /// Creates a signaling session after validating its typed destination.
    ///
    /// # Errors
    ///
    /// Rejects non-WebRTC destinations and invalid signaling endpoints.
    pub fn new(destination: &ProductionDestination) -> Result<Self, GStreamerError> {
        destination.validate_for(OutputProfile::web_rtc_vp8_opus())?;
        Ok(Self {
            state: WebRtcSignalingState::AwaitingLocalDescription,
            retries: 0,
        })
    }

    #[must_use]
    pub const fn state(&self) -> WebRtcSignalingState {
        self.state
    }

    #[must_use]
    pub const fn retries(&self) -> u32 {
        self.retries
    }

    /// Marks a bounded local SDP description ready for application delivery.
    ///
    /// # Errors
    ///
    /// Rejects invalid state, empty/oversized SDP, and embedded NUL bytes.
    pub fn local_description_ready(&mut self, sdp: &str) -> Result<(), GStreamerError> {
        self.require_state(WebRtcSignalingState::AwaitingLocalDescription)?;
        validate_sdp(sdp)?;
        self.state = WebRtcSignalingState::AwaitingRemoteDescription;
        Ok(())
    }

    /// Accepts a bounded remote SDP answer supplied by the application.
    ///
    /// # Errors
    ///
    /// Rejects invalid state or malformed/oversized SDP.
    pub fn remote_description_received(&mut self, sdp: &str) -> Result<(), GStreamerError> {
        self.require_state(WebRtcSignalingState::AwaitingRemoteDescription)?;
        validate_sdp(sdp)?;
        self.state = WebRtcSignalingState::Connecting;
        Ok(())
    }

    /// Marks ICE/media connectivity established.
    ///
    /// # Errors
    ///
    /// Rejects a connection notification in another lifecycle state.
    pub fn connected(&mut self) -> Result<(), GStreamerError> {
        self.require_state(WebRtcSignalingState::Connecting)?;
        self.state = WebRtcSignalingState::Connected;
        Ok(())
    }

    /// Starts one bounded retry, requiring a fresh offer/answer exchange.
    ///
    /// # Errors
    ///
    /// Rejects retries after close/failure or after `maximum_retries`.
    pub fn retry(&mut self, maximum_retries: u32) -> Result<(), GStreamerError> {
        if matches!(
            self.state,
            WebRtcSignalingState::Closed | WebRtcSignalingState::Failed
        ) || self.retries >= maximum_retries
        {
            self.state = WebRtcSignalingState::Failed;
            return Err(GStreamerError::Native(
                "WebRTC signaling retry limit reached".to_owned(),
            ));
        }
        self.state = WebRtcSignalingState::Retrying;
        self.retries = self.retries.saturating_add(1);
        self.state = WebRtcSignalingState::AwaitingLocalDescription;
        Ok(())
    }

    pub const fn close(&mut self) {
        self.state = WebRtcSignalingState::Closed;
    }

    fn require_state(&self, expected: WebRtcSignalingState) -> Result<(), GStreamerError> {
        if self.state == expected {
            Ok(())
        } else {
            Err(GStreamerError::Native(format!(
                "WebRTC signaling is {:?}, expected {expected:?}",
                self.state
            )))
        }
    }
}

fn validate_sdp(sdp: &str) -> Result<(), GStreamerError> {
    if sdp.is_empty() || sdp.len() > MAX_WEBRTC_SIGNALING_BYTES || sdp.contains('\0') {
        Err(GStreamerError::InvalidEndpoint(
            "WebRTC SDP is empty, oversized, or contains NUL".to_owned(),
        ))
    } else {
        Ok(())
    }
}

/// Deterministic metadata used to create native appsrc/queue pipelines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionPipelinePlan {
    profile: OutputProfile,
    video_encoder: String,
    audio_encoder: String,
    bounded_queue_bytes: usize,
    atomic_recording: bool,
}

impl ProductionPipelinePlan {
    /// Negotiates an approved exact profile and bounded worker queues.
    ///
    /// # Errors
    ///
    /// Returns unavailable instead of silently replacing production codecs.
    pub fn negotiate(
        profile: OutputProfile,
        destination: &ProductionDestination,
        capabilities: &GStreamerCapabilitySnapshot,
    ) -> Result<Self, GStreamerError> {
        destination.validate_for(profile)?;
        capabilities
            .output
            .negotiate(profile)
            .map_err(|_| GStreamerError::ProfileUnavailable(profile.kind()))?;
        let video_role = if profile.kind() == OutputProfileKind::WebRtcVp8Opus {
            "vp8"
        } else {
            "h264"
        };
        let audio_role = if profile.kind() == OutputProfileKind::WebRtcVp8Opus {
            "opus"
        } else {
            "aac"
        };
        Ok(Self {
            profile,
            video_encoder: capabilities
                .selected_element(video_role)
                .ok_or(GStreamerError::ProfileUnavailable(profile.kind()))?
                .to_owned(),
            audio_encoder: capabilities
                .selected_element(audio_role)
                .ok_or(GStreamerError::ProfileUnavailable(profile.kind()))?
                .to_owned(),
            bounded_queue_bytes: profile.queue_bytes(),
            atomic_recording: matches!(destination, ProductionDestination::Recording(_)),
        })
    }

    #[must_use]
    pub const fn profile(&self) -> OutputProfile {
        self.profile
    }

    #[must_use]
    pub fn video_encoder(&self) -> &str {
        &self.video_encoder
    }

    #[must_use]
    pub fn audio_encoder(&self) -> &str {
        &self.audio_encoder
    }

    #[must_use]
    pub const fn bounded_queue_bytes(&self) -> usize {
        self.bounded_queue_bytes
    }

    #[must_use]
    pub const fn atomic_recording(&self) -> bool {
        self.atomic_recording
    }

    /// Encodes bounded production metadata for diagnostics/fuzzing.
    #[must_use]
    pub fn serialize(&self) -> String {
        format!(
            "{PRODUCTION_METADATA_MAGIC}\nprofile={:?}\nvideo={}\naudio={}\nqueue_bytes={}\natomic={}\n",
            self.profile.kind(),
            self.video_encoder,
            self.audio_encoder,
            self.bounded_queue_bytes,
            self.atomic_recording,
        )
    }

    /// Validates serialized production metadata without creating a pipeline.
    ///
    /// # Errors
    ///
    /// Rejects oversized, malformed, unknown, and trailing records.
    pub fn validate_serialized(document: &str) -> Result<(), GStreamerError> {
        if document.len() > MAX_PRODUCTION_METADATA_BYTES {
            return Err(GStreamerError::InvalidMetadata(
                "metadata is too large".to_owned(),
            ));
        }
        let lines = document.lines().collect::<Vec<_>>();
        if lines.len() != 6
            || lines[0] != PRODUCTION_METADATA_MAGIC
            || !lines[1].starts_with("profile=")
            || !lines[2].starts_with("video=")
            || !lines[3].starts_with("audio=")
            || lines[4]
                .strip_prefix("queue_bytes=")
                .and_then(|value| value.parse::<usize>().ok())
                .is_none_or(|value| value == 0 || value > 256 * 1_024 * 1_024)
            || !matches!(lines[5], "atomic=true" | "atomic=false")
        {
            return Err(GStreamerError::InvalidMetadata(
                "metadata schema is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

fn first_available(elements: &[&str]) -> Option<String> {
    elements
        .iter()
        .find(|element| element_available(element))
        .map(|element| (*element).to_owned())
}

fn element_available(element: &str) -> bool {
    Command::new("gst-inspect-1.0")
        .args(["--exists", element])
        .status()
        .is_ok_and(|status| status.success())
}

fn valid_url(value: &str, scheme: &str) -> bool {
    value.starts_with(scheme)
        && value.len() <= 2_048
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_stream_url(value: &str, scheme: &str, require_path: bool) -> bool {
    if value.len() > 2_048 || value.bytes().any(|byte| byte.is_ascii_control()) {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    url.scheme() == scheme
        && url.host_str().is_some_and(|host| !host.is_empty())
        && (scheme != "srt" || url.port().is_some())
        && (!require_path || url.path().trim_matches('/').contains('/'))
}

fn srt_passphrase_valid(endpoint: &str, explicit: Option<&str>) -> bool {
    if let Some(value) = explicit {
        return (10..=79).contains(&value.len());
    }
    Url::parse(endpoint).is_ok_and(|url| {
        url.query_pairs()
            .find(|(key, _)| key == "passphrase")
            .is_none_or(|(_, value)| (10..=79).contains(&value.len()))
    })
}

#[cfg(feature = "native")]
mod native;

#[cfg(feature = "native")]
pub use native::{GStreamerOutputSession, NativeOutputState, OutputSessionTelemetry};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_native_feature_never_claims_production_support() {
        if !cfg!(feature = "native") {
            let capabilities = GStreamerCapabilitySnapshot::probe();
            assert!(capabilities.runtime_version().is_none());
            assert!(!capabilities
                .output_capabilities()
                .supports(OutputProfileKind::MatroskaH264Aac));
        }
    }

    #[test]
    fn destinations_validate_schemes_passphrases_and_redact_secrets() {
        let srt = ProductionDestination::Srt {
            endpoint: "srt://example.invalid:9000".to_owned(),
            passphrase: Some("long-enough-secret".to_owned()),
        };
        assert!(srt
            .validate_for(OutputProfile::srt_mpeg_ts_h264_aac())
            .is_ok());
        assert!(!format!("{srt:?}").contains("long-enough-secret"));
        assert!(ProductionDestination::WebRtc {
            signaling_endpoint: String::new()
        }
        .validate_for(OutputProfile::web_rtc_vp8_opus())
        .is_err());

        for (endpoint, expected) in [
            ("rtmp://media.example/live/key", OutputTransport::Rtmp),
            ("rtmps://media.example/live/key", OutputTransport::Rtmps),
            ("srt://media.example:9000", OutputTransport::SrtMpegTs),
        ] {
            let (profile, destination) =
                ProductionDestination::from_stream_endpoint(endpoint).expect("stream endpoint");
            assert_eq!(profile.transport(), expected);
            assert!(destination.validate_for(profile).is_ok());
            assert!(!format!("{destination:?}").contains("media.example"));
        }
        for invalid in [
            "rtmp://media.example/live",
            "rtmps://media.example/",
            "srt://media.example",
            "srt://media.example:9000?passphrase=short",
            "https://media.example/live/key",
        ] {
            assert!(
                ProductionDestination::from_stream_endpoint(invalid).is_err(),
                "{invalid} must be rejected"
            );
        }
    }

    #[test]
    fn production_metadata_decoder_is_bounded_and_rejects_trailing_records() {
        let valid = concat!(
            "OBSRGST1\n",
            "profile=MatroskaH264Aac\n",
            "video=openh264enc\n",
            "audio=avenc_aac\n",
            "queue_bytes=4096\n",
            "atomic=true\n",
        );
        assert_eq!(ProductionPipelinePlan::validate_serialized(valid), Ok(()));
        assert!(
            ProductionPipelinePlan::validate_serialized(&format!("{valid}trailing=x\n")).is_err()
        );
    }

    #[test]
    fn recording_paths_are_typed_and_must_match_matroska() {
        let destination =
            ProductionDestination::Recording(std::path::Path::new("capture.mkv").to_owned());
        assert!(destination
            .validate_for(OutputProfile::matroska_h264_aac())
            .is_ok());
        assert!(destination
            .validate_for(OutputProfile::rtmp_h264_aac())
            .is_err());
    }

    #[test]
    fn webrtc_signaling_lifecycle_is_bounded_and_application_driven() {
        let destination = ProductionDestination::WebRtc {
            signaling_endpoint: "https://signal.invalid/session-secret".to_owned(),
        };
        let mut session = WebRtcSignalingSession::new(&destination).expect("signaling session");
        session
            .local_description_ready("v=0\r\ns=offer\r\n")
            .expect("offer");
        session
            .remote_description_received("v=0\r\ns=answer\r\n")
            .expect("answer");
        session.connected().expect("connected");
        session.retry(2).expect("retry");
        assert_eq!(session.retries(), 1);
        assert_eq!(
            session.state(),
            WebRtcSignalingState::AwaitingLocalDescription
        );
        assert!(session.local_description_ready("").is_err());
        session.close();
        assert_eq!(session.state(), WebRtcSignalingState::Closed);
    }

    #[cfg(feature = "native")]
    #[test]
    fn native_matroska_session_finalizes_atomically_when_runtime_is_available() {
        use obs_rs_audio::{AudioBuffer, AudioFormat};
        use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

        let capabilities = GStreamerCapabilitySnapshot::probe();
        if !capabilities
            .output_capabilities()
            .supports(OutputProfileKind::MatroskaH264Aac)
        {
            return;
        }
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("obs-rs-gstreamer-{token}.mkv"));
        let destination = ProductionDestination::Recording(path.clone());
        let profile = OutputProfile::matroska_h264_aac();
        let plan = ProductionPipelinePlan::negotiate(profile, &destination, &capabilities)
            .expect("approved pipeline");
        let rate = FrameRate::new(30, 1).expect("rate");
        let video = VideoFormat::new(16, 16, rate).expect("video format");
        let audio = AudioFormat::new(48_000, 2).expect("audio format");
        let mut session = GStreamerOutputSession::start(&plan, &destination, video, audio)
            .expect("start native session");
        for index in 0_u64..30 {
            let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
            session
                .push_video(VideoFrame::solid(video, timestamp, [24, 96, 180, 255]))
                .expect("video submission");
            session
                .push_audio(AudioBuffer::silence(audio, timestamp, 1_600).expect("audio buffer"))
                .expect("audio submission");
        }
        session.close().expect("atomic finalization");
        let metadata = std::fs::metadata(&path).expect("published recording");
        assert!(metadata.len() > 0);
        assert!(!path.with_extension("mkv.part").exists());
        assert_eq!(session.telemetry().video_submitted(), 30);
        assert_eq!(session.telemetry().audio_submitted(), 30);
        let _ = std::fs::remove_file(path);
    }
}
