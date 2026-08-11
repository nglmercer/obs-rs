use std::{
    collections::VecDeque,
    io::{BufWriter, Write},
    net::TcpStream,
};

use crate::{
    error::OutputError,
    types::EncodedPacket,
    NETWORK_WRITE_TIMEOUT, TCP_PACKET_MAGIC,
};

use super::PacketTransport;

/// Most packets a [`MemoryPacketTransport`] retains for inspection.
///
/// The capture buffer exists for tests and diagnostics; bounding it stops a
/// long-lived session from growing it without limit.
pub const MAX_MEMORY_TRANSPORT_PACKETS: usize = 1_024;

pub struct MemoryPacketTransport {
    connected: bool,
    fail_next_send: bool,
    sent: VecDeque<EncodedPacket>,
    dropped: u64,
}

impl MemoryPacketTransport {
    /// Creates a disconnected memory transport.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            connected: false,
            fail_next_send: false,
            sent: VecDeque::new(),
            dropped: 0,
        }
    }

    /// Makes the next send fail and disconnect the transport.
    pub fn fail_next_send(&mut self) {
        self.fail_next_send = true;
    }

    /// Returns packets successfully delivered to the transport, oldest first.
    ///
    /// At most [`MAX_MEMORY_TRANSPORT_PACKETS`] are retained; older ones are
    /// discarded and counted by [`MemoryPacketTransport::dropped`].
    #[must_use]
    pub fn sent(&self) -> impl ExactSizeIterator<Item = &EncodedPacket> {
        self.sent.iter()
    }

    /// Returns how many captured packets were discarded to stay within bounds.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.dropped
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
        if self.sent.len() == MAX_MEMORY_TRANSPORT_PACKETS {
            let _ = self.sent.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.sent.push_back(packet.clone());
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
        // The frame header is fixed-size, so it is staged on the stack and the
        // payload is written straight from the packet: no per-packet heap
        // buffer and no copy of the payload.
        let header = packet_header(packet);
        stream
            .write_all(&header)
            .and_then(|()| stream.write_all(packet.payload()))
            .map_err(|error| OutputError::Transport(format!("TCP send failed: {error}")))
    }

    fn send_batch(
        &mut self,
        packets: &[EncodedPacket],
        delivered: &mut usize,
    ) -> Result<(), OutputError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| OutputError::Transport("TCP transport is disconnected".to_owned()))?;
        // One buffered writer over the whole run collapses what used to be two
        // syscalls per packet into as few as one for the batch.
        let mut writer = BufWriter::new(stream);
        for packet in packets {
            let header = packet_header(packet);
            writer
                .write_all(&header)
                .and_then(|()| writer.write_all(packet.payload()))
                .map_err(|error| OutputError::Transport(format!("TCP send failed: {error}")))?;
            *delivered += 1;
        }
        writer
            .flush()
            .map_err(|error| OutputError::Transport(format!("TCP send failed: {error}")))
    }

    fn disconnect(&mut self) {
        self.stream = None;
    }
}

/// Bytes in one framed TCP packet header.
const TCP_HEADER_BYTES: usize = 8 + 1 + 1 + 8 + 8;

/// Builds the fixed-size frame header for one packet.
fn packet_header(packet: &EncodedPacket) -> [u8; TCP_HEADER_BYTES] {
    let mut header = [0_u8; TCP_HEADER_BYTES];
    header[..8].copy_from_slice(TCP_PACKET_MAGIC);
    header[8] = packet.kind.tag();
    header[9] = u8::from(packet.is_keyframe());
    header[10..18].copy_from_slice(&packet.timestamp().as_nanos().to_le_bytes());
    header[18..26].copy_from_slice(&(packet.byte_len() as u64).to_le_bytes());
    header
}
