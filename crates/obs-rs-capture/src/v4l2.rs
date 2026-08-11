//! Linux V4L2 camera capture through a bounded `ffmpeg` process boundary.
//!
//! The project deliberately keeps native capture out of the GUI and engine
//! crates. `ffmpeg` negotiates the camera's native format and writes a fixed
//! RGBA frame stream to stdout; this adapter only owns that stream and turns
//! each complete frame into the portable [`VideoFrame`] contract.

#![cfg(target_os = "linux")]

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{
    device::VideoCaptureDevice,
    error::CaptureError,
    raw_reader::RawFrameReader,
    types::{CaptureDeviceInfo, CaptureKind, CapturePermission},
};

/// A process-backed V4L2 camera that emits one RGBA frame per read request.
pub struct V4l2CaptureDevice {
    info: CaptureDeviceInfo,
    path: PathBuf,
    format: Option<VideoFormat>,
    reader: Option<RawFrameReader>,
    frame_index: u64,
}

impl V4l2CaptureDevice {
    /// Creates a camera for an explicit `/dev/video*` path.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] when the descriptor or path is
    /// invalid. Starting the process is deferred until [`Self::start`].
    pub fn new(id: &str, name: &str, path: impl Into<PathBuf>) -> Result<Self, CaptureError> {
        let path = path.into();
        if !is_video_device_path(&path) {
            return Err(CaptureError::InvalidDevice {
                reason: format!("V4L2 path is not a /dev/video device: {}", path.display()),
            });
        }
        Ok(Self {
            info: CaptureDeviceInfo::new(id, name, CaptureKind::Camera)?,
            path,
            format: None,
            reader: None,
            frame_index: 0,
        })
    }

    /// Creates a camera from the stable ID returned by platform discovery.
    ///
    /// IDs use the `v4l2-videoN` form, so the mapping remains deterministic and
    /// does not expose an arbitrary path in project files.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] when `id` does not use the
    /// stable V4L2 naming convention.
    pub fn from_device_id(id: &str, name: &str) -> Result<Self, CaptureError> {
        let node = id
            .strip_prefix("v4l2-")
            .ok_or_else(|| CaptureError::InvalidDevice {
                reason: format!("unsupported V4L2 device ID: {id}"),
            })?;
        Self::new(id, name, Path::new("/dev").join(node))
    }

    /// Returns the OS path used by this adapter.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the number of decoded camera frames.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Updates the permission state used by the lifecycle gate.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.info.set_permission(permission);
    }

    fn spawn_capture(&mut self, format: VideoFormat) -> Result<(), CaptureError> {
        let frame_rate = format.frame_rate();
        let mut command = Command::new("ffmpeg");
        command
            .args(["-hide_banner", "-loglevel", "error"])
            .args(["-f", "v4l2"])
            .args([
                "-framerate",
                &format!("{}/{}", frame_rate.numerator(), frame_rate.denominator()),
            ])
            .args(["-i", self.path.to_string_lossy().as_ref()])
            .args([
                "-vf",
                &format!("scale={}:{}", format.width(), format.height()),
            ])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
        self.reader = Some(RawFrameReader::spawn(
            command,
            format.rgba_bytes(),
            &format!("ffmpeg for {}", self.path.display()),
        )?);
        Ok(())
    }
}

impl VideoCaptureDevice for V4l2CaptureDevice {
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
        self.spawn_capture(format)?;
        self.format = Some(format);
        self.frame_index = 0;
        Ok(())
    }

    fn stop(&mut self) {
        self.reader = None;
        self.format = None;
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(format) = self.format else {
            return Err(CaptureError::NotRunning);
        };
        let reader = self.reader.as_ref().ok_or(CaptureError::NotRunning)?;
        let what = self.path.display().to_string();
        let pixels = reader.latest_frame(&what)?;
        let Some(pixels) = pixels else {
            // The reader has not received the first camera frame yet. A
            // non-blocking empty result keeps the GUI timer responsive; the
            // following tick will consume the frame once it arrives.
            return Ok(None);
        };
        let frame = VideoFrame::new(format, timestamp, pixels).map_err(CaptureError::Media)?;
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or(CaptureError::FrameCounterExhausted)?;
        Ok(Some(frame))
    }
}

impl Drop for V4l2CaptureDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

fn is_video_device_path(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    path.starts_with("/dev")
        && file_name.strip_prefix("video").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_v4l2_ids_map_to_device_nodes() {
        let device =
            V4l2CaptureDevice::from_device_id("v4l2-video3", "Camera 3").expect("stable ID");
        assert_eq!(device.path(), Path::new("/dev/video3"));
        assert!(!device.is_running());
    }

    #[test]
    fn arbitrary_paths_are_rejected() {
        let result = V4l2CaptureDevice::new("v4l2-camera", "Camera", "/tmp/camera");
        let error = result.err().expect("non-video path");
        assert!(error.to_string().contains("not a /dev/video device"));
    }

    #[test]
    #[ignore = "requires a live V4L2 camera and ffmpeg"]
    fn live_camera_produces_a_non_blocking_rgba_frame() {
        let format = VideoFormat::new(64, 36, obs_rs_media::FrameRate::new(30, 1).expect("rate"))
            .expect("format");
        let mut device = V4l2CaptureDevice::from_device_id("v4l2-video0", "Camera 0")
            .expect("camera descriptor");
        device.start(format).expect("camera should start");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let frame = loop {
            if let Some(frame) = device
                .next_frame(Timestamp::ZERO)
                .expect("camera frame read")
            {
                break frame;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "camera did not deliver a frame"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(frame.format(), format);
        assert_eq!(frame.pixels().len(), format.rgba_bytes());
        device.stop();
    }
}
