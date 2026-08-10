use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
/// A thread-safe cancellation request for a coordinated media session.
#[derive(Clone, Debug)]
pub struct SessionCancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl Default for SessionCancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCancellationToken {
    /// Creates an uncancelled session token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests cancellation before the next coordinated tick.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Clears a previous cancellation request for reuse.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}
