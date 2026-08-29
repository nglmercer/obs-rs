use std::sync::Arc;

use obs_rs_audio::{
    AudioDelayLine, AudioFilter, AudioFilterChain, AudioFormat, AudioMonitorMode, AudioOutputWorker,
};
use obs_rs_clock::MediaTimeline;
use obs_rs_output::RawAudioEncoder;

use super::{open_audio_input, open_desktop_audio, EngineAudioChannel, EngineError, EngineSession};

#[allow(
    clippy::missing_errors_doc,
    reason = "the session control methods share the documented EngineError boundary"
)]
impl EngineSession {
    /// Updates the gain of the live input source in thousandths.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the gain is outside the mixer contract.
    pub fn set_channel_gain_milli(
        &mut self,
        channel: EngineAudioChannel,
        gain_milli: u16,
    ) -> Result<(), EngineError> {
        let source = match channel {
            EngineAudioChannel::Desktop => self.desktop_audio_source,
            EngineAudioChannel::Microphone => self.microphone_audio_source,
        };
        self.mixer.set_gain_milli(source, gain_milli)?;
        Ok(())
    }

    /// Updates the stereo pan of a live input source in thousandths of a full
    /// left/right turn (`-1000` is left, `0` is center, `1000` is right).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the pan is outside the bounded mixer
    /// contract or the channel source is unavailable.
    pub fn set_channel_pan_milli(
        &mut self,
        channel: EngineAudioChannel,
        pan_milli: i32,
    ) -> Result<(), EngineError> {
        let source = match channel {
            EngineAudioChannel::Desktop => self.desktop_audio_source,
            EngineAudioChannel::Microphone => self.microphone_audio_source,
        };
        self.mixer.set_pan_milli(source, pan_milli)?;
        Ok(())
    }

    /// Sets a bounded positive sync offset on one live audio channel.
    ///
    /// The delay is quantized to complete sample frames and clearing or
    /// changing it resets only that channel's queued audio. No unbounded
    /// samples are retained and the video timeline is not blocked.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the offset exceeds the audio delay-line
    /// bound or the channel cannot be reconfigured.
    pub fn set_channel_sync_offset_millis(
        &mut self,
        channel: EngineAudioChannel,
        milliseconds: u32,
    ) -> Result<(), EngineError> {
        match channel {
            EngineAudioChannel::Desktop => {
                self.desktop_audio_delay
                    .set_delay_milliseconds(milliseconds)?;
                self.config.desktop_audio_sync_offset_millis = milliseconds;
            }
            EngineAudioChannel::Microphone => {
                self.audio_input_delay
                    .set_delay_milliseconds(milliseconds)?;
                self.config.audio_input_sync_offset_millis = milliseconds;
            }
        }
        Ok(())
    }

    /// Replaces the ordered audio-filter chain on one live mixer channel.
    ///
    /// The chain is owned by the engine and applied to each captured block
    /// before metering and mixing. Replacing it is a control-plane operation;
    /// applying it remains allocation-free on the audio path.
    pub fn set_channel_audio_filters(
        &mut self,
        channel: EngineAudioChannel,
        filters: AudioFilterChain,
    ) {
        match channel {
            EngineAudioChannel::Desktop => self.desktop_audio_filters = filters,
            EngineAudioChannel::Microphone => self.microphone_audio_filters = filters,
        }
    }

