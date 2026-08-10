use std::time::{Duration, Instant};

use obs_rs_audio::AudioClock;
use obs_rs_media::{sleep_precise, Timestamp};
use obs_rs_video::VideoClock;
/// One monotonic wall-clock origin that can drive both worker traits.
pub struct MonotonicMediaClock {
    origin: Instant,
}

impl MonotonicMediaClock {
    /// Starts a shared media clock at the current monotonic instant.
    #[must_use]
    pub fn start() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    /// Returns elapsed nanoseconds since [`Self::start`].
    #[must_use]
    pub fn now(&self) -> Timestamp {
        Timestamp::from_nanos(u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }

    fn sleep_until(&self, deadline: Timestamp) {
        let current = self.now();
        let remaining = deadline.as_nanos().saturating_sub(current.as_nanos());
        // Sub-millisecond accuracy so high frame rates do not miss deadlines.
        sleep_precise(Duration::from_nanos(remaining));
    }
}

impl AudioClock for MonotonicMediaClock {
    fn now(&self) -> Timestamp {
        Self::now(self)
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        Self::sleep_until(self, deadline);
    }
}

impl VideoClock for MonotonicMediaClock {
    fn now(&self) -> Timestamp {
        Self::now(self)
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        Self::sleep_until(self, deadline);
    }
}
