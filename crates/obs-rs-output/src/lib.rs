//! Deterministic recording, packet, and reference codec contracts for OBS-RS.
//!
//! The raw format is intentionally simple and uncompressed. The lossless RLE video
//! codec and stored-block PNG screenshot encoder below are small software references,
//! not final distribution codecs.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    collections::VecDeque,
    fmt,
    fs::{self, OpenOptions},
    io::{Cursor, Read, Write},
    net::TcpStream,
    path::PathBuf,
};

use obs_rs_audio::{AudioBuffer, AudioFormat, MAX_AUDIO_FRAMES};
use obs_rs_media::{FrameRate, MediaError, Timestamp, VideoFormat, VideoFrame};

const MAGIC: &[u8; 8] = b"OBSRRAW1";
const HEADER_BYTES: usize = 8 + 4 * 4 + 8;
/// Maximum number of frames accepted by one reference recording.
pub const MAX_RECORDING_FRAMES: usize = 100_000;
/// Maximum encoded size accepted by one reference recording.
pub const MAX_RECORDING_BYTES: usize = 256 * 1024 * 1024;
/// Maximum payload accepted by one encoded packet.
pub const MAX_PACKET_BYTES: usize = 16 * 1024 * 1024;

const PACKET_MAGIC: &[u8; 8] = b"OBSRPKT1";
const PACKET_HEADER_BYTES: usize = 8 + 8;
const RLE_MAGIC: &[u8; 8] = b"OBSRRLE1";
const TCP_PACKET_MAGIC: &[u8; 8] = b"OBSRTCP1";
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Whether an encoded packet carries video or audio data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PacketKind {
    /// Encoded video data.
    Video,
    /// Encoded audio data.
    Audio,
}

impl PacketKind {
    fn tag(self) -> u8 {
        match self {
            Self::Video => 0,
            Self::Audio => 1,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, OutputError> {
        match tag {
            0 => Ok(Self::Video),
            1 => Ok(Self::Audio),
            _ => Err(OutputError::InvalidPacketKind { tag }),
        }
    }
}

/// One validated encoded media packet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedPacket {
    kind: PacketKind,
    timestamp: Timestamp,
    keyframe: bool,
    payload: Vec<u8>,
}

impl EncodedPacket {
    /// Creates a packet and enforces the packet-size safety limit.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::EmptyPacket`] for an empty payload or
    /// [`OutputError::PacketTooLarge`] when the payload exceeds the limit.
    pub fn new(
        kind: PacketKind,
        timestamp: Timestamp,
        keyframe: bool,
        payload: Vec<u8>,
    ) -> Result<Self, OutputError> {
        if payload.is_empty() {
            return Err(OutputError::EmptyPacket);
        }
        if payload.len() > MAX_PACKET_BYTES {
            return Err(OutputError::PacketTooLarge {
                bytes: payload.len(),
            });
        }
        Ok(Self {
            kind,
            timestamp,
            keyframe,
            payload,
        })
    }

    /// Returns the packet media kind.
    #[must_use]
    pub const fn kind(&self) -> PacketKind {
        self.kind
    }

    /// Returns the packet timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Returns whether this packet is a keyframe or random-access packet.
    #[must_use]
    pub const fn is_keyframe(&self) -> bool {
        self.keyframe
    }

    /// Returns the encoded payload without copying it.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns the payload size in bytes.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.payload.len()
    }
}

/// Drop policy for a bounded encoded-packet queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketDropPolicy {
    /// Remove the oldest packets until the new packet fits.
    DropOldest,
    /// Keep queued packets and discard the new packet.
    DropNewest,
}

/// Result of submitting an encoded packet to a bounded queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketPushOutcome {
    /// The packet was queued without a drop.
    Enqueued,
    /// Older packets were removed to make room.
    DroppedOldest { packets: usize, bytes: usize },
    /// The submitted packet was discarded.
    DroppedNewest { bytes: usize },
}

/// State of a recording or muxing session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputState {
    /// The session accepts more media.
    Open,
    /// The session has atomically committed its final bytes.
    Finalized,
    /// The session was cancelled and cannot be reused.
    Aborted,
}

/// Lifecycle state of a streaming session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamState {
    /// No transport connection is active; queued packets may remain buffered.
    Disconnected,
    /// Packets can be sent to the transport.
    Connected,
    /// The session has reached its reconnect-attempt limit.
    Failed,
    /// The session has been permanently closed.
    Closed,
}

/// Limits how many reconnect attempts a stream may make.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    max_attempts: u32,
}

impl ReconnectPolicy {
    /// Creates a policy with a fixed maximum number of reconnect attempts.
    #[must_use]
    pub const fn new(max_attempts: u32) -> Self {
        Self { max_attempts }
    }

    /// Returns the maximum number of reconnect attempts.
    #[must_use]
    pub const fn max_attempts(self) -> u32 {
        self.max_attempts
    }
}

/// Counters collected by a streaming session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamMetrics {
    submitted: u64,
    dropped_packets: u64,
    sent_packets: u64,
    send_failures: u64,
    reconnects: u64,
}

impl StreamMetrics {
    /// Number of packets submitted to the stream queue.
    #[must_use]
    pub const fn submitted(self) -> u64 {
        self.submitted
    }

    /// Number of packets dropped by bounded queue pressure.
    #[must_use]
    pub const fn dropped_packets(self) -> u64 {
        self.dropped_packets
    }

    /// Number of packets successfully sent.
    #[must_use]
    pub const fn sent_packets(self) -> u64 {
        self.sent_packets
    }

    /// Number of transport send failures.
    #[must_use]
    pub const fn send_failures(self) -> u64 {
        self.send_failures
    }

    /// Number of successful reconnect operations.
    #[must_use]
    pub const fn reconnects(self) -> u64 {
        self.reconnects
    }
}

/// Errors raised by the reference recorder or decoder.
#[derive(Debug, Eq, PartialEq)]
pub enum OutputError {
    /// The input does not contain a recognized header.
    InvalidHeader,
    /// The input ended before the declared recording was complete.
    Truncated,
    /// The recording declares more frames than the safety limit.
    TooManyFrames { frames: u64 },
    /// The recording exceeds the encoded byte limit.
    TooLarge { bytes: u64 },
    /// A frame does not match the recording format.
    FormatMismatch {
        /// Format expected by the recording.
        expected: VideoFormat,
        /// Format supplied by the caller.
        actual: VideoFormat,
    },
    /// A standard output format cannot represent the requested video layout.
    UnsupportedFormat { reason: String },
    /// An audio buffer does not match an audio encoder's format.
    AudioFormatMismatch {
        /// Format expected by the encoder.
        expected: AudioFormat,
        /// Format supplied by the caller.
        actual: AudioFormat,
    },
    /// A media invariant failed while decoding.
    Media(MediaError),
    /// The byte stream could not be written.
    Write(String),
    /// A packet payload is empty.
    EmptyPacket,
    /// A packet payload exceeds [`MAX_PACKET_BYTES`].
    PacketTooLarge { bytes: usize },
    /// A serialized packet has an unknown media-kind tag.
    InvalidPacketKind { tag: u8 },
    /// A serialized packet has a keyframe flag other than zero or one.
    InvalidPacketFlag { value: u8 },
    /// A packet timestamp moved backward within one muxed stream.
    NonMonotonicTimestamp {
        /// Timestamp of the previously accepted packet.
        previous: Timestamp,
        /// Timestamp of the packet that moved backward.
        actual: Timestamp,
    },
    /// A packet queue or recording capacity is zero.
    ZeroCapacity,
    /// An encoded reference-codec payload is structurally invalid.
    InvalidCodecPayload(String),
    /// A packet cannot fit in the configured queue capacity.
    PacketDoesNotFit {
        /// Packet size that was submitted.
        packet_bytes: usize,
        /// Queue capacity in bytes.
        capacity_bytes: usize,
    },
    /// An operation was attempted after a session changed state.
    InvalidState {
        /// Operation that was requested.
        operation: &'static str,
        /// Current session state.
        state: OutputState,
    },
    /// The final and temporary recording paths are not usable together.
    InvalidPaths { reason: String },
    /// A transport operation failed.
    Transport(String),
    /// The stream exhausted its reconnect budget.
    ReconnectExhausted { attempts: u32 },
    /// An operation was attempted in an incompatible stream state.
    InvalidStreamState {
        /// Operation that was requested.
        operation: &'static str,
        /// Current stream state.
        state: StreamState,
    },
}

impl fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader => formatter.write_str("invalid OBS-RS raw recording header"),
            Self::Truncated => formatter.write_str("recording ended before all frames were read"),
            Self::TooManyFrames { frames } => {
                write!(formatter, "recording contains too many frames: {frames}")
            }
            Self::TooLarge { bytes } => {
                write!(formatter, "recording contains too many bytes: {bytes}")
            }
            Self::FormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "frame format {actual:?} does not match {expected:?}"
                )
            }
            Self::UnsupportedFormat { reason } => {
                write!(formatter, "output format is unsupported: {reason}")
            }
            Self::AudioFormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio format {actual:?} does not match {expected:?}"
                )
            }
            Self::Media(error) => error.fmt(formatter),
            Self::Write(error) => write!(formatter, "recording write failed: {error}"),
            Self::EmptyPacket => formatter.write_str("encoded packet payload must be non-empty"),
            Self::PacketTooLarge { bytes } => {
                write!(formatter, "encoded packet is too large: {bytes} bytes")
            }
            Self::InvalidPacketKind { tag } => {
                write!(formatter, "unknown encoded packet kind tag: {tag}")
            }
            Self::InvalidPacketFlag { value } => {
                write!(formatter, "invalid encoded packet keyframe flag: {value}")
            }
            Self::NonMonotonicTimestamp { previous, actual } => write!(
                formatter,
                "packet timestamp {actual:?} is before the previous {previous:?}"
            ),
            Self::ZeroCapacity => formatter.write_str("output queue capacity must be non-zero"),
            Self::InvalidCodecPayload(reason) => {
                write!(formatter, "invalid reference codec payload: {reason}")
            }
            Self::PacketDoesNotFit {
                packet_bytes,
                capacity_bytes,
            } => write!(
                formatter,
                "packet of {packet_bytes} bytes cannot fit in {capacity_bytes}-byte queue"
            ),
            Self::InvalidState { operation, state } => {
                write!(formatter, "cannot {operation} output in {state:?} state")
            }
            Self::InvalidPaths { reason } => write!(formatter, "invalid output paths: {reason}"),
            Self::Transport(reason) => write!(formatter, "output transport failed: {reason}"),
            Self::ReconnectExhausted { attempts } => {
                write!(
                    formatter,
                    "stream reconnect limit exhausted after {attempts} attempts"
                )
            }
            Self::InvalidStreamState { operation, state } => {
                write!(formatter, "cannot {operation} stream in {state:?} state")
            }
        }
    }
}

impl std::error::Error for OutputError {}

/// A bounded queue for encoded packets.
pub struct PacketQueue {
    capacity_bytes: usize,
    policy: PacketDropPolicy,
    queued_bytes: usize,
    packets: VecDeque<EncodedPacket>,
}

