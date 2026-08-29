use obs_rs_audio::{AudioBuffer, AudioFormat};
use obs_rs_media::{RawVideoFrame, VideoFormat, VideoFrame};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output::StreamTarget;
use obs_rs_output::{
    AtomicPacketFileWriter, AudioEncoderConfig, AudioInputRequirement, EncodedPacket,
    PacketDropPolicy, ReconnectOutcome, ReconnectPolicy, SegmentedPacketFileWriter, StreamMetrics,
    StreamSession, StreamState, StreamingTransport, TcpPacketTransport, VideoEncoderConfig,
    VideoInputRequirement, WebSocketPacketTransport,
};
#[cfg(feature = "production-gstreamer")]
use obs_rs_output_gstreamer::{
    GStreamerCapabilitySnapshot, GStreamerOutputSession, NativeOutputState, ProductionDestination,
    ProductionPipelinePlan,
};

use super::{EngineError, ProductionStreamMetrics};

pub(super) enum RecordingOutput {
    Reference(AtomicPacketFileWriter),
    SegmentedReference(SegmentedPacketFileWriter),
    #[cfg(feature = "production-gstreamer")]
    Production {
        session: GStreamerOutputSession,
    },
}

impl RecordingOutput {
    pub(super) const fn video_requirement(&self) -> VideoInputRequirement {
        match self {
            Self::Reference(_) | Self::SegmentedReference(_) => VideoInputRequirement::Packetized,
            #[cfg(feature = "production-gstreamer")]
            Self::Production { .. } => VideoInputRequirement::Raw,
        }
    }

    pub(super) const fn audio_requirement(&self) -> AudioInputRequirement {
        match self {
            Self::Reference(_) | Self::SegmentedReference(_) => AudioInputRequirement::Packetized,
            #[cfg(feature = "production-gstreamer")]
            Self::Production { .. } => AudioInputRequirement::Raw,
        }
    }

    pub(super) fn push_packet(&mut self, packet: EncodedPacket) -> Result<(), EngineError> {
        match self {
            Self::Reference(writer) => writer.push(packet).map_err(Into::into),
            Self::SegmentedReference(writer) => writer.push(packet).map_err(Into::into),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { .. } => Ok(()),
        }
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    pub(super) fn push_video(&mut self, frame: &VideoFrame) -> Result<(), EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = frame;
        match self {
            Self::Reference(_) | Self::SegmentedReference(_) => Ok(()),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { session, .. } => {
                session.push_video(frame.clone()).map_err(Into::into)
            }
        }
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    pub(super) fn push_raw_video(&mut self, frame: &RawVideoFrame) -> Result<(), EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = frame;
        match self {
            Self::Reference(_) | Self::SegmentedReference(_) => Ok(()),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { session, .. } => {
                session.push_raw_video(frame.clone()).map_err(Into::into)
            }
        }
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    pub(super) fn push_audio(&mut self, buffer: &AudioBuffer) -> Result<(), EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = buffer;
        match self {
            Self::Reference(_) | Self::SegmentedReference(_) => Ok(()),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { session, .. } => {
                session.push_audio(buffer.clone()).map_err(Into::into)
            }
        }
    }

    pub(super) fn finalize(&mut self) -> Result<usize, EngineError> {
        match self {
            Self::Reference(writer) => writer.finalize().map_err(Into::into),
            Self::SegmentedReference(writer) => writer.finalize().map_err(Into::into),
            #[cfg(feature = "production-gstreamer")]
            Self::Production { session } => {
                session.close()?;
                session.committed_bytes().ok_or_else(|| {
                    EngineError::InvalidConfiguration(
                        "native recording did not report committed bytes".to_owned(),
                    )
                })
            }
        }
    }

    pub(super) fn abort(&mut self) {
        match self {
            Self::Reference(writer) => {
                let _ = writer.abort();
            }
            Self::SegmentedReference(writer) => {
                let _ = writer.abort();
            }
            #[cfg(feature = "production-gstreamer")]
            Self::Production { .. } => {}
        }
    }
}

pub(super) enum StreamOutput {
    Tcp(StreamSession<TcpPacketTransport>),
    WebSocket(StreamSession<WebSocketPacketTransport>),
    #[cfg(feature = "production-gstreamer")]
    Production(GStreamerOutputSession),
}

