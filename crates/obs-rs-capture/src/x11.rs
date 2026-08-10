//! Direct Linux X11 screen capture using the wire protocol and standard library.
//!
//! This module deliberately talks to the local X server over its Unix socket. It
//! owns the protocol framing, image conversion, and lifecycle instead of relying
//! on a generated binding or a native helper library. The implementation captures
//! the top-left region of the root window and supports `TrueColor` visuals with
//! 16-, 24-, or 32-bit pixels.

use std::{
    env, fs,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
};

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{CaptureDeviceInfo, CaptureError, CaptureKind, CapturePermission, VideoCaptureDevice};

const X11_GET_IMAGE: u8 = 73;
const X11_Z_PIXMAP: u8 = 2;
const X11_MAX_REPLY_BYTES: usize = 256 * 1024 * 1024;
const X11_TRUE_COLOR: u8 = 4;
const X11_DIRECT_COLOR: u8 = 5;
const X11_LOCAL_FAMILY: u16 = 256;
const X11_WILD_FAMILY: u16 = 65_535;
const X11_INTERNET_FAMILY: u16 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageByteOrder {
    LeastSignificantFirst,
    MostSignificantFirst,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VisualMasks {
    red: u32,
    green: u32,
    blue: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ServerInfo {
    root: u32,
    width: u32,
    height: u32,
    depth: u8,
    visual: u32,
    image_byte_order: ImageByteOrder,
    scanline_pad: u8,
    bits_per_pixel: u8,
    masks: VisualMasks,
}

struct Authorization {
    name: Vec<u8>,
    data: Vec<u8>,
}

/// A direct Linux screen device backed by a local X11 server.
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
        let mut request = Vec::with_capacity(20);
        request.push(X11_GET_IMAGE);
        request.push(X11_Z_PIXMAP);
        write_u16_le(&mut request, 5);
        write_u32_le(&mut request, self.server.root);
        write_u16_le(&mut request, 0);
        write_u16_le(&mut request, 0);
        write_u16_le(&mut request, width);
        write_u16_le(&mut request, height);
        let plane_mask = if self.server.depth == 32 {
            u32::MAX
        } else {
            (1_u32 << self.server.depth) - 1
        };
        write_u32_le(&mut request, plane_mask);
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

fn display_socket(display: &str) -> Result<(PathBuf, String), CaptureError> {
    let display = display.trim();
    if display.is_empty() {
        return Err(platform_error("display name is empty"));
    }
    if display.starts_with('/') {
        return Ok((PathBuf::from(display), "0".to_owned()));
    }
    let (host, number) = if let Some(number) = display.strip_prefix("unix:") {
        ("", number)
    } else {
        display
            .rsplit_once(':')
            .ok_or_else(|| platform_error("display must use host:number syntax"))?
    };
    if !host.is_empty() && host != "localhost" {
        return Err(platform_error(
            "only local X11 Unix sockets are supported by this backend",
        ));
    }
    let display_number = number
        .split('.')
        .next()
        .ok_or_else(|| platform_error("display number is missing"))?;
    let parsed = display_number
        .parse::<u16>()
        .map_err(|_| platform_error("display number is invalid"))?;
    Ok((
        PathBuf::from(format!("/tmp/.X11-unix/X{parsed}")),
        display_number.to_owned(),
    ))
}

fn read_authorization(display_number: &str) -> Option<Authorization> {
    let path = env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".Xauthority")))?;
    let bytes = fs::read(path).ok()?;
    parse_authorization(&bytes, display_number)
}

