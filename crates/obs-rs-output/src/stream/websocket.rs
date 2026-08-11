use std::{
    cell::Cell,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    error::OutputError, types::EncodedPacket, MAX_WEBSOCKET_HEADER_BYTES, NETWORK_WRITE_TIMEOUT,
    WEBSOCKET_PACKET_MAGIC,
};

use super::PacketTransport;

thread_local! {
    /// Per-thread masking-key counter.
    ///
    /// RFC 6455 only requires the mask to be unpredictable per frame, not
    /// globally ordered, so a thread-local counter avoids the cache-line
    /// contention a shared atomic caused on every send.
    static NEXT_WEBSOCKET_NONCE: Cell<u64> = const { Cell::new(1) };
}

/// Returns the next masking nonce for this thread.
fn next_websocket_nonce() -> u64 {
    NEXT_WEBSOCKET_NONCE.with(|nonce| {
        let current = nonce.get();
        nonce.set(current.wrapping_add(1));
        current
    })
}

/// A standard RFC 6455 WebSocket client carrying OBS-RS binary packets.
pub struct WebSocketPacketTransport {
    endpoint: String,
    stream: Option<TcpStream>,
}

impl WebSocketPacketTransport {
    /// Creates a disconnected WebSocket transport.
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            stream: None,
        }
    }

    /// Returns the configured endpoint.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Returns whether the WebSocket handshake completed.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.stream.is_some()
    }
}

impl PacketTransport for WebSocketPacketTransport {
    fn connect(&mut self) -> Result<(), OutputError> {
        self.disconnect();
        let (address, host, path) = parse_websocket_endpoint(&self.endpoint)?;
        let socket_address = address
            .to_socket_addrs()
            .map_err(|error| {
                OutputError::Transport(format!("WebSocket address is invalid: {error}"))
            })?
            .next()
            .ok_or_else(|| {
                OutputError::Transport("WebSocket address has no socket targets".to_owned())
            })?;
        let mut stream = TcpStream::connect_timeout(&socket_address, NETWORK_WRITE_TIMEOUT)
            .map_err(|error| {
                OutputError::Transport(format!("WebSocket connect failed: {error}"))
            })?;
        stream
            .set_nodelay(true)
            .map_err(|error| OutputError::Transport(format!("WebSocket setup failed: {error}")))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| {
                OutputError::Transport(format!("WebSocket timeout setup failed: {error}"))
            })?;
        stream
            .set_write_timeout(Some(NETWORK_WRITE_TIMEOUT))
            .map_err(|error| {
                OutputError::Transport(format!("WebSocket write timeout setup failed: {error}"))
            })?;
        let key = websocket_key();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).map_err(|error| {
            OutputError::Transport(format!("WebSocket handshake failed: {error}"))
        })?;
        let response = read_websocket_headers(&mut stream)?;
        validate_websocket_response(&response, &key)?;
        self.stream = Some(stream);
        Ok(())
    }

    fn send(&mut self, packet: &EncodedPacket) -> Result<(), OutputError> {
        let stream = self.stream.as_mut().ok_or_else(|| {
            OutputError::Transport("WebSocket transport is disconnected".to_owned())
        })?;
        write_websocket_packet(stream, packet)
            .map_err(|error| OutputError::Transport(format!("WebSocket send failed: {error}")))
    }

    fn disconnect(&mut self) {
        self.stream = None;
    }
}

pub(crate) fn parse_websocket_endpoint(
    endpoint: &str,
) -> Result<(String, String, String), OutputError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(OutputError::Transport(
            "WebSocket endpoint is empty".to_owned(),
        ));
    }
    if endpoint.starts_with("wss://") {
        return Err(OutputError::Transport(
            "wss:// endpoints require an explicit TLS transport".to_owned(),
        ));
    }
    let endpoint = endpoint.strip_prefix("ws://").unwrap_or(endpoint);
    let (authority, path) = endpoint.split_once('/').map_or_else(
        || (endpoint, "/".to_owned()),
        |(host, path)| (host, format!("/{path}")),
    );
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains('\0')
        || authority.chars().any(char::is_whitespace)
        || authority.contains(['\r', '\n'])
    {
        return Err(OutputError::Transport(
            "WebSocket endpoint authority is invalid".to_owned(),
        ));
    }
    let Some((host, port)) = authority.rsplit_once(':') else {
        return Err(OutputError::Transport(
            "WebSocket endpoint must include host:port".to_owned(),
        ));
    };
    if host.is_empty()
        || (host.starts_with('[') && !host.ends_with(']'))
        || port.parse::<u16>().map_or(true, |port| port == 0)
    {
        return Err(OutputError::Transport(
            "WebSocket endpoint host:port is invalid".to_owned(),
        ));
    }
    let path = if path.is_empty() { "/" } else { path.as_str() };
    if path.chars().any(|character| {
        character == '\0' || character == '\r' || character == '\n' || character.is_control()
    }) {
        return Err(OutputError::Transport(
            "WebSocket endpoint path contains a control character".to_owned(),
        ));
    }
    Ok((authority.to_owned(), authority.to_owned(), path.to_owned()))
}

