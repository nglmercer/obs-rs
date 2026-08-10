//! Bounded subprocess extensions for OBS-RS.
//!
//! The first plugin contract is intentionally in-process and compile-time. This
//! crate adds the next safe boundary: a separately started process emits validated
//! `OBSFRM01` RGBA packets on stdout. The host never loads foreign code or shares
//! pointers with the child. Command arguments are passed directly to
//! [`std::process::Command`]; no shell is involved.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    fmt,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use obs_rs_capture::{
    CaptureDeviceInfo, CaptureError, CaptureKind, CapturePermission, StreamCaptureDevice,
    VideoCaptureDevice,
};
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, MediaError, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::{
    Plugin, PluginApiVersion, PluginError, PluginManifest, Source, SourceError, SourceFactory,
    VideoRequest,
};
use obs_rs_util::Identifier;

/// Header for the bounded line-oriented sandbox manifest.
pub const SANDBOX_MANIFEST_MAGIC: &str = "OBSRPLUGIN1";
/// Maximum UTF-8 size accepted for one sandbox manifest.
pub const MAX_SANDBOX_MANIFEST_BYTES: usize = 32 * 1024;
/// Maximum source kinds declared by one sandbox plugin.
pub const MAX_SANDBOX_SOURCE_KINDS: usize = 64;
/// Maximum command arguments accepted by one sandbox plugin.
pub const MAX_SANDBOX_ARGUMENTS: usize = 64;
/// Maximum UTF-8 size accepted for one command argument.
pub const MAX_SANDBOX_ARGUMENT_BYTES: usize = 4 * 1024;
/// Maximum number of decoded frames allowed to wait between the sandbox
/// process and the render thread.
pub const MAX_SANDBOX_QUEUED_FRAMES: usize = 2;
/// Maximum time a render request waits for a sandbox frame.
pub const SANDBOX_FRAME_DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);
/// Argument used by a subprocess manifest probe.
pub const SANDBOX_MANIFEST_ARGUMENT: &str = "--obs-rs-manifest";

type FrameResult = Result<Option<VideoFrame>, CaptureError>;
type FrameReceiver = Receiver<FrameResult>;
type FrameReader = JoinHandle<()>;

/// Errors raised while validating or running a sandbox extension.
#[derive(Debug, Eq, PartialEq)]
pub enum SandboxError {
    /// The manifest exceeds the bounded parser input.
    ManifestTooLarge,
    /// The manifest is malformed or violates an extension limit.
    InvalidManifest { reason: String },
    /// The executable path is empty or otherwise invalid for a direct process
    /// launch.
    InvalidCommand { reason: String },
    /// A command argument exceeds the configured count or byte bound.
    InvalidArguments { reason: String },
    /// The plugin API version is incompatible.
    Plugin(PluginError),
    /// A child process or frame stream failed.
    Capture(CaptureError),
    /// A media value failed validation.
    Media(MediaError),
}