fn parse_authorization(bytes: &[u8], display_number: &str) -> Option<Authorization> {
    let mut offset = 0_usize;
    let mut selected = None;
    while offset < bytes.len() {
        let family = read_be_field(bytes, &mut offset)?;
        let address = read_bytes_field(bytes, &mut offset)?;
        let number = read_bytes_field(bytes, &mut offset)?;
        let name = read_bytes_field(bytes, &mut offset)?;
        let data = read_bytes_field(bytes, &mut offset)?;
        let family_matches = matches!(
            family,
            X11_LOCAL_FAMILY | X11_WILD_FAMILY | X11_INTERNET_FAMILY
        ) && (family != X11_INTERNET_FAMILY || !address.is_empty());
        let number_matches = number.is_empty() || number == display_number.as_bytes();
        if family_matches && number_matches && name == b"MIT-MAGIC-COOKIE-1" {
            selected = Some(Authorization {
                name: name.to_vec(),
                data: data.to_vec(),
            });
            if number == display_number.as_bytes() {
                break;
            }
        }
    }
    selected
}

fn handshake(
    stream: &mut UnixStream,
    authorization: Option<&Authorization>,
) -> Result<ServerInfo, CaptureError> {
    let request = setup_request(authorization)?;
    stream
        .write_all(&request)
        .map_err(|error| x11_io_error(&error))?;
    let mut header = [0_u8; 8];
    read_exact_x11(stream, &mut header)?;
    let additional_words = usize::from(read_u16_le(&header, 6)?);
    let additional_bytes = additional_words
        .checked_mul(4)
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    if additional_bytes > X11_MAX_REPLY_BYTES {
        return Err(CaptureError::ReplyTooLarge {
            bytes: additional_bytes as u64,
        });
    }
    let mut body = vec![0_u8; additional_bytes];
    read_exact_x11(stream, &mut body)?;
    let mut setup = Vec::with_capacity(8 + body.len());
    setup.extend_from_slice(&header);
    setup.extend_from_slice(&body);
    if setup[0] != 1 {
        return Err(protocol_error("X11 setup handshake was rejected"));
    }
    parse_setup(&setup)
}

fn setup_request(authorization: Option<&Authorization>) -> Result<Vec<u8>, CaptureError> {
    let (name, data) = authorization.map_or((&[][..], &[][..]), |value| {
        (value.name.as_slice(), value.data.as_slice())
    });
    let name_len =
        u16::try_from(name.len()).map_err(|_| protocol_error("X11 auth name too long"))?;
    let data_len =
        u16::try_from(data.len()).map_err(|_| protocol_error("X11 auth data too long"))?;
    let mut request = Vec::with_capacity(12 + name.len() + data.len() + 6);
    request.extend_from_slice(b"l\0");
    write_u16_le(&mut request, 11);
    write_u16_le(&mut request, 0);
    write_u16_le(&mut request, name_len);
    write_u16_le(&mut request, data_len);
    write_u16_le(&mut request, 0);
    request.extend_from_slice(name);
    append_padding(&mut request);
    request.extend_from_slice(data);
    append_padding(&mut request);
    Ok(request)
}

