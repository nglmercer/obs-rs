use std::{io::Write, net::TcpStream};

use crate::{
    codec::{write_all, write_u64},
    error::OutputError,
    types::EncodedPacket,
    NETWORK_WRITE_TIMEOUT, TCP_PACKET_MAGIC,
};

use super::PacketTransport;

pub struct MemoryPacketTransport {
    connected: bool,
    fail_next_send: bool,
    sent: Vec<EncodedPacket>,
}

impl MemoryPacketTransport {
    /// Creates a disconnected memory transport.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            connected: false,
            fail_next_send: false,
            sent: Vec::new(),
        }
    }

    /// Makes the next send fail and disconnect the transport.
    pub fn fail_next_send(&mut self) {
        self.fail_next_send = true;
    }

    /// Returns packets successfully delivered to the transport.
    #[must_use]
    pub fn sent(&self) -> &[EncodedPacket] {
        &self.sent
    }

    /// Returns whether the transport is connected.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.connected
    }
}

impl Default for MemoryPacketTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketTransport for MemoryPacketTransport {
    fn connect(&mut self) -> Result<(), OutputError> {
        self.connected = true;
        Ok(())
    }

    fn send(&mut self, packet: &EncodedPacket) -> Result<(), OutputError> {
        if !self.connected {
            return Err(OutputError::Transport(
                "transport is disconnected".to_owned(),
            ));
        }
        if self.fail_next_send {
            self.fail_next_send = false;
            self.connected = false;
            return Err(OutputError::Transport("injected send failure".to_owned()));
        }
        self.sent.push(packet.clone());
        Ok(())
    }

    fn disconnect(&mut self) {
        self.connected = false;
    }
}

/// A standard-library TCP transport using explicit length-framed OBS-RS packets.
///
/// The framing is a transport fixture, not a claim of compatibility with RTMP,
/// SRT, WebRTC, or another production streaming protocol. It gives the stream
/// session a real Rust-owned network path while protocol selection remains open.
pub struct TcpPacketTransport {
    address: String,
    stream: Option<TcpStream>,
}

impl TcpPacketTransport {
    /// Creates a disconnected transport for a `host:port` address.
    #[must_use]
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            stream: None,
        }
    }

    /// Returns the configured destination address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Returns whether a TCP stream is currently connected.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.stream.is_some()
    }
}

impl PacketTransport for TcpPacketTransport {
    fn connect(&mut self) -> Result<(), OutputError> {
        let stream = TcpStream::connect(&self.address)
            .map_err(|error| OutputError::Transport(format!("TCP connect failed: {error}")))?;
        stream
            .set_nodelay(true)
            .map_err(|error| OutputError::Transport(format!("TCP setup failed: {error}")))?;
        stream
            .set_write_timeout(Some(NETWORK_WRITE_TIMEOUT))
            .map_err(|error| {
                OutputError::Transport(format!("TCP timeout setup failed: {error}"))
            })?;
        self.stream = Some(stream);
        Ok(())
    }

    fn send(&mut self, packet: &EncodedPacket) -> Result<(), OutputError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| OutputError::Transport("TCP transport is disconnected".to_owned()))?;
        let mut bytes = Vec::with_capacity(30 + packet.byte_len());
        write_all(&mut bytes, TCP_PACKET_MAGIC)?;
        bytes.push(packet.kind.tag());
        bytes.push(u8::from(packet.is_keyframe()));
        write_u64(&mut bytes, packet.timestamp().as_nanos())?;
        write_u64(&mut bytes, packet.byte_len() as u64)?;
        write_all(&mut bytes, packet.payload())?;
        stream
            .write_all(&bytes)
            .map_err(|error| OutputError::Transport(format!("TCP send failed: {error}")))
    }

    fn disconnect(&mut self) {
        self.stream = None;
    }
}
