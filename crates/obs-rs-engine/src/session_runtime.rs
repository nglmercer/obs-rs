use std::time::Instant;

use obs_rs_audio::AudioInputProvider;
use obs_rs_output::AudioEncoder;

use super::{
    audio::{open_live_audio_input, open_live_desktop_audio},
    audio_routes::{AudioRouteRequest, AudioRouteUpdate, ROUTE_REFRESH_INTERVAL_NANOS},
    AudioBuffer, AudioInputRequirement, EncodedPacket, EngineError, EngineSession, RawVideoFrame,
    SimulatedAudioProvider, Timestamp, VideoFrame, VideoInputRequirement, VideoRequest,
    AUDIO_RECONNECT_INTERVAL_NANOS,
};

impl EngineSession {
    pub(super) fn render_scene_at(
        &mut self,
        scene: &str,
        timestamp: Timestamp,
    ) -> Result<Option<VideoFrame>, EngineError> {
        Ok(self
            .runtime
            .render_scene(scene, &VideoRequest::new(timestamp, self.format))?)
    }

    pub(super) fn invalidate_audio_route_requests(&mut self) {
        self.audio_route_request_sequence = self.audio_route_request_sequence.saturating_add(1);
        self.audio_route_request_pending = false;
        self.audio_route_refresh_at = Timestamp::ZERO;
        while self.audio_route_worker.take_result().is_some() {}
    }

    /// Polls and schedules automatic route work without performing provider
    /// discovery or device opening on the engine/audio tick.
    fn poll_automatic_audio_routes(&mut self, timestamp: Timestamp) {
        while let Some(result) = self.audio_route_worker.take_result() {
            self.audio_route_request_pending = false;
            if result.sequence != self.audio_route_request_sequence {
                continue;
            }
            match result.microphone {
                AudioRouteUpdate::Opened(route) => {
                    self.audio_input.stop();
                    self.audio_input = route.input;
                    self.audio_backend = route.device_name;
                    self.audio_active_device_id = Some(route.device_id);
                    self.audio_fallback = false;
                    self.audio_reconnect_at = None;
                    self.audio_input_delay.reset();
                    self.last_error = None;
                }
                AudioRouteUpdate::Unavailable(reason) => {
                    let _ = reason;
                }
                AudioRouteUpdate::Unchanged => {}
            }
            match result.desktop {
                AudioRouteUpdate::Opened(route) => {
                    if let Some(desktop) = self.desktop_audio.as_mut() {
                        desktop.stop();
                    }
                    self.desktop_audio = Some(route.input);
                    self.desktop_audio_backend = route.device_name;
                    self.desktop_audio_active_device_id = Some(route.device_id);
                    self.desktop_audio_reconnect_at = None;
                    self.desktop_audio_delay.reset();
                    self.last_error = None;
                }
                AudioRouteUpdate::Unavailable(reason) => {
                    let _ = reason;
                }
                AudioRouteUpdate::Unchanged => {}
            }
        }

        // Keep automatic routes under the worker even when startup discovery
        // found no device, or when the live route has already degraded to a
        // fallback/silent state. That is what lets a microphone, playback
        // endpoint, or default-device change recover without making the
        // engine/audio tick perform native discovery or opening work.
        let watches_microphone = self.config.audio_input_id.is_none();
        let watches_desktop = self.config.desktop_audio_id.is_none();
        if (!watches_microphone && !watches_desktop)
            || timestamp < self.audio_route_refresh_at
            || self.audio_route_request_pending
        {
            return;
        }
        self.audio_route_refresh_at = timestamp
            .checked_add(ROUTE_REFRESH_INTERVAL_NANOS)
            .unwrap_or(timestamp);
        self.audio_route_request_sequence = self.audio_route_request_sequence.saturating_add(1);
        let request = AudioRouteRequest {
            sequence: self.audio_route_request_sequence,
            format: self.config.audio_format,
            microphone_requested_id: self.config.audio_input_id.clone(),
            microphone_active_id: self.audio_active_device_id.clone(),
            desktop_requested_id: self.config.desktop_audio_id.clone(),
            desktop_active_id: self.desktop_audio_active_device_id.clone(),
        };
        self.audio_route_request_pending = self.audio_route_worker.try_refresh(request);
    }