fn parse_setup(bytes: &[u8]) -> Result<ServerInfo, CaptureError> {
    if bytes.len() < 40 || bytes[0] != 1 {
        return Err(protocol_error("X11 setup response is truncated"));
    }
    let vendor_len = usize::from(read_u16_le(bytes, 24)?);
    let root_count = usize::from(bytes[28]);
    let format_count = usize::from(bytes[29]);
    let image_byte_order = match bytes[30] {
        0 => ImageByteOrder::LeastSignificantFirst,
        1 => ImageByteOrder::MostSignificantFirst,
        _ => return Err(protocol_error("X11 image byte order is invalid")),
    };
    let mut offset = 40_usize;
    offset = offset
        .checked_add(padded_length(vendor_len)?)
        .ok_or_else(|| protocol_error("X11 vendor field overflows"))?;
    let mut formats = Vec::with_capacity(format_count);
    for _ in 0..format_count {
        ensure_range(bytes, offset, 8)?;
        let depth = bytes[offset];
        let bits_per_pixel = bytes[offset + 1];
        let scanline_pad = bytes[offset + 2];
        formats.push((depth, bits_per_pixel, scanline_pad));
        offset += 8;
    }

    let mut root_info = None;
    for root_index in 0..root_count {
        ensure_range(bytes, offset, 40)?;
        let root = read_u32_le(bytes, offset)?;
        let width = u32::from(read_u16_le(bytes, offset + 20)?);
        let height = u32::from(read_u16_le(bytes, offset + 22)?);
        let visual = read_u32_le(bytes, offset + 32)?;
        let depth = bytes[offset + 38];
        let allowed_depths = usize::from(bytes[offset + 39]);
        offset += 40;
        let mut masks = None;
        for _ in 0..allowed_depths {
            ensure_range(bytes, offset, 8)?;
            let visual_depth = bytes[offset];
            let visual_count = usize::from(read_u16_le(bytes, offset + 2)?);
            offset += 8;
            let visual_bytes = visual_count
                .checked_mul(24)
                .ok_or_else(|| protocol_error("X11 visual list overflows"))?;
            ensure_range(bytes, offset, visual_bytes)?;
            for visual_index in 0..visual_count {
                let visual_offset = offset + visual_index * 24;
                let visual_id = read_u32_le(bytes, visual_offset)?;
                let class = bytes[visual_offset + 4];
                if visual_depth == depth
                    && visual_id == visual
                    && matches!(class, X11_TRUE_COLOR | X11_DIRECT_COLOR)
                {
                    masks = Some(VisualMasks {
                        red: read_u32_le(bytes, visual_offset + 8)?,
                        green: read_u32_le(bytes, visual_offset + 12)?,
                        blue: read_u32_le(bytes, visual_offset + 16)?,
                    });
                }
            }
            offset += visual_bytes;
        }
        if root_index == 0 {
            root_info = Some((root, width, height, depth, visual, masks));
        }
    }

    let (root, width, height, depth, visual, masks) =
        root_info.ok_or_else(|| protocol_error("X11 setup contains no root screen"))?;
    let (_, bits_per_pixel, scanline_pad) = formats
        .into_iter()
        .find(|(format_depth, _, _)| *format_depth == depth)
        .ok_or_else(|| protocol_error("X11 setup has no root-depth pixmap format"))?;
    if !matches!(bits_per_pixel, 16 | 24 | 32) {
        return Err(protocol_error(format!(
            "X11 root visual uses an unsupported pixel size: {bits_per_pixel} bits per pixel"
        )));
    }
    let masks = masks.ok_or_else(|| protocol_error("X11 root visual is not TrueColor"))?;
    if masks.red == 0 || masks.green == 0 || masks.blue == 0 {
        return Err(protocol_error("X11 root visual has an empty color mask"));
    }
    Ok(ServerInfo {
        root,
        width,
        height,
        depth,
        visual,
        image_byte_order,
        scanline_pad,
        bits_per_pixel,
        masks,
    })
}

fn decode_pixels(
    width: usize,
    height: usize,
    row_stride: usize,
    bits_per_pixel: u8,
    byte_order: ImageByteOrder,
    masks: VisualMasks,
    data: &[u8],
) -> Result<Vec<u8>, CaptureError> {
    let bytes_per_pixel = usize::from(bits_per_pixel / 8);
    let pixel_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let mut output = vec![0_u8; pixel_bytes];
    for y in 0..height {
        let row_start = y
            .checked_mul(row_stride)
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        for x in 0..width {
            let offset = row_start
                .checked_add(
                    x.checked_mul(bytes_per_pixel)
                        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?,
                )
                .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
            let end = offset
                .checked_add(bytes_per_pixel)
                .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
            let source = data
                .get(offset..end)
                .ok_or_else(|| protocol_error("X11 pixel row is truncated"))?;
            let pixel = read_pixel(source, byte_order);
            let destination = (y * width + x) * 4;
            output[destination] = scale_channel(pixel, masks.red);
            output[destination + 1] = scale_channel(pixel, masks.green);
            output[destination + 2] = scale_channel(pixel, masks.blue);
            output[destination + 3] = 255;
        }
    }
    Ok(output)
}

