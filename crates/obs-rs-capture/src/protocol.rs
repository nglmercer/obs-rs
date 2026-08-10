use std::io::{self, Read, Write};

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
    let mut packet = Vec::with_capacity(packet_bytes_for(frame)?);
    write_frame_packet(frame, &mut packet)?;
    Ok(packet)
}

/// Writes one encoded frame packet directly into `writer`.
///
/// Senders that already own an output sink — a pipe, socket, or a reused
/// buffer — should prefer this over [`encode_frame_packet`], which exists for
/// callers that need an owned packet and allocates one per frame.
///
/// # Errors
///
/// Returns [`CaptureError::FramePacketTooLarge`] when the frame exceeds
/// [`MAX_FRAME_STREAM_PACKET_BYTES`], or [`CaptureError::Io`] from `writer`.
pub fn write_frame_packet(
    frame: &VideoFrame,
    writer: &mut impl Write,
) -> Result<(), CaptureError> {
    packet_bytes_for(frame)?;
    let payload_bytes = frame.pixels().len();
    let format = frame.format();
    let rate = format.frame_rate();

    // The header is fixed-size, so it is staged on the stack and written with
    // the payload instead of being grown byte-group by byte-group in a Vec.
    let mut header = [0_u8; FRAME_STREAM_HEADER_BYTES];
    header[..8].copy_from_slice(FRAME_STREAM_MAGIC);
    header[8..12].copy_from_slice(&format.width().to_le_bytes());
    header[12..16].copy_from_slice(&format.height().to_le_bytes());
    header[16..20].copy_from_slice(&rate.numerator().to_le_bytes());
    header[20..24].copy_from_slice(&rate.denominator().to_le_bytes());
    header[24..32].copy_from_slice(&frame.timestamp().as_nanos().to_le_bytes());
    header[32..40].copy_from_slice(
        &u64::try_from(payload_bytes)
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );

    writer
        .write_all(&header)
        .and_then(|()| writer.write_all(frame.pixels()))
        .map_err(|error| io_error(&error))
}

/// Returns the encoded packet size, rejecting frames past the bounded limit.
fn packet_bytes_for(frame: &VideoFrame) -> Result<usize, CaptureError> {
    let packet_bytes = FRAME_STREAM_HEADER_BYTES
        .checked_add(frame.pixels().len())
        .ok_or(CaptureError::FramePacketTooLarge { bytes: u64::MAX })?;
    if packet_bytes > MAX_FRAME_STREAM_PACKET_BYTES {
        return Err(CaptureError::FramePacketTooLarge {
            bytes: u64::try_from(packet_bytes).unwrap_or(u64::MAX),
        });
    }
    Ok(packet_bytes)
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
