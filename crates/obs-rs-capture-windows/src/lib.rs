//! Safe Rust host boundary for the native Windows capture helper.
//!
//! The separately built helper owns Windows Graphics Capture/Media Foundation
//! objects and writes bounded OBSFRM01 packets. Native COM/D3D handles never
//! cross into portable Rust. Discovery is a small, line-oriented OBSRWIN1
//! reply so the GUI can list real displays and windows without linking native
//! Windows APIs into the workspace.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::{
    io::{BufReader, Read},
    process::{Child, ChildStderr, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use obs_rs_capture::{
    CaptureCapabilities, CaptureDeviceInfo, CaptureError, PlatformCaptureAdapter,
    VideoCaptureDevice,
};
#[cfg(target_os = "windows")]
use obs_rs_capture::{CaptureKind, StreamCaptureDevice};
#[cfg(target_os = "windows")]
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

/// Discovery and frame-stream handshake used by the Windows helper.
pub const WINDOWS_HELPER_PROTOCOL: &str = "OBSRWIN1";
/// Version embedded in the helper diagnostics reply.
pub const WINDOWS_HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");
const HELPER_EXE: &str = "obs-rs-capture-windows-helper.exe";
const MAX_DISCOVERY_REPLY_BYTES: u64 = 256 * 1024;
const MAX_DISCOVERY_RECORDS: usize = 512;
#[cfg(target_os = "windows")]
const HELPER_FRAME_QUEUE_CAPACITY: usize = 2;
#[cfg(target_os = "windows")]
const MAX_HELPER_DIAGNOSTICS_BYTES: u64 = 32 * 1024;
#[cfg(target_os = "windows")]
const HELPER_SHUTDOWN_GRACE: Duration = Duration::from_millis(750);
#[cfg(target_os = "windows")]
const HELPER_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A display returned by Windows Graphics Capture discovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsDisplayInfo {
    /// Stable display capture ID, for example `wgc-screen-1`.
    pub id: String,
    /// Friendly display name.
    pub name: String,
    /// Desktop-space left coordinate.
    pub x: i32,
    /// Desktop-space top coordinate.
    pub y: i32,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
    /// Whether Windows reports this as the primary display.
    pub primary: bool,
}

/// Windows Graphics Capture/Media Foundation adapter using a native helper.
#[derive(Clone, Debug)]
pub struct WindowsCaptureAdapter {
    helper: PathBuf,
}

impl Default for WindowsCaptureAdapter {
    fn default() -> Self {
        Self::new(default_helper_path())
    }
}

impl WindowsCaptureAdapter {
    /// Creates an adapter using an explicit helper path.
    #[must_use]
    pub fn new(helper: impl Into<PathBuf>) -> Self {
        Self {
            helper: helper.into(),
        }
    }

    /// Returns the helper executable path.
    #[must_use]
    pub fn helper(&self) -> &Path {
        &self.helper
    }

    /// Returns the bounded helper lookup order used by packaged builds.
    #[must_use]
    pub fn helper_search_paths() -> Vec<PathBuf> {
        helper_search_paths()
    }

    /// Queries the helper version for diagnostics.
    ///
    /// # Errors
    ///
    /// Returns a typed platform, protocol, or I/O error when the helper cannot
    /// be launched or does not return a valid version line.
    pub fn helper_version(&self) -> Result<String, CaptureError> {
        #[cfg(target_os = "windows")]
        {
            let output = self.run_helper(&["--protocol", WINDOWS_HELPER_PROTOCOL, "--version"])?;
            parse_version_output(&output)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(CaptureError::PlatformUnavailable {
                message: "Windows capture helper requires Windows".to_owned(),
            })
        }
    }

    /// Discovers displays without returning the camera/window catalog.
    ///
    /// # Errors
    ///
    /// Returns a typed platform, protocol, or I/O error when display discovery
    /// cannot be completed.
    pub fn discover_displays(&self) -> Result<Vec<WindowsDisplayInfo>, CaptureError> {
        #[cfg(target_os = "windows")]
        {
            let output = self.run_helper(&["--protocol", WINDOWS_HELPER_PROTOCOL, "--discover"])?;
            let (_, displays) = parse_discovery_output(&output)?;
            Ok(displays)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(CaptureError::PlatformUnavailable {
                message: "Windows Graphics Capture requires Windows".to_owned(),
            })
        }
    }
}

impl PlatformCaptureAdapter for WindowsCaptureAdapter {
    fn backend_name(&self) -> &'static str {
        "windows-graphics-capture-media-foundation"
    }

    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        #[cfg(target_os = "windows")]
        {
            let output = self.run_helper(&["--protocol", WINDOWS_HELPER_PROTOCOL, "--discover"])?;
            let (devices, _) = parse_discovery_output(&output)?;
            Ok(devices)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(CaptureError::PlatformUnavailable {
                message: "Windows Graphics Capture and Media Foundation require Windows".to_owned(),
            })
        }
    }

    fn open(&self, stable_id: &str) -> Result<Box<dyn VideoCaptureDevice>, CaptureError> {
        #[cfg(target_os = "windows")]
        {
            let kind = if stable_id == "wgc-screen-picker" || stable_id.starts_with("wgc-screen-") {
                CaptureKind::Screen
            } else if stable_id == "wgc-window-picker" || stable_id.starts_with("wgc-window-") {
                CaptureKind::Window
            } else {
                return Err(CaptureError::InvalidDevice {
                    reason: format!(
                        "unknown Windows Graphics Capture stable ID {stable_id}; cameras use Nokhwa"
                    ),
                });
            };
            NativeHelperDevice::new(&self.helper, stable_id, kind)
                .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = stable_id;
            Err(CaptureError::PlatformUnavailable {
                message: "Windows Graphics Capture and Media Foundation require Windows".to_owned(),
            })
        }
    }
}