impl fmt::Display for SandboxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManifestTooLarge => formatter.write_str("sandbox manifest is too large"),
            Self::InvalidManifest { reason } => {
                write!(formatter, "invalid sandbox manifest: {reason}")
            }
            Self::InvalidCommand { reason } => {
                write!(formatter, "invalid sandbox command: {reason}")
            }
            Self::InvalidArguments { reason } => {
                write!(formatter, "invalid sandbox arguments: {reason}")
            }
            Self::Plugin(error) => error.fmt(formatter),
            Self::Capture(error) => error.fmt(formatter),
            Self::Media(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SandboxError {}

impl From<CaptureError> for SandboxError {
    fn from(error: CaptureError) -> Self {
        Self::Capture(error)
    }
}

impl From<MediaError> for SandboxError {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

impl From<PluginError> for SandboxError {
    fn from(error: PluginError) -> Self {
        Self::Plugin(error)
    }
}

/// A validated manifest for a subprocess plugin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxedPluginManifest {
    manifest: PluginManifest,
    source_kinds: Vec<Identifier>,
}

impl SandboxedPluginManifest {
    /// Creates a manifest from an existing plugin manifest and source kinds.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError::InvalidManifest`] when no source kinds are
    /// supplied, a kind is duplicated, or the kind limit is exceeded.
    pub fn new(
        manifest: PluginManifest,
        source_kinds: impl IntoIterator<Item = Identifier>,
    ) -> Result<Self, SandboxError> {
        let source_kinds = source_kinds.into_iter().collect::<Vec<_>>();
        validate_source_kinds(&source_kinds)?;
        Ok(Self {
            manifest,
            source_kinds,
        })
    }

    /// Parses the bounded manifest format:
    ///
    /// ```text
    /// OBSRPLUGIN1
    /// plugin_id|display name|version|major|minor|source_kind,other_kind
    /// ```
    ///
    /// Names and versions must not contain `|`; source kinds use the shared
    /// identifier alphabet. The parser rejects trailing records so a caller can
    /// safely treat one document as one manifest.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError`] when the document is oversized, malformed, or
    /// incompatible with the current plugin API.
    pub fn parse(document: &str) -> Result<Self, SandboxError> {
        if document.len() > MAX_SANDBOX_MANIFEST_BYTES {
            return Err(SandboxError::ManifestTooLarge);
        }
        let mut lines = document.lines();
        if lines.next() != Some(SANDBOX_MANIFEST_MAGIC) {
            return Err(invalid_manifest("invalid sandbox manifest header"));
        }
        let record = lines
            .next()
            .ok_or_else(|| invalid_manifest("missing plugin record"))?;
        if lines.next().is_some() {
            return Err(invalid_manifest("trailing manifest records"));
        }
        let fields = record.split('|').collect::<Vec<_>>();
        if fields.len() != 6 || fields.iter().any(|field| field.is_empty()) {
            return Err(invalid_manifest("plugin record needs six non-empty fields"));
        }
        let major = fields[3]
            .parse::<u16>()
            .map_err(|_| invalid_manifest("API major version is invalid"))?;
        let minor = fields[4]
            .parse::<u16>()
            .map_err(|_| invalid_manifest("API minor version is invalid"))?;
        let manifest = PluginManifest::with_api_version(
            fields[0],
            fields[1],
            fields[2],
            PluginApiVersion::new(major, minor),
        )?;
        let source_kinds = fields[5]
            .split(',')
            .map(Identifier::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid_manifest(format!("source kind is invalid: {error}")))?;
        Self::new(manifest, source_kinds)
    }

    /// Returns the validated plugin metadata.
    #[must_use]
    pub const fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Returns source kinds in manifest order.
    #[must_use]
    pub fn source_kinds(&self) -> &[Identifier] {
        &self.source_kinds
    }

    /// Serializes the manifest deterministically.
    #[must_use]
    pub fn serialize(&self) -> String {
        let api = self.manifest.api_version();
        format!(
            "{SANDBOX_MANIFEST_MAGIC}\n{}|{}|{}|{}|{}|{}\n",
            self.manifest.id(),
            self.manifest.name(),
            self.manifest.version(),
            api.major(),
            api.minor(),
            self.source_kinds
                .iter()
                .map(Identifier::as_str)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

/// Probes one extension process for a bounded, versioned manifest.
///
/// The command is launched directly with `arguments` followed by
/// [`SANDBOX_MANIFEST_ARGUMENT`]. The child must write exactly one serialized
/// [`SandboxedPluginManifest`] to stdout and exit successfully. The probe has
/// the same byte and time bounds as source delivery, and stderr is discarded.
///
/// # Errors
///
/// Returns [`SandboxError`] when launch, timeout, output, or manifest validation
/// fails.
pub fn discover_sandbox_manifest(
    command: impl AsRef<Path>,
    arguments: &[String],
) -> Result<SandboxedPluginManifest, SandboxError> {
    let command = command.as_ref().to_owned();
    let mut probe_arguments = arguments.to_vec();
    probe_arguments.push(SANDBOX_MANIFEST_ARGUMENT.to_owned());
    validate_command(&command, &probe_arguments)?;

    let mut child = Command::new(&command)
        .args(&probe_arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| SandboxError::InvalidCommand {
            reason: error.to_string(),
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        SandboxError::InvalidCommand {
            reason: "manifest probe did not expose stdout".to_owned(),
        }
    })?;
    let (sender, receiver) = sync_channel(1);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let read_result = stdout
            .take((MAX_SANDBOX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .map(|_| output)
            .map_err(|error| error.to_string());
        let _ = sender.send(read_result);
    });

    let read_result = match receiver.recv_timeout(SANDBOX_FRAME_DELIVERY_TIMEOUT) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(SandboxError::InvalidCommand {
                reason: format!(
                    "manifest probe did not finish within {SANDBOX_FRAME_DELIVERY_TIMEOUT:?}"
                ),
            });
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(SandboxError::InvalidCommand {
                reason: "manifest probe reader disconnected".to_owned(),
            });
        }
    };
    let output = match read_result {
        Ok(output) => output,
        Err(reason) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(SandboxError::InvalidCommand { reason });
        }
    };
    if output.len() > MAX_SANDBOX_MANIFEST_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return Err(SandboxError::ManifestTooLarge);
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            let _ = child.kill();
            let _ = reader.join();
            return Err(SandboxError::InvalidCommand {
                reason: error.to_string(),
            });
        }
    };
    let _ = reader.join();
    if !status.success() {
        return Err(SandboxError::InvalidCommand {
            reason: format!("manifest probe exited with {status}"),
        });
    }
    let document = String::from_utf8(output)
        .map_err(|_| invalid_manifest("manifest probe output is not UTF-8"))?;
    SandboxedPluginManifest::parse(&document)
}