fn read_pixel(bytes: &[u8], byte_order: ImageByteOrder) -> u32 {
    match byte_order {
        ImageByteOrder::LeastSignificantFirst => bytes
            .iter()
            .enumerate()
            .fold(0_u32, |value, (index, byte)| {
                value | (u32::from(*byte) << (index * 8))
            }),
        ImageByteOrder::MostSignificantFirst => bytes
            .iter()
            .fold(0_u32, |value, byte| (value << 8) | u32::from(*byte)),
    }
}

fn scale_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let maximum = u64::from(mask >> shift);
    let value = u64::from((pixel & mask) >> shift);
    let scaled = value.saturating_mul(u64::from(u8::MAX)) / maximum.max(1);
    u8::try_from(scaled.min(u64::from(u8::MAX))).unwrap_or(u8::MAX)
}

fn packed_row_bytes(width: usize, bits_per_pixel: u8) -> Result<usize, CaptureError> {
    width
        .checked_mul(usize::from(bits_per_pixel / 8))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })
}

fn padded_row_bytes(row_bytes: usize, scanline_pad: u8) -> Result<usize, CaptureError> {
    let pad = usize::from(scanline_pad / 8);
    if pad == 0 {
        return Err(protocol_error("X11 scanline padding is zero"));
    }
    let rounded = row_bytes
        .checked_add(pad - 1)
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    Ok(rounded / pad * pad)
}

fn read_exact_x11(stream: &mut UnixStream, bytes: &mut [u8]) -> Result<(), CaptureError> {
    stream
        .read_exact(bytes)
        .map_err(|error| x11_io_error(&error))
}

fn x11_io_error(error: &io::Error) -> CaptureError {
    CaptureError::Io {
        message: format!("X11 socket: {error}"),
    }
}

fn platform_error(message: impl Into<String>) -> CaptureError {
    CaptureError::PlatformUnavailable {
        message: message.into(),
    }
}

fn protocol_error(message: impl Into<String>) -> CaptureError {
    CaptureError::Protocol {
        message: message.into(),
    }
}

fn ensure_range(bytes: &[u8], offset: usize, length: usize) -> Result<(), CaptureError> {
    offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .map(|_| ())
        .ok_or_else(|| protocol_error("X11 setup response is truncated"))
}

fn padded_length(length: usize) -> Result<usize, CaptureError> {
    length
        .checked_add(3)
        .map(|value| value / 4 * 4)
        .ok_or_else(|| protocol_error("X11 setup length overflows"))
}

