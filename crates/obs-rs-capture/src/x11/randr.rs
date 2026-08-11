//! `RandR` monitor enumeration over the same safe X11 socket as capture.
//!
//! A multi-head desktop is one X11 screen whose root window spans every
//! monitor, so capturing "the screen" without asking `RandR` always yields the
//! full desktop. `RRGetMonitors` (`RandR` 1.5) reports the rectangle and name of
//! each active monitor, which is what lets the UI offer "Display 1 / DP-1"
//! choices and what lets capture crop the root image to one of them.

use std::{io::Write, os::unix::net::UnixStream};

use super::super::CaptureError;
use super::{
    error::{read_exact_x11, x11_io_error},
    protocol::{read_u16_le, read_u32_le, X11_MAX_REPLY_BYTES},
};

/// Core-protocol `QueryExtension` opcode.
const X11_QUERY_EXTENSION: u8 = 98;
/// Core-protocol `GetAtomName` opcode.
const X11_GET_ATOM_NAME: u8 = 17;
/// `RandR` minor opcode for `RRQueryVersion`.
const RANDR_QUERY_VERSION: u8 = 0;
/// `RandR` minor opcode for `RRGetMonitors`, added in `RandR` 1.5.
const RANDR_GET_MONITORS: u8 = 42;
/// Fixed size of one `MONITORINFO` before its output list.
const MONITOR_INFO_BYTES: usize = 24;

/// One active monitor rectangle inside the X11 root window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct X11Monitor {
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    primary: bool,
}

impl X11Monitor {
    /// Creates a monitor descriptor.
    #[must_use]
    pub const fn new(name: String, x: i32, y: i32, width: u32, height: u32, primary: bool) -> Self {
        Self {
            name,
            x,
            y,
            width,
            height,
            primary,
        }
    }

    /// Returns the `RandR` output name, such as `DP-1`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the monitor's left edge inside the root window.
    #[must_use]
    pub const fn x(&self) -> i32 {
        self.x
    }

    /// Returns the monitor's top edge inside the root window.
    #[must_use]
    pub const fn y(&self) -> i32 {
        self.y
    }

    /// Returns the monitor width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns the monitor height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns whether `RandR` marks this monitor as primary.
    #[must_use]
    pub const fn primary(&self) -> bool {
        self.primary
    }

    /// Returns the stable capture-device ID for this monitor.
    #[must_use]
    pub fn device_id(&self) -> String {
        format!("x11-monitor-{}", sanitize_id(&self.name))
    }

    /// Returns the label a device catalog or picker shows.
    #[must_use]
    pub fn label(&self) -> String {
        let primary = if self.primary { " · primary" } else { "" };
        format!(
            "{} ({}x{} at {},{}){primary}",
            self.name, self.width, self.height, self.x, self.y
        )
    }
}

/// Rewrites a `RandR` output name into an identifier the catalog accepts.
///
/// Identifiers are restricted to ASCII alphanumerics, `-`, and `_`, while
/// `RandR` names may contain other characters on unusual drivers.
fn sanitize_id(name: &str) -> String {
    let mapped = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    if mapped.is_empty() {
        "unknown".to_owned()
    } else {
        mapped
    }
}

/// Reads the active monitor list from an X11 connection.
///
/// Returns an empty list when the server does not offer `RandR` 1.5, which is
/// the caller's signal to treat the whole root window as the only monitor.
///
/// # Errors
///
/// Returns [`CaptureError::Io`] when the socket fails and
/// [`CaptureError::Protocol`] when a reply cannot be decoded.
pub(crate) fn query_monitors(
    stream: &mut UnixStream,
    root: u32,
) -> Result<Vec<X11Monitor>, CaptureError> {
    let Some(opcode) = query_extension(stream, b"RANDR")? else {
        return Ok(Vec::new());
    };
    let (major, minor) = query_version(stream, opcode)?;
    if major < 1 || (major == 1 && minor < 5) {
        return Ok(Vec::new());
    }
    get_monitors(stream, opcode, root)
}

/// Returns the major opcode of `name`, or `None` when the server lacks it.
fn query_extension(stream: &mut UnixStream, name: &[u8]) -> Result<Option<u8>, CaptureError> {
    let name_length =
        u16::try_from(name.len()).map_err(|_| protocol("extension name is too long"))?;
    let padded = name.len().div_ceil(4) * 4;
    let request_words = u16::try_from(2 + padded / 4)
        .map_err(|_| protocol("extension request length overflows"))?;
    let mut request = Vec::with_capacity(8 + padded);
    request.push(X11_QUERY_EXTENSION);
    request.push(0);
    request.extend_from_slice(&request_words.to_le_bytes());
    request.extend_from_slice(&name_length.to_le_bytes());
    request.extend_from_slice(&0_u16.to_le_bytes());
    request.extend_from_slice(name);
    request.resize(8 + padded, 0);
    write_request(stream, &request)?;

    let reply = read_reply(stream, "QueryExtension")?;
    if reply.header[8] == 0 {
        return Ok(None);
    }
    Ok(Some(reply.header[9]))
}

/// Negotiates the `RandR` version, returning what the server settled on.
fn query_version(stream: &mut UnixStream, opcode: u8) -> Result<(u32, u32), CaptureError> {
    let mut request = [0_u8; 12];
    request[0] = opcode;
    request[1] = RANDR_QUERY_VERSION;
    request[2..4].copy_from_slice(&3_u16.to_le_bytes());
    request[4..8].copy_from_slice(&1_u32.to_le_bytes());
    request[8..12].copy_from_slice(&5_u32.to_le_bytes());
    write_request(stream, &request)?;

    let reply = read_reply(stream, "RRQueryVersion")?;
    Ok((
        read_u32_le(&reply.header, 8)?,
        read_u32_le(&reply.header, 12)?,
    ))
}

