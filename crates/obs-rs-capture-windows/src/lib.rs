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
    io::{self, BufReader, Read},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
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
const MAX_HELPER_DIAGNOSTICS_BYTES: u64 = 32 * 1024;
#[cfg(target_os = "windows")]
const HELPER_SHUTDOWN_GRACE: Duration = Duration::from_millis(750);
#[cfg(target_os = "windows")]
const HELPER_POLL_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(target_os = "windows")]
const HELPER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "windows")]
const HELPER_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

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
    capture_cursor: Option<bool>,
    capture_border: Option<bool>,
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
            capture_cursor: None,
            capture_border: None,
        }
    }

    /// Overrides the Windows Graphics Capture cursor policy for newly opened
    /// screen and window devices.
    #[must_use]
    pub fn with_capture_cursor(mut self, capture_cursor: bool) -> Self {
        self.capture_cursor = Some(capture_cursor);
        self
    }

    /// Overrides the Windows Graphics Capture border policy for newly opened
    /// screen and window devices.
    #[must_use]
    pub fn with_capture_border(mut self, capture_border: bool) -> Self {
        self.capture_border = Some(capture_border);
        self
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
            NativeHelperDevice::new(
                &self.helper,
                stable_id,
                kind,
                self.capture_cursor,
                self.capture_border,
            )
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
        // A normal `cargo run` starts the GUI from the workspace target
        // directory while the native helper deliberately has its own Cargo
        // manifest. Include that sibling build output during development, but
        // only when the repository layout is actually present so packaged
        // lookup remains deterministic.
        for ancestor in exe.ancestors().skip(1) {
            let helper_manifest = ancestor
                .join("packaging")
                .join("windows")
                .join("capture-helper")
                .join("Cargo.toml");
            if !helper_manifest.is_file() {
                continue;
            }
            for profile in ["dev-fast", "debug", "release"] {
                paths.push(
                    ancestor
                        .join("packaging")
                        .join("windows")
                        .join("capture-helper")
                        .join("target")
                        .join(profile)
                        .join(HELPER_EXE),
                );
            }
            break;
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
    capture_cursor: Option<bool>,
    capture_border: Option<bool>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
    frames: Option<Arc<HelperFrameMailbox>>,
    reader: Option<JoinHandle<()>>,
    format: Option<VideoFormat>,
    started_at: Instant,
    first_frame_received: bool,
}

#[cfg(target_os = "windows")]
impl NativeHelperDevice {
    fn new(
        helper: &Path,
        stable_id: &str,
        kind: CaptureKind,
        capture_cursor: Option<bool>,
        capture_border: Option<bool>,
    ) -> Result<Self, CaptureError> {
        Ok(Self {
            helper: helper.to_owned(),
            info: CaptureDeviceInfo::new(stable_id, stable_id, kind)?,
            capture_cursor,
            capture_border,
            child: None,
            stdin: None,
            stderr: None,
            frames: None,
            reader: None,
            format: None,
            started_at: Instant::now(),
            first_frame_received: false,
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
            .args(capture_helper_args(
                self.info.id().as_str(),
                format,
                self.capture_cursor,
                self.capture_border,
            ))
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
        // The compositor only needs the newest frame. A bounded FIFO makes a
        // temporary render stall display stale frames after the helper has
        // caught up; the latest-frame mailbox drops those stale frames at the
        // capture boundary instead.
        let frame_mailbox = Arc::new(HelperFrameMailbox::default());
        let reader = match thread::Builder::new()
            .name("obs-rs-capture-helper-frames".to_owned())
            .spawn({
                let frame_mailbox = Arc::clone(&frame_mailbox);
                move || read_helper_frames(stream, frame_mailbox)
            }) {
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
        self.frames = Some(frame_mailbox);
        self.reader = Some(reader);
        self.format = Some(format);
        self.started_at = Instant::now();
        self.first_frame_received = false;
        Ok(())
    }

    fn stop(&mut self) {
        // Dropping the mailbox handle lets the reader thread release its
        // capture frame as soon as the child is being shut down. Killing the
        // child below closes stdout, which unblocks the reader if it is inside
        // a packet read.
        self.frames = None;
        self.format = None;
        self.first_frame_received = false;
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
        let mailbox = self.frames.as_ref().ok_or(CaptureError::NotRunning)?;
        if let Some(frame) = mailbox.take_latest() {
            self.first_frame_received = true;
            return Ok(Some(frame));
        }
        match mailbox.failure() {
            Some(error) => Err(self.enrich_helper_failure(error)),
            None if !self.first_frame_received
                && self.started_at.elapsed() >= HELPER_FIRST_FRAME_TIMEOUT =>
            {
                Err(
                    self.enrich_helper_failure(CaptureError::PlatformUnavailable {
                        message: format!(
                            "Windows capture helper produced no frame within {} seconds",
                            HELPER_FIRST_FRAME_TIMEOUT.as_secs()
                        ),
                    }),
                )
            }
            None => Ok(None),
        }
    }
}

#[cfg(target_os = "windows")]
impl NativeHelperDevice {
    /// Adds the native process exit and bounded stderr to a frame failure.
    ///
    /// The frame reader owns stdout and therefore cannot inspect the helper's
    /// exit status. Waiting for that status here is non-blocking; when the
    /// process has already exited, its stderr pipe is closed and joining the
    /// bounded diagnostics reader is safe. A permission/privacy failure is
    /// kept as a denied lifecycle state so the source does not retry a native
    /// request that requires an operator action.
    fn enrich_helper_failure(&mut self, fallback: CaptureError) -> CaptureError {
        let status = match self.child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    return CaptureError::Io {
                        message: format!(
                            "inspect Windows capture helper after failure: {error}; {fallback}"
                        ),
                    };
                }
            },
            None => return fallback,
        };
        let Some(status) = status else {
            return fallback;
        };
        let diagnostics = join_stderr_reader(self.stderr.take());
        if status.success() {
            if diagnostics.is_empty() {
                fallback
            } else {
                CaptureError::Protocol {
                    message: format!("{fallback}: {diagnostics}"),
                }
            }
        } else {
            classify_helper_exit(status, &diagnostics)
        }
    }
}

#[cfg(target_os = "windows")]
#[derive(Default)]
struct HelperFrameMailbox {
    latest: Mutex<Option<VideoFrame>>,
    failure: Mutex<Option<CaptureError>>,
}

#[cfg(target_os = "windows")]
impl HelperFrameMailbox {
    fn publish(&self, frame: VideoFrame) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(frame);
        }
    }

    fn fail(&self, error: CaptureError) {
        if let Ok(mut failure) = self.failure.lock() {
            *failure = Some(error);
        }
    }

    fn take_latest(&self) -> Option<VideoFrame> {
        self.latest.lock().ok()?.take()
    }

    fn failure(&self) -> Option<CaptureError> {
        self.failure.lock().ok()?.clone()
    }
}