fn append_padding(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

fn write_u16_le(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn write_u32_le(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16, CaptureError> {
    ensure_range(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, CaptureError> {
    ensure_range(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

fn read_be_field(bytes: &[u8], offset: &mut usize) -> Option<u16> {
    let start = *offset;
    let end = start.checked_add(2)?;
    let value = u16::from_be_bytes(bytes.get(start..end)?.try_into().ok()?);
    *offset = end;
    Some(value)
}

fn read_bytes_field<'a>(bytes: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    let length = usize::from(read_be_field(bytes, offset)?);
    let start = *offset;
    let end = start.checked_add(length)?;
    let value = bytes.get(start..end)?;
    *offset = end;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_fixture() -> Vec<u8> {
        let mut bytes = vec![0_u8; 40 + 8 + 40 + 8 + 24];
        bytes[0] = 1;
        bytes[28] = 1;
        bytes[29] = 1;
        bytes[30] = 0;
        bytes[32] = 32;
        bytes[33] = 32;
        bytes[40] = 24;
        bytes[41] = 32;
        bytes[42] = 32;
        let root = 48;
        bytes[root..root + 4].copy_from_slice(&7_u32.to_le_bytes());
        bytes[root + 20..root + 22].copy_from_slice(&640_u16.to_le_bytes());
        bytes[root + 22..root + 24].copy_from_slice(&480_u16.to_le_bytes());
        bytes[root + 32..root + 36].copy_from_slice(&7_u32.to_le_bytes());
        bytes[root + 38] = 24;
        bytes[root + 39] = 1;
        let depth = root + 40;
        bytes[depth] = 24;
        bytes[depth + 2..depth + 4].copy_from_slice(&1_u16.to_le_bytes());
        let visual = depth + 8;
        bytes[visual..visual + 4].copy_from_slice(&7_u32.to_le_bytes());
        bytes[visual + 4] = X11_TRUE_COLOR;
        bytes[visual + 8..visual + 12].copy_from_slice(&0x00ff_0000_u32.to_le_bytes());
        bytes[visual + 12..visual + 16].copy_from_slice(&0x0000_ff00_u32.to_le_bytes());
        bytes[visual + 16..visual + 20].copy_from_slice(&0x0000_00ff_u32.to_le_bytes());
        bytes
    }

    #[test]
    fn setup_parser_extracts_root_and_visual_layout() {
        let server = parse_setup(&setup_fixture()).expect("valid setup");
        assert_eq!(server.root, 7);
        assert_eq!((server.width, server.height), (640, 480));
        assert_eq!(server.depth, 24);
        assert_eq!(server.visual, 7);
        assert_eq!(server.bits_per_pixel, 32);
        assert_eq!(server.scanline_pad, 32);
        assert_eq!(
            server.image_byte_order,
            ImageByteOrder::LeastSignificantFirst
        );
        assert_eq!(server.masks.red, 0x00ff_0000);
    }

    #[test]
    fn pixel_decoder_converts_masked_little_endian_true_color() {
        let pixels = decode_pixels(
            2,
            1,
            8,
            32,
            ImageByteOrder::LeastSignificantFirst,
            VisualMasks {
                red: 0x00ff_0000,
                green: 0x0000_ff00,
                blue: 0x0000_00ff,
            },
            &[0, 0, 255, 0, 0, 255, 0, 0],
        )
        .expect("decode pixels");
        assert_eq!(pixels, vec![255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn display_parser_accepts_local_display_forms_and_rejects_remote_hosts() {
        assert_eq!(
            display_socket(":3.0").expect("display").0,
            PathBuf::from("/tmp/.X11-unix/X3")
        );
        assert_eq!(
            display_socket("unix:4").expect("display").0,
            PathBuf::from("/tmp/.X11-unix/X4")
        );
        assert!(matches!(
            display_socket("remote.example:0"),
            Err(CaptureError::PlatformUnavailable { .. })
        ));
    }

    #[test]
    fn setup_request_contains_optional_auth_and_aligned_fields() {
        let auth = Authorization {
            name: b"MIT-MAGIC-COOKIE-1".to_vec(),
            data: vec![1, 2, 3, 4],
        };
        let request = setup_request(Some(&auth)).expect("request");
        assert_eq!(&request[..2], b"l\0");
        assert_eq!(u16::from_le_bytes([request[6], request[7]]), 18);
        assert_eq!(u16::from_le_bytes([request[8], request[9]]), 4);
        assert_eq!(request.len() % 4, 0);
    }

    #[test]
    fn malformed_setup_is_rejected_before_visual_access() {
        assert!(matches!(
            parse_setup(&[1, 0, 11, 0, 0, 0, 0, 0]),
            Err(CaptureError::Protocol { .. })
        ));
        assert_eq!(scale_channel(0x00ff_0000, 0x00ff_0000), u8::MAX);
        assert_eq!(packed_row_bytes(640, 32).expect("row bytes"), 2_560);
        assert_eq!(padded_row_bytes(2_561, 32).expect("padded row"), 2_564);
    }

    #[test]
    #[ignore = "requires a live local X11 server"]
    fn live_x11_capture_decodes_a_root_screen_frame() {
        let mut device = X11CaptureDevice::connect_from_environment("x11-root", "X11 root")
            .expect("connect to local X11");
        let (width, height) = device.screen_size();
        assert!(width >= 2 && height >= 2);
        let format = VideoFormat::new(2, 2, obs_rs_media::FrameRate::new(30, 1).expect("rate"))
            .expect("format");
        device.start(format).expect("start X11 device");
        let frame = device
            .next_frame(Timestamp::ZERO)
            .expect("capture frame")
            .expect("root frame");
        assert_eq!(frame.format(), format);
        assert_eq!(device.frame_index(), 1);
    }
}