/// Reads `RRGetMonitors` and resolves each monitor's name atom.
fn get_monitors(
    stream: &mut UnixStream,
    opcode: u8,
    root: u32,
) -> Result<Vec<X11Monitor>, CaptureError> {
    let mut request = [0_u8; 12];
    request[0] = opcode;
    request[1] = RANDR_GET_MONITORS;
    request[2..4].copy_from_slice(&3_u16.to_le_bytes());
    request[4..8].copy_from_slice(&root.to_le_bytes());
    // Only active monitors are useful for capture.
    request[8] = 1;
    write_request(stream, &request)?;

    let reply = read_reply(stream, "RRGetMonitors")?;
    let count = usize::try_from(read_u32_le(&reply.header, 12)?)
        .map_err(|_| protocol("RRGetMonitors reports too many monitors"))?;
    let mut monitors = Vec::with_capacity(count);
    let mut offset = 0_usize;
    for _ in 0..count {
        let end = offset
            .checked_add(MONITOR_INFO_BYTES)
            .filter(|end| *end <= reply.body.len())
            .ok_or_else(|| protocol("RRGetMonitors reply is truncated"))?;
        let atom = read_u32_le(&reply.body, offset)?;
        let primary = reply.body[offset + 4] != 0;
        let outputs = usize::from(read_u16_le(&reply.body, offset + 6)?);
        let x = i32::from(read_i16_le(&reply.body, offset + 8)?);
        let y = i32::from(read_i16_le(&reply.body, offset + 10)?);
        let width = u32::from(read_u16_le(&reply.body, offset + 12)?);
        let height = u32::from(read_u16_le(&reply.body, offset + 14)?);
        offset = end
            .checked_add(outputs.saturating_mul(4))
            .filter(|end| *end <= reply.body.len())
            .ok_or_else(|| protocol("RRGetMonitors output list is truncated"))?;
        if width == 0 || height == 0 {
            continue;
        }
        monitors.push(X11Monitor {
            // The atom lookup is a separate round trip, so it is only made for
            // monitors that will actually be offered.
            name: atom_name(stream, atom)?,
            x,
            y,
            width,
            height,
            primary,
        });
    }
    Ok(monitors)
}

/// Resolves one atom to its name, falling back to a numeric label.
fn atom_name(stream: &mut UnixStream, atom: u32) -> Result<String, CaptureError> {
    if atom == 0 {
        return Ok("Display".to_owned());
    }
    let mut request = [0_u8; 8];
    request[0] = X11_GET_ATOM_NAME;
    request[2..4].copy_from_slice(&2_u16.to_le_bytes());
    request[4..8].copy_from_slice(&atom.to_le_bytes());
    write_request(stream, &request)?;

    let reply = read_reply(stream, "GetAtomName")?;
    let length = usize::from(read_u16_le(&reply.header, 8)?);
    let name = reply
        .body
        .get(..length)
        .ok_or_else(|| protocol("GetAtomName reply is truncated"))?;
    Ok(String::from_utf8_lossy(name).into_owned())
}

/// A decoded X11 reply: its fixed header plus the variable payload.
struct Reply {
    header: [u8; 32],
    body: Vec<u8>,
}

/// Reads one reply, turning an X11 error response into a typed failure.
fn read_reply(stream: &mut UnixStream, request: &str) -> Result<Reply, CaptureError> {
    let mut header = [0_u8; 32];
    read_exact_x11(stream, &mut header)?;
    if header[0] != 1 {
        return Err(protocol(format!(
            "{request} returned X11 error code {}",
            header[1]
        )));
    }
    let extra_words = u64::from(read_u32_le(&header, 4)?);
    let extra_bytes = extra_words
        .checked_mul(4)
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let extra_bytes = usize::try_from(extra_bytes)
        .ok()
        .filter(|bytes| *bytes <= X11_MAX_REPLY_BYTES)
        .ok_or(CaptureError::ReplyTooLarge { bytes: extra_bytes })?;
    let mut body = vec![0_u8; extra_bytes];
    read_exact_x11(stream, &mut body)?;
    Ok(Reply { header, body })
}

fn write_request(stream: &mut UnixStream, request: &[u8]) -> Result<(), CaptureError> {
    stream
        .write_all(request)
        .map_err(|error| x11_io_error(&error))
}

fn read_i16_le(bytes: &[u8], offset: usize) -> Result<i16, CaptureError> {
    read_u16_le(bytes, offset).map(|value| i16::from_le_bytes(value.to_le_bytes()))
}

fn protocol(message: impl Into<String>) -> CaptureError {
    super::error::protocol_error(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_derived_from_randr_names() {
        assert_eq!(sanitize_id("DP-1"), "dp-1");
        assert_eq!(sanitize_id("HDMI A 2"), "hdmi-a-2");
        assert_eq!(sanitize_id(""), "unknown");
    }

    #[test]
    fn monitor_labels_report_geometry_and_primary_state() {
        let monitor = X11Monitor::new("DP-1".to_owned(), 1920, 0, 2560, 1440, true);

        assert_eq!(monitor.device_id(), "x11-monitor-dp-1");
        assert_eq!(monitor.label(), "DP-1 (2560x1440 at 1920,0) · primary");
    }

    #[test]
    fn negative_monitor_offsets_decode_as_signed_values() {
        let bytes = (-1080_i16).to_le_bytes();

        assert_eq!(read_i16_le(&bytes, 0).expect("decodes"), -1080);
    }
}