impl PacketQueue {
    /// Creates a queue with a byte bound and explicit drop policy.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::ZeroCapacity`] when `capacity_bytes` is zero.
    pub fn new(capacity_bytes: usize, policy: PacketDropPolicy) -> Result<Self, OutputError> {
        if capacity_bytes == 0 {
            return Err(OutputError::ZeroCapacity);
        }
        if capacity_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: capacity_bytes as u64,
            });
        }
        Ok(Self {
            capacity_bytes,
            policy,
            queued_bytes: 0,
            packets: VecDeque::new(),
        })
    }

    /// Pushes one packet while enforcing the configured byte bound.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::PacketDoesNotFit`] when a single packet is larger
    /// than the queue capacity, or [`OutputError::InvalidState`] is never raised
    /// because queues do not have a terminal state.
    pub fn push(&mut self, packet: EncodedPacket) -> Result<PacketPushOutcome, OutputError> {
        let incoming_bytes = packet.byte_len();
        if incoming_bytes > self.capacity_bytes {
            return Err(OutputError::PacketDoesNotFit {
                packet_bytes: incoming_bytes,
                capacity_bytes: self.capacity_bytes,
            });
        }

        let free_bytes = self.capacity_bytes.saturating_sub(self.queued_bytes);
        if incoming_bytes <= free_bytes {
            self.queued_bytes += incoming_bytes;
            self.packets.push_back(packet);
            return Ok(PacketPushOutcome::Enqueued);
        }

        match self.policy {
            PacketDropPolicy::DropNewest => Ok(PacketPushOutcome::DroppedNewest {
                bytes: incoming_bytes,
            }),
            PacketDropPolicy::DropOldest => {
                let mut dropped_packets = 0;
                let mut dropped_bytes = 0;
                while incoming_bytes > self.capacity_bytes.saturating_sub(self.queued_bytes) {
                    let Some(dropped) = self.packets.pop_front() else {
                        break;
                    };
                    let bytes = dropped.byte_len();
                    self.queued_bytes -= bytes;
                    dropped_packets += 1;
                    dropped_bytes += bytes;
                }
                self.queued_bytes += incoming_bytes;
                self.packets.push_back(packet);
                Ok(PacketPushOutcome::DroppedOldest {
                    packets: dropped_packets,
                    bytes: dropped_bytes,
                })
            }
        }
    }

    /// Re-inserts one packet at the front without applying a drop policy.
    ///
    /// This is used to preserve packet order when a transport fails after the
    /// packet has been removed for delivery.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::PacketDoesNotFit`] when the packet cannot fit in the
    /// queue's byte capacity.
    pub fn push_front(&mut self, packet: EncodedPacket) -> Result<(), OutputError> {
        let incoming_bytes = packet.byte_len();
        if incoming_bytes > self.capacity_bytes
            || incoming_bytes > self.capacity_bytes.saturating_sub(self.queued_bytes)
        {
            return Err(OutputError::PacketDoesNotFit {
                packet_bytes: incoming_bytes,
                capacity_bytes: self.capacity_bytes,
            });
        }
        self.queued_bytes += incoming_bytes;
        self.packets.push_front(packet);
        Ok(())
    }

    /// Removes and returns the oldest packet.
    pub fn pop(&mut self) -> Option<EncodedPacket> {
        let packet = self.packets.pop_front()?;
        self.queued_bytes -= packet.byte_len();
        Some(packet)
    }

    /// Removes all queued packets.
    pub fn clear(&mut self) {
        self.packets.clear();
        self.queued_bytes = 0;
    }

    /// Returns the number of queued bytes.
    #[must_use]
    pub const fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    /// Returns the configured byte capacity.
    #[must_use]
    pub const fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }

    /// Returns the number of queued packets.
    #[must_use]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    /// Returns whether no packets are queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

/// Encoder boundary for video frames.
pub trait VideoEncoder {
    /// Returns the fixed input format accepted by the encoder.
    fn format(&self) -> VideoFormat;

    /// Encodes one frame into one validated packet.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the frame format or encoded payload is
    /// invalid.
    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError>;

    /// Flushes delayed packets, if the codec has any.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when flushing the codec fails.
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError>;
}

/// A deterministic one-packet-per-frame reference encoder.
pub struct RawVideoEncoder {
    format: VideoFormat,
}

impl RawVideoEncoder {
    /// Creates an encoder for one fixed RGBA video format.
    #[must_use]
    pub const fn new(format: VideoFormat) -> Self {
        Self { format }
    }
}

impl VideoEncoder for RawVideoEncoder {
    fn format(&self) -> VideoFormat {
        self.format
    }

    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        EncodedPacket::new(
            PacketKind::Video,
            frame.timestamp(),
            true,
            frame.pixels().to_vec(),
        )
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError> {
        Ok(Vec::new())
    }
}

/// A standards-based PNG screenshot encoder implemented without native codecs.
///
/// The PNG uses RGBA8 pixels and zlib stored blocks. Stored blocks are intentionally
/// simple and deterministic; they provide interoperability and a safe reference
/// path, while a later production encoder may replace the compression strategy
/// behind the same [`VideoEncoder`] contract.
pub struct PngVideoEncoder {
    format: VideoFormat,
}

impl PngVideoEncoder {
    /// Creates an encoder for one fixed RGBA video format.
    #[must_use]
    pub const fn new(format: VideoFormat) -> Self {
        Self { format }
    }
}

impl VideoEncoder for PngVideoEncoder {
    fn format(&self) -> VideoFormat {
        self.format
    }

    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        EncodedPacket::new(
            PacketKind::Video,
            frame.timestamp(),
            true,
            encode_png(frame)?,
        )
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError> {
        Ok(Vec::new())
    }
}

/// Encodes one owned RGBA frame as an interoperable PNG image.
///
/// # Errors
///
/// Returns [`OutputError::TooLarge`] when the deterministic stored-block PNG would
/// exceed the packet safety limit.
pub fn encode_png(frame: &VideoFrame) -> Result<Vec<u8>, OutputError> {
    let format = frame.format();
    let width =
        usize::try_from(format.width()).map_err(|_| OutputError::TooLarge { bytes: u64::MAX })?;
    let height =
        usize::try_from(format.height()).map_err(|_| OutputError::TooLarge { bytes: u64::MAX })?;
    let scanline_bytes = width
        .checked_mul(4)
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    let uncompressed_bytes = scanline_bytes
        .checked_add(1)
        .and_then(|bytes| bytes.checked_mul(height))
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    let blocks = uncompressed_bytes.saturating_add(65_534) / 65_535;
    let zlib_bytes = 2_usize
        .checked_add(uncompressed_bytes)
        .and_then(|bytes| bytes.checked_add(blocks.saturating_mul(5)))
        .and_then(|bytes| bytes.checked_add(4))
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    let encoded_bytes = PNG_SIGNATURE
        .len()
        .checked_add(12 + 13)
        .and_then(|bytes| bytes.checked_add(12 + zlib_bytes))
        .and_then(|bytes| bytes.checked_add(12))
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    if encoded_bytes > MAX_PACKET_BYTES {
        return Err(OutputError::TooLarge {
            bytes: u64::try_from(encoded_bytes).unwrap_or(u64::MAX),
        });
    }

    let mut raw = Vec::with_capacity(uncompressed_bytes);
    for row in frame.pixels().chunks_exact(scanline_bytes) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let zlib = zlib_stored(&raw);
    let mut png = Vec::with_capacity(encoded_bytes);
    png.extend_from_slice(PNG_SIGNATURE);
    let mut header = Vec::with_capacity(13);
    push_u32_be(&mut header, format.width());
    push_u32_be(&mut header, format.height());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    append_png_chunk(&mut png, *b"IHDR", &header);
    append_png_chunk(&mut png, *b"IDAT", &zlib);
    append_png_chunk(&mut png, *b"IEND", &[]);
    Ok(png)
}

fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let blocks = raw.len().saturating_add(65_534) / 65_535;
    let mut output = Vec::with_capacity(2 + raw.len() + blocks.saturating_mul(5) + 4);
    output.extend_from_slice(&[0x78, 0x01]);
    if raw.is_empty() {
        output.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    } else {
        for (index, block) in raw.chunks(65_535).enumerate() {
            let final_block = index + 1 == blocks;
            output.push(u8::from(final_block));
            let length = u16::try_from(block.len()).unwrap_or(u16::MAX);
            output.extend_from_slice(&length.to_le_bytes());
            output.extend_from_slice(&(!length).to_le_bytes());
            output.extend_from_slice(block);
        }
    }
    push_u32_be(&mut output, adler32(raw));
    output
}

fn append_png_chunk(output: &mut Vec<u8>, kind: [u8; 4], payload: &[u8]) {
    push_u32_be(output, u32::try_from(payload.len()).unwrap_or(u32::MAX));
    output.extend_from_slice(&kind);
    output.extend_from_slice(payload);
    let mut crc_input = Vec::with_capacity(kind.len() + payload.len());
    crc_input.extend_from_slice(&kind);
    crc_input.extend_from_slice(payload);
    push_u32_be(output, crc32(&crc_input));
}

fn push_u32_be(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn adler32(bytes: &[u8]) -> u32 {
    let mut sum_a = 1_u32;
    let mut sum_b = 0_u32;
    for byte in bytes {
        sum_a = (sum_a + u32::from(*byte)) % 65_521;
        sum_b = (sum_b + sum_a) % 65_521;
    }
    (sum_b << 16) | sum_a
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A deterministic lossless RGBA run-length video encoder.
pub struct RleVideoEncoder {
    format: VideoFormat,
}

impl RleVideoEncoder {
    /// Creates an encoder for one fixed RGBA video format.
    #[must_use]
    pub const fn new(format: VideoFormat) -> Self {
        Self { format }
    }
}

impl VideoEncoder for RleVideoEncoder {
    fn format(&self) -> VideoFormat {
        self.format
    }

    fn encode(&mut self, frame: &VideoFrame) -> Result<EncodedPacket, OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }

        let mut payload = Vec::new();
        write_all(&mut payload, RLE_MAGIC)?;
        let mut current = None;
        let mut run_length = 0_u32;
        for chunk in frame.pixels().chunks_exact(4) {
            let pixel = [chunk[0], chunk[1], chunk[2], chunk[3]];
            if current == Some(pixel) && run_length < u32::MAX {
                run_length += 1;
                continue;
            }
            if let Some(previous) = current {
                write_u32(&mut payload, run_length)?;
                write_all(&mut payload, &previous)?;
            }
            current = Some(pixel);
            run_length = 1;
        }
        if let Some(last) = current {
            write_u32(&mut payload, run_length)?;
            write_all(&mut payload, &last)?;
        }
        EncodedPacket::new(PacketKind::Video, frame.timestamp(), true, payload)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError> {
        Ok(Vec::new())
    }
}

/// Decoder for the deterministic lossless RLE video payload.
pub struct RleVideoDecoder;

impl RleVideoDecoder {
    /// Decodes one RLE video packet payload into an owned RGBA frame.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidCodecPayload`] for invalid runs, a wrong
    /// header, or trailing bytes; returns [`OutputError::Truncated`] for a short
    /// run record and [`OutputError::Media`] for a violated frame invariant.
    pub fn decode(
        format: VideoFormat,
        timestamp: Timestamp,
        payload: &[u8],
    ) -> Result<VideoFrame, OutputError> {
        if payload.len() > MAX_PACKET_BYTES {
            return Err(OutputError::PacketTooLarge {
                bytes: payload.len(),
            });
        }
        let mut cursor = Cursor::new(payload);
        let mut magic = [0_u8; 8];
        read_exact(&mut cursor, &mut magic)?;
        if &magic != RLE_MAGIC {
            return Err(OutputError::InvalidCodecPayload(
                "invalid RLE header".to_owned(),
            ));
        }

        let expected_pixels = format.pixel_count();
        let mut decoded_pixels = Vec::with_capacity(format.rgba_bytes());
        let mut decoded_count = 0_usize;
        while decoded_count < expected_pixels {
            let run = usize::try_from(read_u32(&mut cursor)?).unwrap_or(usize::MAX);
            if run == 0 || run > expected_pixels.saturating_sub(decoded_count) {
                return Err(OutputError::InvalidCodecPayload(
                    "RLE run exceeds the expected pixel count".to_owned(),
                ));
            }
            let mut pixel = [0_u8; 4];
            read_exact(&mut cursor, &mut pixel)?;
            for _ in 0..run {
                decoded_pixels.extend_from_slice(&pixel);
            }
            decoded_count += run;
        }
        if cursor.position() != u64::try_from(payload.len()).unwrap_or(u64::MAX) {
            return Err(OutputError::InvalidCodecPayload(
                "RLE payload has trailing bytes".to_owned(),
            ));
        }
        VideoFrame::new(format, timestamp, decoded_pixels).map_err(OutputError::Media)
    }
}

/// Encoder boundary for interleaved audio buffers.
pub trait AudioEncoder {
    /// Returns the fixed input format accepted by the encoder.
    fn format(&self) -> AudioFormat;

    /// Encodes one audio buffer into one validated packet.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the buffer format or encoded payload is
    /// invalid.
    fn encode(&mut self, buffer: &AudioBuffer) -> Result<EncodedPacket, OutputError>;

    /// Flushes delayed packets, if the codec has any.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when flushing the codec fails.
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError>;
}

/// A deterministic little-endian PCM audio encoder.
pub struct RawAudioEncoder {
    format: AudioFormat,
}

impl RawAudioEncoder {
    /// Creates an encoder for one fixed interleaved audio format.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self { format }
    }
}