    fn read_audio_block(&mut self, timestamp: Timestamp) -> Result<AudioBuffer, EngineError> {
        // Explicit selections retain their bounded synchronous retry for
        // compatibility with the selected-route error path. Automatic routes
        // are retried by `AudioRouteWorker` so a native provider cannot block
        // the media tick while a device is absent or being re-enumerated.
        if self.audio_fallback && self.config.audio_input_id.is_some() {
            self.try_reconnect_audio(timestamp);
        }
        match self
            .audio_input
            .read_block(timestamp, self.config.audio_block_frames)
        {
            Ok(buffer) => Ok(buffer),
            Err(error) => {
                self.audio_input.stop();
                self.audio_input_delay.reset();
                self.audio_active_device_id = None;
                self.audio_fallback = true;
                self.audio_backend = format!("simulated fallback ({error})");
                self.last_error = Some(error.to_string());
                self.audio_reconnect_at = self
                    .config
                    .audio_input_id
                    .is_some()
                    .then(|| timestamp.checked_add(AUDIO_RECONNECT_INTERVAL_NANOS))
                    .flatten();
                self.audio_input = SimulatedAudioProvider::new()
                    .open_input("test-audio", self.config.audio_format)?;
                // The fallback signal runs on its own clock, so the timeline's
                // idea of the next audio deadline — computed against the real
                // device that just failed — is stale. Dropping it forces the
                // next tick to re-anchors the audio deadlines to the current
                // video timestamp instead of chasing a device that is gone.
                self.next_audio_deadline = None;
                let buffer = self
                    .audio_input
                    .read_block(timestamp, self.config.audio_block_frames)?;
                Ok(buffer)
            }
        }
    }

    fn try_reconnect_audio(&mut self, timestamp: Timestamp) {
        let Some(next_attempt) = self.audio_reconnect_at else {
            return;
        };
        if timestamp < next_attempt {
            return;
        }

        self.audio_reconnect_at = timestamp.checked_add(AUDIO_RECONNECT_INTERVAL_NANOS);
        let Some((audio_input, audio_backend, audio_active_device_id)) = open_live_audio_input(
            &self.config.audio_provider,
            self.config.audio_format,
            self.config.audio_input_id.as_deref(),
        ) else {
            return;
        };

        self.audio_input.stop();
        self.audio_input_delay.reset();
        self.audio_input = audio_input;
        self.audio_backend = audio_backend;
        self.audio_fallback = false;
        self.audio_active_device_id = Some(audio_active_device_id);
        self.audio_reconnect_at = None;
        self.next_audio_deadline = None;
        self.last_error = None;
    }

    /// Reads one desktop block, or silence when no monitor is open.
    ///
    /// A monitor that fails mid-session is closed rather than retried every
    /// block: the desktop channel degrades to silence and says so in the
    /// backend label. Explicit monitor selections are retried at a bounded
    /// media-time interval; automatic routes are reopened by the route worker,
    /// which keeps a broken device from stalling every tick.
    fn read_desktop_block(&mut self, timestamp: Timestamp) -> Result<AudioBuffer, EngineError> {
        let frames = self.config.audio_block_frames;
        // Automatic routes are reopened by `AudioRouteWorker`; only an
        // explicitly selected monitor uses the synchronous compatibility retry.
        if self.config.desktop_audio_id.is_some() {
            self.try_reconnect_desktop_audio(timestamp);
        }
        if let Some(desktop) = self.desktop_audio.as_mut() {
            match desktop.read_block(timestamp, frames) {
                Ok(buffer) => return Ok(buffer),
                Err(error) => {
                    desktop.stop();
                    self.desktop_audio = None;
                    self.desktop_audio_delay.reset();
                    self.desktop_audio_active_device_id = None;
                    self.desktop_audio_backend = format!("unavailable ({error})");
                    self.desktop_audio_reconnect_at = self
                        .config
                        .desktop_audio_id
                        .is_some()
                        .then(|| timestamp.checked_add(AUDIO_RECONNECT_INTERVAL_NANOS))
                        .flatten();
                    self.last_error = Some(error.to_string());
                }
            }
        }
        let buffer = AudioBuffer::silence(self.config.audio_format, timestamp, frames)?;
        Ok(buffer)
    }

    fn try_reconnect_desktop_audio(&mut self, timestamp: Timestamp) {
        let Some(next_attempt) = self.desktop_audio_reconnect_at else {
            return;
        };
        if timestamp < next_attempt {
            return;
        }

        self.desktop_audio_reconnect_at = timestamp.checked_add(AUDIO_RECONNECT_INTERVAL_NANOS);
        let Some((desktop_audio, desktop_audio_backend, desktop_audio_active_device_id)) =
            open_live_desktop_audio(
                &self.config.audio_provider,
                self.config.audio_format,
                self.config.desktop_audio_id.as_deref(),
            )
        else {
            return;
        };

        self.desktop_audio = Some(desktop_audio);
        self.desktop_audio_delay.reset();
        self.desktop_audio_backend = desktop_audio_backend;
        self.desktop_audio_active_device_id = Some(desktop_audio_active_device_id);
        self.desktop_audio_reconnect_at = None;
        self.last_error = None;
    }

