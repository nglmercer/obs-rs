//! Machine-readable live checks for the supported Linux/X11 vertical slice.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{env, error::Error, process::ExitCode, time::SystemTime};

use obs_rs_audio::{AudioDeviceKind, AudioFormat, AudioInputProvider};
use obs_rs_audio_pipewire::PipeWireAudioProvider;
use obs_rs_capture::{CaptureError, VideoCaptureDevice, X11CaptureDevice};
use obs_rs_engine::{EngineConfig, EngineSession};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{MemoryMuxer, PacketKind};

struct CheckResult {
    status: &'static str,
    detail: String,
}

impl CheckResult {
    const fn pass(detail: String) -> Self {
        Self {
            status: "pass",
            detail,
        }
    }

    const fn skip(detail: String) -> Self {
        Self {
            status: "skip",
            detail,
        }
    }

    const fn fail(detail: String) -> Self {
        Self {
            status: "fail",
            detail,
        }
    }
}

fn main() -> ExitCode {
    let checks = [
        ("x11", check_x11()),
        ("pipewire", check_pipewire()),
        ("av_soak", check_av_soak()),
    ];
    let mut failed = false;
    for (name, result) in checks {
        failed |= result.status == "fail";
        println!(
            "check={name} status={} detail={}",
            result.status,
            result.detail.replace('\n', " ")
        );
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn check_x11() -> CheckResult {
    let Ok(display) = env::var("DISPLAY") else {
        return CheckResult::skip("DISPLAY is not set".to_owned());
    };
    let mut device = match X11CaptureDevice::connect(&display, "x11-root", "X11 root") {
        Ok(device) => device,
        Err(CaptureError::PlatformUnavailable { message }) => {
            return CheckResult::skip(message);
        }
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let format = match VideoFormat::new(320, 180, FrameRate::new(30, 1).expect("valid rate")) {
        Ok(format) => format,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    if let Err(error) = device.start(format) {
        return CheckResult::fail(error.to_string());
    }
    match device.next_frame(Timestamp::ZERO) {
        Ok(Some(frame)) if frame.format() == format => {
            CheckResult::pass(format!("frame={}x{}", format.width(), format.height()))
        }
        Ok(Some(frame)) => {
            CheckResult::fail(format!("captured format mismatch: {:?}", frame.format()))
        }
        Ok(None) => CheckResult::fail("X11 returned no frame".to_owned()),
        Err(CaptureError::Protocol { message })
            if message.contains("GetImage returned X11 error code 8") =>
        {
            CheckResult::skip(format!(
                "X11 root is not directly capturable on this display: {message}"
            ))
        }
        Err(error) => CheckResult::fail(error.to_string()),
    }
}

fn check_pipewire() -> CheckResult {
    let provider = PipeWireAudioProvider::new();
    let devices = match provider.discover() {
        Ok(devices) => devices,
        Err(error) => return CheckResult::skip(error.to_string()),
    };
    let Some(device) = devices
        .iter()
        .find(|device| device.kind() == AudioDeviceKind::Input && device.available())
    else {
        return CheckResult::skip("no available PipeWire audio source".to_owned());
    };
    let format = match AudioFormat::new(48_000, 2) {
        Ok(format) => format,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let mut input = match provider.open_input(device.id(), format) {
        Ok(input) => input,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    match input.read_block(Timestamp::ZERO, 480) {
        Ok(block) => CheckResult::pass(format!(
            "device={} frames={} channels={}",
            device.id(),
            block.frames(),
            block.format().channels()
        )),
        Err(error) => CheckResult::fail(error.to_string()),
    }
}

fn check_av_soak() -> CheckResult {
    let format = match VideoFormat::new(64, 36, FrameRate::new(30, 1).expect("valid rate")) {
        Ok(format) => format,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let mut engine = match EngineSession::for_format(format, EngineConfig::default()) {
        Ok(engine) => engine,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let token = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let path = env::temp_dir().join(format!("obs-rs-linux-check-{token}.obsr"));
    let result: Result<String, Box<dyn Error>> = (|| {
        engine.start_recording(&path)?;
        let period = format
            .frame_rate()
            .period_nanos()
            .ok_or_else(|| std::io::Error::other("frame rate period does not fit in u64"))?;
        for index in 0_u64..300 {
            let timestamp = Timestamp::from_nanos(index.saturating_mul(period));
            let frame = VideoFrame::solid(format, timestamp, [24, 96, 180, 255]);
            engine.push_program_frame(&frame)?;
        }
        let bytes = engine.finish_recording()?;
        let persisted = std::fs::read(&path)?;
        let packets = MemoryMuxer::decode(&persisted)
            .map_err(|error| std::io::Error::other(format!("decode OBSRPKT1: {error}")))?;
        let stats = engine.stats();
        if persisted.len() != bytes
            || persisted.len() > 32 * 1024 * 1024
            || !packets
                .iter()
                .any(|packet| packet.kind() == PacketKind::Video)
            || !packets
                .iter()
                .any(|packet| packet.kind() == PacketKind::Audio)
            || !packets
                .windows(2)
                .all(|window| window[0].timestamp() <= window[1].timestamp())
        {
            return Err(std::io::Error::other("OBSRPKT1 A/V soak invariants failed").into());
        }
        Ok(format!(
            "ticks=300 packets={} bytes={} audio_blocks={} audio_fallback_blocks={}",
            packets.len(),
            persisted.len(),
            stats.audio_blocks,
            stats.audio_fallback_blocks
        ))
    })();
    let _ = std::fs::remove_file(&path);
    match result {
        Ok(detail) => CheckResult::pass(detail),
        Err(error) => CheckResult::fail(error.to_string()),
    }
}