impl AudioEncoder for RawAudioEncoder {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn encode(&mut self, buffer: &AudioBuffer) -> Result<EncodedPacket, OutputError> {
        if buffer.format() != self.format {
            return Err(OutputError::AudioFormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            });
        }
        let mut payload = Vec::with_capacity(buffer.samples().len().saturating_mul(4));
        for sample in buffer.samples() {
            payload.extend_from_slice(&sample.to_le_bytes());
        }
        EncodedPacket::new(PacketKind::Audio, buffer.timestamp(), false, payload)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, OutputError> {
        Ok(Vec::new())
    }
}

/// An offline PCM16 WAV recording assembled from one fixed audio format.
pub struct WavRecording {
    format: AudioFormat,
    buffers: Vec<AudioBuffer>,
    frames: usize,
}

impl WavRecording {
    /// Creates an empty WAV recording.
    #[must_use]
    pub const fn new(format: AudioFormat) -> Self {
        Self {
            format,
            buffers: Vec::new(),
            frames: 0,
        }
    }

    /// Appends one complete interleaved buffer.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::AudioFormatMismatch`] for a different format or
    /// [`OutputError::TooLarge`] when the resulting WAV exceeds the reference
    /// byte budget.
    pub fn push(&mut self, buffer: AudioBuffer) -> Result<(), OutputError> {
        if buffer.format() != self.format {
            return Err(OutputError::AudioFormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            });
        }
        let frames = self
            .frames
            .checked_add(buffer.frames())
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        if frames > MAX_AUDIO_FRAMES {
            return Err(OutputError::TooLarge {
                bytes: u64::try_from(frames).unwrap_or(u64::MAX),
            });
        }
        let data_bytes = frames
            .checked_mul(usize::from(self.format.channels()))
            .and_then(|samples| samples.checked_mul(2))
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let encoded_bytes = 44_usize
            .checked_add(data_bytes)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        if encoded_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: encoded_bytes as u64,
            });
        }
        self.frames = frames;
        self.buffers.push(buffer);
        Ok(())
    }

    /// Returns the fixed recording format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Returns the number of interleaved audio frames.
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.frames
    }

    /// Returns whether no audio frames have been appended.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// Encodes the recording as a canonical little-endian PCM16 WAV file.
    ///
    /// Samples are clamped to `[-1.0, 1.0]` and converted deterministically to
    /// signed 16-bit PCM. Buffer timestamps are intentionally not serialized by
    /// WAV; timestamped packet output remains available through the packet API.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::TooLarge`] when the encoded length cannot fit in a
    /// standard WAV 32-bit chunk size.
    pub fn encode(&self) -> Result<Vec<u8>, OutputError> {
        let data_bytes = self
            .frames
            .checked_mul(usize::from(self.format.channels()))
            .and_then(|samples| samples.checked_mul(2))
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let riff_size = 36_usize
            .checked_add(data_bytes)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let data_bytes_u32 = u32::try_from(data_bytes).map_err(|_| OutputError::TooLarge {
            bytes: data_bytes as u64,
        })?;
        let riff_size_u32 = u32::try_from(riff_size).map_err(|_| OutputError::TooLarge {
            bytes: riff_size as u64,
        })?;
        let byte_rate = self
            .format
            .sample_rate()
            .checked_mul(u32::from(self.format.channels()))
            .and_then(|value| value.checked_mul(2))
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let block_align = self
            .format
            .channels()
            .checked_mul(2)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;

        let mut bytes = Vec::with_capacity(44 + data_bytes);
        write_all(&mut bytes, b"RIFF")?;
        write_u32(&mut bytes, riff_size_u32)?;
        write_all(&mut bytes, b"WAVEfmt ")?;
        write_u32(&mut bytes, 16)?;
        write_u16(&mut bytes, 1)?;
        write_u16(&mut bytes, self.format.channels())?;
        write_u32(&mut bytes, self.format.sample_rate())?;
        write_u32(&mut bytes, byte_rate)?;
        write_u16(&mut bytes, block_align)?;
        write_u16(&mut bytes, 16)?;
        write_all(&mut bytes, b"data")?;
        write_u32(&mut bytes, data_bytes_u32)?;
        for buffer in &self.buffers {
            for sample in buffer.samples() {
                write_u16(&mut bytes, pcm16_bits(*sample))?;
            }
        }
        Ok(bytes)
    }
}

/// An interoperable YUV4MPEG2 recording with pure-Rust RGBA-to-4:2:0 conversion.
///
/// Y4M is an uncompressed frame container intended for exchange and reference
/// tooling. It is deliberately not presented as a distribution codec; it gives the
/// output pipeline a standard file artifact without a native encoder dependency.
pub struct Y4mRecording {
    format: VideoFormat,
    frames: Vec<VideoFrame>,
    encoded_bytes: usize,
    last_timestamp: Option<Timestamp>,
}

impl Y4mRecording {
    /// Creates an empty Y4M recording for one fixed video format.
    #[must_use]
    pub const fn new(format: VideoFormat) -> Self {
        Self {
            format,
            frames: Vec::new(),
            encoded_bytes: 0,
            last_timestamp: None,
        }
    }

    /// Adds one RGBA frame after validating dimensions, timestamps, and budget.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::UnsupportedFormat`] for odd dimensions,
    /// [`OutputError::FormatMismatch`] for another video format,
    /// [`OutputError::NonMonotonicTimestamp`] for backward timestamps, or
    /// [`OutputError::TooLarge`] when the Y4M recording exceeds the reference
    /// recording budget.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        self.validate_format()?;
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        if let Some(previous) = self.last_timestamp {
            if frame.timestamp() < previous {
                return Err(OutputError::NonMonotonicTimestamp {
                    previous,
                    actual: frame.timestamp(),
                });
            }
        }
        if self.frames.len() >= MAX_RECORDING_FRAMES {
            return Err(OutputError::TooManyFrames {
                frames: self.frames.len() as u64 + 1,
            });
        }
        let encoded_bytes = y4m_encoded_size(self.format, self.frames.len() + 1)?;
        if encoded_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: encoded_bytes as u64,
            });
        }
        self.frames.push(frame);
        self.encoded_bytes = encoded_bytes;
        self.last_timestamp = self.frames.last().map(VideoFrame::timestamp);
        Ok(())
    }

    /// Returns the fixed recording format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the number of accepted frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Returns whether no frames have been accepted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Encodes the recording as a YUV4MPEG2 byte stream.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::UnsupportedFormat`] for odd dimensions or a write
    /// error from the in-memory output path.
    pub fn encode(&self) -> Result<Vec<u8>, OutputError> {
        self.validate_format()?;
        let header = y4m_header(self.format);
        let mut bytes = Vec::with_capacity(self.encoded_bytes.max(header.len()));
        write_all(&mut bytes, header.as_bytes())?;
        for frame in &self.frames {
            write_y4m_frame(&mut bytes, frame);
        }
        Ok(bytes)
    }

    fn validate_format(&self) -> Result<(), OutputError> {
        if !self.format.width().is_multiple_of(2) || !self.format.height().is_multiple_of(2) {
            return Err(OutputError::UnsupportedFormat {
                reason: "YUV4MPEG2 4:2:0 requires even width and height".to_owned(),
            });
        }
        Ok(())
    }
}

fn y4m_header(format: VideoFormat) -> String {
    format!(
        "YUV4MPEG2 W{} H{} F{}:{} Ip A0:0 C420jpeg\n",
        format.width(),
        format.height(),
        format.frame_rate().numerator(),
        format.frame_rate().denominator()
    )
}

fn y4m_encoded_size(format: VideoFormat, frames: usize) -> Result<usize, OutputError> {
    let header_bytes = y4m_header(format).len();
    let frame_bytes = 6_usize
        .checked_add(format.pixel_count())
        .and_then(|bytes| bytes.checked_add(format.pixel_count() / 2))
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
    header_bytes
        .checked_add(
            frames
                .checked_mul(frame_bytes)
                .ok_or(OutputError::TooLarge { bytes: u64::MAX })?,
        )
        .ok_or(OutputError::TooLarge { bytes: u64::MAX })
}

fn write_y4m_frame(output: &mut Vec<u8>, frame: &VideoFrame) {
    output.extend_from_slice(b"FRAME\n");
    let format = frame.format();
    let width = usize::try_from(format.width()).unwrap_or(usize::MAX);
    let height = usize::try_from(format.height()).unwrap_or(usize::MAX);
    let mut luma = Vec::with_capacity(format.pixel_count());
    for pixel in frame.pixels().chunks_exact(4) {
        let (y, _, _) = rgb_to_yuv(pixel[0], pixel[1], pixel[2]);
        luma.push(y);
    }
    output.extend_from_slice(&luma);

    let mut chroma_u = Vec::with_capacity(format.pixel_count() / 4);
    let mut chroma_v = Vec::with_capacity(format.pixel_count() / 4);
    for block_y in (0..height).step_by(2) {
        for block_x in (0..width).step_by(2) {
            let mut u_sum = 0_u32;
            let mut v_sum = 0_u32;
            for y in block_y..block_y + 2 {
                for x in block_x..block_x + 2 {
                    let offset = (y * width + x) * 4;
                    let (_, u, v) = rgb_to_yuv(
                        frame.pixels()[offset],
                        frame.pixels()[offset + 1],
                        frame.pixels()[offset + 2],
                    );
                    u_sum += u32::from(u);
                    v_sum += u32::from(v);
                }
            }
            chroma_u.push(u8::try_from((u_sum + 2) / 4).unwrap_or(u8::MAX));
            chroma_v.push(u8::try_from((v_sum + 2) / 4).unwrap_or(u8::MAX));
        }
    }
    output.extend_from_slice(&chroma_u);
    output.extend_from_slice(&chroma_v);
}

