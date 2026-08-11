//! Linux V4L2 camera capture through a bounded `ffmpeg` process boundary.
//!
//! The project deliberately keeps native capture out of the GUI and engine
//! crates. `ffmpeg` negotiates the camera's native format and writes a fixed
//! RGBA frame stream to stdout; this adapter only owns that stream and turns
//! each complete frame into the portable [`VideoFrame`] contract.

#![cfg(target_os = "linux")]

use std::{
    io::ErrorKind,
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

    /// Returns the discrete capture sizes this camera advertises.
    ///
    /// An empty list means the camera could not be interrogated, which is not
    /// an error: `ffmpeg` will then negotiate the camera's own default and the
    /// scaler still produces the canvas size.
    #[must_use]
    pub fn supported_sizes(&self) -> Vec<(u32, u32)> {
        let Ok(output) = Command::new("v4l2-ctl")
            .args([
                "--device",
                self.path.to_string_lossy().as_ref(),
                "--list-formats-ext",
            ])
            .output()
        else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        parse_discrete_sizes(&String::from_utf8_lossy(&output.stdout))
    }

    /// Checks the device node is present and readable before spawning anything.
    ///
    /// A camera that another application holds open, or that this user has no
    /// permission for, is a different situation from one that is missing, and
    /// the operator needs to be told which. Mapping the OS error here also
    /// means the failure is reported before an `ffmpeg` process is spawned only
    /// to die immediately.
    fn check_access(&self) -> Result<(), CaptureError> {
        match std::fs::File::open(&self.path) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                Err(CaptureError::PermissionDenied)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                Err(CaptureError::PlatformUnavailable {
                    message: format!("camera {} is not connected", self.path.display()),
                })
            }
            Err(error) => Err(CaptureError::PlatformUnavailable {
                message: format!("camera {} cannot be opened: {error}", self.path.display()),
            }),
        }
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
            ]);
        // Asking for a mode the camera actually has avoids the driver either
        // refusing the request or silently handing back its default, which is
        // how a camera ends up delivering a differently shaped image than the
        // scaler was told to expect.
        if let Some((width, height)) = negotiate_size(&self.supported_sizes(), format) {
            command.args(["-video_size", &format!("{width}x{height}")]);
        }
        command
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

/// Chooses the capture mode closest to the canvas without going under it.
///
/// Upscaling a camera loses detail that a larger available mode would have
/// kept, so the smallest mode that still covers the canvas wins. When nothing
/// covers it — a 640x480 webcam feeding a 1080p canvas — the largest mode is
/// the best available, and the scaler letterboxes from there.
#[must_use]
fn negotiate_size(sizes: &[(u32, u32)], format: VideoFormat) -> Option<(u32, u32)> {
    if sizes.is_empty() {
        return None;
    }
    let (width, height) = (format.width(), format.height());
    sizes
        .iter()
        .filter(|(candidate_width, candidate_height)| {
            *candidate_width >= width && *candidate_height >= height
        })
        .min_by_key(|(candidate_width, candidate_height)| {
            u64::from(*candidate_width) * u64::from(*candidate_height)
        })
        .or_else(|| {
            sizes
                .iter()
                .max_by_key(|(candidate_width, candidate_height)| {
                    u64::from(*candidate_width) * u64::from(*candidate_height)
                })
        })
        .copied()
}

