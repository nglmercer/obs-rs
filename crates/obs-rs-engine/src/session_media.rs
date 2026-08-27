use obs_rs_audio::AudioBuffer;
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFrame};

use super::{audio_peak_milli, EngineError, EngineSession, EngineTick};

#[allow(
    clippy::missing_errors_doc,
    reason = "media clock methods share the documented EngineError boundary"
)]
impl EngineSession {
    /// Renders one scene using the session's independent preview clock.
    pub fn render_scene(&mut self, scene: &str) -> Result<Option<VideoFrame>, EngineError> {
        let timestamp = self.render_timestamp;
        let frame = self.render_scene_at(scene, timestamp)?;
        let period = self
            .format
            .frame_rate()
            .period_nanos()
            .unwrap_or(33_333_333);
        self.render_timestamp = timestamp.checked_add(period).unwrap_or(Timestamp::ZERO);
        Ok(frame)
    }

    /// Advances one video deadline and enough audio deadlines to keep packet
    /// timestamps monotonic in the output container.
    pub fn tick(
        &mut self,
        preview_scene: Option<&str>,
        program_scene: Option<&str>,
    ) -> Result<EngineTick, EngineError> {
        let video_deadline = self.timeline.next_video_frame()?;
        let timestamp = video_deadline.timestamp();
        let audio_blocks = self.drain_audio_until(timestamp)?;

        let preview_frame = preview_scene
            .map(|scene| self.render_scene_at(scene, timestamp))
            .transpose()?;
        let program_frame = program_scene
            .map(|scene| self.render_scene_at(scene, timestamp))
            .transpose()?;
        let program_frame = program_frame.flatten();
        let preview_frame = preview_frame.flatten();

        for audio in &audio_blocks {
            self.dispatch_audio(audio)?;
        }
        if let Some(frame) = program_frame.as_ref() {
            self.dispatch_video(frame)?;
            self.stats.video_frames = self.stats.video_frames.saturating_add(1);
            self.stats.last_video_timestamp = Some(frame.timestamp());
        }
        self.stats.ticks = self.stats.ticks.saturating_add(1);
        self.stats.audio_blocks_per_video_tick =
            u32::try_from(audio_blocks.len()).unwrap_or(u32::MAX);
        self.stats.audio_peak_milli = audio_blocks.last().map_or(0, audio_peak_milli);
        self.observe_av_sync(
            timestamp,
            audio_blocks
                .last()
                .map_or(timestamp, AudioBuffer::timestamp),
        );

        Ok(EngineTick {
            preview_frame,
            program_frame,
            audio_blocks,
            timestamp,
            audio_peak_milli: self.stats.audio_peak_milli,
        })
    }

    /// Encodes and queues a program frame rendered by an external preview
    /// adapter, adding every audio block due before its timestamp.
    pub fn push_program_frame(&mut self, frame: &VideoFrame) -> Result<(), EngineError> {
        if frame.format() != self.format {
            return Err(EngineError::InvalidConfiguration(
                "program frame format does not match the output canvas".to_owned(),
            ));
        }
        if self
            .stats
            .last_video_timestamp
            .is_some_and(|last| frame.timestamp() < last)
        {
            return Err(EngineError::InvalidConfiguration(
                "program frame timestamp moved backwards".to_owned(),
            ));
        }
        let audio_blocks = self.drain_audio_until(frame.timestamp())?;
        for audio in &audio_blocks {
            self.dispatch_audio(audio)?;
        }
        self.dispatch_video(frame)?;
        self.stats.ticks = self.stats.ticks.saturating_add(1);
        self.stats.audio_blocks_per_video_tick =
            u32::try_from(audio_blocks.len()).unwrap_or(u32::MAX);
        self.stats.video_frames = self.stats.video_frames.saturating_add(1);
        self.stats.last_video_timestamp = Some(frame.timestamp());
        self.stats.audio_peak_milli = audio_blocks.last().map_or(0, audio_peak_milli);
        self.observe_av_sync(
            frame.timestamp(),
            audio_blocks
                .last()
                .map_or(frame.timestamp(), AudioBuffer::timestamp),
        );
        Ok(())
    }

    /// Queues a validated packed/planar program frame from an accelerated
    /// compositor and schedules audio against its timestamp.
    pub fn push_program_raw_frame(&mut self, frame: &RawVideoFrame) -> Result<(), EngineError> {
        if frame.format() != self.format {
            return Err(EngineError::InvalidConfiguration(
                "program frame format does not match the output canvas".to_owned(),
            ));
        }
        if self
            .stats
            .last_video_timestamp
            .is_some_and(|last| frame.timestamp() < last)
        {
            return Err(EngineError::InvalidConfiguration(
                "program frame timestamp moved backwards".to_owned(),
            ));
        }
        let audio_blocks = self.drain_audio_until(frame.timestamp())?;
        for audio in &audio_blocks {
            self.dispatch_audio(audio)?;
        }
        self.dispatch_raw_video(frame)?;
        self.stats.ticks = self.stats.ticks.saturating_add(1);
        self.stats.audio_blocks_per_video_tick =
            u32::try_from(audio_blocks.len()).unwrap_or(u32::MAX);
        self.stats.video_frames = self.stats.video_frames.saturating_add(1);
        self.stats.last_video_timestamp = Some(frame.timestamp());
        self.observe_av_sync(
            frame.timestamp(),
            audio_blocks
                .last()
                .map_or(frame.timestamp(), AudioBuffer::timestamp),
        );
        Ok(())
    }

    /// Samples and mixes audio up to a preview timestamp without encoding or
    /// emitting media. Frontends use this while outputs are idle so their
    /// mixer meters stay live without paying for an idle video encode.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when capture, mixing, or timeline advancement
    /// fails.
    pub fn monitor_audio_until(&mut self, timestamp: Timestamp) -> Result<(), EngineError> {
        let audio_blocks = self.drain_audio_until(timestamp)?;
        if let Some(latest) = audio_blocks.last() {
            self.stats.audio_peak_milli = audio_peak_milli(latest);
            self.stats.last_audio_timestamp = Some(latest.timestamp());
        }
        Ok(())
    }
}
