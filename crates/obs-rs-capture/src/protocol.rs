use std::io::{self, Read};

use obs_rs_media::VideoFrame;

use super::error::CaptureError;

/// Magic header for the safe Rust RGBA frame-stream protocol.
pub const FRAME_STREAM_MAGIC: &[u8; 8] = b"OBSFRM01";
pub(crate) const FRAME_STREAM_HEADER_BYTES: usize = 8 + 4 * 4 + 8 + 8;
/// Maximum encoded frame-stream packet accepted by one device.
pub const MAX_FRAME_STREAM_PACKET_BYTES: usize = 64 * 1024 * 1024 + 64;

/// Encodes one RGBA frame for the safe Rust frame-stream protocol.
///
/// The packet carries dimensions, reduced frame-rate components, the source
/// timestamp, and an exact RGBA8 payload. It is suitable for a platform adapter in
/// another Rust process to send over a pipe or [`std::net::TcpStream`].
///
/// # Errors
///
/// Returns [`CaptureError::FramePacketTooLarge`] only when the bounded packet size
/// cannot represent the validated frame.
pub fn encode_frame_packet(frame: &VideoFrame) -> Result<Vec<u8>, CaptureError> {
    let payload_bytes = frame.pixels().len();
    let packet_bytes = FRAME_STREAM_HEADER_BYTES
        .checked_add(payload_bytes)
        .ok_or(CaptureError::FramePacketTooLarge { bytes: u64::MAX })?;
    if packet_bytes > MAX_FRAME_STREAM_PACKET_BYTES {
        return Err(CaptureError::FramePacketTooLarge {
            bytes: u64::try_from(packet_bytes).unwrap_or(u64::MAX),
        });
    }

    let format = frame.format();
    let rate = format.frame_rate();
    let mut packet = Vec::with_capacity(packet_bytes);
    packet.extend_from_slice(FRAME_STREAM_MAGIC);
    packet.extend_from_slice(&format.width().to_le_bytes());
    packet.extend_from_slice(&format.height().to_le_bytes());
    packet.extend_from_slice(&rate.numerator().to_le_bytes());
    packet.extend_from_slice(&rate.denominator().to_le_bytes());
    packet.extend_from_slice(&frame.timestamp().as_nanos().to_le_bytes());
    packet.extend_from_slice(
        &u64::try_from(payload_bytes)
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    packet.extend_from_slice(frame.pixels());
    Ok(packet)
}
pub(crate) fn read_exact_capture(
    reader: &mut impl Read,
    bytes: &mut [u8],
) -> Result<(), CaptureError> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            CaptureError::TruncatedFrame
        } else {
            io_error(&error)
        }
    })
}

pub(crate) fn io_error(error: &io::Error) -> CaptureError {
    CaptureError::Io {
        message: error.to_string(),
    }
}
