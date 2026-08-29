use std::error::Error;

use obs_rs_engine::OutputEvent;
use obs_rs_output::{
    AudioCodec, AudioEncoderConfig, EncoderImplementation, StreamProtocol, VideoCodec,
    VideoEncoderConfig,
};

use crate::AppSettings;

use super::OutputRuntime;

impl OutputRuntime {
    #[cfg(test)]
    pub(crate) fn start_streaming(&mut self, address: &str) -> Result<(), Box<dyn Error>> {
        self.worker.start_streaming(address)?;
        self.stream_protocol = Some(stream_protocol_label(address));
        Ok(())
    }

    pub(crate) fn configure_stream(&mut self, settings: &AppSettings) {
        self.configured_stream = settings.stream_target();
        // The recording encoder is derived from the quality preset at the
        // encoded geometry, so a preset means the same thing to the encoder as
        // it does on the Output page.
        let codec = settings.effective_recording_codec();
        self.recording_video_encoder = VideoEncoderConfig {
            implementation: self
                .capabilities
                .video_encoders()
                .iter()
                .find(|encoder| encoder.codec() == codec)
                .map_or_else(EncoderImplementation::default, |encoder| {
                    EncoderImplementation::new(encoder.id())
                }),
            ..settings.recording_video_encoder(self.output_format)
        };
        self.recording_audio_encoder = AudioEncoderConfig {
            implementation: if settings.recording_audio_encoder.is_automatic() {
                self.capabilities
                    .audio_encoders()
                    .iter()
                    .find(|encoder| encoder.codec() == AudioCodec::Aac)
                    .map_or_else(EncoderImplementation::default, |encoder| {
                        EncoderImplementation::new(encoder.id())
                    })
            } else {
                settings.recording_audio_encoder.clone()
            },
            ..settings.recording_audio_encoder_config()
        };
        self.configured_video_encoder = settings.rtmp.video.clone();
        self.configured_audio_encoder = settings.rtmp.audio.clone();
        if settings.stream_protocol == StreamProtocol::Whip {
            self.configured_video_encoder.codec = VideoCodec::Vp8;
            self.configured_video_encoder.implementation = EncoderImplementation::default();
            self.configured_video_encoder.profile = None;
            self.configured_audio_encoder.codec = AudioCodec::Opus;
            self.configured_audio_encoder.implementation = EncoderImplementation::default();
        }
        if self.configured_video_encoder.implementation.is_automatic() {
            if let Some(encoder) = self
                .capabilities
                .video_encoders()
                .iter()
                .find(|encoder| encoder.codec() == self.configured_video_encoder.codec)
            {
                self.configured_video_encoder.implementation =
                    obs_rs_output::EncoderImplementation::new(encoder.id());
            }
        }
        if self.configured_audio_encoder.implementation.is_automatic() {
            if let Some(encoder) = self
                .capabilities
                .audio_encoders()
                .iter()
                .find(|encoder| encoder.codec() == self.configured_audio_encoder.codec)
            {
                self.configured_audio_encoder.implementation =
                    obs_rs_output::EncoderImplementation::new(encoder.id());
            }
        }
    }

    pub(crate) fn start_configured_stream(&mut self) -> Result<&'static str, Box<dyn Error>> {
        let protocol = stream_protocol_name(self.configured_stream.protocol());
        self.worker.start_streaming_target_configured(
            self.configured_stream.clone(),
            self.configured_video_encoder.clone(),
            self.configured_audio_encoder.clone(),
        )?;
        self.stream_protocol = Some(protocol);
        Ok(protocol)
    }

    pub(crate) fn finish_streaming(&mut self) -> Result<(), Box<dyn Error>> {
        self.worker.finish_streaming()?;
        Ok(())
    }

    pub(crate) fn take_output_events(&mut self) -> Vec<OutputEvent> {
        let events = self.worker.take_output_events();
        if events
            .iter()
            .any(|event| matches!(event, OutputEvent::Stopped | OutputEvent::Failed { .. }))
        {
            self.stream_protocol = None;
        }
        events
    }
}

const fn stream_protocol_name(protocol: StreamProtocol) -> &'static str {
    match protocol {
        StreamProtocol::Rtmp => "RTMP",
        StreamProtocol::Rtmps => "RTMPS",
        StreamProtocol::Srt => "SRT",
        StreamProtocol::Whip => "WHIP",
        StreamProtocol::Hls => "HLS",
        StreamProtocol::Rist => "RIST",
        StreamProtocol::Reference => "Reference",
    }
}

#[cfg(test)]
pub(crate) fn stream_protocol_label(address: &str) -> &'static str {
    let scheme = address.trim().split(':').next().unwrap_or_default();
    if scheme.eq_ignore_ascii_case("srt") {
        "SRT"
    } else if scheme.eq_ignore_ascii_case("rtmp") {
        "RTMP"
    } else if scheme.eq_ignore_ascii_case("rtmps") {
        "RTMPS"
    } else if scheme.eq_ignore_ascii_case("rist") {
        "RIST"
    } else if scheme.eq_ignore_ascii_case("whip") || scheme.eq_ignore_ascii_case("webrtc") {
        "WHIP"
    } else if scheme.eq_ignore_ascii_case("hls") {
        "HLS"
    } else if scheme.eq_ignore_ascii_case("ws") || scheme.eq_ignore_ascii_case("wss") {
        "OBSR-WebSocket"
    } else {
        "OBSR-TCP"
    }
}
