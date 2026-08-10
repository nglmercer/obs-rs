use super::format;
use crate::*;
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

#[test]
fn raw_recording_round_trips_frames_and_timestamps() {
    let format = format();
    let mut recording = RawRecording::new(format);
    recording
        .push(VideoFrame::solid(
            format,
            Timestamp::from_millis(10),
            [1, 2, 3, 255],
        ))
        .expect("first frame");
    recording
        .push(VideoFrame::solid(
            format,
            Timestamp::from_millis(20),
            [4, 5, 6, 255],
        ))
        .expect("second frame");

    let encoded = recording.encode().expect("encode succeeds");
    let decoded = RawRecording::decode(&encoded).expect("decode succeeds");

    assert_eq!(decoded, recording);
    assert_eq!(encoded.get(..8), Some(MAGIC.as_slice()));
}

#[test]
fn decoder_rejects_bad_header_and_truncation() {
    let format = format();
    let mut recording = RawRecording::new(format);
    recording
        .push(VideoFrame::solid(format, Timestamp::ZERO, [0, 0, 0, 255]))
        .expect("frame");
    let encoded = recording.encode().expect("encode succeeds");

    assert_eq!(
        RawRecording::decode(b"not-a-recording"),
        Err(OutputError::InvalidHeader)
    );
    assert_eq!(
        RawRecording::decode(&encoded[..encoded.len() - 1]),
        Err(OutputError::Truncated)
    );
}

#[test]
fn recorder_rejects_other_formats() {
    let format = format();
    let other =
        VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format");
    let mut recording = RawRecording::new(format);

    assert!(matches!(
        recording.push(VideoFrame::solid(other, Timestamp::ZERO, [0, 0, 0, 255])),
        Err(OutputError::FormatMismatch { .. })
    ));
}
