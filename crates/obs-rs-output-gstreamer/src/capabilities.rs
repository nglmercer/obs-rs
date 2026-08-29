use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

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

/// Return the capability-probe executable from an explicit override, the
/// packaged runtime beside the current executable, or `PATH`.
///
/// The native adapter uses `gst-inspect-1.0` for its allow-listed element
/// probe. Packaged Windows builds must therefore be able to find the exact
/// tool that belongs to the bundled runtime instead of accidentally probing a
/// different system installation.
pub(super) fn gst_inspect_command() -> Command {
    configure_bundled_runtime();
    if let Some(path) = std::env::var_os("OBSR_GST_INSPECT") {
        if !path.is_empty() {
            return Command::new(path);
        }
    }

    let executable_name = if cfg!(target_os = "windows") {
        "gst-inspect-1.0.exe"
    } else {
        "gst-inspect-1.0"
    };
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            for bundled in [
                parent.join(executable_name),
                parent.join("gstreamer").join("bin").join(executable_name),
            ] {
                if bundled.is_file() {
                    return Command::new(bundled);
                }
            }
        }
    }
    Command::new(executable_name)
}

/// Makes a packaged runtime usable when the GUI executable is launched
/// directly instead of through `run-obs-rs.ps1`.
///
/// The package keeps `GStreamer` below `gstreamer/` while the entry points stay
/// at the archive root. `GStreamer` otherwise has no reliable way to infer the
/// plugin and scanner locations from that layout. This setup is intentionally
/// a no-op for source-tree and reference builds where the bundled directories
/// do not exist.
pub(super) fn configure_bundled_runtime() {
    let Some(executable_directory) = current_executable_directory() else {
        return;
    };
    let runtime = executable_directory.join("gstreamer");
    let runtime_bin = runtime.join("bin");
    let plugins = runtime.join("lib").join("gstreamer-1.0");
    let scanner =
        runtime
            .join("libexec")
            .join("gstreamer-1.0")
            .join(if cfg!(target_os = "windows") {
                "gst-plugin-scanner.exe"
            } else {
                "gst-plugin-scanner"
            });
    if !runtime_bin.is_dir() || !plugins.is_dir() || !scanner.is_file() {
        return;
    }

    prepend_environment_paths(
        "PATH",
        &[runtime_bin.as_path(), executable_directory.as_path()],
    );
    prepend_environment_paths("GST_PLUGIN_PATH", &[plugins.as_path()]);
    prepend_environment_paths("GST_PLUGIN_PATH_1_0", &[plugins.as_path()]);

    if env::var_os("GST_PLUGIN_SCANNER")
        .as_deref()
        .is_none_or(|path| !Path::new(path).is_file())
    {
        env::set_var("GST_PLUGIN_SCANNER", &scanner);
    }
    let registry = runtime.join("registry.bin");
    if env::var_os("GST_REGISTRY").is_none() {
        env::set_var("GST_REGISTRY", registry);
    }
    let inspect = runtime_bin.join(if cfg!(target_os = "windows") {
        "gst-inspect-1.0.exe"
    } else {
        "gst-inspect-1.0"
    });
    if env::var_os("OBSR_GST_INSPECT").is_none() && inspect.is_file() {
        env::set_var("OBSR_GST_INSPECT", inspect);
    }
}

fn current_executable_directory() -> Option<PathBuf> {
    env::current_exe().ok()?.parent().map(Path::to_owned)
}

fn prepend_environment_paths(name: &str, additions: &[&Path]) {
    let existing = env::var_os(name)
        .as_deref()
        .map(env::split_paths)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut paths = additions
        .iter()
        .filter(|addition| !existing.iter().any(|path| path == **addition))
        .map(|path| (*path).to_owned())
        .collect::<Vec<_>>();
    paths.extend(existing);
    if let Ok(value) = env::join_paths(paths) {
        env::set_var(name, value);
    }
}

