use std::{error::Error, sync::Arc};

use obs_rs_audio::AudioFormat;
use obs_rs_audio_pipewire::PipeWireAudioProvider;
use obs_rs_engine::{EngineConfig, EngineSession, EngineWorker};
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_output::StreamState;

/// GUI-owned handle over the portable engine output boundary.
pub(crate) struct OutputRuntime {
    worker: EngineWorker,
}

impl OutputRuntime {
    /// Creates an output with the reference 48 kHz stereo format.
    #[cfg(test)]
    pub(crate) fn new(format: VideoFormat) -> Self {
        let audio_format = AudioFormat::new(48_000, 2)
            .unwrap_or_else(|error| unreachable!("the built-in audio format is valid: {error}"));
        Self::with_audio(format, audio_format)
            .unwrap_or_else(|error| unreachable!("the built-in output session is valid: {error}"))
    }

    /// Creates an output using the audio format selected in settings.
    pub(crate) fn with_audio(
        format: VideoFormat,
        audio_format: AudioFormat,
    ) -> Result<Self, Box<dyn Error>> {
        let config = EngineConfig::new(audio_format)
            .with_audio_provider(Arc::new(PipeWireAudioProvider::new()));
        let engine = EngineSession::for_format(format, config)?;
        Ok(Self {
            worker: EngineWorker::spawn(engine)?,
        })
    }

    pub(crate) fn start_recording(&mut self, path: &str) -> Result<(), Box<dyn Error>> {
        self.worker.start_recording(path)?;
        Ok(())
    }

    pub(crate) fn finish_recording(&mut self) -> Result<usize, Box<dyn Error>> {
        Ok(self.worker.finish_recording()?)
    }

    pub(crate) fn abort_recording(&mut self) {
        self.worker.abort_recording();
    }

    pub(crate) fn start_streaming(&mut self, address: &str) -> Result<(), Box<dyn Error>> {
        self.worker.start_streaming(address)?;
        Ok(())
    }

    pub(crate) fn finish_streaming(&mut self) {
        self.worker.finish_streaming();
    }

    /// Enqueues a program frame and its due audio without blocking the GUI.
    pub(crate) fn push_frame(&mut self, frame: &VideoFrame) {
        // Queue pressure is observable in output_metrics; dropping an animation
        // frame is preferable to stalling scene editing or preview rendering.
        let _ = self.worker.try_push_frame(frame.clone());
    }

    pub(crate) fn set_input_gain_milli(&mut self, gain_milli: u16) -> Result<(), Box<dyn Error>> {
        self.worker.set_input_gain_milli(gain_milli)?;
        Ok(())
    }

    pub(crate) fn set_input_muted(&mut self, muted: bool) -> Result<(), Box<dyn Error>> {
        self.worker.set_input_muted(muted)?;
        Ok(())
    }

    pub(crate) fn output_status(&self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        let recording = if engine.recording {
            "recording open"
        } else {
            "recording stopped"
        };
        let streaming = engine
            .stream_state
            .map_or("stream stopped", |state| match state {
                StreamState::Connected => "stream connected",
                StreamState::Disconnected => "stream reconnecting",
                StreamState::Failed => "stream failed",
                StreamState::Closed => "stream closed",
            });
        let audio = if engine.audio_fallback {
            "audio fallback"
        } else {
            "audio live"
        };
        let worker = if snapshot.alive {
            "worker live"
        } else {
            "worker stopped"
        };
        format!("Output: {recording} · {streaming} · {audio} · {worker}")
    }

    pub(crate) fn output_metrics(&self) -> String {
        let snapshot = self.worker.snapshot();
        let engine = snapshot.engine;
        let (sent, dropped, reconnects) = engine.stream_metrics.map_or((0, 0, 0), |metrics| {
            (
                metrics.sent_packets(),
                metrics.dropped_packets(),
                metrics.reconnects(),
            )
        });
        format!(
            "frames={} · audio_blocks={} · sent={} · dropped={} · queued={} B · reconnects={} · frame_drops={} · peak={}‰",
            engine.stats.video_frames,
            engine.stats.audio_blocks,
            sent,
            dropped,
            engine.stream_queued_bytes,
            reconnects,
            snapshot.dropped_frames,
            engine.stats.audio_peak_milli
        )
    }

    pub(crate) fn stream_failed(&self) -> bool {
        let snapshot = self.worker.snapshot();
        !snapshot.alive || snapshot.engine.stream_state == Some(StreamState::Failed)
    }
}
