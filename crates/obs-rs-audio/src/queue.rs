use super::{buffer::AudioBuffer, error::AudioError, types::AudioFormat};
use std::collections::VecDeque;
/// Drop behavior for a bounded audio queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioDropPolicy {
    /// Remove complete oldest buffers until the new buffer fits.
    DropOldest,
    /// Keep queued buffers and discard the new buffer.
    DropNewest,
}

/// Result of pushing a buffer into an [`AudioQueue`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioPushOutcome {
    /// The buffer was queued without dropping data.
    Enqueued,
    /// This many old frames were removed to make room.
    DroppedOldest { frames: usize },
    /// The submitted buffer was discarded.
    DroppedNewest { frames: usize },
}
/// A bounded queue of complete audio buffers.
pub struct AudioQueue {
    format: AudioFormat,
    capacity_frames: usize,
    policy: AudioDropPolicy,
    queued_frames: usize,
    buffers: VecDeque<AudioBuffer>,
}

impl AudioQueue {
    /// Creates a queue with a maximum number of buffered frames.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::ZeroCapacity`] for a zero capacity.
    pub fn new(
        format: AudioFormat,
        capacity_frames: usize,
        policy: AudioDropPolicy,
    ) -> Result<Self, AudioError> {
        if capacity_frames == 0 {
            return Err(AudioError::ZeroCapacity);
        }
        Ok(Self {
            format,
            capacity_frames,
            policy,
            queued_frames: 0,
            buffers: VecDeque::new(),
        })
    }

    /// Pushes a complete buffer under the queue's drop policy.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::FormatMismatch`] for a different format or
    /// [`AudioError::BufferTooLarge`] when one buffer cannot fit in the queue.
    pub fn push(&mut self, buffer: AudioBuffer) -> Result<AudioPushOutcome, AudioError> {
        if buffer.format() != self.format {
            return Err(AudioError::FormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            });
        }
        let incoming_frames = buffer.frames();
        if incoming_frames > self.capacity_frames {
            return Err(AudioError::BufferTooLarge {
                frames: incoming_frames,
            });
        }

        let free_frames = self.capacity_frames.saturating_sub(self.queued_frames);
        if incoming_frames <= free_frames {
            self.queued_frames += incoming_frames;
            self.buffers.push_back(buffer);
            return Ok(AudioPushOutcome::Enqueued);
        }

        match self.policy {
            AudioDropPolicy::DropNewest => Ok(AudioPushOutcome::DroppedNewest {
                frames: incoming_frames,
            }),
            AudioDropPolicy::DropOldest => {
                let mut dropped_frames = 0;
                while incoming_frames > self.capacity_frames.saturating_sub(self.queued_frames) {
                    let Some(dropped) = self.buffers.pop_front() else {
                        break;
                    };
                    let frames = dropped.frames();
                    self.queued_frames -= frames;
                    dropped_frames += frames;
                }
                self.queued_frames += incoming_frames;
                self.buffers.push_back(buffer);
                Ok(AudioPushOutcome::DroppedOldest {
                    frames: dropped_frames,
                })
            }
        }
    }

    /// Removes and returns the oldest queued buffer.
    pub fn pop(&mut self) -> Option<AudioBuffer> {
        let buffer = self.buffers.pop_front()?;
        self.queued_frames -= buffer.frames();
        Some(buffer)
    }

    /// Returns the number of queued frames.
    #[must_use]
    pub const fn queued_frames(&self) -> usize {
        self.queued_frames
    }

    /// Returns the maximum number of queued frames.
    #[must_use]
    pub const fn capacity_frames(&self) -> usize {
        self.capacity_frames
    }

    /// Returns whether the queue has no buffers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }

    /// Removes every queued buffer without changing the queue configuration.
    pub fn clear(&mut self) {
        self.buffers.clear();
        self.queued_frames = 0;
    }
}