/// Approved plugin selection, including hardware/software encoder choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GStreamerCapabilitySnapshot {
    runtime_version: Option<String>,
    runtime_probe_error: Option<String>,
    selected_elements: BTreeMap<&'static str, String>,
    pub(super) output: OutputCapabilities,
    protocols: Vec<ProtocolCapability>,
    pub(super) video_encoders: Vec<VideoEncoderCapability>,
    pub(super) audio_encoders: Vec<AudioEncoderCapability>,
    segmented_recording: bool,
    remux: bool,
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

/// Explains why encoded production output is or is not available.
///
/// The portable reference output remains usable in every state. The status is
/// deliberately separate from [`OutputCapabilitiesSnapshot::supports_production_output`]
/// so the UI and diagnostics can distinguish a build-time omission from a
/// missing runtime or an incomplete plugin installation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductionOutputStatus {
    /// This binary was built without the optional native adapter.
    NativeAdapterNotCompiled,
    /// The native adapter exists, but its matching `GStreamer` runtime could not
    /// be started or identified.
    RuntimeUnavailable,
    /// The runtime started, but no approved encoded profile was discovered.
    NoUsableProfile,
    /// At least one approved recording or streaming profile is available.
    Ready,
}

impl ProductionOutputStatus {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::NativeAdapterNotCompiled => "native-adapter-not-compiled",
            Self::RuntimeUnavailable => "runtime-unavailable",
            Self::NoUsableProfile => "no-usable-profile",
            Self::Ready => "ready",
        }
    }

    #[must_use]
    pub const fn ready(self) -> bool {
        matches!(self, Self::Ready)
    }
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
    /// Version reported by the native runtime used to probe the approved
    /// elements. `None` means the native adapter is not compiled or the
    /// runtime could not be started.
    runtime_version: Option<String>,
    /// Whether this binary contains the optional native `GStreamer` adapter.
    native_adapter_compiled: bool,
    /// Bounded stderr/launch detail from the runtime probe, when it failed.
    runtime_probe_error: Option<String>,
    protocols: Vec<ProtocolCapability>,
    video_encoders: Vec<VideoEncoderCapability>,
    audio_encoders: Vec<AudioEncoderCapability>,
    recording_codecs: Vec<VideoCodec>,
    recording_formats: Vec<OutputProfileKind>,
    segmented_recording: bool,
    remux: bool,
}

impl OutputCapabilitiesSnapshot {
    /// Returns the native runtime version used for capability discovery.
    #[must_use]
    pub fn native_runtime_version(&self) -> Option<&str> {
        self.runtime_version.as_deref()
    }

    /// Returns whether the optional native adapter was compiled into this
    /// binary.
    #[must_use]
    pub const fn native_adapter_compiled(&self) -> bool {
        self.native_adapter_compiled
    }

    /// Returns the bounded runtime launch detail, if probing failed.
    #[must_use]
    pub fn native_runtime_probe_error(&self) -> Option<&str> {
        self.runtime_probe_error.as_deref()
    }

    /// Returns the reasoned production-output state.
    #[must_use]
    pub fn production_status(&self) -> ProductionOutputStatus {
        if !self.native_adapter_compiled {
            ProductionOutputStatus::NativeAdapterNotCompiled
        } else if self.runtime_version.is_none() {
            ProductionOutputStatus::RuntimeUnavailable
        } else if self.supports_production_output() {
            ProductionOutputStatus::Ready
        } else {
            ProductionOutputStatus::NoUsableProfile
        }
    }

