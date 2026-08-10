use std::{
    io::{self, Read},
    os::unix::net::UnixStream,
};

use super::super::CaptureError;

pub(crate) fn read_exact_x11(
    stream: &mut UnixStream,
    bytes: &mut [u8],
) -> Result<(), CaptureError> {
    stream
        .read_exact(bytes)
        .map_err(|error| x11_io_error(&error))
}

pub(crate) fn x11_io_error(error: &io::Error) -> CaptureError {
    CaptureError::Io {
        message: format!("X11 socket: {error}"),
    }
}

pub(crate) fn platform_error(message: impl Into<String>) -> CaptureError {
    CaptureError::PlatformUnavailable {
        message: message.into(),
    }
}

pub(crate) fn protocol_error(message: impl Into<String>) -> CaptureError {
    CaptureError::Protocol {
        message: message.into(),
    }
}
