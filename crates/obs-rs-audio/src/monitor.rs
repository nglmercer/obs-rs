use super::{buffer::AudioBuffer, error::AudioError};
use std::{collections::VecDeque, sync::Arc};
/// A bounded queue of shared post-mix buffers for monitoring or diagnostics.
///
/// Buffers are retained behind an [`Arc`], so observing a mix costs a
/// reference-count bump rather than a full copy of the sample payload.
pub struct AudioMonitorTap {
    capacity_buffers: usize,
    dropped_buffers: u64,
    buffers: VecDeque<Arc<AudioBuffer>>,
}

impl AudioMonitorTap {
    /// Creates a tap that retains at most `capacity_buffers` complete buffers.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroMonitorCapacity`] for a zero capacity.
    pub fn new(capacity_buffers: usize) -> Result<Self, AudioError> {
        if capacity_buffers == 0 {
            return Err(AudioError::ZeroMonitorCapacity);
        }
        Ok(Self {
            capacity_buffers,
            dropped_buffers: 0,
            buffers: VecDeque::with_capacity(capacity_buffers),
        })
    }

    pub(super) fn observe(&mut self, buffer: &Arc<AudioBuffer>) {
        if self.buffers.len() == self.capacity_buffers {
            let _ = self.buffers.pop_front();
            self.dropped_buffers = self.dropped_buffers.saturating_add(1);
        }
        self.buffers.push_back(Arc::clone(buffer));
    }

    /// Removes and returns the oldest monitored buffer.
    pub fn pop(&mut self) -> Option<Arc<AudioBuffer>> {
        self.buffers.pop_front()
    }

    /// Returns the number of retained monitored buffers.
    #[must_use]
    pub fn queued_buffers(&self) -> usize {
        self.buffers.len()
    }

    /// Returns the number of buffers discarded due to tap pressure.
    #[must_use]
    pub const fn dropped_buffers(&self) -> u64 {
        self.dropped_buffers
    }
}
