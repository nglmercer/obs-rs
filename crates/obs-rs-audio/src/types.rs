use super::error::AudioError;
/// Maximum number of interleaved audio frames in one owned buffer.
pub const MAX_AUDIO_FRAMES: usize = 1_048_576;

/// A validated interleaved audio format.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AudioFormat {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
}

impl AudioFormat {
    /// Creates a format with a positive sample rate and channel count.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidFormat`] when either value is zero.
    pub const fn new(sample_rate: u32, channels: u16) -> Result<Self, AudioError> {
        if sample_rate == 0 || channels == 0 {
            return Err(AudioError::InvalidFormat);
        }
        Ok(Self {
            sample_rate,
            channels,
        })
    }

    /// Returns samples per second.
    #[must_use]
    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    /// Returns the number of interleaved channels.
    #[must_use]
    pub const fn channels(self) -> u16 {
        self.channels
    }
}

/// A stable handle for a mixer source.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AudioSourceId(pub(crate) u64);

impl AudioSourceId {
    /// Returns the numeric value for logs and fixtures.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// A stable handle for a bounded post-mix monitoring tap.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AudioMonitorTapId(pub(crate) u64);

impl AudioMonitorTapId {
    /// Returns the numeric value for logs and fixtures.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}
/// Maximum rate correction accepted by a callback-driven device clock.
pub const MAX_CALLBACK_CORRECTION_PPM: i32 = 10_000;
