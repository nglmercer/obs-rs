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

/// The destination policy for one mixer source.
///
/// The output bus is the signal sent to recording and streaming. The monitor
/// bus is the signal sent to a local monitoring sink. Keeping the policy on
/// the mixer source makes the routing decision part of the audio graph rather
/// than a GUI-only flag.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AudioMonitorMode {
    /// Send the source to the output bus only.
    #[default]
    Off,
    /// Send the source to the monitor bus and mute it from the output bus.
    MonitorOnly,
    /// Send the source to both the monitor and output buses.
    MonitorAndOutput,
}

impl AudioMonitorMode {
    /// Returns whether this mode contributes the source to the output bus.
    #[must_use]
    pub const fn sends_to_output(self) -> bool {
        matches!(self, Self::Off | Self::MonitorAndOutput)
    }

    /// Returns whether this mode contributes the source to the monitor bus.
    #[must_use]
    pub const fn sends_to_monitor(self) -> bool {
        matches!(self, Self::MonitorOnly | Self::MonitorAndOutput)
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
