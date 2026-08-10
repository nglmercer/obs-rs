use crate::*;
use obs_rs_media::Timestamp;
use std::time::Duration;

#[test]
fn packet_decoder_rejects_oversized_declared_payload_before_allocation() {
    let mut bytes = Vec::from(PACKET_MAGIC.as_slice());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.push(0);
    bytes.push(1);
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&(MAX_PACKET_BYTES as u64 + 1).to_le_bytes());

    assert_eq!(
        MemoryMuxer::decode(&bytes),
        Err(OutputError::PacketTooLarge {
            bytes: MAX_PACKET_BYTES + 1
        })
    );
}

#[test]
fn stream_requeues_failed_packets_and_reconnects_without_loss() {
    let packet = EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, vec![1, 2, 3])
        .expect("packet");
    let mut transport = MemoryPacketTransport::new();
    transport.fail_next_send();
    let mut stream = StreamSession::new(
        transport,
        32,
        PacketDropPolicy::DropNewest,
        ReconnectPolicy::new(2),
    )
    .expect("stream");
    stream.connect().expect("connect stream");
    stream.submit(packet).expect("queue packet");

    assert!(matches!(
        stream.flush(),
        Err(OutputError::Transport(reason)) if reason == "injected send failure"
    ));
    assert_eq!(stream.state(), StreamState::Disconnected);
    assert_eq!(stream.queued_bytes(), 3);
    stream.reconnect().expect("reconnect stream");
    assert_eq!(stream.flush().expect("retry packet"), 1);
    assert_eq!(stream.state(), StreamState::Connected);
    assert_eq!(stream.transport().sent().len(), 1);
    assert_eq!(stream.metrics().send_failures(), 1);
    assert_eq!(stream.metrics().sent_packets(), 1);
    assert_eq!(stream.metrics().reconnects(), 1);
    assert_eq!(stream.queued_bytes(), 0);
}

#[test]
fn tcp_transport_rejects_send_when_disconnected() {
    let mut transport = TcpPacketTransport::new("127.0.0.1:1");
    assert_eq!(transport.address(), "127.0.0.1:1");
    let packet = EncodedPacket::new(
        PacketKind::Audio,
        Timestamp::from_millis(3),
        false,
        vec![4, 5, 6],
    )
    .expect("packet");
    assert_eq!(
        transport.send(&packet),
        Err(OutputError::Transport(
            "TCP transport is disconnected".to_owned()
        ))
    );
    assert!(!transport.is_connected());
}

#[test]
fn websocket_transport_performs_upgrade_and_sends_masked_packet() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind WebSocket fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept client");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set fixture timeout");
        let request = read_websocket_headers(&mut stream).expect("read upgrade request");
        assert!(request.starts_with("GET /ingest HTTP/1.1\r\n"));
        assert!(request.contains("Upgrade: websocket\r\n"));
        let key = request
            .split("\r\n")
            .find_map(|line| line.strip_prefix("Sec-WebSocket-Key:").map(str::trim))
            .expect("client key");
        let accept = base64_encode(&sha1_digest(
            format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
        ));
        write!(
                stream,
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            )
            .expect("write upgrade response");

        let mut header = [0_u8; 2];
        stream.read_exact(&mut header).expect("read frame header");
        assert_eq!(header[0], 0x82);
        assert_ne!(header[1] & 0x80, 0);
        let mut length = usize::from(header[1] & 0x7f);
        if length == 126 {
            let mut bytes = [0_u8; 2];
            stream.read_exact(&mut bytes).expect("read short length");
            length = usize::from(u16::from_be_bytes(bytes));
        } else if length == 127 {
            let mut bytes = [0_u8; 8];
            stream.read_exact(&mut bytes).expect("read long length");
            length = usize::try_from(u64::from_be_bytes(bytes)).expect("fixture length");
        }
        let mut mask = [0_u8; 4];
        stream.read_exact(&mut mask).expect("read mask");
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body).expect("read frame body");
        for (index, byte) in body.iter_mut().enumerate() {
            *byte ^= mask[index % mask.len()];
        }
        body
    });

    let mut transport = WebSocketPacketTransport::new(format!("ws://{address}/ingest"));
    transport.connect().expect("WebSocket upgrade");
    assert!(transport.is_connected());
    let packet = EncodedPacket::new(
        PacketKind::Audio,
        Timestamp::from_millis(3),
        false,
        vec![4, 5, 6],
    )
    .expect("packet");
    transport.send(&packet).expect("send packet");
    transport.disconnect();
    let body = server.join().expect("fixture server");
    assert_eq!(body, websocket_packet_body(&packet).expect("packet body"));
}

#[test]
fn websocket_endpoint_rejects_tls_without_an_explicit_boundary() {
    let error = parse_websocket_endpoint("wss://example.test:443/live")
        .expect_err("TLS endpoint must be rejected");
    assert_eq!(
        error,
        OutputError::Transport("wss:// endpoints require an explicit TLS transport".to_owned())
    );
}

#[test]
fn websocket_endpoint_validates_authority_and_path_bounds() {
    for endpoint in [
        "ws://example.test:not-a-port/live",
        "ws://example.test:0/live",
        "ws://example.test:443/live\r\nHost: injected",
        "ws://example.test:443/li\nve",
    ] {
        assert!(
            parse_websocket_endpoint(endpoint).is_err(),
            "endpoint should be rejected: {endpoint:?}"
        );
    }
    assert_eq!(
        parse_websocket_endpoint("ws://[::1]:443/live").expect("IPv6 endpoint"),
        (
            "[::1]:443".to_owned(),
            "[::1]:443".to_owned(),
            "/live".to_owned()
        )
    );
}
