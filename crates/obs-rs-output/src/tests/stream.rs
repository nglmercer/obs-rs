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
        ReconnectPolicy::immediate(2),
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
fn reference_stream_implements_the_shared_transport_lifecycle() {
    let mut stream = StreamSession::new(
        MemoryPacketTransport::new(),
        32,
        PacketDropPolicy::DropNewest,
        ReconnectPolicy::new(1),
    )
    .expect("stream");
    stream.connect().expect("connect stream");

    assert_eq!(
        StreamingTransport::poll(&mut stream).expect("poll stream"),
        0
    );
    StreamingTransport::close(&mut stream).expect("close stream");
    assert_eq!(stream.state(), StreamState::Closed);
}

#[test]
fn reconnect_policy_is_capped_and_stream_reconnect_is_deferred_without_sleeping() {
    let policy =
        ReconnectPolicy::with_backoff(4, Duration::from_millis(100), Duration::from_millis(250));
    assert_eq!(policy.delay_for_attempt(0), Duration::from_millis(100));
    assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(200));
    assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(250));
    assert_eq!(policy.delay_for_attempt(3), Duration::from_millis(250));
    assert_eq!(
        ReconnectPolicy::immediate(4).delay_for_attempt(u32::MAX),
        Duration::ZERO
    );

    let mut stream = StreamSession::new(
        MemoryPacketTransport::new(),
        32,
        PacketDropPolicy::DropNewest,
        policy,
    )
    .expect("stream");
    stream.connect().expect("connect stream");
    let now = std::time::Instant::now();
    stream.disconnect_at(now);

    assert_eq!(
        stream.reconnect_at(now),
        Ok(ReconnectOutcome::Deferred {
            retry_after: Duration::from_millis(100),
        })
    );
    assert_eq!(
        stream.reconnect_at(now + Duration::from_millis(100)),
        Ok(ReconnectOutcome::Reconnected)
    );
}

#[test]
fn cloning_a_packet_shares_its_payload_instead_of_copying_it() {
    // The engine hands the same packet to the recorder and the stream when both
    // outputs are live. That fan-out is a refcount bump, not a second copy of
    // the payload, which is what keeps peak memory flat at 60fps.
    let payload = vec![7_u8; 4096];
    let packet =
        EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, payload).expect("packet");
    let duplicate = packet.clone();

    assert_eq!(
        packet.payload().as_ptr(),
        duplicate.payload().as_ptr(),
        "a cloned packet must alias the original payload"
    );
    assert_eq!(packet, duplicate);
}

#[test]
fn a_partially_sent_batch_is_requeued_in_timestamp_order() {
    // P1 delivers, P2 fails, and P3 never leaves the batch. The undelivered
    // tail has to go back on the front of the queue as [P2, P3]: the queue pops
    // from the front, so pushing the tail front-first would re-send it as
    // P3, P2 and break the monotonic timestamp guarantee downstream.
    let packets = (1..=3)
        .map(|index| {
            EncodedPacket::new(
                PacketKind::Video,
                Timestamp::from_millis(index),
                true,
                vec![u8::try_from(index).expect("small index")],
            )
            .expect("packet")
        })
        .collect::<Vec<_>>();

    let mut transport = MemoryPacketTransport::new();
    transport.fail_send_after(1);
    let mut stream = StreamSession::new(
        transport,
        32,
        PacketDropPolicy::DropNewest,
        ReconnectPolicy::immediate(2),
    )
    .expect("stream");
    stream.connect().expect("connect stream");
    for packet in packets {
        stream.submit(packet).expect("queue packet");
    }

    assert!(stream.flush().is_err(), "the second send fails");
    assert_eq!(stream.metrics().sent_packets(), 1);
    assert_eq!(stream.queued_bytes(), 2, "two packets are re-queued");

    stream.reconnect().expect("reconnect stream");
    assert_eq!(stream.flush().expect("retry the tail"), 2);

    let delivered = stream
        .transport()
        .sent()
        .map(|packet| packet.timestamp().as_nanos())
        .collect::<Vec<_>>();
    assert_eq!(
        delivered,
        vec![1_000_000, 2_000_000, 3_000_000],
        "packets must arrive in order"
    );
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
#[ignore = "requires permission to bind a local TCP fixture socket"]
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
