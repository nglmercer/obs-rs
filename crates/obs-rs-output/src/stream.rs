mod session;
mod transport;
mod websocket;

use crate::ReconnectOutcome;

/// Lifecycle boundary shared by packet and native production transports.
///
/// Construction and media submission remain transport-specific. This contract
/// only gives the engine one bounded control-plane surface for polling health,
/// attempting one reconnect, and closing a session.
pub trait StreamingTransport {
    type Error;

    /// Polls transport health without waiting for a new media item.
    ///
    /// # Errors
    ///
    /// Returns the transport-specific failure without growing the media queue.
    fn poll(&mut self) -> Result<usize, Self::Error>;

    /// Attempts one reconnect under the transport's configured budget and
    /// backoff schedule without sleeping.
    ///
    /// # Errors
    ///
    /// Returns the transport-specific failure when the attempt is rejected.
    fn reconnect(&mut self) -> Result<ReconnectOutcome, Self::Error>;

    /// Closes the transport and releases its bounded queues.
    ///
    /// # Errors
    ///
    /// Returns the transport-specific shutdown or finalization failure.
    fn close(&mut self) -> Result<(), Self::Error>;
}

pub use session::{PacketMuxer, PacketTransport, StreamSession};
pub use transport::{MemoryPacketTransport, TcpPacketTransport};
pub use websocket::{validate_websocket_handshake, WebSocketPacketTransport};

#[cfg(test)]
pub(crate) use websocket::{
    base64_encode, parse_websocket_endpoint, read_websocket_headers, sha1_digest,
    websocket_packet_body,
};
