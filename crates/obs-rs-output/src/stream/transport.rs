use std::{
    collections::VecDeque,
    io::{BufWriter, IoSlice, Write},
    net::{TcpStream, ToSocketAddrs},
};

use crate::{error::OutputError, types::EncodedPacket, NETWORK_WRITE_TIMEOUT, TCP_PACKET_MAGIC};

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
    stream: Option<BufWriter<TcpStream>>,
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
        let address = self
            .address
            .to_socket_addrs()
            .map_err(|error| OutputError::Transport(format!("TCP address is invalid: {error}")))?
            .next()
            .ok_or_else(|| {
                OutputError::Transport("TCP address has no socket targets".to_owned())
            })?;
        let stream = TcpStream::connect_timeout(&address, NETWORK_WRITE_TIMEOUT)
            .map_err(|error| OutputError::Transport(format!("TCP connect failed: {error}")))?;
        stream
            .set_nodelay(true)
            .map_err(|error| OutputError::Transport(format!("TCP setup failed: {error}")))?;
        stream
            .set_write_timeout(Some(NETWORK_WRITE_TIMEOUT))
            .map_err(|error| {
                OutputError::Transport(format!("TCP timeout setup failed: {error}"))
            })?;
        self.stream = Some(BufWriter::with_capacity(16 * 1024, stream));
        Ok(())
    }

    fn send(&mut self, packet: &EncodedPacket) -> Result<(), OutputError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| OutputError::Transport("TCP transport is disconnected".to_owned()))?;
        // The frame header is fixed-size, so it is staged on the stack and the
        // payload is written straight from the packet: no per-packet heap
        // buffer and no copy of the payload. Vectored output avoids a separate
        // write call for the header and payload when the socket accepts both.
        let header = packet_header(packet);
        write_packet_vectored(stream, &header, packet.payload())
            .and_then(|()| stream.flush())
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
        // The writer is retained by the transport, so repeated batches do not
        // allocate and discard an 8 KiB buffer.
        for packet in packets {
            let header = packet_header(packet);
            write_packet_vectored(stream, &header, packet.payload())
                .map_err(|error| OutputError::Transport(format!("TCP send failed: {error}")))?;
            *delivered += 1;
        }
        stream
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

fn write_packet_vectored(
    writer: &mut impl Write,
    header: &[u8],
    payload: &[u8],
) -> std::io::Result<()> {
    let mut header_offset = 0_usize;
    let mut payload_offset = 0_usize;
    while header_offset < header.len() || payload_offset < payload.len() {
        let header_remaining = &header[header_offset..];
        let payload_remaining = &payload[payload_offset..];
        let written = if header_remaining.is_empty() {
            writer.write(payload_remaining)?
        } else {
            writer.write_vectored(&[
                IoSlice::new(header_remaining),
                IoSlice::new(payload_remaining),
            ])?
        };
        if written == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "TCP transport wrote zero bytes",
            ));
        }
        if written <= header_remaining.len() {
            header_offset += written;
        } else {
            header_offset = header.len();
            payload_offset += written - header_remaining.len();
        }
    }
    Ok(())
}