fn rgb_to_yuv(red: u8, green: u8, blue: u8) -> (u8, u8, u8) {
    let red = i32::from(red);
    let green = i32::from(green);
    let blue = i32::from(blue);
    let y = (77 * red + 150 * green + 29 * blue + 128) >> 8;
    let u = (-43 * red - 85 * green + 128 * blue + 32_768) >> 8;
    let v = (128 * red - 107 * green - 21 * blue + 32_768) >> 8;
    (clamp_byte(y), clamp_byte(u), clamp_byte(v))
}

fn clamp_byte(value: i32) -> u8 {
    u8::try_from(value.clamp(0, i32::from(u8::MAX))).unwrap_or_default()
}

/// Muxer boundary for encoded packets.
pub trait PacketMuxer {
    /// Accepts one packet in timestamp order.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the session is closed, the packet limit is
    /// exceeded, or the packet cannot be represented.
    fn push(&mut self, packet: EncodedPacket) -> Result<(), OutputError>;

    /// Atomically commits all accepted packets and returns the final bytes.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the session is not open or serialization
    /// fails.
    fn finalize(&mut self) -> Result<Vec<u8>, OutputError>;

    /// Cancels the session and discards uncommitted data.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the session is not open.
    fn abort(&mut self) -> Result<(), OutputError>;

    /// Returns the current lifecycle state.
    fn state(&self) -> OutputState;
}

/// Transport boundary used by the streaming session.
pub trait PacketTransport {
    /// Establishes or re-establishes the transport connection.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::Transport`] when the connection cannot be opened.
    fn connect(&mut self) -> Result<(), OutputError>;

    /// Sends one packet without taking ownership of the queued copy.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::Transport`] when delivery fails. The stream session
    /// re-queues the packet before returning the error.
    fn send(&mut self, packet: &EncodedPacket) -> Result<(), OutputError>;

    /// Closes the transport without changing queued packet ownership.
    fn disconnect(&mut self);
}

/// A bounded, reconnectable packet stream session.
pub struct StreamSession<T: PacketTransport> {
    transport: T,
    queue: PacketQueue,
    reconnect_policy: ReconnectPolicy,
    reconnect_attempts: u32,
    state: StreamState,
    metrics: StreamMetrics,
}

impl<T: PacketTransport> StreamSession<T> {
    /// Creates a disconnected stream session with bounded packet storage.
    ///
    /// # Errors
    ///
    /// Returns queue-capacity errors from [`PacketQueue::new`].
    pub fn new(
        transport: T,
        capacity_bytes: usize,
        drop_policy: PacketDropPolicy,
        reconnect_policy: ReconnectPolicy,
    ) -> Result<Self, OutputError> {
        Ok(Self {
            transport,
            queue: PacketQueue::new(capacity_bytes, drop_policy)?,
            reconnect_policy,
            reconnect_attempts: 0,
            state: StreamState::Disconnected,
            metrics: StreamMetrics::default(),
        })
    }

    /// Connects the transport for the first time.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] for a closed or failed stream, or
    /// [`OutputError::Transport`] when the transport rejects the connection.
    pub fn connect(&mut self) -> Result<(), OutputError> {
        self.ensure_connectable("connect")?;
        self.transport.connect()?;
        self.state = StreamState::Connected;
        Ok(())
    }

    /// Queues one packet without blocking on the transport.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] for a closed or failed stream, or a
    /// queue-capacity error.
    pub fn submit(&mut self, packet: EncodedPacket) -> Result<PacketPushOutcome, OutputError> {
        if matches!(self.state, StreamState::Closed | StreamState::Failed) {
            return Err(OutputError::InvalidStreamState {
                operation: "submit a stream packet",
                state: self.state,
            });
        }
        let outcome = self.queue.push(packet)?;
        self.metrics.submitted = self.metrics.submitted.saturating_add(1);
        match outcome {
            PacketPushOutcome::DroppedOldest { packets, .. } => {
                self.metrics.dropped_packets =
                    self.metrics.dropped_packets.saturating_add(packets as u64);
            }
            PacketPushOutcome::DroppedNewest { .. } => {
                self.metrics.dropped_packets = self.metrics.dropped_packets.saturating_add(1);
            }
            PacketPushOutcome::Enqueued => {}
        }
        Ok(outcome)
    }

    /// Sends all currently queued packets.
    ///
    /// A failed packet is put back at the front before the error is returned, so
    /// reconnecting can retry it without blocking the producer.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] while disconnected, or the transport
    /// error from the failed send.
    pub fn flush(&mut self) -> Result<usize, OutputError> {
        if self.state != StreamState::Connected {
            return Err(OutputError::InvalidStreamState {
                operation: "flush stream packets",
                state: self.state,
            });
        }
        let mut sent = 0;
        while let Some(packet) = self.queue.pop() {
            if let Err(error) = self.transport.send(&packet) {
                self.metrics.send_failures = self.metrics.send_failures.saturating_add(1);
                self.state = StreamState::Disconnected;
                self.queue.push_front(packet)?;
                return Err(error);
            }
            sent += 1;
            self.metrics.sent_packets = self.metrics.sent_packets.saturating_add(1);
        }
        Ok(sent)
    }

    /// Attempts one reconnect under the configured budget.
    ///
    /// # Errors
    ///
    /// Returns the transport error when this attempt fails, or
    /// [`OutputError::ReconnectExhausted`] after the limit is reached.
    pub fn reconnect(&mut self) -> Result<(), OutputError> {
        self.ensure_connectable("reconnect")?;
        if self.reconnect_attempts >= self.reconnect_policy.max_attempts {
            self.state = StreamState::Failed;
            return Err(OutputError::ReconnectExhausted {
                attempts: self.reconnect_attempts,
            });
        }
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        match self.transport.connect() {
            Ok(()) => {
                self.state = StreamState::Connected;
                self.metrics.reconnects = self.metrics.reconnects.saturating_add(1);
                Ok(())
            }
            Err(error) => {
                if self.reconnect_attempts >= self.reconnect_policy.max_attempts {
                    self.state = StreamState::Failed;
                }
                Err(error)
            }
        }
    }

    /// Disconnects while preserving queued packets for a later reconnect.
    pub fn disconnect(&mut self) {
        if self.state != StreamState::Closed {
            self.transport.disconnect();
            self.state = StreamState::Disconnected;
        }
    }

    /// Permanently closes the stream and discards queued packets.
    pub fn close(&mut self) {
        if self.state != StreamState::Closed {
            self.transport.disconnect();
            self.queue.clear();
            self.state = StreamState::Closed;
        }
    }

    /// Returns the stream lifecycle state.
    #[must_use]
    pub const fn state(&self) -> StreamState {
        self.state
    }

    /// Returns the number of queued bytes.
    #[must_use]
    pub const fn queued_bytes(&self) -> usize {
        self.queue.queued_bytes()
    }

    /// Returns stream metrics.
    #[must_use]
    pub const fn metrics(&self) -> StreamMetrics {
        self.metrics
    }

    /// Returns the reconnect attempts consumed so far.
    #[must_use]
    pub const fn reconnect_attempts(&self) -> u32 {
        self.reconnect_attempts
    }

    /// Borrows the transport for inspection.
    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    fn ensure_connectable(&self, operation: &'static str) -> Result<(), OutputError> {
        match self.state {
            StreamState::Disconnected | StreamState::Connected => Ok(()),
            state => Err(OutputError::InvalidStreamState { operation, state }),
        }
    }
}
/// A deterministic in-memory packet transport for stream tests and offline demos.
pub struct MemoryPacketTransport {
    connected: bool,
    fail_next_send: bool,
    sent: Vec<EncodedPacket>,
}

impl MemoryPacketTransport {
    /// Creates a disconnected memory transport.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            connected: false,
            fail_next_send: false,
            sent: Vec::new(),
        }
    }

    /// Makes the next send fail and disconnect the transport.
    pub fn fail_next_send(&mut self) {
        self.fail_next_send = true;
    }

    /// Returns packets successfully delivered to the transport.
    #[must_use]
    pub fn sent(&self) -> &[EncodedPacket] {
        &self.sent
    }

    /// Returns whether the transport is connected.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.connected
    }
}

impl Default for MemoryPacketTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketTransport for MemoryPacketTransport {
    fn connect(&mut self) -> Result<(), OutputError> {
        self.connected = true;
        Ok(())
    }

    fn send(&mut self, packet: &EncodedPacket) -> Result<(), OutputError> {
        if !self.connected {
            return Err(OutputError::Transport(
                "transport is disconnected".to_owned(),
            ));
        }
        if self.fail_next_send {
            self.fail_next_send = false;
            self.connected = false;
            return Err(OutputError::Transport("injected send failure".to_owned()));
        }
        self.sent.push(packet.clone());
        Ok(())
    }

    fn disconnect(&mut self) {
        self.connected = false;
    }
}

/// A standard-library TCP transport using explicit length-framed OBS-RS packets.
///
/// The framing is a transport fixture, not a claim of compatibility with RTMP,
/// SRT, WebRTC, or another production streaming protocol. It gives the stream
/// session a real Rust-owned network path while protocol selection remains open.
pub struct TcpPacketTransport {
    address: String,
    stream: Option<TcpStream>,
}

impl TcpPacketTransport {
    /// Creates a disconnected transport for a `host:port` address.
    #[must_use]
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            stream: None,
        }
    }

    /// Returns the configured destination address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Returns whether a TCP stream is currently connected.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.stream.is_some()
    }
}

impl PacketTransport for TcpPacketTransport {
    fn connect(&mut self) -> Result<(), OutputError> {
        let stream = TcpStream::connect(&self.address)
            .map_err(|error| OutputError::Transport(format!("TCP connect failed: {error}")))?;
        stream
            .set_nodelay(true)
            .map_err(|error| OutputError::Transport(format!("TCP setup failed: {error}")))?;
        self.stream = Some(stream);
        Ok(())
    }

    fn send(&mut self, packet: &EncodedPacket) -> Result<(), OutputError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| OutputError::Transport("TCP transport is disconnected".to_owned()))?;
        let mut bytes = Vec::with_capacity(30 + packet.byte_len());
        write_all(&mut bytes, TCP_PACKET_MAGIC)?;
        bytes.push(packet.kind.tag());
        bytes.push(u8::from(packet.is_keyframe()));
        write_u64(&mut bytes, packet.timestamp().as_nanos())?;
        write_u64(&mut bytes, packet.byte_len() as u64)?;
        write_all(&mut bytes, packet.payload())?;
        stream
            .write_all(&bytes)
            .map_err(|error| OutputError::Transport(format!("TCP send failed: {error}")))
    }

    fn disconnect(&mut self) {
        self.stream = None;
    }
}

