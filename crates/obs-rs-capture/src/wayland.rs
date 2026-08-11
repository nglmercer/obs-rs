//! Wayland screen capture through the desktop portal and `PipeWire`.
//!
//! The portal hands out a `PipeWire` node rather than pixels, so the frames are
//! read from that node with the same bounded "newest complete frame" reader the
//! X11 process fallback uses. `gst-launch-1.0` is the `PipeWire` reader here:
//! it is the standard userspace tool for the job, it is driven with a fixed
//! argument list and no shell, and it keeps this crate free of C bindings.

use std::{env, process::Command};

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use crate::{
    dbus::{open_screencast, CursorMode, ScreenCastSession},
    device::VideoCaptureDevice,
    error::CaptureError,
    raw_reader::RawFrameReader,
    types::{CaptureDeviceInfo, CaptureKind, CapturePermission},
};

/// Returns whether this process is running in a Wayland session.
///
/// The X11 adapter still works under Xwayland but only ever sees Xwayland's own
/// surfaces, which is why the session type decides which backend a screen
/// source should use rather than merely which one is reachable.
#[must_use]
pub fn wayland_session_available() -> bool {
    env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
}

/// A screen capture device backed by the desktop portal.
pub struct WaylandCaptureDevice {
    info: CaptureDeviceInfo,
    /// Held for the lifetime of the capture: dropping it stops the stream.
    session: ScreenCastSession,
    reader: Option<RawFrameReader>,
    format: Option<VideoFormat>,
    frame_index: u64,
}

impl WaylandCaptureDevice {
    /// Opens a portal session, asking the user which screen to share.
    ///
    /// `restore_token` reopens a previous selection without a dialog when the
    /// compositor still honours it.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::PermissionDenied`] when the user cancels the
    /// portal dialog, and [`CaptureError::PlatformUnavailable`] when no portal
    /// is reachable.
    pub fn open(id: &str, name: &str, restore_token: Option<&str>) -> Result<Self, CaptureError> {
        let session = open_screencast(restore_token, CursorMode::Embedded)?;
        let info = CaptureDeviceInfo::new(id, name, CaptureKind::Screen)?;
        Ok(Self {
            info,
            session,
            reader: None,
            format: None,
            frame_index: 0,
        })
    }

    /// Returns the token that reopens this selection without prompting.
    #[must_use]
    pub fn restore_token(&self) -> Option<&str> {
        self.session.restore_token()
    }

    /// Returns the size the compositor is streaming, when it reported one.
    #[must_use]
    pub const fn stream_size(&self) -> Option<(u32, u32)> {
        self.session.size()
    }

    /// Returns the number of successfully decoded frames.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Updates the permission state used by the lifecycle gate.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.info.set_permission(permission);
    }
}

impl VideoCaptureDevice for WaylandCaptureDevice {
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
        self.reader = Some(start_pipewire_reader(self.session.node_id(), format)?);
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
        let Some(pixels) = reader.latest_frame("the PipeWire screen cast")? else {
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

/// Starts `gst-launch-1.0` reading the portal's node as RGBA frames.
fn start_pipewire_reader(
    node_id: u32,
    format: VideoFormat,
) -> Result<RawFrameReader, CaptureError> {
    let frame_rate = format.frame_rate();
    let mut command = Command::new("gst-launch-1.0");
    command.args([
        "-q",
        "pipewiresrc",
        &format!("path={node_id}"),
        // Dropping late frames keeps the reader at the scene's cadence
        // instead of queueing the compositor's backlog.
        "do-timestamp=true",
        "!",
        "videoconvert",
        "!",
        "videoscale",
        "!",
        "videorate",
        "!",
        &format!(
            "video/x-raw,format=RGBA,width={},height={},framerate={}/{}",
            format.width(),
            format.height(),
            frame_rate.numerator(),
            frame_rate.denominator()
        ),
        "!",
        "fdsink",
        "fd=1",
        "sync=false",
    ]);
    RawFrameReader::spawn(
        command,
        format.rgba_bytes(),
        &format!("the PipeWire reader for node {node_id}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_type_is_read_from_the_environment() {
        // The value is whatever this host runs; the contract is only that the
        // check agrees with the variable rather than guessing.
        assert_eq!(
            wayland_session_available(),
            env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
        );
    }

    #[test]
    #[ignore = "opens the compositor's screen-sharing dialog"]
    fn live_portal_capture_produces_a_frame() {
        use obs_rs_media::FrameRate;

        let mut device = WaylandCaptureDevice::open("wayland-screen", "Wayland screen", None)
            .expect("open the screen-cast portal");
        let format =
            VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("rate")).expect("format");
        device.start(format).expect("start the portal capture");
        let mut frame = None;
        for _ in 0..200 {
            if let Some(value) = device.next_frame(Timestamp::ZERO).expect("read a frame") {
                frame = Some(value);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let frame = frame.expect("the portal stream produced a frame");
        assert_eq!(frame.format(), format);
        assert!(
            frame.pixels().iter().any(|byte| *byte != 0),
            "a real desktop frame is not uniformly black"
        );
    }
}
