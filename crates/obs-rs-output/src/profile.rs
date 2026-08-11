use std::collections::BTreeSet;

use super::OutputError;

/// Current version of the production output-profile contract.
pub const OUTPUT_PROFILE_VERSION: u16 = 1;

/// Stable built-in output profile identifiers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OutputProfileKind {
    /// Deterministic OBSRPKT1 reference container and codecs.
    ReferencePacket,
    /// Matroska recording with H.264 video and AAC audio.
    MatroskaH264Aac,
    /// RTMP stream carrying H.264 and AAC.
    RtmpH264Aac,
    /// SRT stream carrying MPEG-TS, H.264, and AAC.
    SrtMpegTsH264Aac,
    /// WebRTC stream carrying VP8 and Opus.
    WebRtcVp8Opus,
}

/// Video codec selected by a profile.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OutputVideoCodec {
    ReferenceRle,
    H264,
    Vp8,
}

/// Audio codec selected by a profile.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OutputAudioCodec {
    Pcm,
    Aac,
    Opus,
}

/// Container or live transport family selected by a profile.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OutputTransport {
    ObsrPkt1,
    Matroska,
    Rtmp,
    SrtMpegTs,
    WebRtc,
}

/// Versioned, bounded output policy independent of a native media runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputProfile {
    version: u16,
    kind: OutputProfileKind,
    video: OutputVideoCodec,
    audio: OutputAudioCodec,
    transport: OutputTransport,
    queue_bytes: usize,
    latency_millis: u32,
}

impl OutputProfile {
    /// Deterministic portable fallback profile.
    #[must_use]
    pub const fn reference() -> Self {
        Self::new(
            OutputProfileKind::ReferencePacket,
            OutputVideoCodec::ReferenceRle,
            OutputAudioCodec::Pcm,
            OutputTransport::ObsrPkt1,
            8 * 1_024 * 1_024,
            2_000,
        )
    }

    /// Default production local-recording profile.
    #[must_use]
    pub const fn matroska_h264_aac() -> Self {
        Self::new(
            OutputProfileKind::MatroskaH264Aac,
            OutputVideoCodec::H264,
            OutputAudioCodec::Aac,
            OutputTransport::Matroska,
            32 * 1_024 * 1_024,
            4_000,
        )
    }

    /// Default RTMP profile.
    #[must_use]
    pub const fn rtmp_h264_aac() -> Self {
        Self::new(
            OutputProfileKind::RtmpH264Aac,
            OutputVideoCodec::H264,
            OutputAudioCodec::Aac,
            OutputTransport::Rtmp,
            8 * 1_024 * 1_024,
            2_000,
        )
    }

    /// Default bounded-latency SRT profile.
    #[must_use]
    pub const fn srt_mpeg_ts_h264_aac() -> Self {
        Self::new(
            OutputProfileKind::SrtMpegTsH264Aac,
            OutputVideoCodec::H264,
            OutputAudioCodec::Aac,
            OutputTransport::SrtMpegTs,
            8 * 1_024 * 1_024,
            1_000,
        )
    }

    /// Default WebRTC profile; signaling remains application-provided.
    #[must_use]
    pub const fn web_rtc_vp8_opus() -> Self {
        Self::new(
            OutputProfileKind::WebRtcVp8Opus,
            OutputVideoCodec::Vp8,
            OutputAudioCodec::Opus,
            OutputTransport::WebRtc,
            4 * 1_024 * 1_024,
            500,
        )
    }

    const fn new(
        kind: OutputProfileKind,
        video: OutputVideoCodec,
        audio: OutputAudioCodec,
        transport: OutputTransport,
        queue_bytes: usize,
        latency_millis: u32,
    ) -> Self {
        Self {
            version: OUTPUT_PROFILE_VERSION,
            kind,
            video,
            audio,
            transport,
            queue_bytes,
            latency_millis,
        }
    }

    #[must_use]
    pub const fn version(self) -> u16 {
        self.version
    }

    #[must_use]
    pub const fn kind(self) -> OutputProfileKind {
        self.kind
    }

    #[must_use]
    pub const fn video_codec(self) -> OutputVideoCodec {
        self.video
    }

    #[must_use]
    pub const fn audio_codec(self) -> OutputAudioCodec {
        self.audio
    }

    #[must_use]
    pub const fn transport(self) -> OutputTransport {
        self.transport
    }

    #[must_use]
    pub const fn queue_bytes(self) -> usize {
        self.queue_bytes
    }

    #[must_use]
    pub const fn latency_millis(self) -> u32 {
        self.latency_millis
    }
}

/// Approved host capabilities reported by an optional native output adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputCapabilities {
    available: BTreeSet<OutputProfileKind>,
    hardware_h264: bool,
}

impl Default for OutputCapabilities {
    fn default() -> Self {
        Self::reference_only()
    }
}

impl OutputCapabilities {
    /// Portable capability snapshot containing only OBSRPKT1.
    #[must_use]
    pub fn reference_only() -> Self {
        Self {
            available: BTreeSet::from([OutputProfileKind::ReferencePacket]),
            hardware_h264: false,
        }
    }

    /// Creates a snapshot from profiles whose runtime plugins passed the
    /// adapter's license/approval policy.
    #[must_use]
    pub fn approved(
        profiles: impl IntoIterator<Item = OutputProfileKind>,
        hardware_h264: bool,
    ) -> Self {
        let mut available = BTreeSet::from([OutputProfileKind::ReferencePacket]);
        available.extend(profiles);
        Self {
            available,
            hardware_h264,
        }
    }

    #[must_use]
    pub fn supports(&self, profile: OutputProfileKind) -> bool {
        self.available.contains(&profile)
    }

    #[must_use]
    pub const fn hardware_h264(&self) -> bool {
        self.hardware_h264
    }

    /// Negotiates an exact profile without silently substituting codecs.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::ProfileUnavailable`] when the approved runtime
    /// cannot satisfy the requested profile.
    pub fn negotiate(&self, profile: OutputProfile) -> Result<NegotiatedOutput, OutputError> {
        if profile.version != OUTPUT_PROFILE_VERSION {
            return Err(OutputError::UnsupportedProfileVersion {
                version: profile.version,
            });
        }
        if !self.supports(profile.kind) {
            return Err(OutputError::ProfileUnavailable {
                profile: profile.kind,
            });
        }
        Ok(NegotiatedOutput {
            profile,
            hardware_video: self.hardware_h264 && profile.video == OutputVideoCodec::H264,
        })
    }
}

/// Exact approved profile selected for one output session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiatedOutput {
    profile: OutputProfile,
    hardware_video: bool,
}

impl NegotiatedOutput {
    #[must_use]
    pub const fn profile(self) -> OutputProfile {
        self.profile
    }

    #[must_use]
    pub const fn hardware_video(self) -> bool {
        self.hardware_video
    }
}