fn websocket_key() -> String {
    let counter = next_websocket_nonce();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0_u64, |duration| {
            u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
        });
    let process = u64::from(std::process::id());
    let first = counter ^ now.rotate_left(17) ^ process.rotate_right(7);
    let second = first.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29);
    let mut nonce = [0_u8; 16];
    nonce[..8].copy_from_slice(&first.to_be_bytes());
    nonce[8..].copy_from_slice(&second.to_be_bytes());
    base64_encode(&nonce)
}

pub(crate) fn read_websocket_headers(stream: &mut TcpStream) -> Result<String, OutputError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).map_err(|error| {
            OutputError::Transport(format!("WebSocket handshake read failed: {error}"))
        })?;
        if read == 0 {
            return Err(OutputError::Transport(
                "WebSocket handshake ended before headers".to_owned(),
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > MAX_WEBSOCKET_HEADER_BYTES {
            return Err(OutputError::Transport(
                "WebSocket handshake headers exceed the configured limit".to_owned(),
            ));
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).map_err(|_| {
                OutputError::Transport("WebSocket handshake headers are not UTF-8".to_owned())
            });
        }
    }
}

fn validate_websocket_response(response: &str, key: &str) -> Result<(), OutputError> {
    let mut lines = response.split("\r\n");
    let status = lines.next().unwrap_or_default();
    if !status.starts_with("HTTP/1.1 101 ") && !status.starts_with("HTTP/1.0 101 ") {
        return Err(OutputError::Transport(format!(
            "WebSocket server rejected upgrade: {status}"
        )));
    }
    let mut upgraded = false;
    let mut connection = false;
    let mut accept = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("upgrade") && value.eq_ignore_ascii_case("websocket") {
            upgraded = true;
        }
        if name.eq_ignore_ascii_case("connection")
            && value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        {
            connection = true;
        }
        if name.eq_ignore_ascii_case("sec-websocket-accept") {
            accept = Some(value);
        }
    }
    let expected = base64_encode(&sha1_digest(
        format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
    ));
    if upgraded && connection && accept == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(OutputError::Transport(
            "WebSocket handshake response is missing a valid upgrade or accept token".to_owned(),
        ))
    }
}

/// Validates a bounded RFC 6455 server handshake without performing network I/O.
///
/// This entry point is useful to protocol adapters, tests, and fuzzers. The
/// caller supplies the request key whose accept digest must appear in the
/// response.
///
/// # Errors
///
/// Rejects oversized/non-UTF-8 input and malformed or unsuccessful upgrades.
pub fn validate_websocket_handshake(response: &[u8], request_key: &str) -> Result<(), OutputError> {
    if response.len() > MAX_WEBSOCKET_HEADER_BYTES {
        return Err(OutputError::Transport(
            "WebSocket handshake headers exceed the configured limit".to_owned(),
        ));
    }
    let response = std::str::from_utf8(response).map_err(|_| {
        OutputError::Transport("WebSocket handshake headers are not UTF-8".to_owned())
    })?;
    validate_websocket_response(response, request_key)
}

#[cfg(test)]
pub(crate) fn websocket_packet_body(packet: &EncodedPacket) -> Result<Vec<u8>, OutputError> {
    let mut body = Vec::with_capacity(26 + packet.byte_len());
    body.extend_from_slice(WEBSOCKET_PACKET_MAGIC);
    body.push(packet.kind.tag());
    body.push(u8::from(packet.is_keyframe()));
    body.extend_from_slice(&packet.timestamp().as_nanos().to_le_bytes());
    body.extend_from_slice(
        &u64::try_from(packet.byte_len())
            .map_err(|_| OutputError::PacketTooLarge {
                bytes: packet.byte_len(),
            })?
            .to_le_bytes(),
    );
    body.extend_from_slice(packet.payload());
    Ok(body)
}

const WEBSOCKET_PACKET_HEADER_BYTES: usize = 26;

