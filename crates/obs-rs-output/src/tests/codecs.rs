use super::format;
use crate::*;
use obs_rs_audio::{AudioBuffer, AudioFormat};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

#[test]
fn packet_queue_applies_byte_drop_policy() {
    let first = EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, vec![1, 2, 3])
        .expect("first packet");
    let second = EncodedPacket::new(
        PacketKind::Video,
        Timestamp::from_millis(1),
        false,
        vec![4, 5, 6],
    )
    .expect("second packet");
    let mut queue = PacketQueue::new(4, PacketDropPolicy::DropOldest).expect("queue");
    queue.push(first).expect("first push");
    assert_eq!(
        queue.push(second).expect("second push"),
        PacketPushOutcome::DroppedOldest {
            packets: 1,
            bytes: 3
        }
    );
    assert_eq!(queue.queued_bytes(), 3);
    assert_eq!(queue.pop().expect("remaining packet").payload(), &[4, 5, 6]);
}

#[test]
fn raw_encoder_and_muxer_round_trip_packets() {
    let format = format();
    let frame = VideoFrame::solid(format, Timestamp::from_millis(12), [9, 8, 7, 255]);
    let mut encoder = RawVideoEncoder::new(format);
    let packet = encoder.encode(&frame).expect("encode frame");
    assert_eq!(packet.kind(), PacketKind::Video);
    assert!(packet.is_keyframe());
    assert_eq!(packet.payload(), frame.pixels());

    let mut muxer = MemoryMuxer::new();
    muxer.push(packet).expect("mux packet");
    let bytes = muxer.finalize().expect("finalize muxer");
    assert_eq!(muxer.state(), OutputState::Finalized);
    assert_eq!(
        MemoryMuxer::decode(&bytes).expect("decode packets").len(),
        1
    );
    assert!(matches!(
        muxer.push(
            EncodedPacket::new(PacketKind::Audio, Timestamp::ZERO, false, vec![1])
                .expect("audio packet")
        ),
        Err(OutputError::InvalidState {
            operation: "push a packet",
            state: OutputState::Finalized
        })
    ));
}

#[test]
fn png_encoder_emits_deterministic_crc_checked_rgba_chunks() {
    let format = format();
    let frame = VideoFrame::new(
        format,
        Timestamp::from_millis(7),
        vec![255, 0, 0, 255, 0, 255, 0, 255],
    )
    .expect("frame");
    let mut encoder = PngVideoEncoder::new(format);
    let packet = encoder.encode(&frame).expect("PNG packet");
    let payload = packet.payload();
    assert_eq!(packet.kind(), PacketKind::Video);
    assert!(packet.is_keyframe());
    assert_eq!(payload, encode_png(&frame).expect("PNG image"));
    assert_eq!(payload.get(..8), Some(PNG_SIGNATURE.as_slice()));

    let mut offset = PNG_SIGNATURE.len();
    let mut chunk_kinds = Vec::new();
    while offset < payload.len() {
        let length = usize::try_from(u32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]))
        .expect("chunk length");
        offset += 4;
        let kind = [
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ];
        offset += 4;
        let data = &payload[offset..offset + length];
        offset += length;
        let crc = u32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        offset += 4;
        let mut crc_input = Vec::with_capacity(kind.len() + data.len());
        crc_input.extend_from_slice(&kind);
        crc_input.extend_from_slice(data);
        assert_eq!(crc, crc32(&crc_input));
        chunk_kinds.push(kind);
    }
    assert_eq!(chunk_kinds, vec![*b"IHDR", *b"IDAT", *b"IEND"]);
    assert_eq!(offset, payload.len());
}

#[test]
fn muxer_rejects_backward_timestamps_and_container_trailing_bytes() {
    let mut muxer = MemoryMuxer::new();
    muxer
        .push(
            EncodedPacket::new(PacketKind::Video, Timestamp::from_millis(10), true, vec![1])
                .expect("first packet"),
        )
        .expect("first push");
    assert_eq!(
        muxer.push(
            EncodedPacket::new(PacketKind::Audio, Timestamp::from_millis(9), false, vec![2])
                .expect("backward packet")
        ),
        Err(OutputError::NonMonotonicTimestamp {
            previous: Timestamp::from_millis(10),
            actual: Timestamp::from_millis(9),
        })
    );

    let bytes = muxer.finalize().expect("finalize packet");
    let mut with_trailing = bytes.as_ref().clone();
    with_trailing.push(0);
    assert!(matches!(
        MemoryMuxer::decode(&with_trailing),
        Err(OutputError::InvalidCodecPayload(reason)) if reason == "packet container has trailing bytes"
    ));
}