    pub(super) fn drain_audio_until(
        &mut self,
        timestamp: Timestamp,
    ) -> Result<Vec<AudioBuffer>, EngineError> {
        self.poll_automatic_audio_routes(timestamp);
        let mut audio_blocks = Vec::new();
        while self
            .next_audio_deadline
            .is_none_or(|deadline| deadline.timestamp() <= timestamp)
        {
            let deadline = self.next_audio_deadline.take().map_or_else(
                || {
                    self.timeline
                        .next_audio_block(self.config.audio_block_frames)
                },
                Ok,
            )?;
            let mut input = self.read_audio_block(deadline.timestamp())?;
            let mut desktop = self.read_desktop_block(deadline.timestamp())?;
            self.microphone_audio_filters.apply(&mut input)?;
            self.desktop_audio_filters.apply(&mut desktop)?;
            input = self.audio_input_delay.process(input)?;
            desktop = self.desktop_audio_delay.process(desktop)?;
            let (mixed, monitor) = if self.monitor_output_handle.is_some() {
                let (output, monitor) = self.mixer.mix_buses(
                    deadline.timestamp(),
                    self.config.audio_block_frames,
                    &[
                        (self.desktop_audio_source, &desktop),
                        (self.microphone_audio_source, &input),
                    ],
                )?;
                (output, Some(monitor))
            } else {
                let output = self.mixer.mix(
                    deadline.timestamp(),
                    self.config.audio_block_frames,
                    &[
                        (self.desktop_audio_source, &desktop),
                        (self.microphone_audio_source, &input),
                    ],
                )?;
                (output, None)
            };
            if let (Some(handle), Some(monitor)) = (&self.monitor_output_handle, monitor) {
                if handle.try_write(monitor) {
                    self.stats.monitor_blocks_submitted =
                        self.stats.monitor_blocks_submitted.saturating_add(1);
                } else {
                    self.stats.monitor_blocks_dropped =
                        self.stats.monitor_blocks_dropped.saturating_add(1);
                }
            }
            self.stats.desktop_peak_milli =
                self.mixer.source_peak_milli(self.desktop_audio_source)?;
            self.stats.microphone_peak_milli =
                self.mixer.source_peak_milli(self.microphone_audio_source)?;
            self.stats.desktop_peak_hold_milli = self
                .mixer
                .source_peak_hold_milli(self.desktop_audio_source)?;
            self.stats.microphone_peak_hold_milli = self
                .mixer
                .source_peak_hold_milli(self.microphone_audio_source)?;
            self.stats.desktop_clipped = self.mixer.source_clipped(self.desktop_audio_source)?;
            self.stats.microphone_clipped =
                self.mixer.source_clipped(self.microphone_audio_source)?;
            self.stats.audio_blocks = self.stats.audio_blocks.saturating_add(1);
            if self.audio_fallback {
                self.stats.audio_fallback_blocks =
                    self.stats.audio_fallback_blocks.saturating_add(1);
            }
            self.stats.last_audio_timestamp = Some(mixed.timestamp());
            audio_blocks.push(mixed);
            self.next_audio_deadline = Some(
                self.timeline
                    .next_audio_block(self.config.audio_block_frames)?,
            );
        }
        Ok(audio_blocks)
    }

    fn emit_packet(&mut self, packet: EncodedPacket) -> Result<(), EngineError> {
        if let Some(replay_buffer) = self.replay_buffer.as_mut() {
            replay_buffer.push(packet.clone())?;
        }
        match (self.recording.as_mut(), self.streaming.as_mut()) {
            (Some(recording), Some(stream)) => {
                recording.push_packet(packet.clone())?;
                stream.submit(packet)?;
            }
            (Some(recording), None) => recording.push_packet(packet)?,
            (None, Some(stream)) => stream.submit(packet)?,
            (None, None) => {}
        }
        Ok(())
    }

