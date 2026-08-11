use super::{format, unique_paths};
use crate::*;
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

#[test]
fn recording_session_commits_or_aborts_as_one_lifecycle() {
    let format = format();
    let frame = VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255]);
    let mut session = RawRecordingSession::new(format);
    session.push(frame.clone()).expect("push frame");
    let bytes = session.finalize().expect("finalize recording");
    assert_eq!(session.state(), OutputState::Finalized);
    assert_eq!(session.committed_bytes(), Some(bytes.as_slice()));
    assert!(matches!(
        session.push(frame),
        Err(OutputError::InvalidState {
            operation: "push a frame",
            state: OutputState::Finalized
        })
    ));

    let mut aborted = RawRecordingSession::new(format);
    aborted.abort().expect("abort recording");
    assert_eq!(aborted.state(), OutputState::Aborted);
    assert!(aborted.recording().is_empty());
    assert!(aborted.committed_bytes().is_none());
}

#[test]
fn atomic_packet_writer_round_trips_interleaved_packets() {
    let (final_path, temp_path) = unique_paths("packets");
    let mut writer =
        AtomicPacketFileWriter::new(&final_path, &temp_path).expect("valid packet paths");
    writer
        .push(
            EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, vec![1, 2, 3])
                .expect("video packet"),
        )
        .expect("push video");
    writer
        .push(
            EncodedPacket::new(
                PacketKind::Audio,
                Timestamp::from_millis(1),
                false,
                vec![4, 5],
            )
            .expect("audio packet"),
        )
        .expect("push audio");

    let byte_count = writer.finalize().expect("finalize packet file");
    assert_eq!(writer.state(), OutputState::Finalized);
    assert_eq!(writer.packet_count(), 2);
    assert_eq!(writer.committed_bytes(), Some(byte_count));
    assert!(!temp_path.exists());
    let bytes = std::fs::read(&final_path).expect("read packet file");
    let packets = MemoryMuxer::decode(&bytes).expect("decode packet file");
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].kind(), PacketKind::Video);
    assert_eq!(packets[1].kind(), PacketKind::Audio);
    std::fs::remove_file(final_path).expect("remove packet fixture");
}

#[test]
fn atomic_packet_writer_abort_removes_temp_and_rejects_equal_paths() {
    let (final_path, temp_path) = unique_paths("packet-abort");
    assert!(matches!(
        AtomicPacketFileWriter::new(&final_path, &final_path),
        Err(OutputError::InvalidPaths { .. })
    ));
    let mut writer =
        AtomicPacketFileWriter::new(&final_path, &temp_path).expect("valid packet paths");
    writer.abort().expect("abort packet writer");
    assert_eq!(writer.state(), OutputState::Aborted);
    assert!(!final_path.exists());
    assert!(!temp_path.exists());
    assert!(matches!(
        writer.push(
            EncodedPacket::new(PacketKind::Video, Timestamp::ZERO, true, vec![1]).expect("packet")
        ),
        Err(OutputError::InvalidState {
            operation: "push a packet",
            state: OutputState::Aborted
        })
    ));
}

#[test]
fn atomic_packet_writer_streams_large_payloads_before_finalize() {
    let (final_path, temp_path) = unique_paths("packet-streaming");
    let mut writer =
        AtomicPacketFileWriter::new(&final_path, &temp_path).expect("valid packet paths");
    assert!(temp_path.exists());

    writer
        .push(
            EncodedPacket::new(
                PacketKind::Video,
                Timestamp::ZERO,
                true,
                vec![7; 32 * 1_024],
            )
            .expect("large packet"),
        )
        .expect("stream packet");

    // A payload larger than BufWriter's buffer is forwarded to the temporary
    // file immediately instead of being retained until finalization.
    assert!(std::fs::metadata(&temp_path).expect("temporary file").len() > 32 * 1_024);
    assert!(!final_path.exists());
    writer.abort().expect("abort streamed packet writer");
    assert!(!temp_path.exists());
}

