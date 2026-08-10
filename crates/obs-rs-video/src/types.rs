use obs_rs_media::Timestamp;

/// Policy used when a bounded queue has no free slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropPolicy {
    /// Remove the oldest queued frame and keep the new frame.
    DropOldest,
    /// Keep queued frames and discard the newly submitted frame.
    DropNewest,
}

/// Result of submitting a frame to a bounded queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushOutcome {
    /// The frame was stored without dropping another frame.
    Enqueued,
    /// The oldest frame was removed to store the submitted frame.
    DroppedOldest(Timestamp),
    /// The submitted frame was discarded.
    DroppedNewest(Timestamp),
}