/// An in-memory deterministic packet container used as a muxer fixture.
pub struct MemoryMuxer {
    packets: Vec<EncodedPacket>,
    state: OutputState,
    committed: Option<Vec<u8>>,
    encoded_bytes: usize,
    last_timestamp: Option<Timestamp>,
}

impl MemoryMuxer {
    /// Creates an empty open muxer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            packets: Vec::new(),
            state: OutputState::Open,
            committed: None,
            encoded_bytes: PACKET_HEADER_BYTES,
            last_timestamp: None,
        }
    }

    /// Returns packets accepted before finalization.
    #[must_use]
    pub fn packets(&self) -> &[EncodedPacket] {
        &self.packets
    }

    /// Returns the committed container bytes after finalization.
    #[must_use]
    pub fn committed_bytes(&self) -> Option<&[u8]> {
        self.committed.as_deref()
    }

    /// Decodes the deterministic packet fixture.
    ///
    /// # Errors
    ///
    /// Returns an [`OutputError`] when the stream is malformed or exceeds the
    /// same safety limits used during encoding.
    pub fn decode(bytes: &[u8]) -> Result<Vec<EncodedPacket>, OutputError> {
        let mut cursor = Cursor::new(bytes);
        let mut magic = [0_u8; 8];
        read_exact(&mut cursor, &mut magic)?;
        if &magic != PACKET_MAGIC {
            return Err(OutputError::InvalidHeader);
        }
        let packet_count = read_u64(&mut cursor)?;
        if packet_count > MAX_RECORDING_FRAMES as u64 {
            return Err(OutputError::TooManyFrames {
                frames: packet_count,
            });
        }

        let mut packets = Vec::with_capacity(usize::try_from(packet_count).unwrap_or(0));
        let mut encoded_bytes = PACKET_HEADER_BYTES;
        let mut last_timestamp = None;
        for _ in 0..packet_count {
            let kind = PacketKind::from_tag(read_u8(&mut cursor)?)?;
            let keyframe = match read_u8(&mut cursor)? {
                0 => false,
                1 => true,
                value => return Err(OutputError::InvalidPacketFlag { value }),
            };
            let timestamp = Timestamp::from_nanos(read_u64(&mut cursor)?);
            if let Some(previous) = last_timestamp {
                if timestamp < previous {
                    return Err(OutputError::NonMonotonicTimestamp {
                        previous,
                        actual: timestamp,
                    });
                }
            }
            last_timestamp = Some(timestamp);
            let payload_bytes = read_u64(&mut cursor)?;
            let payload_bytes = usize::try_from(payload_bytes)
                .map_err(|_| OutputError::PacketTooLarge { bytes: usize::MAX })?;
            if payload_bytes > MAX_PACKET_BYTES {
                return Err(OutputError::PacketTooLarge {
                    bytes: payload_bytes,
                });
            }
            let packet_bytes = 18_usize
                .checked_add(payload_bytes)
                .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
            encoded_bytes = encoded_bytes
                .checked_add(packet_bytes)
                .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
            if encoded_bytes > MAX_RECORDING_BYTES {
                return Err(OutputError::TooLarge {
                    bytes: encoded_bytes as u64,
                });
            }
            let mut payload = vec![0_u8; payload_bytes];
            read_exact(&mut cursor, &mut payload)?;
            packets.push(EncodedPacket::new(kind, timestamp, keyframe, payload)?);
        }
        if cursor.position() != u64::try_from(bytes.len()).unwrap_or(u64::MAX) {
            return Err(OutputError::InvalidCodecPayload(
                "packet container has trailing bytes".to_owned(),
            ));
        }
        Ok(packets)
    }
}

impl Default for MemoryMuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketMuxer for MemoryMuxer {
    fn push(&mut self, packet: EncodedPacket) -> Result<(), OutputError> {
        if self.state != OutputState::Open {
            return Err(OutputError::InvalidState {
                operation: "push a packet",
                state: self.state,
            });
        }
        if self.packets.len() >= MAX_RECORDING_FRAMES {
            return Err(OutputError::TooManyFrames {
                frames: self.packets.len() as u64 + 1,
            });
        }
        if let Some(previous) = self.last_timestamp {
            if packet.timestamp() < previous {
                return Err(OutputError::NonMonotonicTimestamp {
                    previous,
                    actual: packet.timestamp(),
                });
            }
        }
        let packet_bytes = 18_usize
            .checked_add(packet.byte_len())
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        let encoded_bytes = self
            .encoded_bytes
            .checked_add(packet_bytes)
            .ok_or(OutputError::TooLarge { bytes: u64::MAX })?;
        if encoded_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: encoded_bytes as u64,
            });
        }
        self.packets.push(packet);
        self.encoded_bytes = encoded_bytes;
        self.last_timestamp = self.packets.last().map(EncodedPacket::timestamp);
        Ok(())
    }

    fn finalize(&mut self) -> Result<Vec<u8>, OutputError> {
        if self.state != OutputState::Open {
            return Err(OutputError::InvalidState {
                operation: "finalize",
                state: self.state,
            });
        }
        let bytes = encode_packets(&self.packets)?;
        self.committed = Some(bytes.clone());
        self.state = OutputState::Finalized;
        Ok(bytes)
    }

    fn abort(&mut self) -> Result<(), OutputError> {
        if self.state != OutputState::Open {
            return Err(OutputError::InvalidState {
                operation: "abort",
                state: self.state,
            });
        }
        self.packets.clear();
        self.last_timestamp = None;
        self.state = OutputState::Aborted;
        Ok(())
    }

    fn state(&self) -> OutputState {
        self.state
    }
}

fn encode_packets(packets: &[EncodedPacket]) -> Result<Vec<u8>, OutputError> {
    let mut bytes = Vec::new();
    write_all(&mut bytes, PACKET_MAGIC)?;
    write_u64(&mut bytes, packets.len() as u64)?;
    for packet in packets {
        bytes.push(packet.kind.tag());
        bytes.push(u8::from(packet.keyframe));
        write_u64(&mut bytes, packet.timestamp.as_nanos())?;
        write_u64(&mut bytes, packet.payload.len() as u64)?;
        write_all(&mut bytes, &packet.payload)?;
    }
    Ok(bytes)
}

/// An owned sequence of raw RGBA frames with one fixed format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawRecording {
    format: VideoFormat,
    frames: Vec<VideoFrame>,
}

impl RawRecording {
    /// Creates an empty recording for one video format.
    #[must_use]
    pub fn new(format: VideoFormat) -> Self {
        Self {
            format,
            frames: Vec::new(),
        }
    }

    /// Adds a frame to the recording.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::FormatMismatch`] for another format or
    /// [`OutputError::TooManyFrames`] after the safety limit.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        if frame.format() != self.format {
            return Err(OutputError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }
        if self.frames.len() >= MAX_RECORDING_FRAMES {
            return Err(OutputError::TooManyFrames {
                frames: self.frames.len() as u64 + 1,
            });
        }
        let frame_bytes = 16_usize.saturating_add(self.format.rgba_bytes());
        let encoded_bytes = HEADER_BYTES.saturating_add(
            self.frames
                .len()
                .saturating_add(1)
                .saturating_mul(frame_bytes),
        );
        if encoded_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: encoded_bytes as u64,
            });
        }
        self.frames.push(frame);
        Ok(())
    }

    /// Returns the fixed recording format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the number of recorded frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Returns whether the recording has no frames.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Returns the recorded frames in timestamp order supplied by the caller.
    #[must_use]
    pub fn frames(&self) -> &[VideoFrame] {
        &self.frames
    }

    /// Encodes the recording into the deterministic raw reference format.
    ///
    /// The header stores the format and frame count. Each frame stores a nanosecond
    /// timestamp followed by its fixed-size RGBA payload.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::Write`] only if the in-memory writer fails.
    pub fn encode(&self) -> Result<Vec<u8>, OutputError> {
        let mut bytes = Vec::with_capacity(
            HEADER_BYTES.saturating_add(
                self.frames
                    .len()
                    .saturating_mul(16 + self.format.rgba_bytes()),
            ),
        );
        write_all(&mut bytes, MAGIC)?;
        write_u32(&mut bytes, self.format.width())?;
        write_u32(&mut bytes, self.format.height())?;
        write_u32(&mut bytes, self.format.frame_rate().numerator())?;
        write_u32(&mut bytes, self.format.frame_rate().denominator())?;
        write_u64(&mut bytes, self.frames.len() as u64)?;
        for frame in &self.frames {
            write_u64(&mut bytes, frame.timestamp().as_nanos())?;
            write_u64(&mut bytes, frame.pixels().len() as u64)?;
            write_all(&mut bytes, frame.pixels())?;
        }
        Ok(bytes)
    }

    /// Decodes a complete raw reference recording.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidHeader`], [`OutputError::Truncated`],
    /// [`OutputError::TooManyFrames`], or [`OutputError::Media`] when the stream is
    /// malformed.
    pub fn decode(bytes: &[u8]) -> Result<Self, OutputError> {
        let mut cursor = Cursor::new(bytes);
        let mut magic = [0_u8; 8];
        read_exact(&mut cursor, &mut magic)?;
        if &magic != MAGIC {
            return Err(OutputError::InvalidHeader);
        }

        let width = read_u32(&mut cursor)?;
        let height = read_u32(&mut cursor)?;
        let numerator = read_u32(&mut cursor)?;
        let denominator = read_u32(&mut cursor)?;
        let frame_count = read_u64(&mut cursor)?;
        if frame_count > MAX_RECORDING_FRAMES as u64 {
            return Err(OutputError::TooManyFrames {
                frames: frame_count,
            });
        }
        let rate = FrameRate::new(numerator, denominator).map_err(OutputError::Media)?;
        let format = VideoFormat::new(width, height, rate).map_err(OutputError::Media)?;
        let estimated_bytes = u128::from(frame_count)
            .saturating_mul(u128::from(format.rgba_bytes() as u64 + 16))
            .saturating_add(u128::from(HEADER_BYTES as u64));
        if estimated_bytes > u128::from(MAX_RECORDING_BYTES as u64) {
            return Err(OutputError::TooLarge {
                bytes: u64::try_from(estimated_bytes).unwrap_or(u64::MAX),
            });
        }
        let mut recording = Self::new(format);

        for _ in 0..frame_count {
            let timestamp = Timestamp::from_nanos(read_u64(&mut cursor)?);
            let payload_bytes = read_u64(&mut cursor)?;
            if payload_bytes != format.rgba_bytes() as u64 {
                return Err(OutputError::Media(MediaError::BufferSize {
                    expected: format.rgba_bytes(),
                    actual: usize::try_from(payload_bytes).unwrap_or(usize::MAX),
                }));
            }
            let mut pixels = vec![0_u8; format.rgba_bytes()];
            read_exact(&mut cursor, &mut pixels)?;
            let frame = VideoFrame::new(format, timestamp, pixels).map_err(OutputError::Media)?;
            recording.push(frame)?;
        }
        Ok(recording)
    }
}

