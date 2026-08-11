use std::{fmt, sync::Arc};

use obs_rs_media::Timestamp;

use super::{AudioBuffer, AudioError, AudioFormat};

/// The direction of a device exposed by an audio provider.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AudioDeviceKind {
    /// A source that supplies samples to the mixer.
    Input,
    /// A sink used for monitoring the mixed signal.
    Output,
}

/// Stable metadata for one audio device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioDeviceInfo {
    id: String,
    name: String,
    kind: AudioDeviceKind,
    available: bool,
}

impl AudioDeviceInfo {
    /// Creates a validated device descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`AudioDeviceError::InvalidDevice`] for an empty ID or name.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        kind: AudioDeviceKind,
    ) -> Result<Self, AudioDeviceError> {
        let id = id.into();
        let name = name.into();
        if id.trim().is_empty() || name.trim().is_empty() {
            return Err(AudioDeviceError::InvalidDevice(
                "audio device id and name must be non-empty".to_owned(),
            ));
        }
        Ok(Self {
            id,
            name,
            kind,
            available: true,
        })
    }

    /// Returns the provider-stable device ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether this is an input or output device.
    #[must_use]
    pub const fn kind(&self) -> AudioDeviceKind {
        self.kind
    }

    /// Returns whether the provider currently considers the device usable.
    #[must_use]
    pub const fn available(&self) -> bool {
        self.available
    }

    /// Marks the descriptor unavailable without removing it from a catalog.
    pub const fn set_available(&mut self, available: bool) {
        self.available = available;
    }
}

/// Lifecycle state of an audio input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioInputState {
    /// The input has not been started.
    Stopped,
    /// The input is delivering blocks.
    Running,
    /// The input failed and must be recreated.
    Failed,
}

/// Errors crossing the platform audio boundary.
#[derive(Debug)]
pub enum AudioDeviceError {
    /// A descriptor or provider argument is invalid.
    InvalidDevice(String),
    /// The requested device or format is not available.
    Unavailable(String),
    /// The underlying device process or stream returned an I/O error.
    Io(std::io::Error),
    /// The device returned a block that cannot satisfy the audio contract.
    Audio(AudioError),
}

impl fmt::Display for AudioDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDevice(reason) => write!(formatter, "invalid audio device: {reason}"),
            Self::Unavailable(reason) => write!(formatter, "audio device unavailable: {reason}"),
            Self::Io(error) => write!(formatter, "audio device I/O failed: {error}"),
            Self::Audio(error) => write!(formatter, "audio device block failed: {error}"),
        }
    }
}

impl std::error::Error for AudioDeviceError {}

impl From<std::io::Error> for AudioDeviceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<AudioError> for AudioDeviceError {
    fn from(error: AudioError) -> Self {
        Self::Audio(error)
    }
}

/// A source of timestamped audio blocks.
pub trait AudioInput: Send {
    /// Returns the fixed format negotiated at construction.
    fn format(&self) -> AudioFormat;

    /// Returns the current lifecycle state.
    fn state(&self) -> AudioInputState;

    /// Reads one complete block at the requested media timestamp.
    ///
    /// Implementations may block while waiting for the device, but must return
    /// exactly `frames` frames or a typed error. The engine keeps this operation
    /// off the UI thread.
    ///
    /// # Errors
    ///
    /// Returns [`AudioDeviceError`] when the device cannot provide a complete
    /// block or the returned samples violate the audio contract.
    fn read_block(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<AudioBuffer, AudioDeviceError>;

    /// Stops the input and releases its device resources.
    fn stop(&mut self);
}

/// Provider boundary for real and deterministic audio inputs.
pub trait AudioInputProvider: Send + Sync {
    /// Discovers a complete, deterministic snapshot of input devices.
    ///
    /// # Errors
    ///
    /// Returns [`AudioDeviceError`] when the host audio service cannot be
    /// queried.
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError>;

    /// Opens one input at a fixed format.
    ///
    /// # Errors
    ///
    /// Returns [`AudioDeviceError`] when the device ID or negotiated format is
    /// unavailable.
    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, AudioDeviceError>;
}

/// Deterministic provider used when a host audio server is unavailable.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimulatedAudioProvider;

impl SimulatedAudioProvider {
    /// Creates the fallback provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl AudioInputProvider for SimulatedAudioProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Ok(vec![AudioDeviceInfo::new(
            "test-audio",
            "Deterministic test signal",
            AudioDeviceKind::Input,
        )?])
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
        if device_id != "test-audio" {
            return Err(AudioDeviceError::Unavailable(device_id.to_owned()));
        }
        Ok(Box::new(SimulatedAudioInput::new(format)?))
    }
}

/// A bounded deterministic stereo signal for tests and fallback operation.
pub struct SimulatedAudioInput {
    format: AudioFormat,
    state: AudioInputState,
    phase: u32,
}

impl SimulatedAudioInput {
    /// Creates a stopped signal source.
    ///
    /// # Errors
    ///
    /// Returns [`AudioDeviceError::InvalidDevice`] for an invalid format.
    pub fn new(format: AudioFormat) -> Result<Self, AudioDeviceError> {
        if format.channels() == 0 {
            return Err(AudioDeviceError::InvalidDevice(
                "audio input requires at least one channel".to_owned(),
            ));
        }
        Ok(Self {
            format,
            state: AudioInputState::Stopped,
            phase: 0,
        })
    }
}

impl AudioInput for SimulatedAudioInput {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn state(&self) -> AudioInputState {
        self.state
    }

    fn read_block(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<AudioBuffer, AudioDeviceError> {
        self.state = AudioInputState::Running;
        let channels = usize::from(self.format.channels());
        let mut samples = Vec::with_capacity(frames.saturating_mul(channels));
        for _ in 0..frames {
            let value = if self.phase < 24_000 { 0.12 } else { -0.12 };
            self.phase = (self.phase + 1) % 48_000;
            samples.extend(std::iter::repeat_n(value, channels));
        }
        Ok(AudioBuffer::new(self.format, timestamp, samples)?)
    }

    fn stop(&mut self) {
        self.state = AudioInputState::Stopped;
    }
}

impl Drop for SimulatedAudioInput {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Shared provider handle convenient for application dependency injection.
pub type SharedAudioInputProvider = Arc<dyn AudioInputProvider>;
