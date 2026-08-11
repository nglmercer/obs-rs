//! Deterministic recording, packet, and reference codec contracts for OBS-RS.
//!
//! The raw format is intentionally simple and uncompressed. The lossless RLE video
//! codec and stored-block PNG screenshot encoder below are small software references,
//! not final distribution codecs.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

pub(crate) const MAGIC: &[u8; 8] = b"OBSRRAW1";
pub(crate) const HEADER_BYTES: usize = 8 + 4 * 4 + 8;
/// Maximum number of frames accepted by one reference recording.
pub const MAX_RECORDING_FRAMES: usize = 100_000;
/// Maximum encoded size accepted by one reference recording.
pub const MAX_RECORDING_BYTES: usize = 256 * 1024 * 1024;
/// Maximum payload accepted by one encoded packet.
pub const MAX_PACKET_BYTES: usize = 16 * 1024 * 1024;

pub(crate) const PACKET_MAGIC: &[u8; 8] = b"OBSRPKT1";
pub(crate) const PACKET_HEADER_BYTES: usize = 8 + 8;
pub(crate) const RLE_MAGIC: &[u8; 8] = b"OBSRRLE1";
pub(crate) const TCP_PACKET_MAGIC: &[u8; 8] = b"OBSRTCP1";
pub(crate) const WEBSOCKET_PACKET_MAGIC: &[u8; 8] = b"OBSRWS01";
pub(crate) const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
/// Maximum HTTP handshake header accepted by the WebSocket transport.
pub const MAX_WEBSOCKET_HEADER_BYTES: usize = 16 * 1024;
/// Maximum time one reference network write may wait before it fails.
pub const NETWORK_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

mod audio;
mod codec;
mod error;
mod muxer;
mod profile;
mod queue;
mod recording;
mod stream;
mod types;
mod video;
mod writers;

#[cfg(test)]
mod tests;

pub use audio::{AudioEncoder, RawAudioEncoder, WavRecording, Y4mRecording};
pub use error::OutputError;
pub use muxer::MemoryMuxer;
pub use profile::{
    NegotiatedOutput, OutputAudioCodec, OutputCapabilities, OutputProfile, OutputProfileKind,
    OutputTransport, OutputVideoCodec, OUTPUT_PROFILE_VERSION,
};
pub use queue::PacketQueue;
pub use recording::{RawRecording, RawRecordingSession};
pub use stream::{
    MemoryPacketTransport, PacketMuxer, PacketTransport, StreamSession, TcpPacketTransport,
    WebSocketPacketTransport,
};
pub use types::{
    EncodedPacket, OutputState, PacketDropPolicy, PacketKind, PacketPushOutcome, ReconnectPolicy,
    StreamMetrics, StreamState,
};
pub use video::{
    encode_png, PngVideoEncoder, RawVideoEncoder, RleVideoDecoder, RleVideoEncoder, VideoEncoder,
};
pub use writers::{AtomicPacketFileWriter, AtomicRawFileWriter, AtomicY4mFileWriter};

#[cfg(test)]
pub(crate) use stream::{
    base64_encode, parse_websocket_endpoint, read_websocket_headers, sha1_digest,
    websocket_packet_body,
};
#[cfg(test)]
#[cfg(test)]
pub(crate) use video::crc32;
