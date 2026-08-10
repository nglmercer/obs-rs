use std::{error::Error, path::PathBuf};

use obs_rs_media::{VideoFormat, VideoFrame};
use obs_rs_output::{
    AtomicY4mFileWriter, PacketDropPolicy, ReconnectPolicy, RleVideoEncoder, StreamSession,
    StreamState, TcpPacketTransport, VideoEncoder, WebSocketPacketTransport,
};

pub(crate) struct OutputRuntime {
    pub(crate) format: VideoFormat,
    recording: Option<AtomicY4mFileWriter>,
    streaming: Option<StreamOutput>,
    encoder: RleVideoEncoder,
    frames_pushed: u64,
}

enum StreamOutput {
    Tcp(StreamSession<TcpPacketTransport>),
    WebSocket(StreamSession<WebSocketPacketTransport>),
}

impl StreamOutput {
    fn connect(address: &str) -> Result<Self, Box<dyn Error>> {
        let address = address.trim();
        if address.starts_with("ws://") {
            let mut stream = StreamSession::new(
                WebSocketPacketTransport::new(address),
                8 * 1024 * 1024,
                PacketDropPolicy::DropNewest,
                ReconnectPolicy::new(3),
            )?;
            stream.connect()?;
            Ok(Self::WebSocket(stream))
        } else {
            let mut stream = StreamSession::new(
                TcpPacketTransport::new(address),
                8 * 1024 * 1024,
                PacketDropPolicy::DropNewest,
                ReconnectPolicy::new(3),
            )?;
            stream.connect()?;
            Ok(Self::Tcp(stream))
        }
    }

    fn state(&self) -> StreamState {
        match self {
            Self::Tcp(stream) => stream.state(),
            Self::WebSocket(stream) => stream.state(),
        }
    }

    fn reconnect(&mut self) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Tcp(stream) => stream.reconnect()?,
            Self::WebSocket(stream) => stream.reconnect()?,
        }
        Ok(())
    }

    fn submit(&mut self, packet: obs_rs_output::EncodedPacket) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Tcp(stream) => {
                stream.submit(packet)?;
            }
            Self::WebSocket(stream) => {
                stream.submit(packet)?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<usize, Box<dyn Error>> {
        match self {
            Self::Tcp(stream) => Ok(stream.flush()?),
            Self::WebSocket(stream) => Ok(stream.flush()?),
        }
    }

    fn close(&mut self) {
        match self {
            Self::Tcp(stream) => stream.close(),
            Self::WebSocket(stream) => stream.close(),
        }
    }

    fn queued_bytes(&self) -> usize {
        match self {
            Self::Tcp(stream) => stream.queued_bytes(),
            Self::WebSocket(stream) => stream.queued_bytes(),
        }
    }

    fn metrics(&self) -> obs_rs_output::StreamMetrics {
        match self {
            Self::Tcp(stream) => stream.metrics(),
            Self::WebSocket(stream) => stream.metrics(),
        }
    }
}

impl OutputRuntime {
    pub(crate) fn new(format: VideoFormat) -> Self {
        Self {
            format,
            recording: None,
            streaming: None,
            encoder: RleVideoEncoder::new(format),
            frames_pushed: 0,
        }
    }

    pub(crate) fn start_recording(&mut self, path: &str) -> Result<(), Box<dyn Error>> {
        if self.recording.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "recording output is already open",
            )
            .into());
        }
        let final_path = PathBuf::from(path.trim());
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| std::io::Error::other("recording path must name a file"))?;
        let temp_path = final_path.with_file_name(format!("{file_name}.tmp"));
        self.recording = Some(AtomicY4mFileWriter::new(
            final_path,
            temp_path,
            self.format,
        )?);
        Ok(())
    }

    pub(crate) fn finish_recording(&mut self) -> Result<usize, Box<dyn Error>> {
        let Some(mut recording) = self.recording.take() else {
            return Err(std::io::Error::other("recording output is not open").into());
        };
        match recording.finalize() {
            Ok(bytes) => Ok(bytes),
            Err(error) => {
                self.recording = Some(recording);
                Err(error.into())
            }
        }
    }

    pub(crate) fn abort_recording(&mut self) {
        if let Some(mut recording) = self.recording.take() {
            let _ = recording.abort();
        }
    }

    pub(crate) fn start_streaming(&mut self, address: &str) -> Result<(), Box<dyn Error>> {
        if self.streaming.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "stream output is already open",
            )
            .into());
        }
        self.streaming = Some(StreamOutput::connect(address)?);
        Ok(())
    }

    pub(crate) fn finish_streaming(&mut self) {
        if let Some(mut stream) = self.streaming.take() {
            let _ = stream.flush();
            stream.close();
        }
    }

    pub(crate) fn push_frame(&mut self, frame: &VideoFrame) -> Result<(), Box<dyn Error>> {
        if let Some(recording) = self.recording.as_mut() {
            recording.push(frame.clone())?;
        }
        if let Some(stream) = self.streaming.as_mut() {
            if stream.state() == StreamState::Disconnected {
                stream.reconnect()?;
            }
            let packet = self.encoder.encode(frame)?;
            stream.submit(packet)?;
            if let Err(error) = stream.flush() {
                stream.reconnect().map_err(|reconnect| {
                    std::io::Error::other(format!("{error}; reconnect failed: {reconnect}"))
                })?;
                stream.flush()?;
            }
        }
        self.frames_pushed = self.frames_pushed.saturating_add(1);
        Ok(())
    }

    pub(crate) fn output_status(&self) -> String {
        let recording = if self.recording.is_some() {
            "recording open"
        } else {
            "recording stopped"
        };
        let streaming =
            self.streaming
                .as_ref()
                .map_or("stream stopped", |stream| match stream.state() {
                    StreamState::Connected => "stream connected",
                    StreamState::Disconnected => "stream reconnecting",
                    StreamState::Failed => "stream failed",
                    StreamState::Closed => "stream closed",
                });
        format!("Output: {recording} · {streaming}")
    }

    pub(crate) fn output_metrics(&self) -> String {
        let stream = self.streaming.as_ref();
        let (sent, dropped, queued, reconnects) = stream.map_or((0, 0, 0, 0), |stream| {
            let metrics = stream.metrics();
            (
                metrics.sent_packets(),
                metrics.dropped_packets(),
                stream.queued_bytes() as u64,
                metrics.reconnects(),
            )
        });
        format!(
            "frames={} · sent={} · dropped={} · queued={} B · reconnects={reconnects}",
            self.frames_pushed, sent, dropped, queued
        )
    }
}