    /// Returns a bounded, operator-facing explanation suitable for diagnostics
    /// and a settings hint. It contains no endpoint or credential data.
    #[must_use]
    pub fn production_status_detail(&self) -> String {
        let video_encoders = self.video_encoders.len();
        let audio_encoders = self.audio_encoders.len();
        let recording_formats = self.recording_formats.len();
        let production_protocols = self
            .protocols
            .iter()
            .filter(|capability| {
                capability.available() && capability.protocol() != ProductionProtocol::Reference
            })
            .count();
        let detail = match self.production_status() {
            ProductionOutputStatus::NativeAdapterNotCompiled => {
                "native GStreamer adapter is not compiled; only the portable reference output is available".to_owned()
            }
            ProductionOutputStatus::RuntimeUnavailable => {
                let reason = self
                    .native_runtime_probe_error()
                    .unwrap_or("gst-inspect-1.0 could not be started");
                format!(
                    "native GStreamer adapter is compiled, but the runtime is unavailable: {reason}"
                )
            }
            ProductionOutputStatus::NoUsableProfile => format!(
                "GStreamer runtime is available, but no approved production profile was found (video_encoders={video_encoders}, audio_encoders={audio_encoders}, recording_formats={recording_formats}, production_protocols={production_protocols})"
            ),
            ProductionOutputStatus::Ready => format!(
                "GStreamer runtime {} is ready (video_encoders={video_encoders}, audio_encoders={audio_encoders}, recording_formats={recording_formats}, production_protocols={production_protocols})",
                self.native_runtime_version().unwrap_or("unknown")
            ),
        };
        bounded_probe_detail(&detail)
    }

