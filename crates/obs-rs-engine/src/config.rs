use std::sync::Arc;

use obs_rs_audio::{
    AudioFormat, AudioInputProvider, AudioMonitorMode, AudioOutputProvider, SimulatedAudioProvider,
};
use obs_rs_media::{FrameRate, VideoFormat};
use obs_rs_output::{RleVideoEncoder, VideoEncoder};

use super::{
    DEFAULT_AUDIO_BLOCK_FRAMES, DEFAULT_MONITOR_OUTPUT_QUEUE_BLOCKS, DEFAULT_OUTPUT_QUEUE_BYTES,
    DEFAULT_RECONNECT_ATTEMPTS, DEFAULT_TIMELINE_TOLERANCE_NANOS,
};

/// Configuration for one portable engine session.
pub struct EngineConfig {
    pub(super) audio_format: AudioFormat,
    pub(super) audio_block_frames: usize,
    pub(super) timeline_tolerance_nanos: u64,
    pub(super) output_queue_bytes: usize,
    pub(super) reconnect_attempts: u32,
    pub(super) audio_input_id: Option<String>,
    pub(super) desktop_audio_id: Option<String>,
    pub(super) audio_input_sync_offset_millis: u32,
    pub(super) desktop_audio_sync_offset_millis: u32,
    pub(super) desktop_monitor_mode: AudioMonitorMode,
    pub(super) microphone_monitor_mode: AudioMonitorMode,
    pub(super) audio_provider: Arc<dyn AudioInputProvider>,
    pub(super) audio_output_provider: Arc<dyn AudioOutputProvider>,
    pub(super) monitor_output_id: Option<String>,
    pub(super) monitor_output_queue_blocks: usize,
    pub(super) video_encoder: Box<dyn VideoEncoder>,
}

impl EngineConfig {
    /// Creates a configuration with the deterministic audio fallback and the
    /// lossless RLE video encoder.
    ///
    /// Use [`Self::with_video_encoder`] to install a different [`VideoEncoder`].
    ///
    /// # Panics
    ///
    /// Panics if the hardcoded default video format is ever invalid, which
    /// cannot happen for the constants used here.
    #[must_use]
    pub fn new(audio_format: AudioFormat) -> Self {
        let simulated_input_provider: Arc<dyn AudioInputProvider> =
            Arc::new(SimulatedAudioProvider::new());
        let simulated_output_provider: Arc<dyn AudioOutputProvider> =
            Arc::new(SimulatedAudioProvider::new());
        Self {
            audio_format,
            audio_block_frames: DEFAULT_AUDIO_BLOCK_FRAMES,
            timeline_tolerance_nanos: DEFAULT_TIMELINE_TOLERANCE_NANOS,
            output_queue_bytes: DEFAULT_OUTPUT_QUEUE_BYTES,
            reconnect_attempts: DEFAULT_RECONNECT_ATTEMPTS,
            audio_input_id: None,
            desktop_audio_id: None,
            audio_input_sync_offset_millis: 0,
            desktop_audio_sync_offset_millis: 0,
            desktop_monitor_mode: AudioMonitorMode::Off,
            microphone_monitor_mode: AudioMonitorMode::Off,
            audio_provider: simulated_input_provider,
            audio_output_provider: simulated_output_provider,
            monitor_output_id: None,
            monitor_output_queue_blocks: DEFAULT_MONITOR_OUTPUT_QUEUE_BLOCKS,
            video_encoder: Box::new(RleVideoEncoder::new(
                VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("valid rate"))
                    .expect("valid format"),
            )),
        }
    }

    /// Replaces the audio provider used before falling back to the test signal.
    #[must_use]
    pub fn with_audio_provider(mut self, provider: Arc<dyn AudioInputProvider>) -> Self {
        self.audio_provider = provider;
        self
    }

    /// Replaces the provider used to open optional local monitor sinks.
    #[must_use]
    pub fn with_audio_output_provider(mut self, provider: Arc<dyn AudioOutputProvider>) -> Self {
        self.audio_output_provider = provider;
        self
    }

    /// Selects a provider-stable input ID, or clears the selection when empty.
    #[must_use]
    pub fn with_audio_input_id(mut self, device_id: impl Into<String>) -> Self {
        let device_id = device_id.into();
        self.audio_input_id = (!device_id.trim().is_empty()).then_some(device_id);
        self
    }

    /// Selects the playback device whose monitor feeds the desktop channel.
    ///
    /// An empty value clears the selection, which makes the session pick the
    /// provider-declared default playback route, then the first available
    /// route. Desktop capture has no deterministic stand-in: when no monitor
    /// can be opened the channel stays silent rather than borrowing the
    /// microphone's test signal, so what the meter shows is what the recording
    /// contains.
    #[must_use]
    pub fn with_desktop_audio_id(mut self, device_id: impl Into<String>) -> Self {
        let device_id = device_id.into();
        self.desktop_audio_id = (!device_id.trim().is_empty()).then_some(device_id);
        self
    }

    /// Selects the optional output device used by the local monitor bus.
    ///
    /// An empty or whitespace-only value clears the sink. Opening and writing
    /// the selected device remains on the dedicated audio-output worker.
    #[must_use]
    pub fn with_monitor_output_id(mut self, device_id: impl Into<String>) -> Self {
        let device_id = device_id.into();
        self.monitor_output_id =
            (!device_id.trim().is_empty()).then(|| device_id.trim().to_owned());
        self
    }

    /// Sets the bounded number of complete monitor blocks held by the output
    /// worker before new blocks are dropped.
    #[must_use]
    pub const fn with_monitor_output_queue_blocks(mut self, blocks: usize) -> Self {
        self.monitor_output_queue_blocks = blocks;
        self
    }

    /// Sets the desktop channel's OBS-compatible monitor destination policy.
    #[must_use]
    pub const fn with_desktop_monitor_mode(mut self, mode: AudioMonitorMode) -> Self {
        self.desktop_monitor_mode = mode;
        self
    }

    /// Sets the microphone channel's OBS-compatible monitor destination policy.
    #[must_use]
    pub const fn with_audio_input_monitor_mode(mut self, mode: AudioMonitorMode) -> Self {
        self.microphone_monitor_mode = mode;
        self
    }

    /// Sets the bounded positive delay applied to the microphone channel.
    #[must_use]
    pub const fn with_audio_input_sync_offset_millis(mut self, milliseconds: u32) -> Self {
        self.audio_input_sync_offset_millis = milliseconds;
        self
    }

    /// Sets the bounded positive delay applied to the desktop channel.
    #[must_use]
    pub const fn with_desktop_audio_sync_offset_millis(mut self, milliseconds: u32) -> Self {
        self.desktop_audio_sync_offset_millis = milliseconds;
        self
    }

    /// Sets the number of sample frames mixed per engine tick.
    ///
    /// A zero value is rejected when the session is created, where the error can
    /// be reported through the normal engine error channel.
    #[must_use]
    pub const fn with_audio_block_frames(mut self, frames: usize) -> Self {
        self.audio_block_frames = frames;
        self
    }

    /// Sets the bounded stream queue capacity in bytes.
    #[must_use]
    pub const fn with_output_queue_bytes(mut self, bytes: usize) -> Self {
        self.output_queue_bytes = bytes;
        self
    }

    /// Replaces the video encoder used for recording and streaming.
    ///
    /// The default is a lossless RLE reference encoder; production hosts swap in
    /// a hardware-accelerated or production-codec implementation behind this
    /// same contract without touching engine source.
    #[must_use]
    pub fn with_video_encoder(mut self, encoder: Box<dyn VideoEncoder>) -> Self {
        self.video_encoder = encoder;
        self
    }

    /// Returns the negotiated audio format.
    #[must_use]
    pub const fn audio_format(&self) -> AudioFormat {
        self.audio_format
    }
}