fn helper_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("OBSR_CAPTURE_HELPER")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        paths.push(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            paths.push(parent.join(HELPER_EXE));
        }
    }
    for variable in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(base) = std::env::var_os(variable) {
            let base = PathBuf::from(base);
            paths.push(base.join("obs-rs").join("bin").join(HELPER_EXE));
            paths.push(base.join("obs-rs").join(HELPER_EXE));
        }
    }
    paths.dedup();
    paths
}

fn default_helper_path() -> PathBuf {
    let paths = helper_search_paths();
    paths
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .or_else(|| paths.into_iter().next())
        .unwrap_or_else(|| PathBuf::from(HELPER_EXE))
}

#[cfg(target_os = "windows")]
struct NativeHelperDevice {
    helper: PathBuf,
    info: CaptureDeviceInfo,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
    frames: Option<Receiver<Result<VideoFrame, CaptureError>>>,
    reader: Option<JoinHandle<()>>,
    format: Option<VideoFormat>,
}

#[cfg(target_os = "windows")]
impl NativeHelperDevice {
    fn new(helper: &Path, stable_id: &str, kind: CaptureKind) -> Result<Self, CaptureError> {
        Ok(Self {
            helper: helper.to_owned(),
            info: CaptureDeviceInfo::new(stable_id, stable_id, kind)?,
            child: None,
            stdin: None,
            stderr: None,
            frames: None,
            reader: None,
            format: None,
        })
    }
}

#[cfg(target_os = "windows")]
impl VideoCaptureDevice for NativeHelperDevice {
    fn info(&self) -> &CaptureDeviceInfo {
        &self.info
    }