#[test]
fn atomic_file_writer_renames_only_after_successful_sync() {
    let format = format();
    let (final_path, temp_path) = unique_paths("finalize");
    let mut writer =
        AtomicRawFileWriter::new(&final_path, &temp_path, format).expect("valid writer paths");
    writer
        .push(VideoFrame::solid(format, Timestamp::ZERO, [7, 8, 9, 255]))
        .expect("push frame");

    let byte_count = writer.finalize().expect("finalize file");
    assert_eq!(writer.state(), OutputState::Finalized);
    assert_eq!(writer.committed_bytes(), Some(byte_count));
    assert!(!temp_path.exists());
    assert_eq!(
        RawRecording::decode(&std::fs::read(&final_path).expect("read final file"))
            .expect("decode final file")
            .len(),
        1
    );
    assert!(matches!(
        writer.finalize(),
        Err(OutputError::InvalidState {
            operation: "finalize",
            state: OutputState::Finalized
        })
    ));
    std::fs::remove_file(final_path).expect("remove final fixture");
}

#[test]
fn atomic_file_writer_abort_removes_temp_and_rejects_equal_paths() {
    let format = format();
    let (final_path, temp_path) = unique_paths("abort");
    assert!(matches!(
        AtomicRawFileWriter::new(&final_path, &final_path, format),
        Err(OutputError::InvalidPaths { .. })
    ));

    let mut writer =
        AtomicRawFileWriter::new(&final_path, &temp_path, format).expect("valid writer paths");
    writer.abort().expect("abort file writer");
    assert_eq!(writer.state(), OutputState::Aborted);
    assert!(!final_path.exists());
    assert!(!temp_path.exists());
    assert!(matches!(
        writer.push(VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 255])),
        Err(OutputError::InvalidState {
            operation: "push a frame",
            state: OutputState::Aborted
        })
    ));
}

#[test]
fn atomic_y4m_writer_publishes_only_a_complete_standard_stream() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("valid rate"))
        .expect("valid Y4M format");
    let (final_path, temp_path) = unique_paths("y4m-finalize");
    let mut writer =
        AtomicY4mFileWriter::new(&final_path, &temp_path, format).expect("valid Y4M writer paths");
    writer
        .push(VideoFrame::solid(format, Timestamp::ZERO, [255, 0, 0, 255]))
        .expect("push Y4M frame");

    let byte_count = writer.finalize().expect("finalize Y4M file");
    assert_eq!(writer.state(), OutputState::Finalized);
    assert_eq!(writer.frame_count(), 1);
    assert_eq!(writer.committed_bytes(), Some(byte_count));
    assert!(!temp_path.exists());
    let bytes = std::fs::read(&final_path).expect("read Y4M file");
    assert_eq!(bytes.len(), byte_count);
    assert!(bytes.starts_with(b"YUV4MPEG2 W2 H2 F30:1 Ip A0:0 C420jpeg\n"));
    assert!(bytes.windows(6).any(|window| window == b"FRAME\n"));
    std::fs::remove_file(final_path).expect("remove Y4M fixture");
}

#[test]
fn atomic_y4m_writer_abort_removes_temp_and_rejects_equal_paths() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("valid rate"))
        .expect("valid Y4M format");
    let (final_path, temp_path) = unique_paths("y4m-abort");
    assert!(matches!(
        AtomicY4mFileWriter::new(&final_path, &final_path, format),
        Err(OutputError::InvalidPaths { .. })
    ));

    let mut writer =
        AtomicY4mFileWriter::new(&final_path, &temp_path, format).expect("valid paths");
    writer.abort().expect("abort Y4M writer");
    assert_eq!(writer.state(), OutputState::Aborted);
    assert_eq!(writer.frame_count(), 0);
    assert!(!final_path.exists());
    assert!(!temp_path.exists());
    assert!(matches!(
        writer.finalize(),
        Err(OutputError::InvalidState {
            operation: "finalize",
            state: OutputState::Aborted
        })
    ));
}