impl StreamOutput {
    pub(super) const fn video_requirement(&self) -> VideoInputRequirement {
        match self {
            Self::Tcp(_) | Self::WebSocket(_) => VideoInputRequirement::Packetized,
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => VideoInputRequirement::Raw,
        }
    }

    pub(super) const fn audio_requirement(&self) -> AudioInputRequirement {
        match self {
            Self::Tcp(_) | Self::WebSocket(_) => AudioInputRequirement::Packetized,
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => AudioInputRequirement::Raw,
        }
    }

    pub(super) fn connect(
        address: &str,
        capacity_bytes: usize,
        reconnect_attempts: u32,
        video_format: VideoFormat,
        audio_format: AudioFormat,
        encoder_config: Option<(&VideoEncoderConfig, &AudioEncoderConfig)>,
    ) -> Result<Self, EngineError> {
        #[cfg(not(feature = "production-gstreamer"))]
        let _ = (video_format, audio_format, encoder_config);
        let address = address.trim();
        if address.is_empty() {
            return Err(EngineError::InvalidConfiguration(
                "stream address is empty".to_owned(),
            ));
        }
        let scheme = address
            .split(':')
            .next()
            .map(|value| value.to_ascii_lowercase());
        let production_scheme = scheme.as_deref().is_some_and(|scheme| {
            matches!(
                scheme,
                "rtmp" | "rtmps" | "srt" | "rist" | "whip" | "webrtc"
            )
        });
        #[cfg(feature = "production-gstreamer")]
        if production_scheme {
            let (profile, destination) = ProductionDestination::from_stream_endpoint(address)?;
            let capabilities = GStreamerCapabilitySnapshot::probe_cached();
            let plan = encoder_config.map_or_else(
                || ProductionPipelinePlan::negotiate(profile, &destination, &capabilities),
                |(video, audio)| {
                    ProductionPipelinePlan::negotiate_configured(
                        profile,
                        &destination,
                        &capabilities,
                        video,
                        audio,
                    )
                },
            )?;
            return Ok(Self::Production(
                GStreamerOutputSession::start_with_reconnect_limit(
                    &plan,
                    &destination,
                    video_format,
                    audio_format,
                    reconnect_attempts,
                )?,
            ));
        }
        #[cfg(not(feature = "production-gstreamer"))]
        if production_scheme {
            return Err(EngineError::InvalidConfiguration(
                "SRT/RTMP/RTMPS/RIST/WHIP support was not compiled into this host".to_owned(),
            ));
        }
        let policy = ReconnectPolicy::new(reconnect_attempts);
        if address.starts_with("ws://") || address.starts_with("wss://") {
            let mut stream = StreamSession::new(
                WebSocketPacketTransport::new(address),
                capacity_bytes,
                PacketDropPolicy::DropNewest,
                policy,
            )?;
            stream.connect()?;
            Ok(Self::WebSocket(stream))
        } else {
            let mut stream = StreamSession::new(
                TcpPacketTransport::new(address),
                capacity_bytes,
                PacketDropPolicy::DropNewest,
                policy,
            )?;
            stream.connect()?;
            Ok(Self::Tcp(stream))
        }
    }

    #[cfg(feature = "production-gstreamer")]
    pub(super) fn connect_target(
        target: &StreamTarget,
        capacity_bytes: usize,
        reconnect_attempts: u32,
        video_format: VideoFormat,
        audio_format: AudioFormat,
        video: &VideoEncoderConfig,
        audio: &AudioEncoderConfig,
    ) -> Result<Self, EngineError> {
        if let StreamTarget::Reference { address } = target {
            return Self::connect(
                address,
                capacity_bytes,
                reconnect_attempts,
                video_format,
                audio_format,
                Some((video, audio)),
            );
        }
        let (profile, destination) = ProductionDestination::from_stream_target(target)?;
        let capabilities = GStreamerCapabilitySnapshot::probe_cached();
        let plan = ProductionPipelinePlan::negotiate_configured(
            profile,
            &destination,
            &capabilities,
            video,
            audio,
        )?;
        Ok(Self::Production(
            GStreamerOutputSession::start_with_reconnect_limit(
                &plan,
                &destination,
                video_format,
                audio_format,
                reconnect_attempts,
            )?,
        ))
    }

    pub(super) fn submit(&mut self, packet: EncodedPacket) -> Result<(), EngineError> {
        match self {
            Self::Tcp(stream) => {
                stream.submit(packet)?;
            }
            Self::WebSocket(stream) => {
                stream.submit(packet)?;
            }
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => {}
        }
        Ok(())
    }

