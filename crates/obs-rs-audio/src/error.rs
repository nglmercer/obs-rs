use super::types::{AudioFormat, AudioMonitorTapId, AudioSourceId, MAX_CALLBACK_CORRECTION_PPM};
use obs_rs_media::Timestamp;
use std::fmt;
/// Errors from a paced audio worker.
#[derive(Debug, Eq, PartialEq)]
pub enum AudioWorkerError<E> {
    /// The sample-clock pacer could not advance its timeline.
    Pacing(AudioError),
    /// The producer callback failed.
    Source(E),
    /// The producer returned a buffer that violated the worker contract.
    Submit(AudioError),
}

impl<E: fmt::Display> fmt::Display for AudioWorkerError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pacing(error) => write!(formatter, "audio worker pacing failed: {error}"),
            Self::Source(error) => write!(formatter, "audio source failed: {error}"),
            Self::Submit(error) => write!(formatter, "audio submission failed: {error}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for AudioWorkerError<E> {}
/// Errors raised by audio values, queues, and mixing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioError {
    /// Sample rate or channel count is zero.
    InvalidFormat,
    /// The sample count is not a complete interleaved frame sequence.
    SamplesNotInterleaved { samples: usize, channels: u16 },
    /// A sample is NaN or infinite.
    NonFiniteSample,
    /// An audio buffer exceeds [`crate::MAX_AUDIO_FRAMES`].
    BufferTooLarge { frames: usize },
    /// A buffer format does not match a queue or mixer.
    FormatMismatch {
        /// Expected format.
        expected: AudioFormat,
        /// Supplied format.
        actual: AudioFormat,
    },
    /// A worker buffer starts at a different sample-clock timestamp.
    BufferTimestampMismatch {
        /// Timestamp required by the worker deadline.
        expected: Timestamp,
        /// Timestamp supplied by the producer.
        actual: Timestamp,
    },
    /// Two audio formats have different channel counts.
    ChannelMismatch,
    /// A buffer has a different frame count from a mix request.
    FrameCountMismatch { expected: usize, actual: usize },
    /// A queue capacity is zero.
    ZeroCapacity,
    /// A mixer source ID is unknown.
    UnknownSource(AudioSourceId),
    /// A source occurs more than once in one mix input list.
    DuplicateInput(AudioSourceId),
    /// A source gain is not finite.
    InvalidGain,
    /// An audio gain filter is outside the bounded OBS-compatible range.
    InvalidFilterGain { milli_db: i32 },
    /// An audio limiter threshold is outside OBS's bounded dB range.
    InvalidLimiterThreshold { milli_db: i32 },
    /// An audio limiter release time is outside OBS's bounded millisecond range.
    InvalidLimiterRelease { milliseconds: u16 },
    /// An audio compressor ratio is outside OBS's bounded ratio range.
    InvalidCompressorRatio { milli_ratio: u16 },
    /// An audio compressor threshold is outside OBS's bounded dB range.
    InvalidCompressorThreshold { milli_db: i32 },
    /// An audio compressor attack time is outside OBS's bounded range.
    InvalidCompressorAttack { milliseconds: u16 },
    /// An audio compressor release time is outside OBS's bounded range.
    InvalidCompressorRelease { milliseconds: u16 },
    /// An audio compressor output gain is outside OBS's bounded dB range.
    InvalidCompressorOutputGain { milli_db: i32 },
    /// An audio expander ratio is outside OBS's bounded ratio range.
    InvalidExpanderRatio { milli_ratio: u16 },
    /// An audio expander threshold is outside OBS's bounded dB range.
    InvalidExpanderThreshold { milli_db: i32 },
    /// An audio expander attack time is outside OBS's bounded range.
    InvalidExpanderAttack { milliseconds: u16 },
    /// An audio expander release time is outside OBS's bounded range.
    InvalidExpanderRelease { milliseconds: u16 },
    /// An audio expander output gain is outside OBS's bounded dB range.
    InvalidExpanderOutputGain { milli_db: i32 },
    /// An ordered audio filter chain reached its fixed capacity.
    FilterChainFull { max: usize },
    /// An audio filter would have produced a non-finite sample.
    FilterOverflow,
    /// A source pan is not finite or is outside `[-1.0, 1.0]`.
    InvalidPan,
    /// A mix sum overflowed the finite `f32` range.
    MixOverflow,
    /// Source IDs are exhausted.
    SourceIdExhausted,
    /// A monitor tap capacity is zero.
    ZeroMonitorCapacity,
    /// A monitor tap ID is unknown.
    UnknownMonitorTap(AudioMonitorTapId),
    /// Monitor tap IDs are exhausted.
    MonitorTapIdExhausted,
    /// An audio pacing block must contain at least one sample frame.
    ZeroBlock,
    /// The sample-clock timestamp or frame index overflowed.
    ScheduleOverflow,
    /// A callback device clock moved backward.
    CallbackTimestampRegression {
        /// Previous callback timestamp.
        previous: Timestamp,
        /// Regressed callback timestamp.
        actual: Timestamp,
    },
    /// A callback clock correction exceeds the safe bound.
    CallbackCorrectionOutOfRange { ppm: i32 },
}