/// A compile-time-safe host for source factories backed by a child process.
pub struct SandboxedPlugin {
    manifest: PluginManifest,
    command: PathBuf,
    arguments: Vec<String>,
    factories: Vec<Arc<dyn SourceFactory>>,
}

impl SandboxedPlugin {
    /// Discovers and configures a subprocess plugin from its manifest probe.
    ///
    /// # Errors
    ///
    /// Propagates manifest-probe or direct-launch policy errors.
    pub fn from_process(
        command: impl AsRef<Path>,
        arguments: Vec<String>,
    ) -> Result<Self, SandboxError> {
        let manifest = discover_sandbox_manifest(command.as_ref(), &arguments)?;
        Self::new(&manifest, command.as_ref(), arguments)
    }

    /// Creates a subprocess plugin without invoking the command.
    ///
    /// The executable is launched directly for each source instance. The child
    /// must write consecutive `OBSFRM01` packets to stdout; stdout is bounded by
    /// the capture decoder and stderr is discarded so an extension cannot corrupt
    /// the media stream with log text.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError`] when the command or argument policy is invalid.
    pub fn new(
        manifest: &SandboxedPluginManifest,
        command: impl Into<PathBuf>,
        arguments: Vec<String>,
    ) -> Result<Self, SandboxError> {
        let command = command.into();
        validate_command(&command, &arguments)?;
        let factories = manifest
            .source_kinds()
            .iter()
            .cloned()
            .map(|kind| {
                Arc::new(ProcessSourceFactory {
                    kind,
                    command: command.clone(),
                    arguments: arguments.clone(),
                }) as Arc<dyn SourceFactory>
            })
            .collect();
        Ok(Self {
            manifest: manifest.manifest().clone(),
            command,
            arguments,
            factories,
        })
    }

    /// Returns the executable selected for this plugin.
    #[must_use]
    pub fn command(&self) -> &Path {
        &self.command
    }

    /// Returns the fixed argument vector passed to every source process.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
}

impl Plugin for SandboxedPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn source_factories(&self) -> Vec<Arc<dyn SourceFactory>> {
        self.factories.clone()
    }
}

struct ProcessSourceFactory {
    kind: Identifier,
    command: PathBuf,
    arguments: Vec<String>,
}

impl SourceFactory for ProcessSourceFactory {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn create(&self, name: &str, settings: &Config) -> Result<Box<dyn Source>, SourceError> {
        let format = parse_video_format(settings)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        let device = ProcessFrameDevice::new(
            name,
            self.kind.clone(),
            self.command.clone(),
            self.arguments.clone(),
            settings.clone(),
        )
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        Ok(Box::new(ProcessSource {
            kind: self.kind.clone(),
            name: name.to_owned(),
            command: self.command.clone(),
            arguments: self.arguments.clone(),
            settings: settings.clone(),
            format,
            device,
        }))
    }
}

struct ProcessSource {
    kind: Identifier,
    name: String,
    command: PathBuf,
    arguments: Vec<String>,
    settings: Config,
    format: VideoFormat,
    device: ProcessFrameDevice,
}

impl Source for ProcessSource {
    fn kind(&self) -> &Identifier {
        &self.kind
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, settings: &Config) -> Result<(), SourceError> {
        let format = parse_video_format(settings)
            .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        let mut replacement = ProcessFrameDevice::new(
            &self.name,
            self.kind.clone(),
            self.command.clone(),
            self.arguments.clone(),
            settings.clone(),
        )
        .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        if self.device.is_running() {
            replacement
                .start(format)
                .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        }
        self.device = replacement;
        self.settings = settings.clone();
        self.format = format;
        Ok(())
    }