#[test]
fn rle_video_codec_round_trips_and_rejects_bad_runs() {
    let format = VideoFormat::new(8, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    let frame = VideoFrame::solid(format, Timestamp::from_millis(12), [9, 8, 7, 255]);
    let mut encoder = RleVideoEncoder::new(format);
    let packet = encoder.encode(&frame).expect("encode frame");
    assert!(packet.byte_len() < frame.pixels().len());
    assert_eq!(
        RleVideoDecoder::decode(format, packet.timestamp(), packet.payload())
            .expect("decode frame"),
        frame
    );

    let mut malformed = packet.payload().to_vec();
    malformed[8..12].copy_from_slice(&0_u32.to_le_bytes());
    assert!(matches!(
        RleVideoDecoder::decode(format, packet.timestamp(), &malformed),
        Err(OutputError::InvalidCodecPayload(_))
    ));
}

#[test]
fn raw_audio_encoder_emits_audio_packets_and_checks_format() {
    let audio_format = AudioFormat::new(48_000, 2).expect("audio format");
    let other_format = AudioFormat::new(44_100, 2).expect("other format");
    let buffer = AudioBuffer::new(audio_format, Timestamp::from_millis(4), vec![0.5, -0.25])
        .expect("audio buffer");
    let mut encoder = RawAudioEncoder::new(audio_format);
    let packet = encoder.encode(&buffer).expect("encode audio");
    assert_eq!(packet.kind(), PacketKind::Audio);
    assert!(!packet.is_keyframe());
    assert_eq!(packet.timestamp(), Timestamp::from_millis(4));
    assert_eq!(packet.payload().len(), 8);
    assert_eq!(&packet.payload()[..4], &0.5_f32.to_le_bytes());

    let other = AudioBuffer::silence(other_format, Timestamp::ZERO, 1).expect("other buffer");
    assert!(matches!(
        encoder.encode(&other),
        Err(OutputError::AudioFormatMismatch {
            expected,
            actual
        }) if expected == audio_format && actual == other_format
    ));
}

#[test]
fn wav_recording_emits_canonical_pcm16_headers_and_samples() {
    let audio_format = AudioFormat::new(48_000, 2).expect("audio format");
    let mut recording = WavRecording::new(audio_format);
    recording
        .push(
            AudioBuffer::new(audio_format, Timestamp::ZERO, vec![1.0, -1.0, 0.5, -0.5])
                .expect("buffer"),
        )
        .expect("append");

    let bytes = recording.encode().expect("WAV encode");
    assert_eq!(recording.frames(), 2);
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..16], b"WAVEfmt ");
    assert_eq!(&bytes[36..40], b"data");
    assert_eq!(
        u32::from_le_bytes(bytes[40..44].try_into().expect("size")),
        8
    );
    assert_eq!(&bytes[44..46], &i16::MAX.to_le_bytes());
    assert_eq!(&bytes[46..48], &i16::MIN.to_le_bytes());
}

#[test]
fn y4m_recording_emits_standard_header_and_420_planes() {
    let format = VideoFormat::new(4, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let frame = VideoFrame::solid(format, Timestamp::from_millis(10), [255, 0, 0, 255]);
    let mut recording = Y4mRecording::new(format);
    recording.push(frame).expect("append frame");
    let bytes = recording.encode().expect("Y4M encode");
    let header = b"YUV4MPEG2 W4 H2 F30:1 Ip A0:0 C420jpeg\n";
    assert_eq!(recording.len(), 1);
    assert_eq!(&bytes[..header.len()], header);
    assert_eq!(&bytes[header.len()..header.len() + 6], b"FRAME\n");
    assert_eq!(bytes.len(), header.len() + 6 + 8 + 2 + 2);
    assert_eq!(bytes[header.len() + 6], 77);
}

#[test]
fn y4m_recording_rejects_odd_dimensions_and_backward_timestamps() {
    let odd_format = VideoFormat::new(3, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut odd_recording = Y4mRecording::new(odd_format);
    assert!(matches!(
        odd_recording.push(VideoFrame::solid(
            odd_format,
            Timestamp::ZERO,
            [0, 0, 0, 255]
        )),
        Err(OutputError::UnsupportedFormat { .. })
    ));

    let format = VideoFormat::new(4, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut recording = Y4mRecording::new(format);
    recording
        .push(VideoFrame::solid(
            format,
            Timestamp::from_millis(10),
            [0, 0, 0, 255],
        ))
        .expect("first frame");
    assert_eq!(
        recording.push(VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 255])),
        Err(OutputError::NonMonotonicTimestamp {
            previous: Timestamp::from_millis(10),
            actual: Timestamp::ZERO,
        })
    );
}