/// A raw recording with explicit open/finalized/aborted lifecycle semantics.
pub struct RawRecordingSession {
    recording: RawRecording,
    state: OutputState,
    committed: Option<Vec<u8>>,
}

impl RawRecordingSession {
    /// Starts an empty open session for one video format.
    #[must_use]
    pub fn new(format: VideoFormat) -> Self {
        Self {
            recording: RawRecording::new(format),
            state: OutputState::Open,
            committed: None,
        }
    }

    /// Appends one frame while the session is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// recording validation error for the frame itself.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        self.ensure_open("push a frame")?;
        self.recording.push(frame)
    }

    /// Encodes and atomically commits the complete recording.
    ///
    /// The session changes to [`OutputState::Finalized`] only after encoding
    /// succeeds, so a failed encode cannot expose a partially committed result.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the session is not open.
    pub fn finalize(&mut self) -> Result<Vec<u8>, OutputError> {
        self.ensure_open("finalize")?;
        let bytes = self.recording.encode()?;
        self.committed = Some(bytes.clone());
        self.state = OutputState::Finalized;
        Ok(bytes)
    }

    /// Aborts the session and discards all uncommitted frames.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the session is not open.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        self.recording.frames.clear();
        self.state = OutputState::Aborted;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the in-progress or completed recording view.
    #[must_use]
    pub const fn recording(&self) -> &RawRecording {
        &self.recording
    }

    /// Returns committed bytes after finalization.
    #[must_use]
    pub fn committed_bytes(&self) -> Option<&[u8]> {
        self.committed.as_deref()
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), OutputError> {
        if self.state == OutputState::Open {
            Ok(())
        } else {
            Err(OutputError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}

/// A crash-safe raw recording writer using temp-file plus rename finalization.
pub struct AtomicRawFileWriter {
    recording: RawRecording,
    final_path: PathBuf,
    temp_path: PathBuf,
    state: OutputState,
    committed_bytes: Option<usize>,
}

impl AtomicRawFileWriter {
    /// Starts an open writer with explicit final and temporary paths.
    ///
    /// The temporary path must differ from the final path. The final path is not
    /// touched until [`Self::finalize`] has written and synchronized the temporary
    /// file successfully.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidPaths`] when both paths are equal or empty.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
        format: VideoFormat,
    ) -> Result<Self, OutputError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty() || temp_path.as_os_str().is_empty() {
            return Err(OutputError::InvalidPaths {
                reason: "paths must be non-empty".to_owned(),
            });
        }
        if final_path == temp_path {
            return Err(OutputError::InvalidPaths {
                reason: "temporary and final paths must differ".to_owned(),
            });
        }
        Ok(Self {
            recording: RawRecording::new(format),
            final_path,
            temp_path,
            state: OutputState::Open,
            committed_bytes: None,
        })
    }

    /// Appends one frame while the writer is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// recording validation error for the frame itself.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        self.ensure_open("push a frame")?;
        self.recording.push(frame)
    }

    /// Writes the temporary file, synchronizes it, then atomically renames it to
    /// the final path.
    ///
    /// On any filesystem failure the temporary file is removed on a best-effort
    /// basis and the final path is left untouched.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open,
    /// [`OutputError::Write`] for filesystem failures, or a recording encoding
    /// error before any path is changed.
    pub fn finalize(&mut self) -> Result<usize, OutputError> {
        self.ensure_open("finalize")?;
        let bytes = self.recording.encode()?;
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| OutputError::Write(format!("open temporary file: {error}")))?;
            file.write_all(&bytes)
                .map_err(|error| OutputError::Write(format!("write temporary file: {error}")))?;
            file.sync_all()
                .map_err(|error| OutputError::Write(format!("sync temporary file: {error}")))?;
            fs::rename(&self.temp_path, &self.final_path)
                .map_err(|error| OutputError::Write(format!("rename recording: {error}")))?;
            Ok::<(), OutputError>(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }

        self.committed_bytes = Some(bytes.len());
        self.state = OutputState::Finalized;
        Ok(bytes.len())
    }

    /// Aborts the writer and removes an uncommitted temporary file if present.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        if let Err(error) = fs::remove_file(&self.temp_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(OutputError::Write(format!(
                    "remove temporary file: {error}"
                )));
            }
        }
        self.recording.frames.clear();
        self.state = OutputState::Aborted;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the final path selected for this writer.
    #[must_use]
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Returns the temporary path selected for this writer.
    #[must_use]
    pub fn temp_path(&self) -> &std::path::Path {
        &self.temp_path
    }

    /// Returns the number of bytes committed by a successful finalization.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), OutputError> {
        if self.state == OutputState::Open {
            Ok(())
        } else {
            Err(OutputError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}

/// A crash-safe YUV4MPEG2 writer using temp-file plus rename finalization.
///
/// The writer keeps the standard Y4M stream in memory while frames are being
/// validated, then publishes it only after the complete file has been written
/// and synchronized. This keeps a cancelled recording from becoming a file
/// that looks complete to another process.
pub struct AtomicY4mFileWriter {
    recording: Y4mRecording,
    final_path: PathBuf,
    temp_path: PathBuf,
    state: OutputState,
    committed_bytes: Option<usize>,
}

impl AtomicY4mFileWriter {
    /// Starts an open Y4M writer with explicit final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidPaths`] when either path is empty or both
    /// paths are equal.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
        format: VideoFormat,
    ) -> Result<Self, OutputError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty() || temp_path.as_os_str().is_empty() {
            return Err(OutputError::InvalidPaths {
                reason: "paths must be non-empty".to_owned(),
            });
        }
        if final_path == temp_path {
            return Err(OutputError::InvalidPaths {
                reason: "temporary and final paths must differ".to_owned(),
            });
        }
        Ok(Self {
            recording: Y4mRecording::new(format),
            final_path,
            temp_path,
            state: OutputState::Open,
            committed_bytes: None,
        })
    }

    /// Appends one frame while the writer is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// Y4M validation error for the frame itself.
    pub fn push(&mut self, frame: VideoFrame) -> Result<(), OutputError> {
        self.ensure_open("push a frame")?;
        self.recording.push(frame)
    }

    /// Writes, synchronizes, and atomically renames the complete Y4M stream.
    ///
    /// On a filesystem failure the temporary path is removed on a best-effort
    /// basis and the final path is left untouched.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open,
    /// [`OutputError::Write`] for filesystem failures, or a Y4M encoding error
    /// before either path is changed.
    pub fn finalize(&mut self) -> Result<usize, OutputError> {
        self.ensure_open("finalize")?;
        let bytes = self.recording.encode()?;
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| OutputError::Write(format!("open temporary Y4M file: {error}")))?;
            file.write_all(&bytes).map_err(|error| {
                OutputError::Write(format!("write temporary Y4M file: {error}"))
            })?;
            file.sync_all()
                .map_err(|error| OutputError::Write(format!("sync temporary Y4M file: {error}")))?;
            fs::rename(&self.temp_path, &self.final_path)
                .map_err(|error| OutputError::Write(format!("rename Y4M recording: {error}")))?;
            Ok::<(), OutputError>(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }

        self.committed_bytes = Some(bytes.len());
        self.state = OutputState::Finalized;
        Ok(bytes.len())
    }

    /// Aborts the writer and removes an uncommitted temporary file if present.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open or
    /// [`OutputError::Write`] when the temporary file cannot be removed.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        if let Err(error) = fs::remove_file(&self.temp_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(OutputError::Write(format!(
                    "remove temporary Y4M file: {error}"
                )));
            }
        }
        self.recording.frames.clear();
        self.recording.encoded_bytes = 0;
        self.recording.last_timestamp = None;
        self.state = OutputState::Aborted;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the number of accepted frames.
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.recording.len()
    }

    /// Returns the final path selected for this writer.
    #[must_use]
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Returns the temporary path selected for this writer.
    #[must_use]
    pub fn temp_path(&self) -> &std::path::Path {
        &self.temp_path
    }

    /// Returns the number of bytes committed by a successful finalization.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), OutputError> {
        if self.state == OutputState::Open {
            Ok(())
        } else {
            Err(OutputError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}

/// A crash-safe packet-container writer using temp-file plus rename finalization.
///
/// The bytes use the deterministic `OBSRPKT1` packet container emitted by
/// [`MemoryMuxer`]. It is an inspectable Rust-native container fixture, not a
/// claim of compatibility with a broadcast container such as Matroska or MPEG-TS.
pub struct AtomicPacketFileWriter {
    muxer: MemoryMuxer,
    final_path: PathBuf,
    temp_path: PathBuf,
    state: OutputState,
    committed_bytes: Option<usize>,
}

impl AtomicPacketFileWriter {
    /// Starts an open writer with explicit final and temporary paths.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidPaths`] when either path is empty or both
    /// paths are equal.
    pub fn new(
        final_path: impl Into<PathBuf>,
        temp_path: impl Into<PathBuf>,
    ) -> Result<Self, OutputError> {
        let final_path = final_path.into();
        let temp_path = temp_path.into();
        if final_path.as_os_str().is_empty() || temp_path.as_os_str().is_empty() {
            return Err(OutputError::InvalidPaths {
                reason: "paths must be non-empty".to_owned(),
            });
        }
        if final_path == temp_path {
            return Err(OutputError::InvalidPaths {
                reason: "temporary and final paths must differ".to_owned(),
            });
        }
        Ok(Self {
            muxer: MemoryMuxer::new(),
            final_path,
            temp_path,
            state: OutputState::Open,
            committed_bytes: None,
        })
    }

    /// Appends one encoded packet while the writer is open.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] after finalization or abort, or a
    /// packet-container validation error.
    pub fn push(&mut self, packet: EncodedPacket) -> Result<(), OutputError> {
        self.ensure_open("push a packet")?;
        self.muxer.push(packet)
    }

    /// Writes, synchronizes, and atomically renames the packet container.
    ///
    /// On a filesystem failure the temporary path is removed on a best-effort
    /// basis and the final path is left untouched.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open,
    /// [`OutputError::Write`] for filesystem failures, or a container encoding
    /// error before any path is changed.
    pub fn finalize(&mut self) -> Result<usize, OutputError> {
        self.ensure_open("finalize")?;
        let bytes = encode_packets(self.muxer.packets())?;
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.temp_path)
                .map_err(|error| OutputError::Write(format!("open temporary file: {error}")))?;
            file.write_all(&bytes)
                .map_err(|error| OutputError::Write(format!("write temporary file: {error}")))?;
            file.sync_all()
                .map_err(|error| OutputError::Write(format!("sync temporary file: {error}")))?;
            fs::rename(&self.temp_path, &self.final_path)
                .map_err(|error| OutputError::Write(format!("rename packet container: {error}")))?;
            Ok::<(), OutputError>(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&self.temp_path);
            return Err(error);
        }

        self.committed_bytes = Some(bytes.len());
        self.state = OutputState::Finalized;
        Ok(bytes.len())
    }

    /// Aborts the writer and removes an uncommitted temporary file if present.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::InvalidState`] when the writer is not open or
    /// [`OutputError::Write`] when the temporary file cannot be removed.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.ensure_open("abort")?;
        if let Err(error) = fs::remove_file(&self.temp_path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(OutputError::Write(format!(
                    "remove temporary file: {error}"
                )));
            }
        }
        self.muxer.abort()?;
        self.state = OutputState::Aborted;
        Ok(())
    }

    /// Returns the current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> OutputState {
        self.state
    }

    /// Returns the number of packets accepted by the writer.
    #[must_use]
    pub fn packet_count(&self) -> usize {
        self.muxer.packets().len()
    }

    /// Returns the final path selected for this writer.
    #[must_use]
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Returns the temporary path selected for this writer.
    #[must_use]
    pub fn temp_path(&self) -> &std::path::Path {
        &self.temp_path
    }

    /// Returns the number of bytes committed by a successful finalization.
    #[must_use]
    pub const fn committed_bytes(&self) -> Option<usize> {
        self.committed_bytes
    }

    fn ensure_open(&self, operation: &'static str) -> Result<(), OutputError> {
        if self.state == OutputState::Open {
            Ok(())
        } else {
            Err(OutputError::InvalidState {
                operation,
                state: self.state,
            })
        }
    }
}