    pub(super) fn dispatch_audio(&mut self, audio: &AudioBuffer) -> Result<(), EngineError> {
        if self.raw_audio_required() {
            let started = Instant::now();
            if let Some(recording) = self
                .recording
                .as_mut()
                .filter(|recording| recording.audio_requirement() == AudioInputRequirement::Raw)
            {
                recording.push_audio(audio)?;
            }
            if let Some(stream) = self
                .streaming
                .as_mut()
                .filter(|stream| stream.audio_requirement() == AudioInputRequirement::Raw)
            {
                stream.push_raw_audio(audio.clone())?;
            }
            self.stats.output_submit_latency.record(started.elapsed());
        }
        if self.packetized_audio_required() {
            #[cfg(test)]
            {
                self.reference_audio_encode_calls =
                    self.reference_audio_encode_calls.saturating_add(1);
            }
            let started = Instant::now();
            let packet = self.audio_encoder.encode(audio)?;
            self.stats.audio_encode_latency.record(started.elapsed());
            let started = Instant::now();
            self.emit_packet(packet)?;
            self.stats.output_submit_latency.record(started.elapsed());
        }
        Ok(())
    }

    pub(super) fn dispatch_video(&mut self, frame: &VideoFrame) -> Result<(), EngineError> {
        if self.raw_video_required() {
            let started = Instant::now();
            if let Some(recording) = self
                .recording
                .as_mut()
                .filter(|recording| recording.video_requirement() == VideoInputRequirement::Raw)
            {
                recording.push_video(frame)?;
            }
            if let Some(stream) = self
                .streaming
                .as_mut()
                .filter(|stream| stream.video_requirement() == VideoInputRequirement::Raw)
            {
                stream.push_video(frame.clone())?;
            }
            self.stats.output_submit_latency.record(started.elapsed());
        }
        if self.packetized_video_required() {
            #[cfg(test)]
            {
                self.reference_video_encode_calls =
                    self.reference_video_encode_calls.saturating_add(1);
            }
            let started = Instant::now();
            let packet = self.video_encoder.encode(frame)?;
            self.stats.video_encode_latency.record(started.elapsed());
            let started = Instant::now();
            self.emit_packet(packet)?;
            self.stats.output_submit_latency.record(started.elapsed());
        }
        Ok(())
    }

    pub(super) fn dispatch_raw_video(&mut self, frame: &RawVideoFrame) -> Result<(), EngineError> {
        if self.raw_video_required() {
            let started = Instant::now();
            if let Some(recording) = self
                .recording
                .as_mut()
                .filter(|recording| recording.video_requirement() == VideoInputRequirement::Raw)
            {
                recording.push_raw_video(frame)?;
            }
            if let Some(stream) = self
                .streaming
                .as_mut()
                .filter(|stream| stream.video_requirement() == VideoInputRequirement::Raw)
            {
                stream.push_raw_video(frame.clone())?;
            }
            self.stats.output_submit_latency.record(started.elapsed());
        }
        if self.packetized_video_required() {
            let rgba = frame
                .clone()
                .into_rgba8()
                .map_err(|error| EngineError::InvalidConfiguration(error.to_string()))?;
            #[cfg(test)]
            {
                self.reference_video_encode_calls =
                    self.reference_video_encode_calls.saturating_add(1);
            }
            let started = Instant::now();
            let packet = self.video_encoder.encode(&rgba)?;
            self.stats.video_encode_latency.record(started.elapsed());
            let started = Instant::now();
            self.emit_packet(packet)?;
            self.stats.output_submit_latency.record(started.elapsed());
        }
        Ok(())
    }

    fn packetized_video_required(&self) -> bool {
        self.replay_buffer.is_some()
            || self.recording.as_ref().is_some_and(|recording| {
                recording.video_requirement() == VideoInputRequirement::Packetized
            })
            || self.streaming.as_ref().is_some_and(|stream| {
                stream.video_requirement() == VideoInputRequirement::Packetized
            })
    }

    fn packetized_audio_required(&self) -> bool {
        self.replay_buffer.is_some()
            || self.recording.as_ref().is_some_and(|recording| {
                recording.audio_requirement() == AudioInputRequirement::Packetized
            })
            || self.streaming.as_ref().is_some_and(|stream| {
                stream.audio_requirement() == AudioInputRequirement::Packetized
            })
    }

    fn raw_video_required(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|recording| recording.video_requirement() == VideoInputRequirement::Raw)
            || self
                .streaming
                .as_ref()
                .is_some_and(|stream| stream.video_requirement() == VideoInputRequirement::Raw)
    }

    fn raw_audio_required(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|recording| recording.audio_requirement() == AudioInputRequirement::Raw)
            || self
                .streaming
                .as_ref()
                .is_some_and(|stream| stream.audio_requirement() == AudioInputRequirement::Raw)
    }
}

impl Drop for EngineSession {
    fn drop(&mut self) {
        self.abort_recording();
        if let Some(stream) = self.streaming.as_mut() {
            let _ = stream.close();
        }
        self.audio_input.stop();
        if let Some(desktop) = self.desktop_audio.as_mut() {
            desktop.stop();
        }
    }
}