    fn render(&mut self, request: &VideoRequest) -> Result<Option<VideoFrame>, SourceError> {
        if request.format() != self.format {
            return Err(SourceError::UnsupportedFormat {
                configured: self.format,
                requested: request.format(),
            });
        }
        if !self.device.is_running() {
            self.device
                .start(self.format)
                .map_err(|error| SourceError::Unavailable(error.to_string()))?;
        }
        self.device
            .next_frame(request.timestamp())
            .map_err(|error| SourceError::Unavailable(error.to_string()))
    }
}

struct ProcessFrameDevice {
    info: CaptureDeviceInfo,
    kind: Identifier,
    command: PathBuf,
    arguments: Vec<String>,
    settings: Config,
    child: Option<Child>,
    frames: Option<Receiver<Result<Option<VideoFrame>, CaptureError>>>,
    reader_thread: Option<JoinHandle<()>>,
    format: Option<VideoFormat>,
}

impl ProcessFrameDevice {
    fn new(
        name: &str,
        kind: Identifier,
        command: PathBuf,
        arguments: Vec<String>,
        settings: Config,
    ) -> Result<Self, SandboxError> {
        let info = CaptureDeviceInfo::new("sandbox_process", name, CaptureKind::External)?;
        Ok(Self {
            info,
            kind,
            command,
            arguments,
            settings,
            child: None,
            frames: None,
            reader_thread: None,
            format: None,
        })
    }

