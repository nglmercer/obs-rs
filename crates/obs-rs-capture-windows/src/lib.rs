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
    process::{Child, Command, Stdio},
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
            let version = output
                .lines()
                .find_map(|line| line.strip_prefix("OBSRWIN1\tVERSION\t"))
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .ok_or_else(|| CaptureError::Protocol {
                    message: "Windows helper version reply is missing".to_owned(),
                })?;
            Ok(version.to_owned())
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
            match self.run_helper(&["--protocol", WINDOWS_HELPER_PROTOCOL, "--discover"]) {
                Ok(output) => {
                    let (devices, _) = parse_discovery_output(&output)?;
                    Ok(devices)
                }
                Err(CaptureError::PlatformUnavailable { .. }) => fallback_catalog(),
                Err(error) => Err(error),
            }
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
            } else if stable_id == "media-foundation-camera-default"
                || stable_id.starts_with("nokhwa-camera-")
            {
                CaptureKind::Camera
            } else {
                return Err(CaptureError::InvalidDevice {
                    reason: format!("unknown Windows stable ID {stable_id}"),
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
    if let Ok(current) = std::env::current_dir() {
        paths.push(current.join(HELPER_EXE));
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
    helper_search_paths()
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(HELPER_EXE))
}

fn fallback_catalog() -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
    Ok(vec![
        fallback_device(
            "wgc-screen-picker",
            "Windows Graphics Capture display picker",
            obs_rs_capture::CaptureKind::Screen,
        )?,
        fallback_device(
            "wgc-window-picker",
            "Windows Graphics Capture window picker",
            obs_rs_capture::CaptureKind::Window,
        )?,
        fallback_device(
            "media-foundation-camera-default",
            "Media Foundation default camera",
            obs_rs_capture::CaptureKind::Camera,
        )?,
    ])
}

fn fallback_device(
    id: &str,
    name: &str,
    kind: obs_rs_capture::CaptureKind,
) -> Result<CaptureDeviceInfo, CaptureError> {
    let mut device = CaptureDeviceInfo::new(id, name, kind)?
        .with_capabilities(CaptureCapabilities::new(Vec::new(), false, true).with_hotplug(true));
    device.set_permission(obs_rs_capture::CapturePermission::PromptRequired);
    Ok(device)
}

#[cfg(target_os = "windows")]
struct NativeHelperDevice {
    helper: PathBuf,
    info: CaptureDeviceInfo,
    child: Option<Child>,
    stream: Option<StreamCaptureDevice<BufReader<std::process::ChildStdout>>>,
    format: Option<VideoFormat>,
}

#[cfg(target_os = "windows")]
impl NativeHelperDevice {
    fn new(helper: &Path, stable_id: &str, kind: CaptureKind) -> Result<Self, CaptureError> {
        Ok(Self {
            helper: helper.to_owned(),
            info: CaptureDeviceInfo::new(stable_id, stable_id, kind)?,
            child: None,
            stream: None,
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
        let mut child = Command::new(&self.helper)
            .args([
                "--protocol",
                WINDOWS_HELPER_PROTOCOL,
                "--device",
                self.info.id().as_str(),
                "--width",
                &format.width().to_string(),
                "--height",
                &format.height().to_string(),
                "--fps-numerator",
                &format.frame_rate().numerator().to_string(),
                "--fps-denominator",
                &format.frame_rate().denominator().to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| CaptureError::PlatformUnavailable {
                message: format!("launch Windows capture helper: {error}"),
            })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            let _ = child.kill();
            let _ = child.wait();
            CaptureError::Protocol {
                message: "Windows capture helper has no frame stream".to_owned(),
            }
        })?;
        let mut stream = match StreamCaptureDevice::new(
            self.info.id().as_str(),
            self.info.name(),
            self.info.kind(),
            BufReader::new(stdout),
        ) {
            Ok(stream) => stream,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if let Err(error) = stream.start(format) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        self.child = Some(child);
        self.stream = Some(stream);
        self.format = Some(format);
        Ok(())
    }

    fn stop(&mut self) {
        self.stream = None;
        self.format = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        self.stream
            .as_mut()
            .ok_or(CaptureError::NotRunning)?
            .next_frame(timestamp)
    }
}

#[cfg(target_os = "windows")]
impl WindowsCaptureAdapter {
    fn run_helper(&self, args: &[&str]) -> Result<String, CaptureError> {
        let mut child = Command::new(&self.helper)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    CaptureError::PlatformUnavailable {
                        message: format!("Windows capture helper is unavailable: {error}"),
                    }
                } else {
                    CaptureError::Io {
                        message: format!("launch Windows capture helper: {error}"),
                    }
                }
            })?;
        let mut stdout = child.stdout.take().ok_or_else(|| CaptureError::Protocol {
            message: "Windows capture helper has no discovery output".to_owned(),
        })?;
        let mut output = String::new();
        let bytes = stdout
            .by_ref()
            .take(MAX_DISCOVERY_REPLY_BYTES)
            .read_to_string(&mut output)
            .map_err(|error| CaptureError::Io {
                message: format!("read Windows capture helper reply: {error}"),
            })?;
        let status = child.wait().map_err(|error| CaptureError::Io {
            message: format!("wait for Windows capture helper: {error}"),
        })?;
        if u64::try_from(bytes).unwrap_or(u64::MAX) >= MAX_DISCOVERY_REPLY_BYTES {
            return Err(CaptureError::ReplyTooLarge {
                bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
            });
        }
        if !status.success() {
            return Err(CaptureError::Protocol {
                message: format!("Windows capture helper exited with {status}"),
            });
        }
        Ok(output)
    }
}