    pub(super) fn pump(&mut self) -> Result<usize, EngineError> {
        match self {
            Self::Tcp(stream) => Ok(StreamingTransport::poll(stream)?),
            Self::WebSocket(stream) => Ok(StreamingTransport::poll(stream)?),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(stream) => Ok(StreamingTransport::poll(stream)?),
        }
    }

    pub(super) fn reconnect(&mut self) -> Result<ReconnectOutcome, EngineError> {
        match self {
            Self::Tcp(stream) => Ok(StreamingTransport::reconnect(stream)?),
            Self::WebSocket(stream) => Ok(StreamingTransport::reconnect(stream)?),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(stream) => Ok(StreamingTransport::reconnect(stream)?),
        }
    }

    pub(super) fn state(&self) -> StreamState {
        match self {
            Self::Tcp(stream) => stream.state(),
            Self::WebSocket(stream) => stream.state(),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(stream) => match stream.state() {
                NativeOutputState::Opening
                | NativeOutputState::Retrying
                | NativeOutputState::Lost => StreamState::Disconnected,
                NativeOutputState::Ready => StreamState::Connected,
                NativeOutputState::Failed => StreamState::Failed,
                NativeOutputState::Closed => StreamState::Closed,
            },
        }
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    pub(super) fn close(&mut self) -> Result<(), EngineError> {
        match self {
            Self::Tcp(stream) => StreamingTransport::close(stream)?,
            Self::WebSocket(stream) => StreamingTransport::close(stream)?,
            #[cfg(feature = "production-gstreamer")]
            Self::Production(stream) => StreamingTransport::close(stream)?,
        }
        Ok(())
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(clippy::unnecessary_wraps)
    )]
    pub(super) fn metrics(&self) -> Option<StreamMetrics> {
        match self {
            Self::Tcp(stream) => Some(stream.metrics()),
            Self::WebSocket(stream) => Some(stream.metrics()),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => None,
        }
    }

    pub(super) fn queued_bytes(&self) -> usize {
        match self {
            Self::Tcp(stream) => stream.queued_bytes(),
            Self::WebSocket(stream) => stream.queued_bytes(),
            #[cfg(feature = "production-gstreamer")]
            Self::Production(_) => 0,
        }
    }

    #[cfg_attr(not(feature = "production-gstreamer"), allow(clippy::unused_self))]
    pub(super) fn production_metrics(&self) -> Option<ProductionStreamMetrics> {
        #[cfg(feature = "production-gstreamer")]
        if let Self::Production(stream) = self {
            let telemetry = stream.telemetry();
            return Some(ProductionStreamMetrics {
                video_submitted: telemetry.video_submitted(),
                audio_submitted: telemetry.audio_submitted(),
                dropped: telemetry.dropped(),
                reconnects: telemetry.reconnects(),
                video_queue_bytes: telemetry.video_queue_bytes(),
                audio_queue_bytes: telemetry.audio_queue_bytes(),
                max_submit_latency_nanos: telemetry.max_submit_latency_nanos(),
            });
        }
        None
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(
            clippy::needless_pass_by_value,
            clippy::unnecessary_wraps,
            clippy::unused_self,
            unused_variables
        )
    )]
    pub(super) fn push_raw_audio(&mut self, buffer: AudioBuffer) -> Result<(), EngineError> {
        #[cfg(feature = "production-gstreamer")]
        if let Self::Production(stream) = self {
            stream.push_audio(buffer)?;
        }
        Ok(())
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(
            clippy::needless_pass_by_value,
            clippy::unnecessary_wraps,
            clippy::unused_self,
            unused_variables
        )
    )]
    pub(super) fn push_raw_video(&mut self, frame: RawVideoFrame) -> Result<(), EngineError> {
        #[cfg(feature = "production-gstreamer")]
        if let Self::Production(stream) = self {
            stream.push_raw_video(frame)?;
        }
        Ok(())
    }

    #[cfg_attr(
        not(feature = "production-gstreamer"),
        allow(
            clippy::needless_pass_by_value,
            clippy::unnecessary_wraps,
            clippy::unused_self,
            unused_variables
        )
    )]
    pub(super) fn push_video(&mut self, frame: VideoFrame) -> Result<(), EngineError> {
        #[cfg(feature = "production-gstreamer")]
        if let Self::Production(stream) = self {
            stream.push_video(frame)?;
        }
        Ok(())
    }
}