    fn spawn_stream(
        &self,
        format: VideoFormat,
    ) -> Result<(Child, FrameReceiver, FrameReader), SandboxError> {
        let mut command = Command::new(&self.command);
        command
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("OBS_RS_PROTOCOL", "OBSFRM01")
            .env("OBS_RS_SOURCE_KIND", self.kind.as_str())
            .env("OBS_RS_SOURCE_NAME", self.info.name())
            .env("OBS_RS_SETTINGS", self.settings.serialize())
            .env("OBS_RS_WIDTH", format.width().to_string())
            .env("OBS_RS_HEIGHT", format.height().to_string())
            .env(
                "OBS_RS_FPS_NUMERATOR",
                format.frame_rate().numerator().to_string(),
            )
            .env(
                "OBS_RS_FPS_DENOMINATOR",
                format.frame_rate().denominator().to_string(),
            );
        let mut child = command
            .spawn()
            .map_err(|error| SandboxError::InvalidCommand {
                reason: error.to_string(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::InvalidCommand {
                reason: "sandbox child did not expose stdout".to_owned(),
            })?;
        let mut stream = StreamCaptureDevice::new(
            "sandbox_process",
            self.info.name(),
            CaptureKind::External,
            BufReader::new(stdout),
        )?;
        if let Err(error) = stream.start(format) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        let (sender, receiver) = sync_channel(MAX_SANDBOX_QUEUED_FRAMES);
        let reader_thread = spawn_frame_reader(stream, sender);
        Ok((child, receiver, reader_thread))
    }
}

impl VideoCaptureDevice for ProcessFrameDevice {
    fn info(&self) -> &CaptureDeviceInfo {
        &self.info
    }

    fn start(&mut self, format: VideoFormat) -> Result<(), CaptureError> {
        if self.format.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        match self.info.permission() {
            CapturePermission::Granted => {}
            CapturePermission::PromptRequired => return Err(CaptureError::PermissionRequired),
            CapturePermission::Denied => return Err(CaptureError::PermissionDenied),
            CapturePermission::Unavailable => return Err(CaptureError::PermissionUnavailable),
        }
        let (child, frames, reader_thread) =
            self.spawn_stream(format).map_err(|error| match error {
                SandboxError::Capture(error) => error,
                SandboxError::Media(error) => CaptureError::Media(error),
                other => CaptureError::PlatformUnavailable {
                    message: other.to_string(),
                },
            })?;
        self.child = Some(child);
        self.frames = Some(frames);
        self.reader_thread = Some(reader_thread);
        self.format = Some(format);
        Ok(())
    }

    fn stop(&mut self) {
        self.frames = None;
        self.format = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, _timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(format) = self.format else {
            return Err(CaptureError::NotRunning);
        };
        let receiver = self.frames.as_ref().ok_or(CaptureError::NotRunning)?;
        let result = receiver.recv_timeout(SANDBOX_FRAME_DELIVERY_TIMEOUT);
        let frame = match result {
            Ok(frame) => frame?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.stop();
                return Err(CaptureError::Io {
                    message: format!(
                        "sandbox process did not deliver a frame within {SANDBOX_FRAME_DELIVERY_TIMEOUT:?}"
                    ),
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CaptureError::Io {
                    message: "sandbox frame reader disconnected".to_owned(),
                });
            }
        };
        if frame.is_none() {
            if let Some(child) = self.child.as_mut() {
                if let Some(status) = child.try_wait().map_err(|error| CaptureError::Io {
                    message: error.to_string(),
                })? {
                    return Err(CaptureError::Io {
                        message: format!("sandbox process exited with {status}"),
                    });
                }
            }
        }
        if let Some(frame) = &frame {
            if frame.format() != format {
                return Err(CaptureError::FrameFormatMismatch {
                    expected: format,
                    actual: frame.format(),
                });
            }
        }
        Ok(frame)
    }
}

fn spawn_frame_reader(
    mut stream: StreamCaptureDevice<BufReader<std::process::ChildStdout>>,
    sender: SyncSender<FrameResult>,
) -> JoinHandle<()> {
    thread::spawn(move || loop {
        match stream.next_frame(Timestamp::ZERO) {
            Ok(frame) => {
                let end_of_stream = frame.is_none();
                if sender.send(Ok(frame)).is_err() || end_of_stream {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error));
                break;
            }
        }
    })
}

impl Drop for ProcessFrameDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

fn validate_source_kinds(source_kinds: &[Identifier]) -> Result<(), SandboxError> {
    if source_kinds.is_empty() {
        return Err(invalid_manifest("at least one source kind is required"));
    }
    if source_kinds.len() > MAX_SANDBOX_SOURCE_KINDS {
        return Err(invalid_manifest("too many source kinds"));
    }
    for (index, kind) in source_kinds.iter().enumerate() {
        if source_kinds[..index].contains(kind) {
            return Err(invalid_manifest(format!(
                "source kind {kind} is duplicated"
            )));
        }
    }
    Ok(())
}

fn validate_command(command: &Path, arguments: &[String]) -> Result<(), SandboxError> {
    if command.as_os_str().is_empty() || command.to_string_lossy().contains('\0') {
        return Err(SandboxError::InvalidCommand {
            reason: "executable path is empty or contains NUL".to_owned(),
        });
    }
    if arguments.len() > MAX_SANDBOX_ARGUMENTS {
        return Err(SandboxError::InvalidArguments {
            reason: format!("argument count exceeds {MAX_SANDBOX_ARGUMENTS}"),
        });
    }
    if arguments
        .iter()
        .any(|argument| argument.len() > MAX_SANDBOX_ARGUMENT_BYTES || argument.contains('\0'))
    {
        return Err(SandboxError::InvalidArguments {
            reason: format!(
                "an argument exceeds {MAX_SANDBOX_ARGUMENT_BYTES} bytes or contains NUL"
            ),
        });
    }
    Ok(())
}

fn invalid_manifest(reason: impl Into<String>) -> SandboxError {
    SandboxError::InvalidManifest {
        reason: reason.into(),
    }
}

fn parse_video_format(settings: &Config) -> Result<VideoFormat, SandboxError> {
    let width = setting_u32(settings, "width")?;
    let height = setting_u32(settings, "height")?;
    let numerator = setting_u32_or(settings, "fps_numerator", 30)?;
    let denominator = setting_u32_or(settings, "fps_denominator", 1)?;
    let rate = FrameRate::new(numerator, denominator)?;
    VideoFormat::new(width, height, rate).map_err(SandboxError::Media)
}

fn setting_u32(settings: &Config, key: &str) -> Result<u32, SandboxError> {
    let value = settings
        .get(key)
        .ok_or_else(|| invalid_manifest(format!("sandbox source setting {key} is required")))?;
    value
        .parse::<u32>()
        .map_err(|_| invalid_manifest(format!("sandbox source setting {key} is invalid")))
}

fn setting_u32_or(settings: &Config, key: &str, default: u32) -> Result<u32, SandboxError> {
    settings.get(key).map_or(Ok(default), |value| {
        value
            .parse::<u32>()
            .map_err(|_| invalid_manifest(format!("sandbox source setting {key} is invalid")))
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write;

    use super::*;
    use obs_rs_capture::encode_frame_packet;
    use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
    use obs_rs_plugin_api::{Plugin, VideoRequest};

    fn manifest() -> SandboxedPluginManifest {
        let plugin =
            PluginManifest::new("sandbox_plugin", "Sandbox plugin", "0.1.0").expect("manifest");
        SandboxedPluginManifest::new(plugin, [Identifier::new("external_pattern").expect("kind")])
            .expect("sandbox manifest")
    }

    #[test]
    fn sandbox_manifest_round_trips_and_rejects_duplicates() {
        let manifest = manifest();
        let encoded = manifest.serialize();
        assert_eq!(SandboxedPluginManifest::parse(&encoded), Ok(manifest));
        assert!(matches!(
            SandboxedPluginManifest::parse(
                "OBSRPLUGIN1\nsandbox_plugin|Sandbox|0.1.0|1|0|external_pattern,external_pattern\n"
            ),
            Err(SandboxError::InvalidManifest { .. })
        ));
    }

    #[test]
    fn sandbox_plugin_exposes_versioned_process_factories_without_spawning() {
        let manifest = manifest();
        let plugin =
            SandboxedPlugin::new(&manifest, "obs-rs-extension", vec!["--source".to_owned()])
                .expect("plugin configuration");
        assert_eq!(plugin.manifest().id().as_str(), "sandbox_plugin");
        assert_eq!(plugin.source_factories().len(), 1);
        assert_eq!(plugin.command(), Path::new("obs-rs-extension"));
        assert_eq!(plugin.arguments(), &["--source".to_owned()]);
    }

    #[test]
    fn sandbox_plugin_rejects_unbounded_process_configuration() {
        let too_many = vec!["x".to_owned(); MAX_SANDBOX_ARGUMENTS + 1];
        assert!(matches!(
            SandboxedPlugin::new(&manifest(), "extension", too_many),
            Err(SandboxError::InvalidArguments { .. })
        ));
        assert!(matches!(
            SandboxedPluginManifest::parse("invalid"),
            Err(SandboxError::InvalidManifest { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sandbox_manifest_can_be_discovered_before_source_creation() {
        let script = r#"if [ "$1" = "--obs-rs-manifest" ]; then printf 'OBSRPLUGIN1\nsandbox_plugin|Sandbox plugin|0.1.0|1|0|external_pattern\n'; else exit 1; fi"#;
        let arguments = vec![
            "-c".to_owned(),
            script.to_owned(),
            "sandbox-probe".to_owned(),
        ];
        let discovered = discover_sandbox_manifest("/bin/sh", &arguments)
            .expect("manifest probe should complete");
        assert_eq!(discovered, manifest());
        let plugin = SandboxedPlugin::from_process("/bin/sh", arguments)
            .expect("process plugin should use discovered manifest");
        assert_eq!(plugin.source_factories().len(), 1);

        let oversized_arguments = vec![
            "-c".to_owned(),
            "head -c 32769 /dev/zero".to_owned(),
            "sandbox-oversized".to_owned(),
        ];
        assert_eq!(
            discover_sandbox_manifest("/bin/sh", &oversized_arguments),
            Err(SandboxError::ManifestTooLarge)
        );
    }

    #[cfg(unix)]
    #[test]
    fn sandbox_source_reads_one_bounded_frame_from_a_child_process() {
        let format = VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
        let expected = VideoFrame::solid(format, Timestamp::from_millis(7), [1, 2, 3, 255]);
        let packet = encode_frame_packet(&expected).expect("frame packet");
        let mut escaped = String::new();
        for byte in packet {
            write!(&mut escaped, r"\{byte:03o}").expect("escape packet byte");
        }
        let script = format!("printf '%b' '{escaped}'");
        let manifest = manifest();
        let plugin = SandboxedPlugin::new(&manifest, "/bin/sh", vec!["-c".to_owned(), script])
            .expect("sandbox process configuration");
        let factory = plugin
            .source_factories()
            .into_iter()
            .next()
            .expect("sandbox source factory");
        let mut settings = Config::new();
        settings.set("width", "1").expect("width");
        settings.set("height", "1").expect("height");
        let mut source = factory.create("fixture", &settings).expect("source");
        let received = source
            .render(&VideoRequest::new(Timestamp::ZERO, format))
            .expect("frame from sandbox")
            .expect("one frame");
        assert_eq!(received, expected);
    }
}
