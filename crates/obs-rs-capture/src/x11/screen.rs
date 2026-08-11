use std::{env, io::Write, os::unix::net::UnixStream};

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::super::{
    CaptureDeviceInfo, CaptureError, CaptureKind, CapturePermission, VideoCaptureDevice,
};
use super::{
    connection::{display_socket, handshake, read_authorization},
    error::{protocol_error, read_exact_x11, x11_io_error},
    image::{decode_pixels, packed_row_bytes, padded_row_bytes},
    protocol::{read_u32_le, ServerInfo, X11_GET_IMAGE, X11_MAX_REPLY_BYTES, X11_Z_PIXMAP},
};

pub struct X11CaptureDevice {
    info: CaptureDeviceInfo,
    stream: UnixStream,
    server: ServerInfo,
    format: Option<VideoFormat>,
    frame_index: u64,
}

impl X11CaptureDevice {
    /// Connects to a local X11 display such as `:0` or `unix:0`.
    ///
    /// The constructor performs the X11 setup handshake and reads the first
    /// screen's root window, visual masks, and image layout. If `XAUTHORITY` is
    /// set, or if `$HOME/.Xauthority` exists, a matching magic-cookie record is
    /// sent during the handshake.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::PlatformUnavailable`] when the display cannot be
    /// reached, or [`CaptureError::Protocol`] when the server setup is invalid.
    pub fn connect(display: &str, id: &str, name: &str) -> Result<Self, CaptureError> {
        let (socket_path, display_number) = display_socket(display)?;
        let authorization = read_authorization(&display_number);
        let mut stream = UnixStream::connect(&socket_path).map_err(|error| {
            CaptureError::PlatformUnavailable {
                message: format!("connect to {}: {error}", socket_path.display()),
            }
        })?;
        let server = handshake(&mut stream, authorization.as_ref())?;
        let info = CaptureDeviceInfo::new(id, name, CaptureKind::Screen)?;
        Ok(Self {
            info,
            stream,
            server,
            format: None,
            frame_index: 0,
        })
    }

    /// Connects using the `DISPLAY` environment setting.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::PlatformUnavailable`] when `DISPLAY` is missing,
    /// or propagates the display connection and protocol errors.
    pub fn connect_from_environment(id: &str, name: &str) -> Result<Self, CaptureError> {
        let display = env::var("DISPLAY").map_err(|error| CaptureError::PlatformUnavailable {
            message: format!("DISPLAY is unavailable: {error}"),
        })?;
        Self::connect(&display, id, name)
    }

    /// Returns the root screen dimensions reported by X11.
    #[must_use]
    pub const fn screen_size(&self) -> (u32, u32) {
        (self.server.width, self.server.height)
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

    fn read_frame(
        &mut self,
        format: VideoFormat,
        timestamp: Timestamp,
    ) -> Result<VideoFrame, CaptureError> {
        let width =
            u16::try_from(format.width()).map_err(|_| CaptureError::UnsupportedFormat(format))?;
        let height =
            u16::try_from(format.height()).map_err(|_| CaptureError::UnsupportedFormat(format))?;
        let plane_mask = if self.server.depth == 32 {
            u32::MAX
        } else {
            (1_u32 << self.server.depth) - 1
        };
        // GetImage is a fixed 20-byte request, so it is built on the stack
        // rather than in a per-frame heap buffer. The x and y fields at
        // [8..12] stay zero.
        let mut request = [0_u8; 20];
        request[0] = X11_GET_IMAGE;
        request[1] = X11_Z_PIXMAP;
        request[2..4].copy_from_slice(&5_u16.to_le_bytes());
        request[4..8].copy_from_slice(&self.server.root.to_le_bytes());
        request[12..14].copy_from_slice(&width.to_le_bytes());
        request[14..16].copy_from_slice(&height.to_le_bytes());
        request[16..20].copy_from_slice(&plane_mask.to_le_bytes());
        self.stream
            .write_all(&request)
            .map_err(|error| x11_io_error(&error))?;

        let mut response = [0_u8; 32];
        read_exact_x11(&mut self.stream, &mut response)?;
        if response[0] != 1 {
            return Err(protocol_error(format!(
                "GetImage returned X11 error code {} for opcode {} (resource 0x{:x}, root 0x{:x})",
                response[1],
                response[10],
                read_u32_le(&response, 4).unwrap_or_default(),
                self.server.root
            )));
        }
        if response[1] != self.server.depth {
            return Err(protocol_error("GetImage returned an unexpected root depth"));
        }
        let visual = read_u32_le(&response, 8)?;
        if visual != self.server.visual {
            return Err(protocol_error("GetImage returned an unexpected visual"));
        }
        let data_words = u64::from(read_u32_le(&response, 4)?);
        let data_bytes = data_words
            .checked_mul(4)
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        let data_bytes_usize = usize::try_from(data_bytes)
            .map_err(|_| CaptureError::ReplyTooLarge { bytes: data_bytes })?;
        if data_bytes_usize > X11_MAX_REPLY_BYTES {
            return Err(CaptureError::ReplyTooLarge { bytes: data_bytes });
        }
        let mut data = vec![0_u8; data_bytes_usize];
        read_exact_x11(&mut self.stream, &mut data)?;

        let row_bytes = packed_row_bytes(usize::from(width), self.server.bits_per_pixel)?;
        let row_stride = padded_row_bytes(row_bytes, self.server.scanline_pad)?;
        let required_bytes = row_stride
            .checked_mul(usize::from(height))
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        if data.len() < required_bytes {
            return Err(protocol_error(
                "GetImage payload is shorter than its scanlines",
            ));
        }
        let pixels = decode_pixels(
            usize::from(width),
            usize::from(height),
            row_stride,
            self.server.bits_per_pixel,
            self.server.image_byte_order,
            self.server.masks,
            &data,
        )?;
        VideoFrame::new(format, timestamp, pixels).map_err(CaptureError::Media)
    }
}

impl VideoCaptureDevice for X11CaptureDevice {
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
        if format.width() > self.server.width
            || format.height() > self.server.height
            || format.width() > u32::from(u16::MAX)
            || format.height() > u32::from(u16::MAX)
        {
            return Err(CaptureError::UnsupportedFormat(format));
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
        let frame = self.read_frame(format, timestamp)?;
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or(CaptureError::FrameCounterExhausted)?;
        Ok(Some(frame))
    }
}