#[cfg(target_os = "windows")]
impl Drop for NativeHelperDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

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
    for line in lines {
        if line.trim().is_empty() || line.starts_with("OBSRWIN1\tVERSION\t") {
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
                let display = WindowsDisplayInfo {
                    id: field(fields[1])?,
                    name: field(fields[2])?,
                    x: parse_field(fields[3], "display x")?,
                    y: parse_field(fields[4], "display y")?,
                    width: parse_field(fields[5], "display width")?,
                    height: parse_field(fields[6], "display height")?,
                    primary: fields[7] == "1",
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
                let name = field(fields[2])?;
                let mut device =
                    CaptureDeviceInfo::new(&id, &name, obs_rs_capture::CaptureKind::Window)?
                        .with_capabilities(
                            CaptureCapabilities::new(Vec::new(), false, true).with_hotplug(true),
                        );
                device.set_permission(obs_rs_capture::CapturePermission::Granted);
                devices.push(device);
            }
            Some("camera") if fields.len() == 3 => {
                let id = field(fields[1])?;
                let name = field(fields[2])?;
                let mut device =
                    CaptureDeviceInfo::new(&id, &name, obs_rs_capture::CaptureKind::Camera)?
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
    devices.sort_by(|left, right| left.id().cmp(right.id()));
    devices.dedup_by(|left, right| left.id() == right.id());
    displays.sort_by(|left, right| left.id.cmp(&right.id));
    displays.dedup_by(|left, right| left.id == right.id);
    Ok((devices, displays))
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
        let output = "OBSRWIN1\tDISCOVERY\t1\nOBSRWIN1\tVERSION\t0.1.0\nscreen\twgc-screen-1\tPrimary\t0\t0\t1920\t1080\t1\nwindow\twgc-window-abcd\tEditor\ncamera\tnokhwa-camera-0\tUSB camera\n";
        let (devices, displays) = parse_discovery_output(output).expect("valid discovery");
        assert_eq!(devices.len(), 3);
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
        {
            let devices = WindowsCaptureAdapter::new("__missing_obs_rs_helper__.exe")
                .discover()
                .expect("fallback catalog");
            assert_eq!(devices.len(), 3);
        }
        #[cfg(not(target_os = "windows"))]
        assert!(matches!(
            WindowsCaptureAdapter::default().discover(),
            Err(CaptureError::PlatformUnavailable { .. })
        ));
    }
}
