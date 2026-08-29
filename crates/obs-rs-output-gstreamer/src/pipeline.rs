use std::collections::BTreeSet;
#[cfg(feature = "native")]
use std::sync::OnceLock;

use obs_rs_output::{
    AudioCodec, AudioEncoderConfig, OutputAudioCodec, OutputProfile, OutputProfileKind,
    OutputVideoCodec, VideoCodec, VideoEncoderConfig,
};
use url::Url;

#[cfg(feature = "native")]
use super::capabilities::gst_inspect_command;
use super::capabilities::GStreamerCapabilitySnapshot;
use super::destination::ProductionDestination;
use super::{GStreamerError, MAX_PRODUCTION_METADATA_BYTES, PRODUCTION_METADATA_MAGIC};

/// Deterministic metadata used to create native appsrc/queue pipelines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductionPipelinePlan {
    pub(super) profile: OutputProfile,
    pub(super) video_encoder: String,
    pub(super) audio_encoder: String,
    pub(super) bounded_queue_bytes: usize,
    pub(super) atomic_recording: bool,
    pub(super) video_config: VideoEncoderConfig,
    pub(super) audio_config: AudioEncoderConfig,
    pub(super) rtmp_sink: Option<String>,
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
        if matches!(
            destination,
            ProductionDestination::SegmentedRecording { .. }
        ) && !capabilities.supports_segmented_recording()
        {
            return Err(GStreamerError::Native(
                "GStreamer splitmuxsink is unavailable for segmented recording".to_owned(),
            ));
        }
        if matches!(destination, ProductionDestination::RemuxRecording { .. })
            && !capabilities.supports_remux()
        {
            return Err(GStreamerError::Native(
                "GStreamer Matroska-to-MP4 remux is unavailable".to_owned(),
            ));
        }
        capabilities
            .output
            .negotiate(profile)
            .map_err(|_| GStreamerError::ProfileUnavailable(profile.kind()))?;
        let video_role = match profile.video_codec() {
            OutputVideoCodec::H264 => "h264",
            OutputVideoCodec::Hevc => "hevc",
            OutputVideoCodec::Av1 => "av1",
            OutputVideoCodec::Vp8 => "vp8",
            OutputVideoCodec::ReferenceRle => "reference",
        };
        let audio_role = if profile.kind() == OutputProfileKind::WebRtcVp8Opus {
            "opus"
        } else {
            "aac"
        };
        let video_encoder = capabilities
            .selected_element(video_role)
            .ok_or(GStreamerError::ProfileUnavailable(profile.kind()))?
            .to_owned();
        let audio_encoder = capabilities
            .selected_element(audio_role)
            .ok_or(GStreamerError::ProfileUnavailable(profile.kind()))?
            .to_owned();
        let mut video_config = VideoEncoderConfig {
            codec: profile_video_codec(profile.video_codec()),
            implementation: obs_rs_output::EncoderImplementation::new(&video_encoder),
            ..VideoEncoderConfig::default()
        };
        if video_config.codec != VideoCodec::H264 {
            video_config.profile = None;
        }
        let audio_config = AudioEncoderConfig {
            codec: profile_audio_codec(profile.audio_codec()),
            implementation: obs_rs_output::EncoderImplementation::new(&audio_encoder),
            ..AudioEncoderConfig::default()
        };
        Ok(Self {
            profile,
            video_encoder,
            audio_encoder,
            bounded_queue_bytes: profile.queue_bytes(),
            atomic_recording: matches!(
                destination,
                ProductionDestination::Recording(_)
                    | ProductionDestination::RemuxRecording { .. }
                    | ProductionDestination::SegmentedRecording { .. }
            ),
            video_config,
            audio_config,
            rtmp_sink: selected_rtmp_sink(profile, capabilities)?,
        })
    }

    /// Negotiates explicit encoder implementations and typed tuning.
    ///
    /// # Errors
    ///
    /// Rejects codec/profile mismatches and implementations absent from the
    /// runtime capability snapshot.
    pub fn negotiate_configured(
        profile: OutputProfile,
        destination: &ProductionDestination,
        capabilities: &GStreamerCapabilitySnapshot,
        video: &VideoEncoderConfig,
        audio: &AudioEncoderConfig,
    ) -> Result<Self, GStreamerError> {
        destination.validate_for(profile)?;
        if matches!(
            destination,
            ProductionDestination::SegmentedRecording { .. }
        ) && !capabilities.supports_segmented_recording()
        {
            return Err(GStreamerError::Native(
                "GStreamer splitmuxsink is unavailable for segmented recording".to_owned(),
            ));
        }
        if matches!(destination, ProductionDestination::RemuxRecording { .. })
            && !capabilities.supports_remux()
        {
            return Err(GStreamerError::Native(
                "GStreamer Matroska-to-MP4 remux is unavailable".to_owned(),
            ));
        }
        capabilities
            .output
            .negotiate(profile)
            .map_err(|_| GStreamerError::ProfileUnavailable(profile.kind()))?;
        if video.codec != profile_video_codec(profile.video_codec())
            || audio.codec != profile_audio_codec(profile.audio_codec())
        {
            return Err(GStreamerError::InvalidMetadata(
                "encoder codecs do not match the output profile".to_owned(),
            ));
        }
        if !(8_000..=192_000).contains(&audio.sample_rate)
            || !(1..=8).contains(&audio.channels)
            || audio.complexity.is_some_and(|complexity| complexity > 10)
        {
            return Err(GStreamerError::InvalidMetadata(
                "audio sample rate, channels, or complexity are out of range".to_owned(),
            ));
        }
        let video_capability = capabilities
            .video_encoders
            .iter()
            .find(|encoder| encoder.id() == video.implementation.id())
            .ok_or_else(|| {
                GStreamerError::InvalidMetadata("selected video encoder is unavailable".to_owned())
            })?;
        let audio_capability = capabilities
            .audio_encoders
            .iter()
            .find(|encoder| encoder.id() == audio.implementation.id())
            .ok_or_else(|| {
                GStreamerError::InvalidMetadata("selected audio encoder is unavailable".to_owned())
            })?;
        if video_capability.codec() != video.codec || audio_capability.codec() != audio.codec {
            return Err(GStreamerError::InvalidMetadata(
                "selected encoder implementation has the wrong codec".to_owned(),
            ));
        }
        Ok(Self {
            profile,
            video_encoder: video.implementation.id().to_owned(),
            audio_encoder: audio.implementation.id().to_owned(),
            bounded_queue_bytes: profile.queue_bytes(),
            atomic_recording: matches!(
                destination,
                ProductionDestination::Recording(_)
                    | ProductionDestination::RemuxRecording { .. }
                    | ProductionDestination::SegmentedRecording { .. }
            ),
            video_config: video.clone(),
            audio_config: audio.clone(),
            rtmp_sink: selected_rtmp_sink(profile, capabilities)?,
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

    #[must_use]
    pub const fn video_config(&self) -> &VideoEncoderConfig {
        &self.video_config
    }

    #[must_use]
    pub const fn audio_config(&self) -> &AudioEncoderConfig {
        &self.audio_config
    }

    #[must_use]
    pub fn rtmp_sink(&self) -> Option<&str> {
        self.rtmp_sink.as_deref()
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

pub(super) fn first_available(elements: &[&str]) -> Option<String> {
    first_matching(elements, element_available)
}

pub(super) fn first_matching(
    elements: &[&str],
    mut available: impl FnMut(&str) -> bool,
) -> Option<String> {
    elements
        .iter()
        .find(|element| available(element))
        .map(|element| (*element).to_owned())
}

fn selected_rtmp_sink(
    profile: OutputProfile,
    capabilities: &GStreamerCapabilitySnapshot,
) -> Result<Option<String>, GStreamerError> {
    if matches!(
        profile.kind(),
        OutputProfileKind::RtmpH264Aac | OutputProfileKind::RtmpsH264Aac
    ) {
        capabilities
            .selected_element("rtmp_sink")
            .map(|sink| Some(sink.to_owned()))
            .ok_or(GStreamerError::ProfileUnavailable(profile.kind()))
    } else {
        Ok(None)
    }
}

const fn profile_video_codec(codec: OutputVideoCodec) -> VideoCodec {
    match codec {
        OutputVideoCodec::H264 => VideoCodec::H264,
        OutputVideoCodec::Hevc => VideoCodec::Hevc,
        OutputVideoCodec::Av1 => VideoCodec::Av1,
        OutputVideoCodec::Vp8 => VideoCodec::Vp8,
        OutputVideoCodec::ReferenceRle => VideoCodec::ReferenceRle,
    }
}

const fn profile_audio_codec(codec: OutputAudioCodec) -> AudioCodec {
    match codec {
        OutputAudioCodec::Aac => AudioCodec::Aac,
        OutputAudioCodec::Opus => AudioCodec::Opus,
        OutputAudioCodec::Pcm => AudioCodec::Pcm,
    }
}

pub(super) fn element_available(element: &str) -> bool {
    #[cfg(feature = "native")]
    {
        return match element_catalog() {
            ElementCatalog::Listed(elements) => elements.contains(element),
            ElementCatalog::ProbeEach => gst_inspect_command()
                .args(["--exists", element])
                .status()
                .is_ok_and(|status| status.success()),
        };
    }

    #[cfg(not(feature = "native"))]
    {
        let _ = element;
        false
    }
}

/// The output adapter asks about the same small allow-list of elements several
/// times while choosing a profile. Running `gst-inspect-1.0 --exists` once per
/// element is especially expensive on Windows, where every child process also
/// has to load the packaged DLL search path. Prefer one registry listing and
/// keep the per-element command as a compatibility fallback for unusual
/// runtimes whose listing command fails.
#[cfg(feature = "native")]
enum ElementCatalog {
    Listed(BTreeSet<String>),
    ProbeEach,
}

#[cfg(feature = "native")]
fn element_catalog() -> &'static ElementCatalog {
    static CATALOG: OnceLock<ElementCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let Ok(output) = gst_inspect_command().output() else {
            return ElementCatalog::ProbeEach;
        };
        if !output.status.success() {
            return ElementCatalog::ProbeEach;
        }
        let elements = parse_element_names(&String::from_utf8_lossy(&output.stdout));
        if elements.is_empty() {
            ElementCatalog::ProbeEach
        } else {
            ElementCatalog::Listed(elements)
        }
    })
}

/// Parses the stable `plugin: element` rows emitted by `gst-inspect-1.0`
/// without accepting summary lines or arbitrary text as element names.
#[allow(dead_code)]
pub(super) fn parse_element_names(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim().splitn(3, ':');
            let _plugin = fields.next()?;
            let element = fields.next()?.trim();
            if is_element_name(element) {
                Some(element.to_owned())
            } else {
                None
            }
        })
        .collect()
}

#[allow(dead_code)]
fn is_element_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub(super) fn valid_url(value: &str, scheme: &str) -> bool {
    value.starts_with(scheme)
        && value.len() <= 2_048
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

pub(super) fn valid_stream_url(value: &str, scheme: &str, require_path: bool) -> bool {
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

pub(super) fn srt_passphrase_valid(endpoint: &str, explicit: Option<&str>) -> bool {
    if let Some(value) = explicit {
        return (10..=79).contains(&value.len());
    }
    Url::parse(endpoint).is_ok_and(|url| {
        url.query_pairs()
            .find(|(key, _)| key == "passphrase")
            .is_none_or(|(_, value)| (10..=79).contains(&value.len()))
    })
}
