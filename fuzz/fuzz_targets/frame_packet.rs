#![no_main]

use libfuzzer_sys::fuzz_target;
use obs_rs_capture::{CaptureKind, StreamCaptureDevice, VideoCaptureDevice};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat};
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let rate = FrameRate::new(30, 1).expect("constant frame rate");
    let format = VideoFormat::new(16, 16, rate).expect("constant format");
    if let Ok(mut device) = StreamCaptureDevice::new(
        "fuzz-stream", "Fuzz stream", CaptureKind::External, Cursor::new(data),
    ) {
        if device.start(format).is_ok() {
            let _ = device.next_frame(Timestamp::ZERO);
        }
    }
});
