//! Safe Rust host boundary for the native Windows capture helper.
//!
//! The separately built helper owns Windows Graphics Capture/Media Foundation
//! objects and writes bounded OBSFRM01 packets. Native COM/D3D handles never
//! cross into portable Rust.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::{
    io::BufReader,
    process::{Child, Command, Stdio},
};

#[cfg(target_os = "windows")]
use obs_rs_capture::{CaptureCapabilities, CaptureKind, CapturePermission, StreamCaptureDevice};
use obs_rs_capture::{CaptureDeviceInfo, CaptureError, PlatformCaptureAdapter, VideoCaptureDevice};
#[cfg(target_os = "windows")]
use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

pub const WINDOWS_HELPER_PROTOCOL: &str = "OBSRWIN1";

/// Windows Graphics Capture/Media Foundation adapter using a native helper.
#[derive(Clone, Debug)]
pub struct WindowsCaptureAdapter {
    helper: PathBuf,
}

impl Default for WindowsCaptureAdapter {
    fn default() -> Self {
        Self::new("obs-rs-capture-windows-helper.exe")
    }
}

impl WindowsCaptureAdapter {
    #[must_use]
    pub fn new(helper: impl Into<PathBuf>) -> Self {
        Self {
            helper: helper.into(),
        }
    }

    #[must_use]
    pub fn helper(&self) -> &Path {
        &self.helper
    }
}

impl PlatformCaptureAdapter for WindowsCaptureAdapter {
    fn backend_name(&self) -> &'static str {
        "windows-graphics-capture-media-foundation"
    }

    fn discover(&self) -> Result<Vec<CaptureDeviceInfo>, CaptureError> {
        #[cfg(target_os = "windows")]
        {
            let mut screen = CaptureDeviceInfo::new(
                "wgc-screen-picker",
                "Windows Graphics Capture display picker",
                CaptureKind::Screen,
            )?
            .with_capabilities(CaptureCapabilities::new(Vec::new(), false, true));
            screen.set_permission(CapturePermission::PromptRequired);
            let mut window = CaptureDeviceInfo::new(
                "wgc-window-picker",
                "Windows Graphics Capture window picker",
                CaptureKind::Window,
            )?
            .with_capabilities(CaptureCapabilities::new(Vec::new(), false, true));
            window.set_permission(CapturePermission::PromptRequired);
            let mut camera = CaptureDeviceInfo::new(
                "media-foundation-camera-default",
                "Media Foundation default camera",
                CaptureKind::Camera,
            )?
            .with_capabilities(CaptureCapabilities::new(Vec::new(), false, true));
            camera.set_permission(CapturePermission::PromptRequired);
            Ok(vec![screen, window, camera])
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
            let kind = if stable_id == "wgc-screen-picker" {
                CaptureKind::Screen
            } else if stable_id == "wgc-window-picker" {
                CaptureKind::Window
            } else if stable_id == "media-foundation-camera-default" {
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
        let stdout = child.stdout.take().ok_or_else(|| CaptureError::Protocol {
            message: "Windows capture helper has no frame stream".to_owned(),
        })?;
        let mut stream = StreamCaptureDevice::new(
            self.info.id().as_str(),
            self.info.name(),
            self.info.kind(),
            BufReader::new(stdout),
        )?;
        stream.start(format)?;
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
impl Drop for NativeHelperDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_windows_hosts_report_a_typed_unavailable_state() {
        #[cfg(target_os = "windows")]
        {
            let devices = WindowsCaptureAdapter::default()
                .discover()
                .expect("catalog");
            assert_eq!(devices.len(), 3);
            assert!(devices
                .iter()
                .all(|device| matches!(device.permission(), CapturePermission::PromptRequired)));
        }
        #[cfg(not(target_os = "windows"))]
        assert!(matches!(
            WindowsCaptureAdapter::default().discover(),
            Err(CaptureError::PlatformUnavailable { .. })
        ));
    }
}