impl Clone for EngineConfig {
    // `Clone::clone` has no fallible contract, so documenting panics would be
    // misleading. The `expect` calls here guard values this constructor itself
    // produced, so they cannot fire in practice.
    #[allow(clippy::missing_panics_doc)]
    fn clone(&self) -> Self {
        // The video encoder is a trait object that has no `Clone`, and the
        // format it was built for is not readable back out of the trait. A
        // cloned config therefore installs a fresh default RLE encoder rather
        // than producing a half-populated config the session constructor would
        // have to special-case.
        let format = VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid format");
        Self {
            audio_format: self.audio_format,
            audio_block_frames: self.audio_block_frames,
            timeline_tolerance_nanos: self.timeline_tolerance_nanos,
            output_queue_bytes: self.output_queue_bytes,
            reconnect_attempts: self.reconnect_attempts,
            audio_input_id: self.audio_input_id.clone(),
            desktop_audio_id: self.desktop_audio_id.clone(),
            audio_input_sync_offset_millis: self.audio_input_sync_offset_millis,
            desktop_audio_sync_offset_millis: self.desktop_audio_sync_offset_millis,
            desktop_monitor_mode: self.desktop_monitor_mode,
            microphone_monitor_mode: self.microphone_monitor_mode,
            audio_provider: Arc::clone(&self.audio_provider),
            audio_output_provider: Arc::clone(&self.audio_output_provider),
            monitor_output_id: self.monitor_output_id.clone(),
            monitor_output_queue_blocks: self.monitor_output_queue_blocks,
            video_encoder: Box::new(RleVideoEncoder::new(format)),
        }
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        let format = AudioFormat::new(48_000, 2)
            .unwrap_or_else(|error| unreachable!("the built-in audio format is valid: {error}"));
        Self::new(format)
    }
}
