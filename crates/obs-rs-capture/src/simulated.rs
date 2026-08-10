use obs_rs_media::{MediaError, Timestamp, VideoFormat, VideoFrame};

use super::{
    device::VideoCaptureDevice,
    error::CaptureError,
    types::{CaptureDeviceInfo, CaptureKind, CapturePermission},
};

pub struct TestPatternDevice {
    info: CaptureDeviceInfo,
    format: Option<VideoFormat>,
    frame_index: u64,
}

impl TestPatternDevice {
    /// Creates a test device with stable metadata.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] when the descriptor is invalid.
    pub fn new(id: &str, name: &str) -> Result<Self, CaptureError> {
        Ok(Self {
            info: CaptureDeviceInfo::new(id, name, CaptureKind::TestPattern)?,
            format: None,
            frame_index: 0,
        })
    }

    /// Returns the current frame counter.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Updates the simulated permission state for lifecycle tests.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.info.set_permission(permission);
    }
}

impl VideoCaptureDevice for TestPatternDevice {
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
        self.format = Some(format);
        self.frame_index = 0;
        Ok(())
    }

    fn stop(&mut self) {
        self.format = None;
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(format) = self.format else {
            return Err(CaptureError::NotRunning);
        };
        let frame = simulated_frame(format, timestamp, self.frame_index, self.info.kind())?;
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or(CaptureError::FrameCounterExhausted)?;
        Ok(Some(frame))
    }
}

/// A deterministic CPU fallback for screen, window, or camera capture.
pub struct SimulatedCaptureDevice {
    info: CaptureDeviceInfo,
    format: Option<VideoFormat>,
    frame_index: u64,
}

impl SimulatedCaptureDevice {
    /// Creates a simulated device for any supported capture kind.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] when the descriptor is invalid.
    pub fn new(id: &str, name: &str, kind: CaptureKind) -> Result<Self, CaptureError> {
        Ok(Self {
            info: CaptureDeviceInfo::new(id, name, kind)?,
            format: None,
            frame_index: 0,
        })
    }

    /// Returns the current frame counter.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Updates the simulated permission state for lifecycle tests.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.info.set_permission(permission);
    }
}

impl VideoCaptureDevice for SimulatedCaptureDevice {
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
        self.format = Some(format);
        self.frame_index = 0;
        Ok(())
    }

    fn stop(&mut self) {
        self.format = None;
    }

    fn is_running(&self) -> bool {
        self.format.is_some()
    }

    fn next_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(format) = self.format else {
            return Err(CaptureError::NotRunning);
        };
        let frame = simulated_frame(format, timestamp, self.frame_index, self.info.kind())?;
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or(CaptureError::FrameCounterExhausted)?;
        Ok(Some(frame))
    }
}

fn simulated_frame(
    format: VideoFormat,
    timestamp: Timestamp,
    frame_index: u64,
    kind: CaptureKind,
) -> Result<VideoFrame, CaptureError> {
    let width = usize::try_from(format.width())
        .map_err(|_| CaptureError::Media(MediaError::FrameTooLarge))?;
    let height = usize::try_from(format.height())
        .map_err(|_| CaptureError::Media(MediaError::FrameTooLarge))?;
    let mut pixels = vec![0_u8; format.rgba_bytes()];
    let phase = frame_index % 2;
    let variant = match kind {
        CaptureKind::TestPattern => 0,
        CaptureKind::Screen => 16,
        CaptureKind::Window => 32,
        CaptureKind::Camera => 48,
        CaptureKind::External => 64,
    };
    // The column gradient repeats for every scanline and the row gradient is
    // constant across one, so both are tabulated once instead of recomputed per
    // pixel.
    let column_gradient: Vec<u8> = (0..width)
        .map(|x| gradient_byte(x, width).saturating_add(variant / 2))
        .collect();

    for (y, row) in pixels.chunks_exact_mut(width * 4).enumerate() {
        let row_gradient = gradient_byte(y, height).saturating_add(variant / 3);
        let row_tile = y / 16;
        for (x, (pixel, column)) in row.chunks_exact_mut(4).zip(&column_gradient).enumerate() {
            let tile = ((x / 16 + row_tile) as u64 + phase) % 2;
            pixel[0] = if tile == 0 {
                32_u8.saturating_add(variant)
            } else {
                224_u8.saturating_sub(variant)
            };
            pixel[1] = *column;
            pixel[2] = row_gradient;
            pixel[3] = 255;
        }
    }
    VideoFrame::new(format, timestamp, pixels).map_err(CaptureError::Media)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "min constrains the value to 0..=255, so the cast is exact"
)]
fn gradient_byte(value: usize, size: usize) -> u8 {
    // Both inputs are frame dimensions or indices bounded by them, so widening
    // to u64 is lossless on every supported target.
    let value = value as u64;
    let size = (size.max(1)) as u64;
    let scaled = value.saturating_mul(255) / size;
    scaled.min(u64::from(u8::MAX)) as u8
}
