//! Optional `GStreamer` production output capability and pipeline contracts.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{collections::BTreeMap, fmt, path::PathBuf, process::Command};

use obs_rs_output::{
    AudioCodec, EncoderPreset, OutputCapabilities, OutputProfile, OutputProfileKind,
    OutputTransport, RateControl, VideoCodec,
};
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
    protocols: Vec<ProtocolCapability>,
    video_encoders: Vec<VideoEncoderCapability>,
    audio_encoders: Vec<AudioEncoderCapability>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductionProtocol {
    Reference,
    Rtmp,
    Rtmps,
    Srt,
    WebRtc,
    Matroska,
}

impl ProductionProtocol {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Rtmp => "rtmp",
            Self::Rtmps => "rtmps",
            Self::Srt => "srt",
            Self::WebRtc => "webrtc",
            Self::Matroska => "matroska",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Reference => "Custom reference transport",
            Self::Rtmp => "RTMP",
            Self::Rtmps => "RTMPS",
            Self::Srt => "SRT",
            Self::WebRtc => "WHIP / WebRTC",
            Self::Matroska => "Matroska",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolCapability {
    protocol: ProductionProtocol,
    available: bool,
}

impl ProtocolCapability {
    #[must_use]
    pub const fn protocol(&self) -> ProductionProtocol {
        self.protocol
    }

