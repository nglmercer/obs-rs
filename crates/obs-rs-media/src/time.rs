use super::error::MediaError;
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
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
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
    pub fn period_nanos(self) -> Option<u64> {
        let period = 1_000_000_000_u128 * u128::from(self.denominator) / u128::from(self.numerator);
        u64::try_from(period).ok()
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
