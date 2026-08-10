use std::io::Read;

use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

use super::{
    device::VideoCaptureDevice,
    error::CaptureError,
    protocol::{
        io_error, read_exact_capture, FRAME_STREAM_HEADER_BYTES, FRAME_STREAM_MAGIC,
        MAX_FRAME_STREAM_PACKET_BYTES,
    },
    types::{CaptureDeviceInfo, CaptureKind, CapturePermission},
};

/// A capture device that reads length-checked RGBA frames from any Rust reader.
///
/// `R` can be a file, pipe, in-memory cursor, or `TcpStream`. The reader is kept
/// behind the same lifecycle and permission contract as platform capture devices;
/// no native ABI or unchecked callback is required.
pub struct StreamCaptureDevice<R> {
    info: CaptureDeviceInfo,
    reader: R,
    format: Option<VideoFormat>,
    frame_index: u64,
}

impl<R> StreamCaptureDevice<R>
where
    R: Read + Send,
{
    /// Creates a stream-backed device with a caller-selected capture kind.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::InvalidDevice`] when the ID or name is invalid.
    pub fn new(id: &str, name: &str, kind: CaptureKind, reader: R) -> Result<Self, CaptureError> {
        Ok(Self {
            info: CaptureDeviceInfo::new(id, name, kind)?,
            reader,
            format: None,
            frame_index: 0,
        })
    }

    /// Returns the number of packets decoded since the last start.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Updates the permission state used by the lifecycle gate.
    pub const fn set_permission(&mut self, permission: CapturePermission) {
        self.info.set_permission(permission);
    }

    fn read_packet(&mut self, format: VideoFormat) -> Result<Option<VideoFrame>, CaptureError> {
        let mut header = [0_u8; FRAME_STREAM_HEADER_BYTES];
        let first_read = self
            .reader
            .read(&mut header)
            .map_err(|error| io_error(&error))?;
        if first_read == 0 {
            return Ok(None);
        }
        if first_read < header.len() {
            read_exact_capture(&mut self.reader, &mut header[first_read..])?;
        }
        if &header[..FRAME_STREAM_MAGIC.len()] != FRAME_STREAM_MAGIC {
            return Err(CaptureError::InvalidFrameHeader);
        }

        let width = u32::from_le_bytes(header[8..12].try_into().expect("fixed header width"));
        let height = u32::from_le_bytes(header[12..16].try_into().expect("fixed header height"));
        let numerator =
            u32::from_le_bytes(header[16..20].try_into().expect("fixed header numerator"));
        let denominator =
            u32::from_le_bytes(header[20..24].try_into().expect("fixed header denominator"));
        let timestamp =
            u64::from_le_bytes(header[24..32].try_into().expect("fixed header timestamp"));
        let payload_bytes = u64::from_le_bytes(
            header[32..40]
                .try_into()
                .expect("fixed header payload length"),
        );
        let rate = FrameRate::new(numerator, denominator).map_err(CaptureError::Media)?;
        let actual_format = VideoFormat::new(width, height, rate).map_err(CaptureError::Media)?;
        if actual_format != format {
            return Err(CaptureError::FrameFormatMismatch {
                expected: format,
                actual: actual_format,
            });
        }
        let expected_bytes = format.rgba_bytes();
        let actual_bytes = usize::try_from(payload_bytes).unwrap_or(usize::MAX);
        if actual_bytes != expected_bytes {
            return Err(CaptureError::FrameBufferSize {
                expected: expected_bytes,
                actual: actual_bytes,
            });
        }
        let packet_bytes = FRAME_STREAM_HEADER_BYTES
            .checked_add(actual_bytes)
            .ok_or(CaptureError::FramePacketTooLarge { bytes: u64::MAX })?;
        if packet_bytes > MAX_FRAME_STREAM_PACKET_BYTES {
            return Err(CaptureError::FramePacketTooLarge {
                bytes: u64::try_from(packet_bytes).unwrap_or(u64::MAX),
            });
        }
        let mut pixels = vec![0_u8; expected_bytes];
        read_exact_capture(&mut self.reader, &mut pixels)?;
        VideoFrame::new(format, Timestamp::from_nanos(timestamp), pixels)
            .map(Some)
            .map_err(CaptureError::Media)
    }
}

impl<R> VideoCaptureDevice for StreamCaptureDevice<R>
where
    R: Read + Send,
{
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

    fn next_frame(&mut self, _timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let Some(format) = self.format else {
            return Err(CaptureError::NotRunning);
        };
        let frame = self.read_packet(format)?;
        if frame.is_some() {
            self.frame_index = self
                .frame_index
                .checked_add(1)
                .ok_or(CaptureError::FrameCounterExhausted)?;
        }
        Ok(frame)
    }
}
