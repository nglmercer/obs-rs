use obs_rs_media::Timestamp;
use std::sync::Arc;

use super::{error::OutputError, MAX_PACKET_BYTES};

/// Video representation consumed by an output session.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VideoInputRequirement {
    /// The output performs its own video encoding from raw frames.
    Raw,
    /// The output consumes packets produced by an OBS-RS video encoder.
    Packetized,
}

/// Audio representation consumed by an output session.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AudioInputRequirement {
    /// The output performs its own audio encoding from raw buffers.
    Raw,
    /// The output consumes packets produced by an OBS-RS audio encoder.
    Packetized,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PacketKind {
    /// Encoded video data.
    Video,
    /// Encoded audio data.
    Audio,
}

impl PacketKind {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Video => 0,
            Self::Audio => 1,
        }
    }

    pub(crate) fn from_tag(tag: u8) -> Result<Self, OutputError> {
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
    pub(crate) kind: PacketKind,
    pub(crate) timestamp: Timestamp,
    pub(crate) keyframe: bool,
    pub(crate) payload: Arc<Vec<u8>>,
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
            payload: Arc::new(payload),
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
    pub(crate) max_attempts: u32,
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
    pub(crate) submitted: u64,
    pub(crate) dropped_packets: u64,
    pub(crate) sent_packets: u64,
    pub(crate) send_failures: u64,
    pub(crate) reconnects: u64,
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
