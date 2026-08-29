use super::error::AudioError;
/// Maximum number of interleaved audio frames in one owned buffer.
pub const MAX_AUDIO_FRAMES: usize = 1_048_576;

/// Common endpoint formats tried after the engine's preferred mix format.
///
/// WASAPI shared-mode devices usually accept 48 kHz or 44.1 kHz, but fixed
/// native endpoints such as some USB interfaces and virtual cables may expose
/// only 96 kHz, 32 kHz, or 16 kHz. Keeping this list bounded lets the route and
/// monitor workers negotiate those devices without probing arbitrary formats.
pub const COMMON_AUDIO_DEVICE_FORMATS: [(u32, u16); 10] = [
    (48_000, 2),
    (44_100, 2),
    (48_000, 1),
    (44_100, 1),
    (96_000, 2),
    (32_000, 2),
    (16_000, 2),
    (96_000, 1),
    (32_000, 1),
    (16_000, 1),
];

/// A validated interleaved audio format.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AudioFormat {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) layout: AudioChannelLayout,
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
            layout: AudioChannelLayout::from_channels(channels),
        })
    }

    /// Creates a format from a named channel layout.
    ///
    /// The layout is part of the format identity, so downstream mixers and
    /// output adapters cannot accidentally treat a five-channel signal as an
    /// unlabeled collection of interleaved samples.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidFormat`] when the sample rate or layout
    /// contains no channels.
    pub const fn with_layout(
        sample_rate: u32,
        layout: AudioChannelLayout,
    ) -> Result<Self, AudioError> {
        if sample_rate == 0 || layout.channels() == 0 {
            return Err(AudioError::InvalidFormat);
        }
        Ok(Self {
            sample_rate,
            channels: layout.channels(),
            layout,
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

    /// Returns the semantic layout associated with the interleaved channels.
    #[must_use]
    pub const fn layout(self) -> AudioChannelLayout {
        self.layout
    }
}

/// Standard interleaved channel layouts accepted by the portable audio graph.
///
/// `Discrete` preserves compatibility with provider formats that expose a
/// positive channel count without a standard speaker assignment. It is still
/// bounded by the `u16` channel-count contract on [`AudioFormat`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AudioChannelLayout {
    /// One front-center channel.
    Mono,
    /// Front-left and front-right.
    Stereo,
    /// Front-left, front-right, and low-frequency effects.
    TwoPointOne,
    /// Front-left, front-right, rear-left, and rear-right.
    Quad,
    /// Front-left, front-right, front-center, low-frequency effects, side-left,
    /// and side-right.
    FivePointOne,
    /// Front-left, front-right, front-center, low-frequency effects, rear-left,
    /// rear-right, side-left, and side-right.
    SevenPointOne,
    /// A positive channel count with no standard speaker assignment.
    Discrete(u16),
}

impl AudioChannelLayout {
    /// Infers the standard layout for a channel count, retaining unknown
    /// positive counts as [`Self::Discrete`].
    #[must_use]
    pub const fn from_channels(channels: u16) -> Self {
        match channels {
            1 => Self::Mono,
            2 => Self::Stereo,
            3 => Self::TwoPointOne,
            4 => Self::Quad,
            6 => Self::FivePointOne,
            8 => Self::SevenPointOne,
            other => Self::Discrete(other),
        }
    }

    /// Returns the number of interleaved channels in this layout.
    #[must_use]
    pub const fn channels(self) -> u16 {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::TwoPointOne => 3,
            Self::Quad => 4,
            Self::FivePointOne => 6,
            Self::SevenPointOne => 8,
            Self::Discrete(channels) => channels,
        }
    }

    /// Returns a stable identifier for standard layouts.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Mono => "mono",
            Self::Stereo => "stereo",
            Self::TwoPointOne => "2.1",
            Self::Quad => "quad",
            Self::FivePointOne => "5.1",
            Self::SevenPointOne => "7.1",
            Self::Discrete(_) => "discrete",
        }
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
