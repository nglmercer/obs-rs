use std::{collections::BTreeMap, process::Command};

use obs_rs_output::{
    AudioCodec, EncoderPreset, OutputCapabilities, OutputProfileKind, RateControl, VideoCodec,
};

use super::pipeline::{element_available, first_available};

const H264_ENCODERS: &[&str] = &["vah264enc", "vaapih264enc", "nvh264enc", "openh264enc"];
const HEVC_ENCODERS: &[&str] = &[
    "vah265enc",
    "vaapih265enc",
    "nvh265enc",
    "x265enc",
    "svthevcenc",
];
const AV1_ENCODERS: &[&str] = &[
    "vaav1enc",
    "vaapiav1enc",
    "nvav1enc",
    "svtav1enc",
    "rav1enc",
    "av1enc",
    "aomenc",
];

/// Approved plugin selection, including hardware/software encoder choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GStreamerCapabilitySnapshot {
    runtime_version: Option<String>,
    selected_elements: BTreeMap<&'static str, String>,
    pub(super) output: OutputCapabilities,
    protocols: Vec<ProtocolCapability>,
    pub(super) video_encoders: Vec<VideoEncoderCapability>,
    pub(super) audio_encoders: Vec<AudioEncoderCapability>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductionProtocol {
    Reference,
    Rtmp,
    Rtmps,
    Srt,
    WebRtc,
    Matroska,
    Hls,
    Rist,
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
            Self::Hls => "hls",
            Self::Rist => "rist",
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
            Self::Hls => "HLS",
            Self::Rist => "RIST",
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
    recording_codecs: Vec<VideoCodec>,
    recording_formats: Vec<OutputProfileKind>,
    segmented_recording: bool,
    remux: bool,
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

    #[must_use]
    pub fn recording_codecs(&self) -> &[VideoCodec] {
        &self.recording_codecs
    }

    #[must_use]
    pub fn recording_formats(&self) -> &[OutputProfileKind] {
        &self.recording_formats
    }

    /// Reports whether the native bounded split-muxer boundary is available.
    #[must_use]
    pub const fn supports_segmented_recording(&self) -> bool {
        self.segmented_recording
    }

    /// Reports whether the native H.264/AAC Matroska-to-MP4 remux boundary is
    /// available.
    #[must_use]
    pub const fn supports_remux(&self) -> bool {
        self.remux
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
        let h264 = first_available(H264_ENCODERS);
        let hevc = first_available(HEVC_ENCODERS);
        let av1 = first_available(AV1_ENCODERS);
        let aac = first_available(&["avenc_aac"]);
        let vp8 = first_available(&["vp8enc"]);
        let opus = first_available(&["opusenc"]);
        let rtmp_sink = first_available(&["rtmp2sink", "rtmpsink"]);
        if let Some(value) = &h264 {
            selected.insert("h264", value.clone());
        }
        if let Some(value) = &hevc {
            selected.insert("hevc", value.clone());
        }
        if let Some(value) = &av1 {
            selected.insert("av1", value.clone());
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
        if let Some(value) = &rtmp_sink {
            selected.insert("rtmp_sink", value.clone());
        }

        let profiles = production_profiles(&selected);
        let hardware_h264 = h264
            .as_deref()
            .is_some_and(|encoder| matches!(encoder, "vah264enc" | "vaapih264enc" | "nvh264enc"));
        let output = OutputCapabilities::approved(profiles, hardware_h264);
        let protocols = protocol_capabilities(&output);
        let video_encoders = H264_ENCODERS
            .iter()
            .chain(HEVC_ENCODERS)
            .chain(AV1_ENCODERS)
            .copied()
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

    /// Reports whether the native split-muxer boundary is available.
    #[must_use]
    pub fn supports_segmented_recording(&self) -> bool {
        self.runtime_version.is_some() && element_available("splitmuxsink")
    }

    /// Reports whether the approved native elements can remux the production
    /// Matroska profile without decoding or re-encoding media.
    #[must_use]
    pub fn supports_remux(&self) -> bool {
        self.runtime_version.is_some()
            && [
                "filesrc",
                "matroskademux",
                "h264parse",
                "aacparse",
                "mp4mux",
                "filesink",
            ]
            .into_iter()
            .chain(["queue"])
            .all(element_available)
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
            recording_codecs: [
                (OutputProfileKind::MatroskaH264Aac, VideoCodec::H264),
                (OutputProfileKind::Mp4H264Aac, VideoCodec::H264),
                (OutputProfileKind::FragmentedMp4H264Aac, VideoCodec::H264),
                (OutputProfileKind::MovH264Aac, VideoCodec::H264),
                (OutputProfileKind::FlvH264Aac, VideoCodec::H264),
                (OutputProfileKind::MatroskaHevcAac, VideoCodec::Hevc),
                (OutputProfileKind::MatroskaAv1Aac, VideoCodec::Av1),
            ]
            .into_iter()
            .filter_map(|(profile, codec)| self.output.supports(profile).then_some(codec))
            .fold(Vec::new(), |mut codecs, codec| {
                if !codecs.contains(&codec) {
                    codecs.push(codec);
                }
                codecs
            }),
            recording_formats: [
                OutputProfileKind::MatroskaH264Aac,
                OutputProfileKind::MatroskaHevcAac,
                OutputProfileKind::MatroskaAv1Aac,
                OutputProfileKind::Mp4H264Aac,
                OutputProfileKind::FragmentedMp4H264Aac,
                OutputProfileKind::MovH264Aac,
                OutputProfileKind::FlvH264Aac,
            ]
            .into_iter()
            .filter(|profile| self.output.supports(*profile))
            .collect(),
            segmented_recording: self.supports_segmented_recording(),
            remux: self.supports_remux(),
        }
    }
}

fn production_profiles(selected: &BTreeMap<&'static str, String>) -> Vec<OutputProfileKind> {
    let has = |role| selected.contains_key(role);
    let mut profiles = Vec::new();
    if has("h264") && has("aac") && element_available("matroskamux") {
        profiles.push(OutputProfileKind::MatroskaH264Aac);
    }
    if has("hevc")
        && has("aac")
        && element_available("h265parse")
        && element_available("matroskamux")
    {
        profiles.push(OutputProfileKind::MatroskaHevcAac);
    }
    if has("av1") && has("aac") && element_available("av1parse") && element_available("matroskamux")
    {
        profiles.push(OutputProfileKind::MatroskaAv1Aac);
    }
    if has("h264") && has("aac") && element_available("mp4mux") {
        profiles.extend([
            OutputProfileKind::Mp4H264Aac,
            OutputProfileKind::FragmentedMp4H264Aac,
        ]);
    }
    if has("h264") && has("aac") && element_available("qtmux") {
        profiles.push(OutputProfileKind::MovH264Aac);
    }
    if has("h264") && has("aac") && element_available("flvmux") {
        profiles.push(OutputProfileKind::FlvH264Aac);
    }
    if has("h264") && has("aac") && element_available("flvmux") && has("rtmp_sink") {
        profiles.extend([
            OutputProfileKind::RtmpH264Aac,
            OutputProfileKind::RtmpsH264Aac,
        ]);
    }
    if has("h264") && has("aac") && element_available("mpegtsmux") && element_available("srtsink") {
        profiles.push(OutputProfileKind::SrtMpegTsH264Aac);
    }
    if has("vp8")
        && has("opus")
        && element_available("webrtcbin")
        && element_available("whipclientsink")
    {
        profiles.push(OutputProfileKind::WebRtcVp8Opus);
    }
    if has("h264") && has("aac") && element_available("hlssink2") {
        profiles.push(OutputProfileKind::HlsH264Aac);
    }
    if has("h264")
        && has("aac")
        && element_available("mpegtsmux")
        && element_available("rtpmp2tpay")
        && element_available("ristsink")
    {
        profiles.push(OutputProfileKind::RistMpegTsH264Aac);
    }
    profiles
}

fn unavailable_protocols() -> Vec<ProtocolCapability> {
    [
        ProductionProtocol::Reference,
        ProductionProtocol::Rtmp,
        ProductionProtocol::Rtmps,
        ProductionProtocol::Srt,
        ProductionProtocol::WebRtc,
        ProductionProtocol::Matroska,
        ProductionProtocol::Hls,
        ProductionProtocol::Rist,
    ]
    .into_iter()
    .map(|protocol| ProtocolCapability {
        available: protocol == ProductionProtocol::Reference,
        protocol,
    })
    .collect()
}

pub(super) fn protocol_capabilities(output: &OutputCapabilities) -> Vec<ProtocolCapability> {
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
        (ProductionProtocol::Hls, OutputProfileKind::HlsH264Aac),
        (
            ProductionProtocol::Rist,
            OutputProfileKind::RistMpegTsH264Aac,
        ),
    ]
    .into_iter()
    .map(|(protocol, profile)| ProtocolCapability {
        protocol,
        available: if protocol == ProductionProtocol::Matroska {
            [
                OutputProfileKind::MatroskaH264Aac,
                OutputProfileKind::MatroskaHevcAac,
                OutputProfileKind::MatroskaAv1Aac,
            ]
            .into_iter()
            .any(|candidate| output.supports(candidate))
        } else {
            output.supports(profile)
        },
    })
    .collect()
}

