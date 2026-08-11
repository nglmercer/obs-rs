use std::{error::Error, sync::Arc};

use obs_rs_audio::AudioFormat;
use obs_rs_audio_pipewire::PipeWireAudioProvider;
use obs_rs_engine::{EngineConfig, EngineSession};
use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_output::StreamState;

/// GUI-owned handle over the portable engine output boundary.
pub(crate) struct OutputRuntime {
    engine: EngineSession,
}

impl OutputRuntime {
    /// Creates an output with the reference 48 kHz stereo format.
    #[cfg(test)]
    pub(crate) fn new(format: VideoFormat) -> Self {
        let audio_format = AudioFormat::new(48_000, 2).unwrap_or_else(|error| {
            unreachable!("the built-in audio format is valid: {error}")
        });
        Self::with_audio(format, audio_format).unwrap_or_else(|error| {
            unreachable!("the built-in output session is valid: {error}")
        })
    }

    /// Creates an output using the audio format selected in settings.
    pub(crate) fn with_audio(
        format: VideoFormat,
        audio_format: AudioFormat,
    ) -> Result<Self, Box<dyn Error>> {
        let config = EngineConfig::new(audio_format)
            .with_audio_provider(Arc::new(PipeWireAudioProvider::new()));
        let engine = EngineSession::for_format(format, config)?;
        Ok(Self { engine })
    }

    pub(crate) fn start_recording(&mut self, path: &str) -> Result<(), Box<dyn Error>> {
        self.engine.start_recording(path)?;
        Ok(())
    }

    pub(crate) fn finish_recording(&mut self) -> Result<usize, Box<dyn Error>> {
        Ok(self.engine.finish_recording()?)
    }

    pub(crate) fn abort_recording(&mut self) {
        self.engine.abort_recording();
    }

    pub(crate) fn start_streaming(&mut self, address: &str) -> Result<(), Box<dyn Error>> {
        self.engine.start_streaming(address)?;
        Ok(())
    }

    pub(crate) fn finish_streaming(&mut self) {
        let _ = self.engine.finish_streaming();
    }

    /// Queues a program frame and its due audio without flushing a socket.
    pub(crate) fn push_frame(&mut self, frame: &VideoFrame) -> Result<(), Box<dyn Error>> {
        self.engine.push_program_frame(frame)?;
        Ok(())
    }

    /// Pumps the bounded stream queue from the GUI timer.
    pub(crate) fn pump(&mut self) -> Result<(), Box<dyn Error>> {
        self.engine.pump_stream()?;
        Ok(())
    }

    pub(crate) fn set_input_gain_milli(&mut self, gain_milli: u16) -> Result<(), Box<dyn Error>> {
        self.engine.set_input_gain_milli(gain_milli)?;
        Ok(())
    }

    pub(crate) fn set_input_muted(&mut self, muted: bool) -> Result<(), Box<dyn Error>> {
        self.engine.set_input_muted(muted)?;
        Ok(())
    }

    pub(crate) fn output_status(&self) -> String {
        let snapshot = self.engine.snapshot();
        let recording = if snapshot.recording {
            "recording open"
        } else {
            "recording stopped"
        };
        let streaming = snapshot.stream_state.map_or("stream stopped", |state| match state {
            StreamState::Connected => "stream connected",
            StreamState::Disconnected => "stream reconnecting",
            StreamState::Failed => "stream failed",
            StreamState::Closed => "stream closed",
        });
        let audio = if snapshot.audio_fallback {
            "audio fallback"
        } else {
            "audio live"
        };
        format!("Output: {recording} · {streaming} · {audio}")
    }

    pub(crate) fn output_metrics(&self) -> String {
        let snapshot = self.engine.snapshot();
        let (sent, dropped, queued, reconnects) = self
            .engine
            .stream_metrics()
            .map_or((0, 0, 0, 0), |(metrics, queued)| {
                (
                    metrics.sent_packets(),
                    metrics.dropped_packets(),
                    queued as u64,
                    metrics.reconnects(),
                )
            });
        format!(
            "frames={} · audio_blocks={} · sent={} · dropped={} · queued={} B · reconnects={} · peak={}‰",
            snapshot.stats.video_frames,
            snapshot.stats.audio_blocks,
            sent,
            dropped,
            queued,
            reconnects,
            snapshot.stats.audio_peak_milli
        )
    }
}
