use std::io::Cursor;
use std::sync::Arc;

use obs_rs_media::Timestamp;

use super::{
    codec::{read_exact, read_u64, read_u8, read_vec, write_all, write_u64},
    error::OutputError,
    stream::PacketMuxer,
    types::{EncodedPacket, OutputState, PacketKind},
    MAX_PACKET_BYTES, MAX_RECORDING_BYTES, MAX_RECORDING_FRAMES, PACKET_HEADER_BYTES, PACKET_MAGIC,
};

pub struct MemoryMuxer {
    packets: Vec<EncodedPacket>,
    state: OutputState,
    /// Committed bytes, shared rather than duplicated.
    ///
    /// `finalize` used to keep one copy and hand the caller another, doubling
    /// peak memory at exactly the moment the recording is largest.
    committed: Option<Arc<Vec<u8>>>,
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
        self.committed.as_ref().map(|bytes| bytes.as_slice())
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
            let payload = read_vec(&mut cursor, payload_bytes)?;
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

    fn finalize(&mut self) -> Result<Arc<Vec<u8>>, OutputError> {
        if self.state != OutputState::Open {
            return Err(OutputError::InvalidState {
                operation: "finalize",
                state: self.state,
            });
        }
        let bytes = encode_packets(&self.packets)?;
        let bytes = Arc::new(bytes);
        self.committed = Some(Arc::clone(&bytes));
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

pub(crate) fn encode_packets(packets: &[EncodedPacket]) -> Result<Vec<u8>, OutputError> {
    // Exact reservation from the known record layout: a magic, a count, then a
    // 18-byte header plus payload per packet. A large recording would otherwise
    // reallocate and recopy its way up to the final size.
    let capacity = packets
        .iter()
        .fold(PACKET_MAGIC.len().saturating_add(8), |total, packet| {
            total
                .saturating_add(18)
                .saturating_add(packet.payload.len())
        });
    let mut bytes = Vec::with_capacity(capacity);
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