impl fmt::Display for AudioError {
    #[allow(
        clippy::too_many_lines,
        reason = "the public audio error catalog keeps every bounded contract message together"
    )]
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => formatter.write_str("audio format values must be non-zero"),
            Self::SamplesNotInterleaved { samples, channels } => write!(
                formatter,
                "{samples} samples cannot be interleaved across {channels} channels"
            ),
            Self::NonFiniteSample => formatter.write_str("audio samples must be finite"),
            Self::BufferTooLarge { frames } => {
                write!(formatter, "audio buffer has too many frames: {frames}")
            }
            Self::FormatMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio format {actual:?} does not match {expected:?}"
                )
            }
            Self::BufferTimestampMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio buffer starts at {actual:?}; expected {expected:?}"
                )
            }
            Self::ChannelMismatch => formatter.write_str("audio channel layouts do not match"),
            Self::FrameCountMismatch { expected, actual } => {
                write!(
                    formatter,
                    "audio buffer has {actual} frames; expected {expected}"
                )
            }
            Self::ZeroCapacity => formatter.write_str("audio queue capacity must be non-zero"),
            Self::UnknownSource(source) => {
                write!(formatter, "audio source {} does not exist", source.value())
            }
            Self::DuplicateInput(source) => {
                write!(
                    formatter,
                    "audio source {} occurs more than once",
                    source.value()
                )
            }
            Self::InvalidGain => formatter.write_str("audio gain must be finite"),
            Self::InvalidFilterGain { milli_db } => write!(
                formatter,
                "audio filter gain {milli_db} milli-dB is outside the supported range"
            ),
            Self::InvalidLimiterThreshold { milli_db } => write!(
                formatter,
                "audio limiter threshold {milli_db} milli-dB is outside the supported range"
            ),
            Self::InvalidLimiterRelease { milliseconds } => write!(
                formatter,
                "audio limiter release {milliseconds} ms is outside the supported range"
            ),
            Self::InvalidCompressorRatio { milli_ratio } => write!(
                formatter,
                "audio compressor ratio {milli_ratio} milli-ratio is outside the supported range"
            ),
            Self::InvalidCompressorThreshold { milli_db } => write!(
                formatter,
                "audio compressor threshold {milli_db} milli-dB is outside the supported range"
            ),
            Self::InvalidCompressorAttack { milliseconds } => write!(
                formatter,
                "audio compressor attack {milliseconds} ms is outside the supported range"
            ),
            Self::InvalidCompressorRelease { milliseconds } => write!(
                formatter,
                "audio compressor release {milliseconds} ms is outside the supported range"
            ),
            Self::InvalidCompressorOutputGain { milli_db } => write!(
                formatter,
                "audio compressor output gain {milli_db} milli-dB is outside the supported range"
            ),
            Self::InvalidExpanderRatio { milli_ratio } => write!(
                formatter,
                "audio expander ratio {milli_ratio} milli-ratio is outside the supported range"
            ),
            Self::InvalidExpanderThreshold { milli_db } => write!(
                formatter,
                "audio expander threshold {milli_db} milli-dB is outside the supported range"
            ),
            Self::InvalidExpanderAttack { milliseconds } => write!(
                formatter,
                "audio expander attack {milliseconds} ms is outside the supported range"
            ),
            Self::InvalidExpanderRelease { milliseconds } => write!(
                formatter,
                "audio expander release {milliseconds} ms is outside the supported range"
            ),
            Self::InvalidExpanderOutputGain { milli_db } => write!(
                formatter,
                "audio expander output gain {milli_db} milli-dB is outside the supported range"
            ),
            Self::FilterChainFull { max } => {
                write!(formatter, "audio filter chain is limited to {max} filters")
            }
            Self::FilterOverflow => {
                formatter.write_str("audio filter produced a non-finite sample")
            }
            Self::InvalidPan => {
                formatter.write_str("audio pan must be finite and between -1 and 1")
            }
            Self::MixOverflow => formatter.write_str("audio mix exceeded finite sample range"),
            Self::SourceIdExhausted => formatter.write_str("audio source ID space is exhausted"),
            Self::ZeroMonitorCapacity => {
                formatter.write_str("audio monitor tap capacity must be non-zero")
            }
            Self::UnknownMonitorTap(tap) => {
                write!(
                    formatter,
                    "audio monitor tap {} does not exist",
                    tap.value()
                )
            }
            Self::MonitorTapIdExhausted => {
                formatter.write_str("audio monitor tap ID space is exhausted")
            }
            Self::ZeroBlock => formatter.write_str("audio pacing blocks must be non-empty"),
            Self::ScheduleOverflow => formatter.write_str("audio schedule timestamp overflowed"),
            Self::CallbackTimestampRegression { previous, actual } => write!(
                formatter,
                "audio callback timestamp {actual:?} moved before {previous:?}"
            ),
            Self::CallbackCorrectionOutOfRange { ppm } => write!(
                formatter,
                "audio callback correction {ppm} ppm is outside +/-{MAX_CALLBACK_CORRECTION_PPM} ppm"
            ),
        }
    }
}

impl std::error::Error for AudioError {}
