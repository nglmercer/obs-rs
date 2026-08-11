use std::{
    env,
    io::{Read, Write},
    os::unix::net::UnixStream,
};

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::super::{
    CaptureDeviceInfo, CaptureError, CaptureKind, CapturePermission, VideoCaptureDevice,
};
use super::{
    connection::{display_socket, handshake, read_authorization},
    error::{platform_error, protocol_error, read_exact_x11, x11_io_error},
    image::{decode_pixels, packed_row_bytes, padded_row_bytes},
    protocol::{read_u32_le, ServerInfo, X11_GET_IMAGE, X11_MAX_REPLY_BYTES, X11_Z_PIXMAP},
    randr::{query_monitors, X11Monitor},
};

/// The rectangle of the root window a device captures, in root coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Region {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

pub struct X11CaptureDevice {
    info: CaptureDeviceInfo,
    stream: UnixStream,
    server: ServerInfo,
    /// The captured rectangle; `None` means the complete root window.
    region: Option<Region>,
    format: Option<VideoFormat>,
    frame_index: u64,
    data: Vec<u8>,
}

/// Lists the monitors of `display` on a short-lived connection.
///
/// Returns one entry per active `RandR` monitor. Servers without `RandR` 1.5
/// report a single monitor covering the whole root window, so callers always
/// receive a selectable list.
///
/// # Errors
///
/// Returns [`CaptureError::PlatformUnavailable`] when the display cannot be
/// reached, or [`CaptureError::Protocol`] when a reply cannot be decoded.
pub fn x11_monitors(display: &str) -> Result<Vec<X11Monitor>, CaptureError> {
    let (socket_path, display_number) = display_socket(display)?;
    let authorization = read_authorization(&display_number);
    let mut stream =
        UnixStream::connect(&socket_path).map_err(|error| CaptureError::PlatformUnavailable {
            message: format!("connect to {}: {error}", socket_path.display()),
        })?;
    let server = handshake(&mut stream, authorization.as_ref())?;
    Ok(monitors_or_root(&mut stream, &server))
}