pub(super) fn video_encoder_capability(element: &str) -> VideoEncoderCapability {
    let (display_name, codec, hardware) = match element {
        "vah264enc" => ("VA H.264", VideoCodec::H264, true),
        "vaapih264enc" => ("VA-API H.264", VideoCodec::H264, true),
        "nvh264enc" => ("NVIDIA NVENC H.264", VideoCodec::H264, true),
        "openh264enc" => ("OpenH264", VideoCodec::H264, false),
        "vah265enc" => ("VA HEVC", VideoCodec::Hevc, true),
        "vaapih265enc" => ("VA-API HEVC", VideoCodec::Hevc, true),
        "nvh265enc" => ("NVIDIA NVENC HEVC", VideoCodec::Hevc, true),
        "x265enc" => ("x265 HEVC", VideoCodec::Hevc, false),
        "svthevcenc" => ("SVT-HEVC", VideoCodec::Hevc, false),
        "vaav1enc" => ("VA AV1", VideoCodec::Av1, true),
        "vaapiav1enc" => ("VA-API AV1", VideoCodec::Av1, true),
        "nvav1enc" => ("NVIDIA NVENC AV1", VideoCodec::Av1, true),
        "svtav1enc" => ("SVT-AV1", VideoCodec::Av1, false),
        "rav1enc" => ("rav1e AV1", VideoCodec::Av1, false),
        "av1enc" | "aomenc" => ("AOM AV1", VideoCodec::Av1, false),
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

pub(super) fn encoder_option_capabilities(element: &str) -> VideoEncoderOptionCapabilities {
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

pub(super) fn property_present(inspection: &str, property: &str) -> bool {
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
