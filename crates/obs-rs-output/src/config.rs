//! Typed production-stream settings shared by frontends and output backends.

use std::{fmt, path::PathBuf};
use url::Url;
use zeroize::Zeroize;

/// Text whose formatting traits never reveal its contents.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct SecretString(String);

impl SecretString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Explicitly exposes the value at the narrow boundary that consumes it.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StreamProtocol {
    #[default]
    Rtmp,
    Rtmps,
    Srt,
    Whip,
    Hls,
    Rist,
    Reference,
}

impl StreamProtocol {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Rtmp => "rtmp",
            Self::Rtmps => "rtmps",
            Self::Srt => "srt",
            Self::Whip => "whip",
            Self::Hls => "hls",
            Self::Rist => "rist",
            Self::Reference => "reference",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "rtmp" => Some(Self::Rtmp),
            "rtmps" => Some(Self::Rtmps),
            "srt" => Some(Self::Srt),
            "whip" | "webrtc" => Some(Self::Whip),
            "hls" => Some(Self::Hls),
            "rist" => Some(Self::Rist),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

/// A semantic destination retained without materializing credentials into a URL.
#[derive(Clone, Eq, PartialEq)]
pub enum StreamTarget {
    Rtmp(RtmpConfig),
    Rtmps(RtmpConfig),
    Srt(SrtConfig),
    Whip(WhipConfig),
    Hls(HlsConfig),
    Rist(RistConfig),
    Reference { address: String },
}

impl StreamTarget {
    #[must_use]
    pub const fn protocol(&self) -> StreamProtocol {
        match self {
            Self::Rtmp(_) => StreamProtocol::Rtmp,
            Self::Rtmps(_) => StreamProtocol::Rtmps,
            Self::Srt(_) => StreamProtocol::Srt,
            Self::Whip(_) => StreamProtocol::Whip,
            Self::Hls(_) => StreamProtocol::Hls,
            Self::Rist(_) => StreamProtocol::Rist,
            Self::Reference { .. } => StreamProtocol::Reference,
        }
    }

