use super::error::MediaError;
use std::time::{Duration, Instant};
/// A monotonic media position expressed in nanoseconds.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(u64);

impl Timestamp {
    /// The beginning of a media timeline.
    pub const ZERO: Self = Self(0);

    /// Creates a timestamp from nanoseconds.
    #[must_use]
    pub const fn from_nanos(nanoseconds: u64) -> Self {
        Self(nanoseconds)
    }

    /// Creates a timestamp from milliseconds.
    #[must_use]
    pub const fn from_millis(milliseconds: u64) -> Self {
        Self(milliseconds.saturating_mul(1_000_000))
    }

    /// Returns the timestamp in nanoseconds.
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Adds nanoseconds, returning `None` if the timeline would overflow.
    #[must_use]
    pub const fn checked_add(self, nanoseconds: u64) -> Option<Self> {
        match self.0.checked_add(nanoseconds) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// A reduced, positive rational video frame rate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FrameRate {
    numerator: u32,
    denominator: u32,
    period_nanos: Option<u64>,
}

impl FrameRate {
    /// Creates and reduces a frame rate such as `30/1` or `30000/1001`.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidFrameRate`] when either component is zero.
    pub fn new(numerator: u32, denominator: u32) -> Result<Self, MediaError> {
        if numerator == 0 || denominator == 0 {
            return Err(MediaError::InvalidFrameRate);
        }

        let divisor = greatest_common_divisor(numerator, denominator);
        let numerator = numerator / divisor;
        let denominator = denominator / divisor;
        let period = 1_000_000_000_u128 * u128::from(denominator) / u128::from(numerator);
        Ok(Self {
            numerator,
            denominator,
            period_nanos: u64::try_from(period).ok(),
        })
    }

    /// Returns the reduced numerator.
    #[must_use]
    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    /// Returns the reduced denominator.
    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }

    /// Returns the number of nanoseconds per frame when it fits in `u64`.
    #[must_use]
    pub const fn period_nanos(self) -> Option<u64> {
        self.period_nanos
    }
}

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

/// Window before a deadline in which [`sleep_until_precise`] busy-waits.
///
/// `std::thread::sleep` only guarantees a *minimum* duration and typically
/// overshoots by around a millisecond, which is enough to miss deadlines above
/// 60 fps. The final stretch is therefore spun rather than parked.
pub const SLEEP_SPIN_WINDOW: Duration = Duration::from_micros(1_500);

/// Sleeps for `duration` with sub-millisecond accuracy.
///
/// Parks the thread for everything beyond [`SLEEP_SPIN_WINDOW`] and busy-waits
/// the remainder, trading up to `SLEEP_SPIN_WINDOW` of one core per call for a
/// wake-up that lands close to the requested instant. Callers that do not need
/// tight deadlines should keep using [`std::thread::sleep`].
pub fn sleep_precise(duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let deadline = Instant::now() + duration;
    if let Some(parked) = duration.checked_sub(SLEEP_SPIN_WINDOW) {
        std::thread::sleep(parked);
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}
