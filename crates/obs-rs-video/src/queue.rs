use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};
use std::collections::VecDeque;

use super::{
    error::VideoError,
    types::{DropPolicy, PushOutcome},
};
/// A bounded single-consumer frame queue.
pub struct FrameQueue {
    format: VideoFormat,
    capacity: usize,
    policy: DropPolicy,
    frames: VecDeque<VideoFrame>,
}

impl FrameQueue {
    /// Creates a queue with a fixed format, capacity, and drop policy.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::ZeroCapacity`] when `capacity` is zero.
    pub fn new(
        format: VideoFormat,
        capacity: usize,
        policy: DropPolicy,
    ) -> Result<Self, VideoError> {
        if capacity == 0 {
            return Err(VideoError::ZeroCapacity);
        }

        Ok(Self {
            format,
            capacity,
            policy,
            frames: VecDeque::with_capacity(capacity),
        })
    }

    /// Submits one frame, applying the configured bounded-drop policy.
    ///
    /// # Errors
    ///
    /// Returns [`VideoError::FormatMismatch`] when the frame format differs from
    /// the queue format. The queue is unchanged in that case.
    pub fn push(&mut self, frame: VideoFrame) -> Result<PushOutcome, VideoError> {
        if frame.format() != self.format {
            return Err(VideoError::FormatMismatch {
                expected: self.format,
                actual: frame.format(),
            });
        }

        if self.frames.len() < self.capacity {
            self.frames.push_back(frame);
            return Ok(PushOutcome::Enqueued);
        }

        match self.policy {
            DropPolicy::DropOldest => {
                let dropped = self
                    .frames
                    .pop_front()
                    .map_or(Timestamp::ZERO, |frame| frame.timestamp());
                self.frames.push_back(frame);
                Ok(PushOutcome::DroppedOldest(dropped))
            }
            DropPolicy::DropNewest => Ok(PushOutcome::DroppedNewest(frame.timestamp())),
        }
    }

    /// Removes and returns the oldest queued frame.
    pub fn pop(&mut self) -> Option<VideoFrame> {
        self.frames.pop_front()
    }

    /// Returns the oldest queued frame without removing it.
    #[must_use]
    pub fn front(&self) -> Option<&VideoFrame> {
        self.frames.front()
    }

    /// Removes all queued frames.
    pub fn clear(&mut self) {
        self.frames.clear();
    }

    /// Returns the queue's configured format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the fixed queue capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the current number of queued frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Returns whether no frame is currently queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}