    /// Materializes the destination only for the transport connection call.
    #[must_use]
    pub fn endpoint(&self) -> Option<String> {
        match self {
            Self::Rtmp(config) => config.endpoint(StreamProtocol::Rtmp),
            Self::Rtmps(config) => config.endpoint(StreamProtocol::Rtmps),
            Self::Srt(config) => config.endpoint(),
            Self::Whip(config) => nonempty(&config.endpoint),
            Self::Hls(config) => nonempty(config.directory.to_str()?),
            Self::Rist(config) => config.endpoint(),
            Self::Reference { address } => nonempty(address),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WhipConfig {
    pub endpoint: String,
    pub bearer_token: Option<SecretString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HlsConfig {
    pub directory: PathBuf,
    pub segment_duration_secs: u32,
    pub playlist_size: u32,
    pub low_latency: bool,
}

impl Default for HlsConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("hls"),
            segment_duration_secs: 4,
            playlist_size: 6,
            low_latency: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RistConfig {
    pub host: String,
    pub port: u16,
    pub sender_buffer_ms: u32,
    pub shared_secret: Option<SecretString>,
}

impl Default for RistConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 5_000,
            sender_buffer_ms: 1_000,
            shared_secret: None,
        }
    }
}

impl RistConfig {
    #[must_use]
    pub fn endpoint(&self) -> Option<String> {
        let host = self.host.trim();
        (!host.is_empty() && self.port > 0 && self.port.is_multiple_of(2))
            .then(|| format!("rist://{host}:{}", self.port))
    }
}

impl fmt::Debug for StreamTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamTarget")
            .field("protocol", &self.protocol())
            .field("endpoint", &"[REDACTED]")
            .finish()
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SrtMode {
    #[default]
    Caller,
    Listener,
    Rendezvous,
}

impl SrtMode {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Caller => "caller",
            Self::Listener => "listener",
            Self::Rendezvous => "rendezvous",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "caller" => Some(Self::Caller),
            "listener" => Some(Self::Listener),
            "rendezvous" => Some(Self::Rendezvous),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum SrtKeyLength {
    Bits128 = 16,
    Bits192 = 24,
    Bits256 = 32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
    Av1,
    Vp8,
    ReferenceRle,
}

impl VideoCodec {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::Vp8 => "vp8",
            Self::ReferenceRle => "reference-rle",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "h264" | "avc" => Some(Self::H264),
            "hevc" | "h265" => Some(Self::Hevc),
            "av1" => Some(Self::Av1),
            "vp8" => Some(Self::Vp8),
            "reference-rle" | "rle" => Some(Self::ReferenceRle),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AudioCodec {
    #[default]
    Aac,
    Opus,
    Pcm,
}

impl AudioCodec {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Pcm => "pcm",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "aac" | "avenc_aac" => Some(Self::Aac),
            "opus" | "opusenc" => Some(Self::Opus),
            "pcm" => Some(Self::Pcm),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EncoderImplementation(String);

impl EncoderImplementation {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_automatic(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RateControl {
    #[default]
    Cbr,
    Vbr,
    Cqp,
}

impl RateControl {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Cbr => "cbr",
            Self::Vbr => "vbr",
            Self::Cqp => "cqp",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "cbr" => Some(Self::Cbr),
            "vbr" => Some(Self::Vbr),
            "cqp" | "quality" => Some(Self::Cqp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncoderPreset {
    Speed,
    #[default]
    Balanced,
    Quality,
}

impl EncoderPreset {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Speed => "speed",
            Self::Balanced => "balanced",
            Self::Quality => "quality",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "speed" | "fast" => Some(Self::Speed),
            "balanced" | "medium" => Some(Self::Balanced),
            "quality" | "slow" => Some(Self::Quality),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoEncoderConfig {
    pub codec: VideoCodec,
    pub implementation: EncoderImplementation,
    pub rate_control: RateControl,
    pub bitrate_kbps: u32,
    pub max_bitrate_kbps: Option<u32>,
    pub keyframe_interval_secs: u32,
    pub preset: EncoderPreset,
    pub profile: Option<String>,
    pub b_frames: u8,
}

impl Default for VideoEncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::H264,
            implementation: EncoderImplementation::default(),
            rate_control: RateControl::Cbr,
            bitrate_kbps: 6_000,
            max_bitrate_kbps: None,
            keyframe_interval_secs: 2,
            preset: EncoderPreset::Balanced,
            profile: Some("high".to_owned()),
            b_frames: 2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioEncoderConfig {
    pub codec: AudioCodec,
    pub implementation: EncoderImplementation,
    pub bitrate_kbps: u32,
    pub sample_rate: u32,
    pub channels: u16,
    pub complexity: Option<u8>,
}

impl Default for AudioEncoderConfig {
    fn default() -> Self {
        Self {
            codec: AudioCodec::Aac,
            implementation: EncoderImplementation::default(),
            bitrate_kbps: 160,
            sample_rate: 48_000,
            channels: 2,
            complexity: None,
        }
    }
}

impl SrtKeyLength {
    #[must_use]
    pub const fn bytes(self) -> u16 {
        self as u16
    }

    #[must_use]
    pub const fn bits(self) -> u16 {
        self.bytes() * 8
    }

    #[must_use]
    pub const fn from_bytes(bytes: u16) -> Option<Self> {
        match bytes {
            16 => Some(Self::Bits128),
            24 => Some(Self::Bits192),
            32 => Some(Self::Bits256),
            _ => None,
        }
    }
}

/// Connection and encoder choices common to RTMP and RTMPS services.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RtmpConfig {
    pub service: String,
    pub server: String,
    pub stream_key: SecretString,
    pub video: VideoEncoderConfig,
    pub audio: AudioEncoderConfig,
    pub reconnect: bool,
    pub maximum_retries: u32,
    pub network_buffer_ms: u32,
}

impl Default for RtmpConfig {
    fn default() -> Self {
        Self {
            service: "Custom".to_owned(),
            server: "127.0.0.1/live".to_owned(),
            stream_key: SecretString::new("stream"),
            video: VideoEncoderConfig::default(),
            audio: AudioEncoderConfig::default(),
            reconnect: true,
            maximum_retries: 20,
            network_buffer_ms: 1_000,
        }
    }
}

impl RtmpConfig {
    /// Builds the transport endpoint while percent-encoding the secret path
    /// segment. The returned value is intended only for the connection API.
    #[must_use]
    pub fn endpoint(&self, protocol: StreamProtocol) -> Option<String> {
        let scheme = match protocol {
            StreamProtocol::Rtmp => "rtmp",
            StreamProtocol::Rtmps => "rtmps",
            _ => return None,
        };
        let server = self
            .server
            .trim()
            .trim_start_matches("rtmp://")
            .trim_start_matches("rtmps://")
            .trim_end_matches('/');
        let mut url = Url::parse(&format!("{scheme}://{server}")).ok()?;
        if !self.stream_key.is_empty() {
            url.path_segments_mut()
                .ok()?
                .push(self.stream_key.expose_secret());
        }
        Some(url.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SrtConfig {
    pub host: String,
    pub port: u16,
    pub mode: SrtMode,
    pub latency_ms: u32,
    pub passphrase: Option<SecretString>,
    pub pbkeylen: Option<SrtKeyLength>,
    pub stream_id: Option<String>,
    pub connect_timeout_ms: u32,
}

impl Default for SrtConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 9_000,
            mode: SrtMode::Caller,
            latency_ms: 120,
            passphrase: None,
            pbkeylen: None,
            stream_id: None,
            connect_timeout_ms: 5_000,
        }
    }
}

impl SrtConfig {
    /// Builds an SRT URI using URL query encoding for all optional values.
    #[must_use]
    pub fn endpoint(&self) -> Option<String> {
        if self.host.trim().is_empty() || self.port == 0 {
            return None;
        }
        let mut url = Url::parse(&format!("srt://{}:{}", self.host.trim(), self.port)).ok()?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("mode", self.mode.id());
            query.append_pair("latency", &self.latency_ms.to_string());
            query.append_pair("connect_timeout", &self.connect_timeout_ms.to_string());
            if let Some(passphrase) = &self.passphrase {
                query.append_pair("passphrase", passphrase.expose_secret());
            }
            if let Some(key_length) = self.pbkeylen {
                query.append_pair("pbkeylen", &key_length.bytes().to_string());
            }
            if let Some(stream_id) = self.stream_id.as_deref() {
                query.append_pair("streamid", stream_id);
            }
        }
        Some(url.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_by_all_formatting_traits() {
        let secret = SecretString::new("do-not-print-this");
        assert_eq!(secret.to_string(), "[REDACTED]");
        assert!(!format!("{secret:?}").contains("do-not-print-this"));

        let config = RtmpConfig {
            stream_key: secret,
            ..RtmpConfig::default()
        };
        assert!(!format!("{config:?}").contains("do-not-print-this"));
        let target = StreamTarget::Rtmp(config);
        assert_eq!(
            format!("{target:?}"),
            "StreamTarget { protocol: Rtmp, endpoint: \"[REDACTED]\" }"
        );
    }

    #[test]
    fn protocol_and_srt_values_round_trip_through_stable_ids() {
        for protocol in [
            StreamProtocol::Rtmp,
            StreamProtocol::Rtmps,
            StreamProtocol::Srt,
            StreamProtocol::Whip,
            StreamProtocol::Hls,
            StreamProtocol::Rist,
            StreamProtocol::Reference,
        ] {
            assert_eq!(StreamProtocol::from_id(protocol.id()), Some(protocol));
        }
        for mode in [SrtMode::Caller, SrtMode::Listener, SrtMode::Rendezvous] {
            assert_eq!(SrtMode::from_id(mode.id()), Some(mode));
        }
    }

    #[test]
    fn extended_targets_are_typed_bounded_and_redacted() {
        let whip = StreamTarget::Whip(WhipConfig {
            endpoint: "https://service.example/whip".to_owned(),
            bearer_token: Some(SecretString::new("private-bearer")),
        });
        assert_eq!(whip.protocol(), StreamProtocol::Whip);
        assert!(!format!("{whip:?}").contains("private-bearer"));

        let hls = StreamTarget::Hls(HlsConfig::default());
        assert_eq!(hls.protocol(), StreamProtocol::Hls);
        assert_eq!(hls.endpoint().as_deref(), Some("hls"));

        let rist = StreamTarget::Rist(RistConfig::default());
        assert_eq!(rist.endpoint().as_deref(), Some("rist://127.0.0.1:5000"));
        let invalid = StreamTarget::Rist(RistConfig {
            port: 5_001,
            ..RistConfig::default()
        });
        assert!(invalid.endpoint().is_none());
    }

    #[test]
    fn endpoints_encode_secret_and_stream_identifier_components() {
        let rtmp = RtmpConfig {
            server: "media.example/live".to_owned(),
            stream_key: SecretString::new("key with/slash"),
            ..RtmpConfig::default()
        };
        assert_eq!(
            rtmp.endpoint(StreamProtocol::Rtmps).as_deref(),
            Some("rtmps://media.example/live/key%20with%2Fslash")
        );
        let srt = SrtConfig {
            passphrase: Some(SecretString::new("secret phrase")),
            stream_id: Some("#!::r=feed,m=publish".to_owned()),
            ..SrtConfig::default()
        };
        let endpoint = srt.endpoint().expect("valid SRT endpoint");
        assert!(endpoint.contains("passphrase=secret+phrase"));
        assert!(endpoint.contains("streamid=%23%21%3A%3Ar%3Dfeed%2Cm%3Dpublish"));
    }
}
