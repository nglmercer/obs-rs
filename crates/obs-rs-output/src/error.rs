use std::fmt;

use obs_rs_audio::AudioFormat;
use obs_rs_media::{MediaError, Timestamp, VideoFormat};

use super::types::{OutputState, StreamState};
use super::OutputProfileKind;

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
    /// A serialized or configured output profile uses an unsupported version.
    UnsupportedProfileVersion { version: u16 },
    /// The approved native runtime cannot provide an exact requested profile.
    ProfileUnavailable { profile: OutputProfileKind },
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
    /// A packet payload exceeds [`crate::MAX_PACKET_BYTES`].
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
    /// A replay buffer duration is zero or exceeds the bounded limit.
    InvalidReplayDuration { nanos: u128 },
    /// A replay snapshot has no retained video keyframe from which decoding can start.
    NoKeyframe,
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
    /// A split-recording policy is outside the bounded safety contract.
    InvalidSegmentPolicy { reason: String },
    /// A split recording reached its configured segment count.
    TooManySegments { segments: usize },
    /// One packet cannot fit in the configured segment target.
    SegmentPacketDoesNotFit {
        /// Serialized packet size including its container overhead.
        packet_bytes: usize,
        /// Maximum configured segment size.
        max_bytes: usize,
    },
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
            Self::UnsupportedProfileVersion { version } => {
                write!(formatter, "output profile version {version} is unsupported")
            }
            Self::ProfileUnavailable { profile } => {
                write!(formatter, "output profile {profile:?} is unavailable")
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
            Self::InvalidReplayDuration { nanos } => write!(
                formatter,
                "replay buffer duration must be between 1 nanosecond and the bounded limit: {nanos} ns"
            ),
            Self::NoKeyframe => {
                formatter.write_str("replay buffer has no retained video keyframe")
            }
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
            Self::InvalidSegmentPolicy { reason } => {
                write!(formatter, "invalid split-recording policy: {reason}")
            }
            Self::TooManySegments { segments } => {
                write!(formatter, "split recording reached its {segments}-segment limit")
            }
            Self::SegmentPacketDoesNotFit {
                packet_bytes,
                max_bytes,
            } => write!(
                formatter,
                "packet of {packet_bytes} bytes cannot fit in {max_bytes}-byte segment"
            ),
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