/// Extracts the discrete sizes from `v4l2-ctl --list-formats-ext` output.
///
/// Only discrete sizes are taken: a stepwise range would need the driver's step
/// and alignment rules to pick a legal value from, and guessing one wrong is
/// worse than letting `ffmpeg` negotiate the default.
#[must_use]
fn parse_discrete_sizes(listing: &str) -> Vec<(u32, u32)> {
    let mut sizes = listing
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Size: Discrete ")?;
            let (width, height) = rest.split_once('x')?;
            Some((
                width.trim().parse::<u32>().ok()?,
                height.trim().parse::<u32>().ok()?,
            ))
        })
        .filter(|(width, height)| *width > 0 && *height > 0)
        .collect::<Vec<_>>();
    sizes.sort_unstable();
    sizes.dedup();
    sizes
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
        self.check_access()?;
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
        let pixels = reader.latest_shared_frame(&what)?;
        let Some(pixels) = pixels else {
            // The reader has not received the first camera frame yet. A
            // non-blocking empty result keeps the GUI timer responsive; the
            // following tick will consume the frame once it arrives.
            return Ok(None);
        };
        let frame =
            VideoFrame::from_shared(format, timestamp, pixels).map_err(CaptureError::Media)?;
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

    fn format(width: u32, height: u32) -> VideoFormat {
        VideoFormat::new(
            width,
            height,
            obs_rs_media::FrameRate::new(30, 1).expect("rate"),
        )
        .expect("format")
    }

    #[test]
    fn discrete_camera_modes_are_parsed_and_stepwise_ranges_ignored() {
        let listing = "\
ioctl: VIDIOC_ENUM_FMT
	Type: Video Capture

	[0]: 'YUYV' (YUYV 4:2:2)
		Size: Discrete 640x480
			Interval: Discrete 0.033s (30.000 fps)
		Size: Discrete 1280x720
		Size: Discrete 640x480
	[1]: 'MJPG' (Motion-JPEG, compressed)
		Size: Stepwise 32x32 - 2592x1944 with step 2/2
";

        let sizes = parse_discrete_sizes(listing);

        assert_eq!(
            sizes,
            vec![(640, 480), (1280, 720)],
            "duplicates collapse and a stepwise range is not a discrete choice"
        );
    }

    #[test]
    fn an_uninterrogable_camera_reports_no_modes_rather_than_a_wrong_one() {
        assert!(parse_discrete_sizes("").is_empty());
        assert!(parse_discrete_sizes("v4l2-ctl: not found").is_empty());
        assert!(
            parse_discrete_sizes("\t\tSize: Discrete 0x0").is_empty(),
            "a zero-sized mode is not capturable"
        );
    }

    #[test]
    fn negotiation_picks_the_smallest_mode_that_covers_the_canvas() {
        let sizes = [(320, 240), (640, 480), (1280, 720), (1920, 1080)];

        assert_eq!(
            negotiate_size(&sizes, format(640, 360)),
            Some((640, 480)),
            "the cheapest mode that still covers the canvas is the right one"
        );
        assert_eq!(
            negotiate_size(&sizes, format(1280, 720)),
            Some((1280, 720)),
            "an exact match costs no scaling at all"
        );
    }

    #[test]
    fn negotiation_never_upscales_when_a_larger_mode_exists() {
        let sizes = [(320, 240), (1280, 720)];

        // 320x240 cannot cover a 640x360 canvas, so the larger mode wins even
        // though it costs more bandwidth: upscaling loses detail the camera has.
        assert_eq!(negotiate_size(&sizes, format(640, 360)), Some((1280, 720)));
    }

    #[test]
    fn negotiation_falls_back_to_the_largest_mode_a_small_camera_offers() {
        let sizes = [(320, 240), (640, 480)];

        assert_eq!(
            negotiate_size(&sizes, format(1920, 1080)),
            Some((640, 480)),
            "a webcam that cannot fill the canvas still gives its best mode"
        );
    }

    #[test]
    fn negotiation_defers_to_ffmpeg_when_the_camera_lists_nothing() {
        assert_eq!(
            negotiate_size(&[], format(640, 360)),
            None,
            "an unknown camera must not be forced into a guessed mode"
        );
    }

    #[test]
    fn a_missing_camera_node_is_reported_before_any_process_is_spawned() {
        let mut device =
            V4l2CaptureDevice::from_device_id("v4l2-video99", "Absent camera").expect("stable ID");

        let error = device
            .start(format(64, 36))
            .expect_err("a camera that is not connected cannot start");

        assert!(
            matches!(
                error,
                CaptureError::PlatformUnavailable { .. } | CaptureError::PermissionDenied
            ),
            "the reason has to distinguish missing from forbidden: {error:?}"
        );
        assert!(!device.is_running());
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
