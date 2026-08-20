use std::{env, fs, io::Write, os::unix::net::UnixStream, path::PathBuf, time::Duration};

use super::super::CaptureError;
use super::{
    error::{platform_error, protocol_error, read_exact_x11, x11_io_error},
    protocol::{
        parse_setup, read_be_field, read_bytes_field, read_u16_le, setup_request, Authorization,
        ServerInfo, X11_INTERNET_FAMILY, X11_LOCAL_FAMILY, X11_MAX_REPLY_BYTES, X11_WILD_FAMILY,
    },
};

/// Bounds a stalled X server round-trip without making normal local capture
/// transfers fail under load. The capture worker can then report a typed loss
/// and let the source choose its fallback instead of wedging forever.
pub(crate) const X11_SOCKET_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) fn configure_stream(stream: &UnixStream) -> Result<(), CaptureError> {
    stream
        .set_read_timeout(Some(X11_SOCKET_TIMEOUT))
        .map_err(|error| x11_io_error(&error))?;
    stream
        .set_write_timeout(Some(X11_SOCKET_TIMEOUT))
        .map_err(|error| x11_io_error(&error))
}

pub(crate) fn display_socket(display: &str) -> Result<(PathBuf, String), CaptureError> {
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

pub(crate) fn read_authorization(display_number: &str) -> Option<Authorization> {
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

pub(crate) fn handshake(
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
