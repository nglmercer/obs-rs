use std::{collections::VecDeque, time::Duration};

use obs_rs_media::Timestamp;

use super::{error::OutputError, types::EncodedPacket, MAX_RECORDING_BYTES, MAX_RECORDING_FRAMES};

/// Maximum replay history retained by the portable packet buffer.
pub const MAX_REPLAY_DURATION: Duration = Duration::from_hours(1);

/// Result of accepting a packet into a replay buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayPushOutcome {
    /// The packet was accepted without evicting history.
    Enqueued,
    /// Older history was evicted to enforce time, byte, or packet bounds.
    DroppedOldest { packets: usize, bytes: usize },
}

/// A bounded, packetized replay history.
///
/// The buffer owns encoded packet references rather than raw frame data. It is
/// therefore only useful when the engine's selected encoder produces packets,
/// and it does not claim to replace a production muxer or codec. Payload clones
/// share their `Arc` storage with the encoder and output fan-out.
pub struct ReplayBuffer {
    capacity_bytes: usize,
    duration_nanos: u64,
    queued_bytes: usize,
    packets: VecDeque<EncodedPacket>,
    last_timestamp: Option<Timestamp>,
}

impl ReplayBuffer {
    /// Creates an empty replay buffer with byte and wall-clock bounds.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::ZeroCapacity`] for a zero byte budget,
    /// [`OutputError::TooLarge`] for a budget above the reference output cap,
    /// or [`OutputError::InvalidReplayDuration`] outside the bounded duration.
    pub fn new(capacity_bytes: usize, duration: Duration) -> Result<Self, OutputError> {
        if capacity_bytes == 0 {
            return Err(OutputError::ZeroCapacity);
        }
        if capacity_bytes > MAX_RECORDING_BYTES {
            return Err(OutputError::TooLarge {
                bytes: capacity_bytes as u64,
            });
        }
        if duration.is_zero() || duration > MAX_REPLAY_DURATION {
            return Err(OutputError::InvalidReplayDuration {
                nanos: duration.as_nanos(),
            });
        }
        let duration_nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        Ok(Self {
            capacity_bytes,
            duration_nanos,
            queued_bytes: 0,
            packets: VecDeque::new(),
            last_timestamp: None,
        })
    }