    fn start(&mut self, format: VideoFormat) -> Result<(), CaptureError> {
        if self.format.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        validate_helper_version(&self.helper)?;
        let mut child = Command::new(&self.helper)
            .args(capture_helper_args(self.info.id().as_str(), format))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| launch_error("Windows capture helper", &error))?;
        let stderr = match spawn_stderr_reader(&mut child) {
            Ok(reader) => reader,
            Err(error) => {
                terminate_child(&mut child, None);
                return Err(error);
            }
        };
        let Some(stdin) = child.stdin.take() else {
            let diagnostics = terminate_child(&mut child, Some(stderr));
            return Err(CaptureError::Protocol {
                message: if diagnostics.is_empty() {
                    "Windows capture helper has no control pipe".to_owned()
                } else {
                    format!("Windows capture helper has no control pipe: {diagnostics}")
                },
            });
        };
        let Some(stdout) = child.stdout.take() else {
            let diagnostics = terminate_child(&mut child, Some(stderr));
            return Err(CaptureError::Protocol {
                message: if diagnostics.is_empty() {
                    "Windows capture helper has no frame stream".to_owned()
                } else {
                    format!("Windows capture helper has no frame stream: {diagnostics}")
                },
            });
        };
        let mut stream = match StreamCaptureDevice::new(
            self.info.id().as_str(),
            self.info.name(),
            self.info.kind(),
            BufReader::new(stdout),
        ) {
            Ok(stream) => stream,
            Err(error) => {
                terminate_child(&mut child, Some(stderr));
                return Err(error);
            }
        };
        if let Err(error) = stream.start(format) {
            terminate_child(&mut child, Some(stderr));
            return Err(error);
        }
        let (frame_sender, frame_receiver) = mpsc::sync_channel(HELPER_FRAME_QUEUE_CAPACITY);
        let reader = match thread::Builder::new()
            .name("obs-rs-capture-helper-frames".to_owned())
            .spawn(move || read_helper_frames(stream, frame_sender))
        {
            Ok(reader) => reader,
            Err(error) => {
                let diagnostics = terminate_child(&mut child, Some(stderr));
                let suffix = if diagnostics.is_empty() {
                    String::new()
                } else {
                    format!(": {diagnostics}")
                };
                return Err(CaptureError::Io {
                    message: format!("start Windows capture frame reader: {error}{suffix}"),
                });
            }
        };
        self.child = Some(child);
        self.stdin = Some(stdin);
        self.stderr = Some(stderr);
        self.frames = Some(frame_receiver);
        self.reader = Some(reader);
        self.format = Some(format);
        Ok(())
    }

    fn stop(&mut self) {
        // Dropping the receiver lets the reader thread stop publishing while
        // the child is being shut down. Killing the child below closes its
        // stdout handle, which unblocks the reader if it is inside read().
        self.frames = None;
        self.format = None;
        // Closing stdin is the helper's graceful shutdown request. A bounded
        // wait below keeps a stuck native capture session from blocking the
        // engine or GUI indefinitely.
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + HELPER_SHUTDOWN_GRACE;
            let mut exited = false;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        exited = true;
                        break;
                    }
                    Ok(None) => thread::sleep(HELPER_POLL_INTERVAL),
                    Err(_) => break,
                }
            }
            if !exited {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        join_stderr_reader(self.stderr.take());
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, _timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let result = self
            .frames
            .as_ref()
            .ok_or(CaptureError::NotRunning)?
            .try_recv();
        match result {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(error)) => Err(error),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                let Some(child) = self.child.as_mut() else {
                    return Err(CaptureError::Protocol {
                        message: "Windows capture helper disappeared".to_owned(),
                    });
                };
                match child.try_wait() {
                    Ok(Some(status)) => Err(helper_exit_error(status, self.stderr.take())),
                    Ok(None) => Err(CaptureError::Protocol {
                        message: "Windows capture frame reader stopped unexpectedly".to_owned(),
                    }),
                    Err(error) => Err(CaptureError::Io {
                        message: format!("check Windows capture helper: {error}"),
                    }),
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(
    clippy::needless_pass_by_value,
    reason = "the reader thread owns the channel endpoint for its entire lifetime"
)]
fn read_helper_frames(
    mut stream: StreamCaptureDevice<BufReader<std::process::ChildStdout>>,
    sender: SyncSender<Result<VideoFrame, CaptureError>>,
) {
    loop {
        match stream.next_frame(Timestamp::ZERO) {
            Ok(Some(frame)) => match sender.try_send(Ok(frame)) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => return,
            },
            Ok(None) => return,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl WindowsCaptureAdapter {
    fn run_helper(&self, args: &[&str]) -> Result<String, CaptureError> {
        let mut child = Command::new(&self.helper)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| launch_error("Windows capture helper", &error))?;
        let stderr = match spawn_stderr_reader(&mut child) {
            Ok(reader) => reader,
            Err(error) => {
                terminate_child(&mut child, None);
                return Err(error);
            }
        };
        let Some(mut stdout) = child.stdout.take() else {
            let diagnostics = terminate_child(&mut child, Some(stderr));
            return Err(CaptureError::Protocol {
                message: if diagnostics.is_empty() {
                    "Windows capture helper has no discovery output".to_owned()
                } else {
                    format!("Windows capture helper has no discovery output: {diagnostics}")
                },
            });
        };
        let mut output = String::new();
        let bytes = match stdout
            .by_ref()
            .take(MAX_DISCOVERY_REPLY_BYTES.saturating_add(1))
            .read_to_string(&mut output)
        {
            Ok(bytes) => bytes,
            Err(error) => {
                let diagnostics = terminate_child(&mut child, Some(stderr));
                let suffix = if diagnostics.is_empty() {
                    String::new()
                } else {
                    format!(": {diagnostics}")
                };
                return Err(CaptureError::Io {
                    message: format!("read Windows capture helper reply: {error}{suffix}"),
                });
            }
        };
        let too_large = u64::try_from(bytes).unwrap_or(u64::MAX) > MAX_DISCOVERY_REPLY_BYTES;
        if too_large {
            terminate_child(&mut child, Some(stderr));
            return Err(CaptureError::ReplyTooLarge {
                bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
            });
        }
        let status = match child.wait() {
            Ok(status) => status,
            Err(error) => {
                let diagnostics = join_stderr_reader(Some(stderr));
                let suffix = if diagnostics.is_empty() {
                    String::new()
                } else {
                    format!(": {diagnostics}")
                };
                return Err(CaptureError::Io {
                    message: format!("wait for Windows capture helper: {error}{suffix}"),
                });
            }
        };
        let diagnostics = join_stderr_reader(Some(stderr));
        if !status.success() {
            return Err(CaptureError::Protocol {
                message: helper_exit_message(status, &diagnostics),
            });
        }
        Ok(output)
    }
}

#[cfg(target_os = "windows")]
fn validate_helper_version(helper: &Path) -> Result<(), CaptureError> {
    let adapter = WindowsCaptureAdapter::new(helper.to_owned());
    let output = adapter.run_helper(&["--protocol", WINDOWS_HELPER_PROTOCOL, "--version"])?;
    let _ = parse_version_output(&output)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn capture_helper_args(stable_id: &str, format: VideoFormat) -> Vec<String> {
    vec![
        "--protocol".to_owned(),
        WINDOWS_HELPER_PROTOCOL.to_owned(),
        "--device".to_owned(),
        stable_id.to_owned(),
        "--width".to_owned(),
        format.width().to_string(),
        "--height".to_owned(),
        format.height().to_string(),
        "--fps-numerator".to_owned(),
        format.frame_rate().numerator().to_string(),
        "--fps-denominator".to_owned(),
        format.frame_rate().denominator().to_string(),
    ]
}

#[cfg(target_os = "windows")]
fn spawn_stderr_reader(child: &mut Child) -> Result<JoinHandle<String>, CaptureError> {
    let stderr: ChildStderr = child.stderr.take().ok_or_else(|| CaptureError::Protocol {
        message: "Windows capture helper has no diagnostics pipe".to_owned(),
    })?;
    thread::Builder::new()
        .name("obs-rs-capture-helper-stderr".to_owned())
        .spawn(move || read_stderr(stderr))
        .map_err(|error| CaptureError::Io {
            message: format!("start Windows capture helper diagnostics reader: {error}"),
        })
}

#[cfg(target_os = "windows")]
fn read_stderr(mut stderr: ChildStderr) -> String {
    let mut bytes = Vec::new();
    let limit = usize::try_from(MAX_HELPER_DIAGNOSTICS_BYTES).unwrap_or(0);
    let mut chunk = [0_u8; 4096];
    while let Ok(read) = stderr.read(&mut chunk) {
        if read == 0 {
            break;
        }
        if bytes.len() < limit {
            let retained = (limit - bytes.len()).min(read);
            bytes.extend_from_slice(&chunk[..retained]);
        }
    }
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

#[cfg(target_os = "windows")]
fn terminate_child(child: &mut Child, stderr: Option<JoinHandle<String>>) -> String {
    let _ = child.kill();
    let _ = child.wait();
    join_stderr_reader(stderr)
}

#[cfg(target_os = "windows")]
fn join_stderr_reader(stderr: Option<JoinHandle<String>>) -> String {
    stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn helper_exit_error(
    status: std::process::ExitStatus,
    stderr: Option<JoinHandle<String>>,
) -> CaptureError {
    CaptureError::Protocol {
        message: helper_exit_message(status, &join_stderr_reader(stderr)),
    }
}

#[cfg(target_os = "windows")]
fn helper_exit_message(status: std::process::ExitStatus, diagnostics: &str) -> String {
    if diagnostics.is_empty() {
        format!("Windows capture helper exited with {status}")
    } else {
        format!("Windows capture helper exited with {status}: {diagnostics}")
    }
}

#[cfg(target_os = "windows")]
fn launch_error(operation: &str, error: &std::io::Error) -> CaptureError {
    if error.kind() == std::io::ErrorKind::NotFound {
        CaptureError::PlatformUnavailable {
            message: format!("{operation} is unavailable: {error}"),
        }
    } else {
        CaptureError::Io {
            message: format!("launch {operation}: {error}"),
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for NativeHelperDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the bounded protocol parser validates every discovery record in one pass"
)]
fn parse_discovery_output(
    output: &str,
) -> Result<(Vec<CaptureDeviceInfo>, Vec<WindowsDisplayInfo>), CaptureError> {
    let mut lines = output.lines();
    if lines.next() != Some("OBSRWIN1\tDISCOVERY\t1") {
        return Err(CaptureError::Protocol {
            message: "Windows discovery header is invalid".to_owned(),
        });
    }
    let mut devices = Vec::new();
    let mut displays = Vec::new();
    let mut version = None;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with("OBSRWIN1\tVERSION\t") {
            if version.is_some() {
                return Err(CaptureError::Protocol {
                    message: "Windows discovery contains duplicate version lines".to_owned(),
                });
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 3 || fields[0] != WINDOWS_HELPER_PROTOCOL || fields[1] != "VERSION" {
                return Err(CaptureError::Protocol {
                    message: "Windows discovery version line is invalid".to_owned(),
                });
            }
            version = Some(field(fields[2])?);
            continue;
        }
        if devices.len() >= MAX_DISCOVERY_RECORDS {
            return Err(CaptureError::ReplyTooLarge {
                bytes: u64::try_from(output.len()).unwrap_or(u64::MAX),
            });
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.first().copied() {
            Some("screen") if fields.len() == 8 => {
                let id = field(fields[1])?;
                if !id.starts_with("wgc-screen-") {
                    return Err(CaptureError::Protocol {
                        message: format!("invalid Windows display ID {id}"),
                    });
                }
                let width = parse_field(fields[5], "display width")?;
                let height = parse_field(fields[6], "display height")?;
                if width == 0 || height == 0 {
                    return Err(CaptureError::Protocol {
                        message: "Windows display dimensions must be non-zero".to_owned(),
                    });
                }
                let primary = match fields[7] {
                    "0" => false,
                    "1" => true,
                    _ => {
                        return Err(CaptureError::Protocol {
                            message: "Windows display primary flag must be 0 or 1".to_owned(),
                        });
                    }
                };
                let display = WindowsDisplayInfo {
                    id,
                    name: field(fields[2])?,
                    x: parse_field(fields[3], "display x")?,
                    y: parse_field(fields[4], "display y")?,
                    width,
                    height,
                    primary,
                };
                let mut device = CaptureDeviceInfo::new(
                    &display.id,
                    &display.name,
                    obs_rs_capture::CaptureKind::Screen,
                )?
                .with_capabilities(
                    CaptureCapabilities::new(Vec::new(), false, true).with_hotplug(true),
                );
                device.set_permission(obs_rs_capture::CapturePermission::Granted);
                devices.push(device);
                displays.push(display);
            }
            Some("window") if fields.len() == 3 => {
                let id = field(fields[1])?;
                if !id.starts_with("wgc-window-") {
                    return Err(CaptureError::Protocol {
                        message: format!("invalid Windows window ID {id}"),
                    });
                }
                let name = field(fields[2])?;
                let mut device =
                    CaptureDeviceInfo::new(&id, &name, obs_rs_capture::CaptureKind::Window)?
                        .with_capabilities(
                            CaptureCapabilities::new(Vec::new(), false, true).with_hotplug(true),
                        );
                device.set_permission(obs_rs_capture::CapturePermission::Granted);
                devices.push(device);
            }
            _ => {
                return Err(CaptureError::Protocol {
                    message: format!("invalid Windows discovery record: {line}"),
                });
            }
        }
    }
    let version = version.ok_or_else(|| CaptureError::Protocol {
        message: "Windows discovery version line is missing".to_owned(),
    })?;
    validate_version_compatibility(&version)?;
    devices.sort_by(|left, right| left.id().cmp(right.id()));
    if devices.windows(2).any(|pair| pair[0].id() == pair[1].id()) {
        return Err(CaptureError::Protocol {
            message: "Windows discovery contains duplicate device IDs".to_owned(),
        });
    }
    displays.sort_by(|left, right| left.id.cmp(&right.id));
    if displays.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err(CaptureError::Protocol {
            message: "Windows discovery contains duplicate display IDs".to_owned(),
        });
    }
    Ok((devices, displays))
}

fn parse_version_output(output: &str) -> Result<String, CaptureError> {
    let mut version = None;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3 || fields[0] != WINDOWS_HELPER_PROTOCOL || fields[1] != "VERSION" {
            return Err(CaptureError::Protocol {
                message: "Windows helper version reply is invalid".to_owned(),
            });
        }
        if version.is_some() {
            return Err(CaptureError::Protocol {
                message: "Windows helper version reply is duplicated".to_owned(),
            });
        }
        version = Some(field(fields[2])?);
    }
    let version = version.ok_or_else(|| CaptureError::Protocol {
        message: "Windows helper version reply is missing".to_owned(),
    })?;
    validate_version_compatibility(&version)?;
    Ok(version)
}

fn validate_version_compatibility(version: &str) -> Result<(), CaptureError> {
    let expected_major = WINDOWS_HELPER_VERSION.split('.').next();
    let actual_major = version.split('.').next();
    if expected_major.is_none_or(|major| actual_major != Some(major)) {
        return Err(CaptureError::Protocol {
            message: format!(
                "Windows helper version {version} is incompatible with {WINDOWS_HELPER_VERSION}"
            ),
        });
    }
    Ok(())
}

fn field(value: &str) -> Result<String, CaptureError> {
    if value.trim().is_empty() || value.contains(['\r', '\n']) {
        return Err(CaptureError::Protocol {
            message: "Windows discovery field is empty or contains a newline".to_owned(),
        });
    }
    Ok(value.to_owned())
}

fn parse_field<T>(value: &str, label: &str) -> Result<T, CaptureError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value.parse::<T>().map_err(|error| CaptureError::Protocol {
        message: format!("invalid {label} in Windows discovery: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_parser_returns_displays_and_devices() {
        let output = "OBSRWIN1\tDISCOVERY\t1\nOBSRWIN1\tVERSION\t0.1.0\nscreen\twgc-screen-1\tPrimary\t0\t0\t1920\t1080\t1\nwindow\twgc-window-abcd\tEditor\n";
        let (devices, displays) = parse_discovery_output(output).expect("valid discovery");
        assert_eq!(devices.len(), 2);
        assert_eq!(displays[0].id, "wgc-screen-1");
        assert!(displays[0].primary);
    }

    #[test]
    fn malformed_discovery_is_typed() {
        assert!(matches!(
            parse_discovery_output("wrong\n"),
            Err(CaptureError::Protocol { .. })
        ));
    }

    #[test]
    fn non_windows_hosts_report_a_typed_unavailable_state() {
        #[cfg(target_os = "windows")]
        assert!(matches!(
            WindowsCaptureAdapter::new("__missing_obs_rs_helper__.exe").discover(),
            Err(CaptureError::PlatformUnavailable { .. })
        ));
        #[cfg(not(target_os = "windows"))]
        assert!(matches!(
            WindowsCaptureAdapter::default().discover(),
            Err(CaptureError::PlatformUnavailable { .. })
        ));
    }

    #[test]
    fn discovery_requires_a_compatible_version_line() {
        let missing =
            "OBSRWIN1\tDISCOVERY\t1\nscreen\twgc-screen-1\tPrimary\t0\t0\t1920\t1080\t1\n";
        assert!(matches!(
            parse_discovery_output(missing),
            Err(CaptureError::Protocol { .. })
        ));

        let incompatible = "OBSRWIN1\tDISCOVERY\t1\nOBSRWIN1\tVERSION\t9.0.0\n";
        assert!(matches!(
            parse_discovery_output(incompatible),
            Err(CaptureError::Protocol { .. })
        ));
    }

    #[test]
    fn version_reply_rejects_extra_or_duplicate_records() {
        assert!(parse_version_output("OBSRWIN1\tVERSION\t0.1.0\n").is_ok());
        assert!(matches!(
            parse_version_output("OBSRWIN1\tVERSION\t0.1.0\nother\n"),
            Err(CaptureError::Protocol { .. })
        ));
        assert!(matches!(
            parse_version_output("OBSRWIN1\tVERSION\t0.1.0\nOBSRWIN1\tVERSION\t0.1.0\n"),
            Err(CaptureError::Protocol { .. })
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn capture_command_keeps_the_protocol_target_and_format_together() {
        let format = VideoFormat::new(1_280, 720, obs_rs_media::FrameRate::new(60, 1).unwrap())
            .expect("valid format");
        assert_eq!(
            capture_helper_args("wgc-screen-2", format),
            vec![
                "--protocol",
                "OBSRWIN1",
                "--device",
                "wgc-screen-2",
                "--width",
                "1280",
                "--height",
                "720",
                "--fps-numerator",
                "60",
                "--fps-denominator",
                "1",
            ]
        );
    }
}