    /// Installs one OBS-compatible Gain filter on a live mixer channel.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the dB value is outside the bounded Gain
    /// filter range.
    pub fn set_channel_gain_filter_db_milli(
        &mut self,
        channel: EngineAudioChannel,
        milli_db: i32,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::gain_db_milli(milli_db)?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's Invert Polarity filter on a live mixer channel.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the bounded chain cannot accept the filter.
    pub fn set_channel_invert_polarity(
        &mut self,
        channel: EngineAudioChannel,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::InvertPolarity)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's bounded Limiter filter on a live mixer channel.
    ///
    /// The limiter keeps its attack/release envelope in the engine-owned
    /// filter instance, so captured blocks remain continuous without a
    /// separate runtime state store.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when threshold or release is outside the
    /// supported OBS-compatible range.
    pub fn set_channel_limiter(
        &mut self,
        channel: EngineAudioChannel,
        threshold_db_milli: i32,
        release_ms: u16,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::limiter_db_milli(
            threshold_db_milli,
            release_ms,
        )?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's bounded Compressor filter on a live mixer channel.
    ///
    /// This live-channel slice detects the channel's own signal. Sidechain
    /// compression remains unavailable until the engine has a canonical,
    /// synchronized source-to-source audio route.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when one of the compressor controls is outside
    /// the supported OBS-compatible range.
    pub fn set_channel_compressor(
        &mut self,
        channel: EngineAudioChannel,
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::compressor(
            ratio_milli,
            threshold_db_milli,
            attack_ms,
            release_ms,
            output_gain_db_milli,
        )?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's bounded peak Expander filter on a live mixer channel.
    ///
    /// This slice uses peak detection on the channel's own signal. RMS/gate,
    /// knee, sidechain, and project-source routing require a broader audio
    /// graph and remain outside this control-plane operation.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when one of the expander controls is outside
    /// the supported OBS-compatible range.
    pub fn set_channel_expander(
        &mut self,
        channel: EngineAudioChannel,
        ratio_milli: u16,
        threshold_db_milli: i32,
        attack_ms: u16,
        release_ms: u16,
        output_gain_db_milli: i32,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::expander(
            ratio_milli,
            threshold_db_milli,
            attack_ms,
            release_ms,
            output_gain_db_milli,
        )?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Installs OBS's stateful peak Noise Gate on a live mixer channel.
    ///
    /// The detector uses the channel's own peak signal. RMS detection,
    /// sidechain input, and project-source routing remain outside this
    /// control-plane operation.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when one of the gate controls is outside the
    /// supported OBS-compatible range.
    pub fn set_channel_noise_gate(
        &mut self,
        channel: EngineAudioChannel,
        open_threshold_db_milli: i32,
        close_threshold_db_milli: i32,
        attack_ms: u16,
        hold_ms: u16,
        release_ms: u16,
    ) -> Result<(), EngineError> {
        let mut filters = AudioFilterChain::new();
        filters.try_push(AudioFilter::noise_gate(
            open_threshold_db_milli,
            close_threshold_db_milli,
            attack_ms,
            hold_ms,
            release_ms,
        )?)?;
        self.set_channel_audio_filters(channel, filters);
        Ok(())
    }

    /// Mutes or unmutes the live input source.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the engine source has been removed.
    pub fn set_channel_muted(
        &mut self,
        channel: EngineAudioChannel,
        muted: bool,
    ) -> Result<(), EngineError> {
        let source = match channel {
            EngineAudioChannel::Desktop => self.desktop_audio_source,
            EngineAudioChannel::Microphone => self.microphone_audio_source,
        };
        self.mixer.set_muted(source, muted)?;
        Ok(())
    }

    /// Sets the OBS-compatible monitor destination policy for one live channel.
    ///
    /// The mixer owns this routing state so the output and local monitor buses
    /// cannot drift apart between the engine and a frontend. A monitor sink is
    /// optional; when no sink is configured, the monitor bus is simply not
    /// submitted anywhere.
    pub fn set_channel_monitor_mode(
        &mut self,
        channel: EngineAudioChannel,
        mode: AudioMonitorMode,
    ) -> Result<(), EngineError> {
        let source = match channel {
            EngineAudioChannel::Desktop => {
                self.config.desktop_monitor_mode = mode;
                self.desktop_audio_source
            }
            EngineAudioChannel::Microphone => {
                self.config.microphone_monitor_mode = mode;
                self.microphone_audio_source
            }
        };
        self.mixer.set_monitor_mode(source, mode)?;
        Ok(())
    }

    /// Selects or clears the asynchronous local monitor output sink.
    ///
    /// A replacement worker is spawned before the previous one is cancelled,
    /// so a thread-creation failure leaves the current sink intact. Device
    /// opening itself remains asynchronous and is reported through
    /// [`EngineSnapshot::monitor_output`].
    pub fn set_monitor_output_id(&mut self, device_id: Option<&str>) -> Result<(), EngineError> {
        let device_id = device_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let replacement = if let Some(device_id) = device_id.as_deref() {
            Some(
                AudioOutputWorker::spawn(
                    Arc::clone(&self.config.audio_output_provider),
                    device_id,
                    self.config.audio_format,
                    self.config.monitor_output_queue_blocks,
                )
                .map_err(|error| EngineError::InvalidConfiguration(error.to_string()))?,
            )
        } else {
            None
        };
        let replacement_handle = replacement.as_ref().map(AudioOutputWorker::handle);
        self.monitor_output_worker = replacement;
        self.monitor_output_handle = replacement_handle;
        self.config.monitor_output_id = device_id;
        Ok(())
    }

    /// Rebuilds the audio clock, mixer, device inputs, and optional monitor
    /// sink for a new format while the output is idle.
    ///
    /// Audio format changes alter packet caps and device negotiation, so they
    /// cannot be applied underneath an active recording, stream, or replay
    /// buffer. The worker owns this control-plane operation; the UI only sends
    /// the validated format and never touches device resources directly.
    pub fn set_audio_format(&mut self, audio_format: AudioFormat) -> Result<(), EngineError> {
        if self.config.audio_format == audio_format {
            return Ok(());
        }
        if self.is_recording() || self.is_streaming() || self.is_replay_buffer_active() {
            return Err(EngineError::Busy("change the audio format"));
        }

        let audio_input_delay = AudioDelayLine::with_block_frames(
            audio_format,
            self.config.audio_input_sync_offset_millis,
            self.config.audio_block_frames,
        )?;
        let desktop_audio_delay = AudioDelayLine::with_block_frames(
            audio_format,
            self.config.desktop_audio_sync_offset_millis,
            self.config.audio_block_frames,
        )?;
        let (audio_input, audio_backend, audio_fallback, audio_active_device_id) = open_audio_input(
            &self.config.audio_provider,
            audio_format,
            self.config.audio_input_id.as_deref(),
        );
        let (desktop_audio, desktop_audio_backend, desktop_audio_active_device_id) =
            open_desktop_audio(
                &self.config.audio_provider,
                audio_format,
                self.config.desktop_audio_id.as_deref(),
            );
        let (monitor_output_worker, monitor_output_handle) =
            if let Some(device_id) = self.config.monitor_output_id.as_deref() {
                let worker = AudioOutputWorker::spawn(
                    Arc::clone(&self.config.audio_output_provider),
                    device_id,
                    audio_format,
                    self.config.monitor_output_queue_blocks,
                )
                .map_err(|error| EngineError::InvalidConfiguration(error.to_string()))?;
                let handle = worker.handle();
                (Some(worker), Some(handle))
            } else {
                (None, None)
            };

        self.audio_input.stop();
        if let Some(desktop_audio_input) = self.desktop_audio.as_mut() {
            desktop_audio_input.stop();
        }
        self.audio_input = audio_input;
        self.audio_backend = audio_backend;
        self.audio_fallback = audio_fallback;
        self.audio_active_device_id = audio_active_device_id;
        self.audio_input_delay = audio_input_delay;
        self.desktop_audio = desktop_audio;
        self.desktop_audio_backend = desktop_audio_backend;
        self.desktop_audio_active_device_id = desktop_audio_active_device_id;
        self.desktop_audio_delay = desktop_audio_delay;
        self.mixer.set_format(audio_format);
        self.timeline = MediaTimeline::new(
            self.format.frame_rate(),
            audio_format,
            self.config.timeline_tolerance_nanos,
        );
        self.audio_encoder = RawAudioEncoder::new(audio_format);
        self.monitor_output_worker = monitor_output_worker;
        self.monitor_output_handle = monitor_output_handle;
        self.config.audio_format = audio_format;
        self.next_audio_deadline = None;
        self.invalidate_audio_route_requests();
        self.last_error = None;
        Ok(())
    }

    /// Switches the live audio input without rebuilding the video runtime.
    ///
    /// The provider is queried on the engine worker thread. If the requested
    /// device is unavailable, the deterministic fallback is used without
    /// silently selecting another device. A configured unavailable device, or
    /// a lost automatic route, is retried at a bounded media-time interval.
    pub fn set_audio_input_id(&mut self, device_id: Option<&str>) {
        let device_id = device_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        self.audio_input.stop();
        self.audio_input_delay.reset();
        let (audio_input, audio_backend, audio_fallback, audio_active_device_id) = open_audio_input(
            &self.config.audio_provider,
            self.config.audio_format,
            device_id.as_deref(),
        );
        self.audio_input = audio_input;
        self.audio_backend = audio_backend;
        self.audio_fallback = audio_fallback;
        self.audio_active_device_id = audio_active_device_id;
        self.config.audio_input_id = device_id;
        self.next_audio_deadline = None;
        self.invalidate_audio_route_requests();
        self.last_error = None;
    }

    /// Switches the playback monitor feeding the desktop channel.
    ///
    /// Unlike the microphone there is no fallback signal, so an unavailable
    /// device leaves the channel silent and names the reason in the snapshot.
    /// A configured unavailable device, or a lost automatic route, is retried
    /// at a bounded media-time interval without blocking the UI or silently
    /// substituting a route during an explicit selection.
    pub fn set_desktop_audio_id(&mut self, device_id: Option<&str>) {
        let device_id = device_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if let Some(desktop) = self.desktop_audio.as_mut() {
            desktop.stop();
        }
        self.desktop_audio_delay.reset();
        let (desktop_audio, desktop_audio_backend, desktop_audio_active_device_id) =
            open_desktop_audio(
                &self.config.audio_provider,
                self.config.audio_format,
                device_id.as_deref(),
            );
        self.desktop_audio = desktop_audio;
        self.desktop_audio_backend = desktop_audio_backend;
        self.desktop_audio_active_device_id = desktop_audio_active_device_id;
        self.config.desktop_audio_id = device_id;
        self.next_audio_deadline = None;
        self.invalidate_audio_route_requests();
    }
}