    #[must_use]
    pub const fn available(&self) -> bool {
        self.available
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoEncoderCapability {
    id: String,
    display_name: &'static str,
    codec: VideoCodec,
    hardware: bool,
    options: VideoEncoderOptionCapabilities,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoEncoderOptionCapabilities {
    supported: Vec<VideoEncoderOption>,
    rate_controls: Vec<RateControl>,
    presets: Vec<EncoderPreset>,
    profiles: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoEncoderOption {
    Bitrate,
    MaxBitrate,
    KeyframeInterval,
    BFrames,
}

impl VideoEncoderOptionCapabilities {
    #[must_use]
    pub fn bitrate(&self) -> bool {
        self.supports(VideoEncoderOption::Bitrate)
    }

    #[must_use]
    pub fn max_bitrate(&self) -> bool {
        self.supports(VideoEncoderOption::MaxBitrate)
    }

    #[must_use]
    pub fn keyframe_interval(&self) -> bool {
        self.supports(VideoEncoderOption::KeyframeInterval)
    }

    #[must_use]
    pub fn b_frames(&self) -> bool {
        self.supports(VideoEncoderOption::BFrames)
    }

    #[must_use]
    pub fn supports(&self, option: VideoEncoderOption) -> bool {
        self.supported.contains(&option)
    }

    #[must_use]
    pub fn rate_controls(&self) -> &[RateControl] {
        &self.rate_controls
    }

    #[must_use]
    pub fn presets(&self) -> &[EncoderPreset] {
        &self.presets
    }

    #[must_use]
    pub fn profiles(&self) -> &[String] {
        &self.profiles
    }
}

impl VideoEncoderCapability {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        self.display_name
    }

    #[must_use]
    pub const fn codec(&self) -> VideoCodec {
        self.codec
    }

    #[must_use]
    pub const fn hardware(&self) -> bool {
        self.hardware
    }

    #[must_use]
    pub const fn options(&self) -> &VideoEncoderOptionCapabilities {
        &self.options
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioEncoderCapability {
    id: String,
    display_name: &'static str,
    codec: AudioCodec,
}

impl AudioEncoderCapability {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        self.display_name
    }

    #[must_use]
    pub const fn codec(&self) -> AudioCodec {
        self.codec
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputCapabilitiesSnapshot {
    protocols: Vec<ProtocolCapability>,
    video_encoders: Vec<VideoEncoderCapability>,
    audio_encoders: Vec<AudioEncoderCapability>,
}

impl OutputCapabilitiesSnapshot {
    #[must_use]
    pub fn protocols(&self) -> &[ProtocolCapability] {
        &self.protocols
    }

    #[must_use]
    pub fn video_encoders(&self) -> &[VideoEncoderCapability] {
        &self.video_encoders
    }

    #[must_use]
    pub fn audio_encoders(&self) -> &[AudioEncoderCapability] {
        &self.audio_encoders
    }
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
                protocols: unavailable_protocols(),
                video_encoders: Vec::new(),
                audio_encoders: Vec::new(),
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
                protocols: unavailable_protocols(),
                video_encoders: Vec::new(),
                audio_encoders: Vec::new(),
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
        let output = OutputCapabilities::approved(profiles, hardware_h264);
        let protocols = protocol_capabilities(&output);
        let video_encoders = ["vah264enc", "vaapih264enc", "nvh264enc", "openh264enc"]
            .into_iter()
            .filter(|element| element_available(element))
            .map(video_encoder_capability)
            .chain(element_available("vp8enc").then(|| video_encoder_capability("vp8enc")))
            .collect();
        let audio_encoders = ["avenc_aac", "opusenc"]
            .into_iter()
            .filter(|element| element_available(element))
            .map(audio_encoder_capability)
            .collect();
        Self {
            runtime_version: Some(runtime_version),
            selected_elements: selected,
            output,
            protocols,
            video_encoders,
            audio_encoders,
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

    #[must_use]
    pub fn capabilities(&self) -> OutputCapabilitiesSnapshot {
        OutputCapabilitiesSnapshot {
            protocols: self.protocols.clone(),
            video_encoders: self.video_encoders.clone(),
            audio_encoders: self.audio_encoders.clone(),
        }
    }
}

fn unavailable_protocols() -> Vec<ProtocolCapability> {
    [
        ProductionProtocol::Reference,
        ProductionProtocol::Rtmp,
        ProductionProtocol::Rtmps,
        ProductionProtocol::Srt,
        ProductionProtocol::WebRtc,
        ProductionProtocol::Matroska,
    ]
    .into_iter()
    .map(|protocol| ProtocolCapability {
        available: protocol == ProductionProtocol::Reference,
        protocol,
    })
    .collect()
}

fn protocol_capabilities(output: &OutputCapabilities) -> Vec<ProtocolCapability> {
    [
        (
            ProductionProtocol::Reference,
            OutputProfileKind::ReferencePacket,
        ),
        (ProductionProtocol::Rtmp, OutputProfileKind::RtmpH264Aac),
        (ProductionProtocol::Rtmps, OutputProfileKind::RtmpsH264Aac),
        (ProductionProtocol::Srt, OutputProfileKind::SrtMpegTsH264Aac),
        (ProductionProtocol::WebRtc, OutputProfileKind::WebRtcVp8Opus),
        (
            ProductionProtocol::Matroska,
            OutputProfileKind::MatroskaH264Aac,
        ),
    ]
    .into_iter()
    .map(|(protocol, profile)| ProtocolCapability {
        protocol,
        available: output.supports(profile),
    })
    .collect()
}

fn video_encoder_capability(element: &str) -> VideoEncoderCapability {
    let (display_name, codec, hardware) = match element {
        "vah264enc" => ("VA H.264", VideoCodec::H264, true),
        "vaapih264enc" => ("VA-API H.264", VideoCodec::H264, true),
        "nvh264enc" => ("NVIDIA NVENC H.264", VideoCodec::H264, true),
        "openh264enc" => ("OpenH264", VideoCodec::H264, false),
        "vp8enc" => ("VP8 Software", VideoCodec::Vp8, false),
        _ => ("Unknown encoder", VideoCodec::ReferenceRle, false),
    };
    VideoEncoderCapability {
        id: element.to_owned(),
        display_name,
        codec,
        hardware,
        options: encoder_option_capabilities(element),
    }
}

fn encoder_option_capabilities(element: &str) -> VideoEncoderOptionCapabilities {
    let inspection = Command::new("gst-inspect-1.0")
        .arg(element)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let has = |names: &[&str]| names.iter().any(|name| property_present(&inspection, name));
    let rate_controls = if has(&[
        "rate-control",
        "rate-control-mode",
        "rc-mode",
        "bitrate-type",
    ]) {
        vec![RateControl::Cbr, RateControl::Vbr, RateControl::Cqp]
    } else {
        Vec::new()
    };
    let presets = if has(&["preset", "speed-preset", "complexity", "target-usage"]) {
        vec![
            EncoderPreset::Speed,
            EncoderPreset::Balanced,
            EncoderPreset::Quality,
        ]
    } else {
        Vec::new()
    };
    let profiles = if inspection.contains("constrained-baseline")
        || inspection.contains("profile: { (string)")
    {
        ["baseline", "main", "high"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    let supported = [
        (
            VideoEncoderOption::Bitrate,
            has(&["bitrate", "target-bitrate"]),
        ),
        (
            VideoEncoderOption::MaxBitrate,
            has(&["max-bitrate", "maxrate"]),
        ),
        (
            VideoEncoderOption::KeyframeInterval,
            has(&["gop-size", "key-int-max", "keyframe-period"]),
        ),
        (
            VideoEncoderOption::BFrames,
            has(&["bframes", "b-frames", "max-bframes", "max-b-frames"]),
        ),
    ]
    .into_iter()
    .filter_map(|(option, available)| available.then_some(option))
    .collect();
    VideoEncoderOptionCapabilities {
        supported,
        rate_controls,
        presets,
        profiles,
    }
}

fn property_present(inspection: &str, property: &str) -> bool {
    inspection.lines().any(|line| {
        line.trim_start()
            .strip_prefix(property)
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    })
}

fn audio_encoder_capability(element: &str) -> AudioEncoderCapability {
    let (display_name, codec) = match element {
        "avenc_aac" => ("FFmpeg AAC", AudioCodec::Aac),
        "opusenc" => ("Opus", AudioCodec::Opus),
        _ => ("Unknown encoder", AudioCodec::Pcm),
    };
    AudioEncoderCapability {
        id: element.to_owned(),
        display_name,
        codec,
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
    fn capability_model_separates_protocol_codec_and_encoder_implementation() {
        let output = OutputCapabilities::approved(
            [
                OutputProfileKind::RtmpH264Aac,
                OutputProfileKind::SrtMpegTsH264Aac,
            ],
            true,
        );
        let protocols = protocol_capabilities(&output);
        assert!(protocols.iter().any(|capability| {
            capability.protocol() == ProductionProtocol::Reference && capability.available()
        }));
        assert!(protocols.iter().any(|capability| {
            capability.protocol() == ProductionProtocol::Rtmp && capability.available()
        }));
        assert!(protocols.iter().any(|capability| {
            capability.protocol() == ProductionProtocol::Rtmps && !capability.available()
        }));

        let hardware = video_encoder_capability("nvh264enc");
        let software = video_encoder_capability("openh264enc");
        assert_eq!(hardware.codec(), VideoCodec::H264);
        assert_eq!(software.codec(), VideoCodec::H264);
        assert!(hardware.hardware());
        assert!(!software.hardware());
        assert_ne!(hardware.id(), software.id());
    }

    #[test]
    fn option_discovery_matches_exact_property_names() {
        let inspection =
            "Element Properties:\n  bitrate : target\n  not-bitrate : no\n  gop-size : interval\n";
        assert!(property_present(inspection, "bitrate"));
        assert!(property_present(inspection, "gop-size"));
        assert!(!property_present(inspection, "rate-control"));

        if element_available("openh264enc") {
            let options = encoder_option_capabilities("openh264enc");
            assert!(options.bitrate());
            assert!(options.max_bitrate());
            assert!(options.keyframe_interval());
            assert!(!options.b_frames());
            assert!(options.rate_controls().contains(&RateControl::Cbr));
            assert!(options.presets().contains(&EncoderPreset::Quality));
            assert!(options.profiles().iter().any(|profile| profile == "high"));
        }
    }

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
