use std::collections::VecDeque;

use super::{
    error::OutputError,
    types::{EncodedPacket, PacketDropPolicy, PacketPushOutcome},
    MAX_RECORDING_BYTES,
};

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