#[cfg(target_os = "windows")]
#[allow(
    clippy::needless_pass_by_value,
    reason = "the reader thread owns the mailbox for its entire lifetime"
)]
fn read_helper_frames(
    mut stream: StreamCaptureDevice<BufReader<std::process::ChildStdout>>,
    mailbox: Arc<HelperFrameMailbox>,
) {
    loop {
        match stream.next_frame(Timestamp::ZERO) {
            Ok(Some(frame)) => mailbox.publish(frame),
            Ok(None) => {
                mailbox.fail(CaptureError::Protocol {
                    message: "Windows capture helper frame stream ended".to_owned(),
                });
                return;
            }
            Err(error) => {
                mailbox.fail(error);
                return;
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl WindowsCaptureAdapter {
    #[allow(
        clippy::too_many_lines,
        reason = "bounded process polling, pipe cleanup, and diagnostics stay together at this native boundary"
    )]
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
        let Some(stdout) = child.stdout.take() else {
            let diagnostics = terminate_child(&mut child, Some(stderr));
            return Err(CaptureError::Protocol {
                message: if diagnostics.is_empty() {
                    "Windows capture helper has no discovery output".to_owned()
                } else {
                    format!("Windows capture helper has no discovery output: {diagnostics}")
                },
            });
        };
        let stdout_reader = match thread::Builder::new()
            .name("obs-rs-capture-helper-discovery".to_owned())
            .spawn(move || read_helper_output(stdout))
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
                    message: format!("start Windows capture helper output reader: {error}{suffix}"),
                });
            }
        };
        let mut stdout_reader = Some(stdout_reader);
        let mut output_result = None;
        let deadline = Instant::now() + HELPER_COMMAND_TIMEOUT;
        let status = loop {
            if output_result.is_none()
                && stdout_reader
                    .as_ref()
                    .is_some_and(std::thread::JoinHandle::is_finished)
            {
                output_result =
                    Some(join_helper_output(stdout_reader.take().expect(
                        "finished helper output reader must still be owned by the command",
                    )));
            }
            if matches!(output_result.as_ref(), Some(Err(_))) {
                let output_error = output_result
                    .take()
                    .and_then(Result::err)
                    .expect("helper output error was present");
                let diagnostics = terminate_child(&mut child, Some(stderr));
                let suffix = if diagnostics.is_empty() {
                    String::new()
                } else {
                    format!(": {diagnostics}")
                };
                return Err(CaptureError::Io {
                    message: format!("read Windows capture helper reply: {output_error}{suffix}"),
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(reader) = stdout_reader.take() {
                        let _ = reader.join();
                    }
                    let diagnostics = join_stderr_reader(Some(stderr));
                    let suffix = if diagnostics.is_empty() {
                        String::new()
                    } else {
                        format!(": {diagnostics}")
                    };
                    return Err(CaptureError::Io {
                        message: format!(
                            "Windows capture helper did not finish within {} seconds{suffix}",
                            HELPER_COMMAND_TIMEOUT.as_secs()
                        ),
                    });
                }
                Ok(None) => thread::sleep(HELPER_POLL_INTERVAL),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(reader) = stdout_reader.take() {
                        let _ = reader.join();
                    }
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
            }
        };
        let output_result = output_result.unwrap_or_else(|| {
            join_helper_output(
                stdout_reader
                    .take()
                    .expect("helper output reader must finish when the child process exits"),
            )
        });
        let (output, bytes) = match output_result {
            Ok(output) => output,
            Err(error) => {
                let diagnostics = join_stderr_reader(Some(stderr));
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
        if u64::try_from(bytes).unwrap_or(u64::MAX) > MAX_DISCOVERY_REPLY_BYTES {
            let _ = join_stderr_reader(Some(stderr));
            return Err(CaptureError::ReplyTooLarge {
                bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
            });
        }
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
fn read_helper_output(mut stdout: ChildStdout) -> Result<(String, usize), io::Error> {
    let mut output = String::new();
    let bytes = stdout
        .by_ref()
        .take(MAX_DISCOVERY_REPLY_BYTES.saturating_add(1))
        .read_to_string(&mut output)?;
    Ok((output, bytes))
}

#[cfg(target_os = "windows")]
fn join_helper_output(
    reader: JoinHandle<Result<(String, usize), io::Error>>,
) -> Result<(String, usize), io::Error> {
    reader
        .join()
        .map_err(|_| io::Error::other("Windows capture helper output reader panicked"))?
}

#[cfg(target_os = "windows")]
fn validate_helper_version(helper: &Path) -> Result<(), CaptureError> {
    let adapter = WindowsCaptureAdapter::new(helper.to_owned());
    let output = adapter.run_helper(&["--protocol", WINDOWS_HELPER_PROTOCOL, "--version"])?;
    let _ = parse_version_output(&output)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn capture_helper_args(
    stable_id: &str,
    format: VideoFormat,
    capture_cursor: Option<bool>,
    capture_border: Option<bool>,
) -> Vec<String> {
    let mut args = vec![
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
    ];
    if let Some(capture_cursor) = capture_cursor {
        args.extend(["--capture-cursor".to_owned(), capture_cursor.to_string()]);
    }
    if let Some(capture_border) = capture_border {
        args.extend(["--capture-border".to_owned(), capture_border.to_string()]);
    }
    args
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
fn helper_exit_message(status: std::process::ExitStatus, diagnostics: &str) -> String {
    if diagnostics.is_empty() {
        format!("Windows capture helper exited with {status}")
    } else {
        format!("Windows capture helper exited with {status}: {diagnostics}")
    }
}

#[cfg(target_os = "windows")]
fn classify_helper_exit(status: std::process::ExitStatus, diagnostics: &str) -> CaptureError {
    let message = helper_exit_message(status, diagnostics);
    if is_permission_failure(&message) {
        CaptureError::PermissionDenied
    } else {
        CaptureError::PlatformUnavailable { message }
    }
}

#[cfg(target_os = "windows")]
fn is_permission_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "access is denied",
        "permission denied",
        "privacy",
        "not authorized",
        "unauthorized",
    ]
    .iter()
    .any(|marker| message.contains(marker))
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
            capture_helper_args("wgc-screen-2", format, Some(false), Some(false)),
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
                "--capture-cursor",
                "false",
                "--capture-border",
                "false",
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn helper_frame_mailbox_keeps_only_the_newest_frame() {
        let format = VideoFormat::new(1, 1, obs_rs_media::FrameRate::new(30, 1).unwrap())
            .expect("valid format");
        let mailbox = HelperFrameMailbox::default();
        mailbox.publish(VideoFrame::solid(format, Timestamp::ZERO, [255, 0, 0, 255]));
        mailbox.publish(VideoFrame::solid(
            format,
            Timestamp::from_millis(33),
            [0, 255, 0, 255],
        ));

        let frame = mailbox.take_latest().expect("latest frame");
        assert_eq!(frame.timestamp(), Timestamp::from_millis(33));
        assert_eq!(frame.pixels(), &[0, 255, 0, 255]);
        assert!(mailbox.take_latest().is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn helper_permission_diagnostics_are_classified_without_false_positives() {
        assert!(is_permission_failure(
            "Windows Graphics Capture failed: access is denied"
        ));
        assert!(is_permission_failure(
            "the camera is blocked by a privacy setting"
        ));
        assert!(is_permission_failure("capture is not authorized"));
        assert!(!is_permission_failure(
            "the selected display is no longer available"
        ));
        assert!(!is_permission_failure(
            "Windows Graphics Capture target closed"
        ));
    }
}