/// Returns the reported monitors, or the whole root window when `RandR` is
/// unavailable or reports nothing usable.
fn monitors_or_root(stream: &mut UnixStream, server: &ServerInfo) -> Vec<X11Monitor> {
    let monitors = query_monitors(stream, server.root).unwrap_or_default();
    if monitors.is_empty() {
        return vec![X11Monitor::new(
            "Screen".to_owned(),
            0,
            0,
            server.width,
            server.height,
            true,
        )];
    }
    monitors
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
            region: None,
            format: None,
            frame_index: 0,
            data: Vec::new(),
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

    /// Returns the dimensions of the captured rectangle.
    ///
    /// This is the monitor size once one is selected, and the root size
    /// otherwise, which is what a downstream scaler has to letterbox.
    #[must_use]
    pub fn capture_size(&self) -> (u32, u32) {
        self.region.map_or_else(
            || self.screen_size(),
            |region| (u32::from(region.width), u32::from(region.height)),
        )
    }

    /// Returns the captured rectangle as `(x, y, width, height)`.
    #[must_use]
    pub fn capture_region(&self) -> (u32, u32, u32, u32) {
        self.region.map_or_else(
            || (0, 0, self.server.width, self.server.height),
            |region| {
                (
                    u32::from(region.x),
                    u32::from(region.y),
                    u32::from(region.width),
                    u32::from(region.height),
                )
            },
        )
    }

    /// Lists the monitors this connection's server reports.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::Io`] when the socket fails and
    /// [`CaptureError::Protocol`] when a reply cannot be decoded.
    pub fn monitors(&mut self) -> Result<Vec<X11Monitor>, CaptureError> {
        Ok(monitors_or_root(&mut self.stream, &self.server))
    }

    /// Restricts capture to the `RandR` monitor named `monitor`.
    ///
    /// An empty or missing name captures the whole root window, which is the
    /// behaviour of a device that was never told which monitor to use. A name
    /// that no monitor matches is an error rather than a silent full-desktop
    /// capture, so a stale project setting is visible instead of confusing.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::PlatformUnavailable`] when no monitor matches,
    /// or the socket/protocol errors raised while enumerating monitors.
    pub fn select_monitor(&mut self, monitor: Option<&str>) -> Result<(), CaptureError> {
        let Some(wanted) = monitor.map(str::trim).filter(|name| !name.is_empty()) else {
            self.region = None;
            return Ok(());
        };
        let monitors = self.monitors()?;
        let selected = monitors
            .iter()
            .find(|candidate| candidate.name() == wanted || candidate.device_id() == wanted)
            .ok_or_else(|| platform_error(format!("X11 monitor {wanted} is not connected")))?;
        self.region = Some(Self::region_for(selected, &self.server)?);
        Ok(())
    }

    /// Clamps a monitor rectangle to the root window and converts it to the
    /// unsigned coordinates `GetImage` accepts.
    fn region_for(monitor: &X11Monitor, server: &ServerInfo) -> Result<Region, CaptureError> {
        let left = u32::try_from(monitor.x().max(0))
            .unwrap_or(0)
            .min(server.width);
        let top = u32::try_from(monitor.y().max(0))
            .unwrap_or(0)
            .min(server.height);
        let width = monitor.width().min(server.width.saturating_sub(left));
        let height = monitor.height().min(server.height.saturating_sub(top));
        if width == 0 || height == 0 {
            return Err(platform_error(format!(
                "X11 monitor {} lies outside the {}x{} root window",
                monitor.name(),
                server.width,
                server.height
            )));
        }
        let convert = |value: u32| {
            u16::try_from(value).map_err(|_| protocol_error("X11 region exceeds 16-bit geometry"))
        };
        Ok(Region {
            x: convert(left)?,
            y: convert(top)?,
            width: convert(width)?,
            height: convert(height)?,
        })
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
        // Capture the selected monitor, or the complete root window when none
        // was chosen. Requesting the output canvas dimensions directly only
        // returned the top-left corner whenever the desktop was larger than the
        // configured scene, so the full rectangle is always read and scaled.
        let region = match self.region {
            Some(region) => region,
            None => Region {
                x: 0,
                y: 0,
                width: u16::try_from(self.server.width)
                    .map_err(|_| CaptureError::UnsupportedFormat(format))?,
                height: u16::try_from(self.server.height)
                    .map_err(|_| CaptureError::UnsupportedFormat(format))?,
            },
        };
        let (width, height) = (region.width, region.height);
        let plane_mask = if self.server.depth == 32 {
            u32::MAX
        } else {
            (1_u32 << self.server.depth) - 1
        };
        // GetImage is a fixed 20-byte request, so it is built on the stack
        // rather than in a per-frame heap buffer.
        let mut request = [0_u8; 20];
        request[0] = X11_GET_IMAGE;
        request[1] = X11_Z_PIXMAP;
        request[2..4].copy_from_slice(&5_u16.to_le_bytes());
        request[4..8].copy_from_slice(&self.server.root.to_le_bytes());
        request[8..10].copy_from_slice(&region.x.to_le_bytes());
        request[10..12].copy_from_slice(&region.y.to_le_bytes());
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
                "GetImage returned X11 error code {} for opcode {} (resource 0x{:x}, root 0x{:x}, size={}x{}, depth={}, visual=0x{:x}, bpp={}, plane_mask=0x{:x})",
                response[1],
                response[10],
                read_u32_le(&response, 4).unwrap_or_default(),
                self.server.root,
                self.server.width,
                self.server.height,
                self.server.depth,
                self.server.visual,
                self.server.bits_per_pixel,
                plane_mask
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
        self.data.clear();
        self.data.reserve(data_bytes_usize);
        let read = (&mut self.stream)
            .take(data_bytes)
            .read_to_end(&mut self.data)
            .map_err(|error| x11_io_error(&error))?;
        if read != data_bytes_usize {
            return Err(protocol_error("GetImage payload is truncated"));
        }

        let row_bytes = packed_row_bytes(usize::from(width), self.server.bits_per_pixel)?;
        let row_stride = padded_row_bytes(row_bytes, self.server.scanline_pad)?;
        let required_bytes = row_stride
            .checked_mul(usize::from(height))
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        if self.data.len() < required_bytes {
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
            &self.data,
        )?;
        let pixels = resize_letterbox(
            &pixels,
            usize::from(width),
            usize::from(height),
            usize::try_from(format.width()).unwrap_or(usize::MAX),
            usize::try_from(format.height()).unwrap_or(usize::MAX),
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
        if self.server.width == 0
            || self.server.height == 0
            || self.server.width > u32::from(u16::MAX)
            || self.server.height > u32::from(u16::MAX)
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

/// Resizes a complete root image into the output canvas while preserving its
/// aspect ratio. The unused canvas area is opaque black, so X11 capture remains
/// composable with ordinary scene layers and never exposes uninitialized alpha.
pub(crate) fn resize_letterbox(
    source: &[u8],
    source_width: usize,
    source_height: usize,
    destination_width: usize,
    destination_height: usize,
) -> Result<Vec<u8>, CaptureError> {
    let source_bytes = source_width
        .checked_mul(source_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    if source.len() < source_bytes
        || source_width == 0
        || source_height == 0
        || destination_width == 0
        || destination_height == 0
    {
        return Err(protocol_error("X11 resize geometry is invalid"));
    }
    let destination_bytes = destination_width
        .checked_mul(destination_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let mut output = vec![0_u8; destination_bytes];
    for pixel in output.chunks_exact_mut(4) {
        pixel[3] = u8::MAX;
    }

    let width_limited = destination_width.saturating_mul(source_height)
        <= destination_height.saturating_mul(source_width);
    let (scaled_width, scaled_height) = if width_limited {
        (
            destination_width,
            destination_width
                .saturating_mul(source_height)
                .checked_div(source_width)
                .unwrap_or(0)
                .max(1),
        )
    } else {
        (
            destination_height
                .saturating_mul(source_width)
                .checked_div(source_height)
                .unwrap_or(0)
                .max(1),
            destination_height,
        )
    };
    let offset_x = (destination_width - scaled_width) / 2;
    let offset_y = (destination_height - scaled_height) / 2;
    for destination_y in 0..scaled_height {
        let source_y = destination_y
            .saturating_mul(source_height)
            .checked_div(scaled_height)
            .unwrap_or(0)
            .min(source_height - 1);
        for destination_x in 0..scaled_width {
            let source_x = destination_x
                .saturating_mul(source_width)
                .checked_div(scaled_width)
                .unwrap_or(0)
                .min(source_width - 1);
            let source_offset = (source_y * source_width + source_x) * 4;
            let output_offset =
                ((offset_y + destination_y) * destination_width + offset_x + destination_x) * 4;
            output[output_offset..output_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }
    Ok(output)
}