    /// Accepts one encoded packet and evicts only the oldest history.
    ///
    /// Timestamps must be non-decreasing across both audio and video packets;
    /// rejecting a backward timestamp keeps saved replay files valid for the
    /// existing packet container.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::PacketDoesNotFit`] when the packet cannot fit the
    /// byte budget, or [`OutputError::NonMonotonicTimestamp`] when its timestamp
    /// moves backward.
    pub fn push(&mut self, packet: EncodedPacket) -> Result<ReplayPushOutcome, OutputError> {
        let incoming_bytes = packet.byte_len();
        if incoming_bytes > self.capacity_bytes {
            return Err(OutputError::PacketDoesNotFit {
                packet_bytes: incoming_bytes,
                capacity_bytes: self.capacity_bytes,
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

        let mut dropped_packets = 0_usize;
        let mut dropped_bytes = 0_usize;
        while self
            .packets
            .front()
            .is_some_and(|oldest| self.outside_time_window(oldest.timestamp(), packet.timestamp()))
        {
            self.drop_oldest(&mut dropped_packets, &mut dropped_bytes);
        }
        while self.packets.len() >= MAX_RECORDING_FRAMES
            || incoming_bytes > self.capacity_bytes.saturating_sub(self.queued_bytes)
        {
            if self.packets.is_empty() {
                return Err(OutputError::PacketDoesNotFit {
                    packet_bytes: incoming_bytes,
                    capacity_bytes: self.capacity_bytes,
                });
            }
            self.drop_oldest(&mut dropped_packets, &mut dropped_bytes);
        }
        self.queued_bytes = self.queued_bytes.saturating_add(incoming_bytes);
        self.packets.push_back(packet);
        self.last_timestamp = self.packets.back().map(EncodedPacket::timestamp);
        if dropped_packets == 0 {
            Ok(ReplayPushOutcome::Enqueued)
        } else {
            Ok(ReplayPushOutcome::DroppedOldest {
                packets: dropped_packets,
                bytes: dropped_bytes,
            })
        }
    }

    /// Returns the retained packet history in timestamp order.
    pub fn packets(&self) -> impl Iterator<Item = &EncodedPacket> {
        self.packets.iter()
    }

    /// Clones packet handles for a control-plane save operation.
    #[must_use]
    pub fn snapshot(&self) -> Vec<EncodedPacket> {
        self.packets.iter().cloned().collect()
    }

    /// Removes all retained history.
    pub fn clear(&mut self) {
        self.packets.clear();
        self.queued_bytes = 0;
        self.last_timestamp = None;
    }

    /// Returns the byte budget.
    #[must_use]
    pub const fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }

    /// Returns the configured history duration.
    #[must_use]
    pub const fn duration(&self) -> Duration {
        Duration::from_nanos(self.duration_nanos)
    }

    /// Returns the retained byte count.
    #[must_use]
    pub const fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    /// Returns the retained packet count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    /// Returns whether no packet history is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    fn outside_time_window(&self, oldest: Timestamp, newest: Timestamp) -> bool {
        newest.as_nanos().saturating_sub(oldest.as_nanos()) > self.duration_nanos
    }

    fn drop_oldest(&mut self, dropped_packets: &mut usize, dropped_bytes: &mut usize) {
        if let Some(packet) = self.packets.pop_front() {
            let bytes = packet.byte_len();
            self.queued_bytes = self.queued_bytes.saturating_sub(bytes);
            *dropped_packets = dropped_packets.saturating_add(1);
            *dropped_bytes = dropped_bytes.saturating_add(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{hint::black_box, time::Instant};

    use super::*;
    use crate::types::PacketKind;

    fn packet(timestamp_ms: u64, payload_bytes: usize) -> EncodedPacket {
        EncodedPacket::new(
            PacketKind::Video,
            Timestamp::from_millis(timestamp_ms),
            true,
            vec![0xA5; payload_bytes],
        )
        .expect("valid packet")
    }

    #[test]
    fn replay_buffer_evicts_by_time_and_bytes() {
        let mut buffer = ReplayBuffer::new(10, Duration::from_millis(100)).expect("buffer");
        assert_eq!(buffer.push(packet(0, 4)), Ok(ReplayPushOutcome::Enqueued));
        assert_eq!(buffer.push(packet(50, 4)), Ok(ReplayPushOutcome::Enqueued));
        assert!(matches!(
            buffer.push(packet(120, 4)),
            Ok(ReplayPushOutcome::DroppedOldest { packets: 1, .. })
        ));
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.queued_bytes(), 8);
        assert_eq!(
            buffer.packets().next().map(EncodedPacket::timestamp),
            Some(Timestamp::from_millis(50))
        );

        assert!(matches!(
            buffer.push(packet(130, 8)),
            Ok(ReplayPushOutcome::DroppedOldest { packets: 2, .. })
        ));
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.queued_bytes(), 8);
    }

    #[test]
    fn replay_buffer_rejects_unbounded_settings_and_backward_packets() {
        assert!(matches!(
            ReplayBuffer::new(0, Duration::from_secs(1)),
            Err(OutputError::ZeroCapacity)
        ));
        assert!(matches!(
            ReplayBuffer::new(1, Duration::ZERO),
            Err(OutputError::InvalidReplayDuration { .. })
        ));
        assert!(matches!(
            ReplayBuffer::new(1, MAX_REPLAY_DURATION + Duration::from_nanos(1)),
            Err(OutputError::InvalidReplayDuration { .. })
        ));

        let mut buffer = ReplayBuffer::new(8, Duration::from_secs(1)).expect("buffer");
        assert!(matches!(
            buffer.push(packet(0, 9)),
            Err(OutputError::PacketDoesNotFit { .. })
        ));
        buffer.push(packet(20, 4)).expect("first packet");
        assert!(matches!(
            buffer.push(packet(10, 4)),
            Err(OutputError::NonMonotonicTimestamp { .. })
        ));
    }

    #[test]
    fn replay_snapshot_shares_packet_payloads() {
        let mut buffer = ReplayBuffer::new(64, Duration::from_secs(1)).expect("buffer");
        let original = packet(0, 4);
        let original_payload = original.payload().as_ptr();
        buffer.push(original).expect("packet");
        let snapshot = buffer.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].payload().as_ptr(), original_payload);
    }

    #[test]
    fn replay_buffer_push_timing_report() {
        let mut buffer = ReplayBuffer::new(1024 * 1024, Duration::from_mins(1)).expect("buffer");
        let started = Instant::now();
        for index in 0..1_000_u64 {
            buffer.push(packet(index, 64)).expect("bounded packet");
        }
        let elapsed = started.elapsed();
        black_box(buffer.queued_bytes());
        assert_eq!(buffer.len(), 1_000);
        println!(
            "replay buffer: 1000 packet pushes = {:?} ({:?}/push)",
            elapsed,
            elapsed / 1_000
        );
    }
}