fn write_all(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), OutputError> {
    output
        .write_all(bytes)
        .map_err(|error| OutputError::Write(error.to_string()))
}

fn write_u32(output: &mut Vec<u8>, value: u32) -> Result<(), OutputError> {
    write_all(output, &value.to_le_bytes())
}

fn write_u16(output: &mut Vec<u8>, value: u16) -> Result<(), OutputError> {
    write_all(output, &value.to_le_bytes())
}

fn write_u64(output: &mut Vec<u8>, value: u64) -> Result<(), OutputError> {
    write_all(output, &value.to_le_bytes())
}

fn read_exact(input: &mut Cursor<&[u8]>, bytes: &mut [u8]) -> Result<(), OutputError> {
    input.read_exact(bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            OutputError::Truncated
        } else {
            OutputError::Write(error.to_string())
        }
    })
}

fn read_u32(input: &mut Cursor<&[u8]>) -> Result<u32, OutputError> {
    let mut bytes = [0_u8; 4];
    read_exact(input, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut Cursor<&[u8]>) -> Result<u64, OutputError> {
    let mut bytes = [0_u8; 8];
    read_exact(input, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_u8(input: &mut Cursor<&[u8]>) -> Result<u8, OutputError> {
    let mut byte = [0_u8; 1];
    read_exact(input, &mut byte)?;
    Ok(byte[0])
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pcm16_bits(sample: f32) -> u16 {
    let sample = sample.clamp(-1.0, 1.0);
    let value = if sample <= -1.0 {
        i32::from(i16::MIN)
    } else {
        (sample * f32::from(i16::MAX)).round() as i32
    };
    let value = i16::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i16::MIN
        } else {
            i16::MAX
        }
    });
    u16::from_le_bytes(value.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_paths(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::time::{SystemTime, UNIX_EPOCH};

        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir();
        (
            root.join(format!("obs-rs-{label}-{token}.obsrraw")),
            root.join(format!("obs-rs-{label}-{token}.part")),
        )
    }

    fn format() -> VideoFormat {
        VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format")
    }

    #[test]
    fn raw_recording_round_trips_frames_and_timestamps() {
        let format = format();
        let mut recording = RawRecording::new(format);
        recording
            .push(VideoFrame::solid(
                format,
                Timestamp::from_millis(10),
                [1, 2, 3, 255],
            ))
            .expect("first frame");
        recording
            .push(VideoFrame::solid(
                format,
                Timestamp::from_millis(20),
                [4, 5, 6, 255],
            ))
            .expect("second frame");

        let encoded = recording.encode().expect("encode succeeds");
        let decoded = RawRecording::decode(&encoded).expect("decode succeeds");

        assert_eq!(decoded, recording);
        assert_eq!(encoded.get(..8), Some(MAGIC.as_slice()));
    }

    #[test]
    fn decoder_rejects_bad_header_and_truncation() {
        let format = format();
        let mut recording = RawRecording::new(format);
        recording
            .push(VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 255]))
            .expect("frame");
        let encoded = recording.encode().expect("encode succeeds");

        assert_eq!(
            RawRecording::decode(b"not-a-recording"),
            Err(OutputError::InvalidHeader)
        );
        assert_eq!(
            RawRecording::decode(&encoded[..encoded.len() - 1]),
            Err(OutputError::Truncated)
        );
    }

    #[test]
    fn recorder_rejects_other_formats() {
        let format = format();
        let other = VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid format");
        let mut recording = RawRecording::new(format);

        assert!(matches!(
            recording.push(VideoFrame::solid(other, Timestamp::ZERO, [0, 0, 0, 255])),
            Err(OutputError::FormatMismatch { .. })
        ));
    }

    #[test]
    fn packet_queue_applies_byte_drop_policy() {
        let first = EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, vec![1, 2, 3])
            .expect("first packet");
        let second = EncodedPacket::new(
            PacketKind::Video,
            Timestamp::from_millis(1),
            false,
            vec![4, 5, 6],
        )
        .expect("second packet");
        let mut queue = PacketQueue::new(4, PacketDropPolicy::DropOldest).expect("queue");
        queue.push(first).expect("first push");
        assert_eq!(
            queue.push(second).expect("second push"),
            PacketPushOutcome::DroppedOldest {
                packets: 1,
                bytes: 3
            }
        );
        assert_eq!(queue.queued_bytes(), 3);
        assert_eq!(queue.pop().expect("remaining packet").payload(), &[4, 5, 6]);
    }

    #[test]
    fn raw_encoder_and_muxer_round_trip_packets() {
        let format = format();
        let frame = VideoFrame::solid(format, Timestamp::from_millis(12), [9, 8, 7, 255]);
        let mut encoder = RawVideoEncoder::new(format);
        let packet = encoder.encode(&frame).expect("encode frame");
        assert_eq!(packet.kind(), PacketKind::Video);
        assert!(packet.is_keyframe());
        assert_eq!(packet.payload(), frame.pixels());

        let mut muxer = MemoryMuxer::new();
        muxer.push(packet).expect("mux packet");
        let bytes = muxer.finalize().expect("finalize muxer");
        assert_eq!(muxer.state(), OutputState::Finalized);
        assert_eq!(
            MemoryMuxer::decode(&bytes).expect("decode packets").len(),
            1
        );
        assert!(matches!(
            muxer.push(
                EncodedPacket::new(PacketKind::Audio, Timestamp::ZERO, false, vec![1])
                    .expect("audio packet")
            ),
            Err(OutputError::InvalidState {
                operation: "push a packet",
                state: OutputState::Finalized
            })
        ));
    }

    #[test]
    fn png_encoder_emits_deterministic_crc_checked_rgba_chunks() {
        let format = format();
        let frame = VideoFrame::new(
            format,
            Timestamp::from_millis(7),
            vec![255, 0, 0, 255, 0, 255, 0, 255],
        )
        .expect("frame");
        let mut encoder = PngVideoEncoder::new(format);
        let packet = encoder.encode(&frame).expect("PNG packet");
        let payload = packet.payload();
        assert_eq!(packet.kind(), PacketKind::Video);
        assert!(packet.is_keyframe());
        assert_eq!(payload, encode_png(&frame).expect("PNG image"));
        assert_eq!(payload.get(..8), Some(PNG_SIGNATURE.as_slice()));

        let mut offset = PNG_SIGNATURE.len();
        let mut chunk_kinds = Vec::new();
        while offset < payload.len() {
            let length = usize::try_from(u32::from_be_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]))
            .expect("chunk length");
            offset += 4;
            let kind = [
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ];
            offset += 4;
            let data = &payload[offset..offset + length];
            offset += length;
            let crc = u32::from_be_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]);
            offset += 4;
            let mut crc_input = Vec::with_capacity(kind.len() + data.len());
            crc_input.extend_from_slice(&kind);
            crc_input.extend_from_slice(data);
            assert_eq!(crc, crc32(&crc_input));
            chunk_kinds.push(kind);
        }
        assert_eq!(chunk_kinds, vec![*b"IHDR", *b"IDAT", *b"IEND"]);
        assert_eq!(offset, payload.len());
    }

    #[test]
    fn muxer_rejects_backward_timestamps_and_container_trailing_bytes() {
        let mut muxer = MemoryMuxer::new();
        muxer
            .push(
                EncodedPacket::new(PacketKind::Video, Timestamp::from_millis(10), true, vec![1])
                    .expect("first packet"),
            )
            .expect("first push");
        assert_eq!(
            muxer.push(
                EncodedPacket::new(PacketKind::Audio, Timestamp::from_millis(9), false, vec![2])
                    .expect("backward packet")
            ),
            Err(OutputError::NonMonotonicTimestamp {
                previous: Timestamp::from_millis(10),
                actual: Timestamp::from_millis(9),
            })
        );

        let bytes = muxer.finalize().expect("finalize packet");
        let mut with_trailing = bytes.clone();
        with_trailing.push(0);
        assert!(matches!(
            MemoryMuxer::decode(&with_trailing),
            Err(OutputError::InvalidCodecPayload(reason)) if reason == "packet container has trailing bytes"
        ));
    }

    #[test]
    fn rle_video_codec_round_trips_and_rejects_bad_runs() {
        let format = VideoFormat::new(8, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
        let frame = VideoFrame::solid(format, Timestamp::from_millis(12), [9, 8, 7, 255]);
        let mut encoder = RleVideoEncoder::new(format);
        let packet = encoder.encode(&frame).expect("encode frame");
        assert!(packet.byte_len() < frame.pixels().len());
        assert_eq!(
            RleVideoDecoder::decode(format, packet.timestamp(), packet.payload())
                .expect("decode frame"),
            frame
        );

        let mut malformed = packet.payload().to_vec();
        malformed[8..12].copy_from_slice(&0_u32.to_le_bytes());
        assert!(matches!(
            RleVideoDecoder::decode(format, packet.timestamp(), &malformed),
            Err(OutputError::InvalidCodecPayload(_))
        ));
    }

    #[test]
    fn raw_audio_encoder_emits_audio_packets_and_checks_format() {
        let audio_format = AudioFormat::new(48_000, 2).expect("audio format");
        let other_format = AudioFormat::new(44_100, 2).expect("other format");
        let buffer = AudioBuffer::new(audio_format, Timestamp::from_millis(4), vec![0.5, -0.25])
            .expect("audio buffer");
        let mut encoder = RawAudioEncoder::new(audio_format);
        let packet = encoder.encode(&buffer).expect("encode audio");
        assert_eq!(packet.kind(), PacketKind::Audio);
        assert!(!packet.is_keyframe());
        assert_eq!(packet.timestamp(), Timestamp::from_millis(4));
        assert_eq!(packet.payload().len(), 8);
        assert_eq!(&packet.payload()[..4], &0.5_f32.to_le_bytes());

        let other = AudioBuffer::silence(other_format, Timestamp::ZERO, 1).expect("other buffer");
        assert!(matches!(
            encoder.encode(&other),
            Err(OutputError::AudioFormatMismatch {
                expected,
                actual
            }) if expected == audio_format && actual == other_format
        ));
    }

    #[test]
    fn wav_recording_emits_canonical_pcm16_headers_and_samples() {
        let audio_format = AudioFormat::new(48_000, 2).expect("audio format");
        let mut recording = WavRecording::new(audio_format);
        recording
            .push(
                AudioBuffer::new(audio_format, Timestamp::ZERO, vec![1.0, -1.0, 0.5, -0.5])
                    .expect("buffer"),
            )
            .expect("append");

        let bytes = recording.encode().expect("WAV encode");
        assert_eq!(recording.frames(), 2);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().expect("size")),
            8
        );
        assert_eq!(&bytes[44..46], &i16::MAX.to_le_bytes());
        assert_eq!(&bytes[46..48], &i16::MIN.to_le_bytes());
    }

    #[test]
    fn y4m_recording_emits_standard_header_and_420_planes() {
        let format = VideoFormat::new(4, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
        let frame = VideoFrame::solid(format, Timestamp::from_millis(10), [255, 0, 0, 255]);
        let mut recording = Y4mRecording::new(format);
        recording.push(frame).expect("append frame");
        let bytes = recording.encode().expect("Y4M encode");
        let header = b"YUV4MPEG2 W4 H2 F30:1 Ip A0:0 C420jpeg\n";
        assert_eq!(recording.len(), 1);
        assert_eq!(&bytes[..header.len()], header);
        assert_eq!(&bytes[header.len()..header.len() + 6], b"FRAME\n");
        assert_eq!(bytes.len(), header.len() + 6 + 8 + 2 + 2);
        assert_eq!(bytes[header.len() + 6], 77);
    }

    #[test]
    fn y4m_recording_rejects_odd_dimensions_and_backward_timestamps() {
        let odd_format =
            VideoFormat::new(3, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
        let mut odd_recording = Y4mRecording::new(odd_format);
        assert!(matches!(
            odd_recording.push(VideoFrame::solid(
                odd_format,
                Timestamp::ZERO,
                [0, 0, 0, 255]
            )),
            Err(OutputError::UnsupportedFormat { .. })
        ));

        let format = VideoFormat::new(4, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
        let mut recording = Y4mRecording::new(format);
        recording
            .push(VideoFrame::solid(
                format,
                Timestamp::from_millis(10),
                [0, 0, 0, 255],
            ))
            .expect("first frame");
        assert_eq!(
            recording.push(VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 255])),
            Err(OutputError::NonMonotonicTimestamp {
                previous: Timestamp::from_millis(10),
                actual: Timestamp::ZERO,
            })
        );
    }

    #[test]
    fn packet_decoder_rejects_oversized_declared_payload_before_allocation() {
        let mut bytes = Vec::from(PACKET_MAGIC.as_slice());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&(MAX_PACKET_BYTES as u64 + 1).to_le_bytes());

        assert_eq!(
            MemoryMuxer::decode(&bytes),
            Err(OutputError::PacketTooLarge {
                bytes: MAX_PACKET_BYTES + 1
            })
        );
    }

    #[test]
    fn stream_requeues_failed_packets_and_reconnects_without_loss() {
        let packet = EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, vec![1, 2, 3])
            .expect("packet");
        let mut transport = MemoryPacketTransport::new();
        transport.fail_next_send();
        let mut stream = StreamSession::new(
            transport,
            32,
            PacketDropPolicy::DropNewest,
            ReconnectPolicy::new(2),
        )
        .expect("stream");
        stream.connect().expect("connect stream");
        stream.submit(packet).expect("queue packet");

        assert!(matches!(
            stream.flush(),
            Err(OutputError::Transport(reason)) if reason == "injected send failure"
        ));
        assert_eq!(stream.state(), StreamState::Disconnected);
        assert_eq!(stream.queued_bytes(), 3);
        stream.reconnect().expect("reconnect stream");
        assert_eq!(stream.flush().expect("retry packet"), 1);
        assert_eq!(stream.state(), StreamState::Connected);
        assert_eq!(stream.transport().sent().len(), 1);
        assert_eq!(stream.metrics().send_failures(), 1);
        assert_eq!(stream.metrics().sent_packets(), 1);
        assert_eq!(stream.metrics().reconnects(), 1);
        assert_eq!(stream.queued_bytes(), 0);
    }

    #[test]
    fn tcp_transport_rejects_send_when_disconnected() {
        let mut transport = TcpPacketTransport::new("127.0.0.1:1");
        assert_eq!(transport.address(), "127.0.0.1:1");
        let packet = EncodedPacket::new(
            PacketKind::Audio,
            Timestamp::from_millis(3),
            false,
            vec![4, 5, 6],
        )
        .expect("packet");
        assert_eq!(
            transport.send(&packet),
            Err(OutputError::Transport(
                "TCP transport is disconnected".to_owned()
            ))
        );
        assert!(!transport.is_connected());
    }

    #[test]
    fn recording_session_commits_or_aborts_as_one_lifecycle() {
        let format = format();
        let frame = VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255]);
        let mut session = RawRecordingSession::new(format);
        session.push(frame.clone()).expect("push frame");
        let bytes = session.finalize().expect("finalize recording");
        assert_eq!(session.state(), OutputState::Finalized);
        assert_eq!(session.committed_bytes(), Some(bytes.as_slice()));
        assert!(matches!(
            session.push(frame),
            Err(OutputError::InvalidState {
                operation: "push a frame",
                state: OutputState::Finalized
            })
        ));

        let mut aborted = RawRecordingSession::new(format);
        aborted.abort().expect("abort recording");
        assert_eq!(aborted.state(), OutputState::Aborted);
        assert!(aborted.recording().is_empty());
        assert!(aborted.committed_bytes().is_none());
    }

    #[test]
    fn atomic_packet_writer_round_trips_interleaved_packets() {
        let (final_path, temp_path) = unique_paths("packets");
        let mut writer =
            AtomicPacketFileWriter::new(&final_path, &temp_path).expect("valid packet paths");
        writer
            .push(
                EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, vec![1, 2, 3])
                    .expect("video packet"),
            )
            .expect("push video");
        writer
            .push(
                EncodedPacket::new(
                    PacketKind::Audio,
                    Timestamp::from_millis(1),
                    false,
                    vec![4, 5],
                )
                .expect("audio packet"),
            )
            .expect("push audio");

        let byte_count = writer.finalize().expect("finalize packet file");
        assert_eq!(writer.state(), OutputState::Finalized);
        assert_eq!(writer.packet_count(), 2);
        assert_eq!(writer.committed_bytes(), Some(byte_count));
        assert!(!temp_path.exists());
        let bytes = std::fs::read(&final_path).expect("read packet file");
        let packets = MemoryMuxer::decode(&bytes).expect("decode packet file");
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].kind(), PacketKind::Video);
        assert_eq!(packets[1].kind(), PacketKind::Audio);
        std::fs::remove_file(final_path).expect("remove packet fixture");
    }

    #[test]
    fn atomic_packet_writer_abort_removes_temp_and_rejects_equal_paths() {
        let (final_path, temp_path) = unique_paths("packet-abort");
        assert!(matches!(
            AtomicPacketFileWriter::new(&final_path, &final_path),
            Err(OutputError::InvalidPaths { .. })
        ));
        let mut writer =
            AtomicPacketFileWriter::new(&final_path, &temp_path).expect("valid packet paths");
        writer.abort().expect("abort packet writer");
        assert_eq!(writer.state(), OutputState::Aborted);
        assert!(!final_path.exists());
        assert!(!temp_path.exists());
        assert!(matches!(
            writer.push(
                EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, vec![1])
                    .expect("packet")
            ),
            Err(OutputError::InvalidState {
                operation: "push a packet",
                state: OutputState::Aborted
            })
        ));
    }

    #[test]
    fn atomic_file_writer_renames_only_after_successful_sync() {
        let format = format();
        let (final_path, temp_path) = unique_paths("finalize");
        let mut writer =
            AtomicRawFileWriter::new(&final_path, &temp_path, format).expect("valid writer paths");
        writer
            .push(VideoFrame::solid(format, Timestamp::ZERO, [7, 8, 9, 255]))
            .expect("push frame");

        let byte_count = writer.finalize().expect("finalize file");
        assert_eq!(writer.state(), OutputState::Finalized);
        assert_eq!(writer.committed_bytes(), Some(byte_count));
        assert!(!temp_path.exists());
        assert_eq!(
            RawRecording::decode(&std::fs::read(&final_path).expect("read final file"))
                .expect("decode final file")
                .len(),
            1
        );
        assert!(matches!(
            writer.finalize(),
            Err(OutputError::InvalidState {
                operation: "finalize",
                state: OutputState::Finalized
            })
        ));
        std::fs::remove_file(final_path).expect("remove final fixture");
    }

    #[test]
    fn atomic_file_writer_abort_removes_temp_and_rejects_equal_paths() {
        let format = format();
        let (final_path, temp_path) = unique_paths("abort");
        assert!(matches!(
            AtomicRawFileWriter::new(&final_path, &final_path, format),
            Err(OutputError::InvalidPaths { .. })
        ));

        let mut writer =
            AtomicRawFileWriter::new(&final_path, &temp_path, format).expect("valid writer paths");
        writer.abort().expect("abort file writer");
        assert_eq!(writer.state(), OutputState::Aborted);
        assert!(!final_path.exists());
        assert!(!temp_path.exists());
        assert!(matches!(
            writer.push(VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 255])),
            Err(OutputError::InvalidState {
                operation: "push a frame",
                state: OutputState::Aborted
            })
        ));
    }

    #[test]
    fn atomic_y4m_writer_publishes_only_a_complete_standard_stream() {
        let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid Y4M format");
        let (final_path, temp_path) = unique_paths("y4m-finalize");
        let mut writer = AtomicY4mFileWriter::new(&final_path, &temp_path, format)
            .expect("valid Y4M writer paths");
        writer
            .push(VideoFrame::solid(format, Timestamp::ZERO, [255, 0, 0, 255]))
            .expect("push Y4M frame");

        let byte_count = writer.finalize().expect("finalize Y4M file");
        assert_eq!(writer.state(), OutputState::Finalized);
        assert_eq!(writer.frame_count(), 1);
        assert_eq!(writer.committed_bytes(), Some(byte_count));
        assert!(!temp_path.exists());
        let bytes = std::fs::read(&final_path).expect("read Y4M file");
        assert_eq!(bytes.len(), byte_count);
        assert!(bytes.starts_with(b"YUV4MPEG2 W2 H2 F30:1 Ip A0:0 C420jpeg\n"));
        assert!(bytes.windows(6).any(|window| window == b"FRAME\n"));
        std::fs::remove_file(final_path).expect("remove Y4M fixture");
    }

    #[test]
    fn atomic_y4m_writer_abort_removes_temp_and_rejects_equal_paths() {
        let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid Y4M format");
        let (final_path, temp_path) = unique_paths("y4m-abort");
        assert!(matches!(
            AtomicY4mFileWriter::new(&final_path, &final_path, format),
            Err(OutputError::InvalidPaths { .. })
        ));

        let mut writer =
            AtomicY4mFileWriter::new(&final_path, &temp_path, format).expect("valid paths");
        writer.abort().expect("abort Y4M writer");
        assert_eq!(writer.state(), OutputState::Aborted);
        assert_eq!(writer.frame_count(), 0);
        assert!(!final_path.exists());
        assert!(!temp_path.exists());
        assert!(matches!(
            writer.finalize(),
            Err(OutputError::InvalidState {
                operation: "finalize",
                state: OutputState::Aborted
            })
        ));
    }
}