    /// Returns whether at least one approved production profile is usable.
    ///
    /// The reference protocol is intentionally excluded: it is the portable
    /// development/test path and does not produce a normal encoded media
    /// output.
    #[must_use]
    pub fn supports_production_output(&self) -> bool {
        self.protocols.iter().any(|capability| {
            capability.available() && capability.protocol() != ProductionProtocol::Reference
        }) || !self.recording_formats.is_empty()
    }

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
                runtime_probe_error: None,
                selected_elements: BTreeMap::new(),
                output: OutputCapabilities::reference_only(),
                protocols: unavailable_protocols(),
                video_encoders: Vec::new(),
                audio_encoders: Vec::new(),
                segmented_recording: false,
                remux: false,
            };
        }
        let runtime_probe = probe_runtime_version();
        let (runtime_version, runtime_probe_error) = match runtime_probe {
            Ok(version) => (Some(version), None),
            Err(error) => (None, Some(error)),
        };
        let Some(runtime_version) = runtime_version else {
            return Self {
                runtime_version: None,
                runtime_probe_error,
                selected_elements: BTreeMap::new(),
                output: OutputCapabilities::reference_only(),
                protocols: unavailable_protocols(),
                video_encoders: Vec::new(),
                audio_encoders: Vec::new(),
                segmented_recording: false,
                remux: false,
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
        let segmented_recording = element_available("splitmuxsink");
        let remux = remux_elements_available();
        Self {
            runtime_version: Some(runtime_version),
            runtime_probe_error: None,
            selected_elements: selected,
            output,
            protocols,
            video_encoders,
            audio_encoders,
            segmented_recording,
            remux,
        }
    }

    /// Probes the approved runtime once per process and returns cloned typed
    /// snapshots to later callers. Packaged runtimes are immutable for the
    /// lifetime of a process, and avoiding another dozen `gst-inspect` child
    /// processes on every output start keeps recovery off the UI path.
    #[must_use]
    pub fn probe_cached() -> Self {
        static CACHE: OnceLock<GStreamerCapabilitySnapshot> = OnceLock::new();
        CACHE.get_or_init(Self::probe).clone()
    }

    /// Reports whether the native split-muxer boundary is available.
    #[must_use]
    pub fn supports_segmented_recording(&self) -> bool {
        self.segmented_recording
    }

    /// Reports whether the approved native elements can remux the production
    /// Matroska profile without decoding or re-encoding media.
    #[must_use]
    pub fn supports_remux(&self) -> bool {
        self.remux
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
            runtime_version: self.runtime_version.clone(),
            native_adapter_compiled: cfg!(feature = "native"),
            runtime_probe_error: self.runtime_probe_error.clone(),
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

fn probe_runtime_version() -> Result<String, String> {
    let output = gst_inspect_command()
        .arg("--version")
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = bounded_probe_detail(&stderr);
        return Err(if stderr.is_empty() {
            format!(
                "gst-inspect-1.0 exited with {}",
                output
                    .status
                    .code()
                    .map_or_else(|| "a signal".to_owned(), |code| code.to_string())
            )
        } else {
            stderr
        });
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version.is_empty() {
        return Err("gst-inspect-1.0 returned an empty version".to_owned());
    }
    Ok(version)
}

fn bounded_probe_detail(value: &str) -> String {
    value
        .chars()
        .take(256)
        .map(|character| match character {
            '\r' | '\n' | '\t' => ' ',
            other => other,
        })
        .collect()
}

fn remux_elements_available() -> bool {
    [
        "filesrc",
        "matroskademux",
        "h264parse",
        "aacparse",
        "mp4mux",
        "filesink",
        "queue",
    ]
    .into_iter()
    .all(element_available)
}

fn production_profiles(selected: &BTreeMap<&'static str, String>) -> Vec<OutputProfileKind> {
    production_profiles_with(selected, element_available)
}

pub(super) fn production_profiles_with(
    selected: &BTreeMap<&'static str, String>,
    mut available: impl FnMut(&str) -> bool,
) -> Vec<OutputProfileKind> {
    let has = |role| selected.contains_key(role);
    // Every native graph starts with these bounded appsrc conversion stages.
    // If one is absent, reporting a codec/container as available only moves
    // the failure from capability discovery to output start.
    if ![
        "appsrc",
        "queue",
        "videoconvert",
        "audioconvert",
        "audioresample",
    ]
    .into_iter()
    .all(&mut available)
    {
        return Vec::new();
    }
    let filesink = available("filesink");
    let h264_parser = has("h264") && available("h264parse");
    let hevc_parser = has("hevc") && available("h265parse");
    let av1_parser = has("av1") && available("av1parse");
    let mut profiles = Vec::new();
    if h264_parser && has("aac") && filesink && available("matroskamux") {
        profiles.push(OutputProfileKind::MatroskaH264Aac);
    }
    if hevc_parser && has("aac") && filesink && available("matroskamux") {
        profiles.push(OutputProfileKind::MatroskaHevcAac);
    }
    if av1_parser && has("aac") && filesink && available("matroskamux") {
        profiles.push(OutputProfileKind::MatroskaAv1Aac);
    }
    if h264_parser && has("aac") && filesink && available("mp4mux") {
        profiles.extend([
            OutputProfileKind::Mp4H264Aac,
            OutputProfileKind::FragmentedMp4H264Aac,
        ]);
    }
    if h264_parser && has("aac") && filesink && available("qtmux") {
        profiles.push(OutputProfileKind::MovH264Aac);
    }
    if h264_parser && has("aac") && filesink && available("flvmux") {
        profiles.push(OutputProfileKind::FlvH264Aac);
    }
    if h264_parser && has("aac") && available("flvmux") && has("rtmp_sink") {
        profiles.extend([
            OutputProfileKind::RtmpH264Aac,
            OutputProfileKind::RtmpsH264Aac,
        ]);
    }
    if h264_parser && has("aac") && available("mpegtsmux") && available("srtsink") {
        profiles.push(OutputProfileKind::SrtMpegTsH264Aac);
    }
    if has("vp8") && has("opus") && available("webrtcbin") && available("whipclientsink") {
        profiles.push(OutputProfileKind::WebRtcVp8Opus);
    }
    if h264_parser && has("aac") && available("hlssink2") {
        profiles.push(OutputProfileKind::HlsH264Aac);
    }
    if h264_parser
        && has("aac")
        && available("mpegtsmux")
        && available("rtpmp2tpay")
        && available("ristsink")
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
    let inspection = gst_inspect_command()
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