fn write_websocket_packet(stream: &mut TcpStream, packet: &EncodedPacket) -> std::io::Result<()> {
    let length = WEBSOCKET_PACKET_HEADER_BYTES
        .checked_add(packet.byte_len())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "WebSocket packet too large",
            )
        })?;
    let length = u64::try_from(length).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "WebSocket packet too large",
        )
    })?;

    let mut frame_header = [0_u8; 10];
    frame_header[0] = 0x82;
    let frame_header_len = if length <= 125 {
        frame_header[1] = 0x80 | u8::try_from(length).unwrap_or(125);
        2
    } else if u16::try_from(length).is_ok() {
        frame_header[1] = 0x80 | 0x7e;
        frame_header[2..4]
            .copy_from_slice(&u16::try_from(length).unwrap_or(u16::MAX).to_be_bytes());
        4
    } else {
        frame_header[1] = 0x80 | 127;
        frame_header[2..10].copy_from_slice(&length.to_be_bytes());
        10
    };

    let nonce = next_websocket_nonce();
    let mask = nonce.to_le_bytes()[..4]
        .try_into()
        .unwrap_or([0x4d_u8, 0x53, 0x52, 0x53]);
    let mut body_header = [0_u8; WEBSOCKET_PACKET_HEADER_BYTES];
    body_header[..8].copy_from_slice(WEBSOCKET_PACKET_MAGIC);
    body_header[8] = packet.kind.tag();
    body_header[9] = u8::from(packet.is_keyframe());
    body_header[10..18].copy_from_slice(&packet.timestamp().as_nanos().to_le_bytes());
    body_header[18..26].copy_from_slice(
        &u64::try_from(packet.byte_len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );

    stream.write_all(&frame_header[..frame_header_len])?;
    stream.write_all(&mask)?;
    let mask_index = write_masked(stream, &body_header, mask, 0)?;
    let _ = write_masked(stream, packet.payload(), mask, mask_index)?;
    Ok(())
}

fn write_masked(
    stream: &mut TcpStream,
    bytes: &[u8],
    mask: [u8; 4],
    mut mask_index: usize,
) -> std::io::Result<usize> {
    let mut buffer = [0_u8; 4096];
    for chunk in bytes.chunks(buffer.len()) {
        for (index, byte) in chunk.iter().enumerate() {
            buffer[index] = *byte ^ mask[(mask_index + index) & 3];
        }
        stream.write_all(&buffer[..chunk.len()])?;
        mask_index = (mask_index + chunk.len()) & 3;
    }
    Ok(mask_index)
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(*chunk.get(1).unwrap_or(&0));
        let third = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (first << 16) | (second << 8) | third;
        let index = |shift: u32| usize::from(u8::try_from((triple >> shift) & 0x3f).unwrap_or(0));
        output.push(char::from(TABLE[index(18)]));
        output.push(char::from(TABLE[index(12)]));
        output.push(if chunk.len() > 1 {
            char::from(TABLE[index(6)])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(TABLE[index(0)])
        } else {
            '='
        });
    }
    output
}

pub(crate) fn sha1_digest(input: &[u8]) -> [u8; 20] {
    let bit_length = u64::try_from(input.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(8);
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());
    let mut hash = [
        0x6745_2301_u32,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    for chunk in message.chunks_exact(64) {
        let mut words = [0_u32; 80];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let (mut hash_a, mut hash_b, mut hash_c, mut hash_d, mut hash_e) =
            (hash[0], hash[1], hash[2], hash[3], hash[4]);
        for (index, word) in words.iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((hash_b & hash_c) | ((!hash_b) & hash_d), 0x5a82_7999),
                20..=39 => (hash_b ^ hash_c ^ hash_d, 0x6ed9_eba1),
                40..=59 => (
                    (hash_b & hash_c) | (hash_b & hash_d) | (hash_c & hash_d),
                    0x8f1b_bcdc,
                ),
                _ => (hash_b ^ hash_c ^ hash_d, 0xca62_c1d6),
            };
            let temporary = hash_a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(hash_e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            hash_e = hash_d;
            hash_d = hash_c;
            hash_c = hash_b.rotate_left(30);
            hash_b = hash_a;
            hash_a = temporary;
        }
        hash[0] = hash[0].wrapping_add(hash_a);
        hash[1] = hash[1].wrapping_add(hash_b);
        hash[2] = hash[2].wrapping_add(hash_c);
        hash[3] = hash[3].wrapping_add(hash_d);
        hash[4] = hash[4].wrapping_add(hash_e);
    }
    let mut output = [0_u8; 20];
    for (index, word) in hash.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}
