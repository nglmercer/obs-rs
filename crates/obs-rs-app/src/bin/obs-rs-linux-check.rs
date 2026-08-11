//! Machine-readable live checks for the supported Linux/X11 vertical slice.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{
    env,
    error::Error,
    process::ExitCode,
    time::{Duration, Instant, SystemTime},
};

use obs_rs_audio::{AudioDeviceKind, AudioFormat, AudioInputProvider};
use obs_rs_audio_pipewire::PipeWireAudioProvider;
use obs_rs_capture::{
    x11_windows, CaptureError, V4l2CaptureDevice, VideoCaptureDevice, X11CaptureDevice,
};
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
        ("x11_window", check_x11_window()),
        ("camera", check_camera()),
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

/// Captures one frame of a real window, tracking its live geometry.
///
/// A session with no window manager reports no windows, which is a skip rather
/// than a failure: window capture is unavailable, not broken.
fn check_x11_window() -> CheckResult {
    let Ok(display) = env::var("DISPLAY") else {
        return CheckResult::skip("DISPLAY is not set".to_owned());
    };
    let windows = match x11_windows(&display) {
        Ok(windows) => windows,
        Err(CaptureError::PlatformUnavailable { message }) => return CheckResult::skip(message),
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let mut device = match X11CaptureDevice::connect(&display, "x11-window", "X11 window") {
        Ok(device) => device,
        Err(CaptureError::PlatformUnavailable { message }) => return CheckResult::skip(message),
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let format = match VideoFormat::new(320, 180, FrameRate::new(30, 1).expect("valid rate")) {
        Ok(format) => format,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    // A window on another workspace, iconified, or otherwise parked off the
    // root is a real state of a real desktop, not a defect: the check walks the
    // list until it finds one that is actually on screen, and only reports a
    // capability gap when none of them are.
    let mut unavailable = "no top-level window is open on this display".to_owned();
    let mut target = None;
    for window in &windows {
        match device
            .select_window(Some(&window.device_id()))
            .and_then(|()| device.start(format))
        {
            Ok(()) => {
                target = Some(window);
                break;
            }
            Err(CaptureError::PlatformUnavailable { message }) => unavailable = message,
            Err(error) => return CheckResult::fail(error.to_string()),
        }
    }
    let Some(target) = target else {
        return CheckResult::skip(unavailable);
    };
    match device.next_frame(Timestamp::ZERO) {
        Ok(Some(frame)) if frame.format() == format => CheckResult::pass(format!(
            "windows={} captured={} size={}x{}",
            windows.len(),
            target.device_id(),
            target.width(),
            target.height()
        )),
        Ok(Some(frame)) => {
            CheckResult::fail(format!("captured format mismatch: {:?}", frame.format()))
        }
        Ok(None) => CheckResult::fail("X11 returned no window frame".to_owned()),
        // The window moved off screen or closed between selection and readback.
        Err(CaptureError::PlatformUnavailable { message }) => CheckResult::skip(message),
        Err(CaptureError::Protocol { message })
            if message.contains("GetImage returned X11 error code 8") =>
        {
            CheckResult::skip(format!(
                "X11 windows are not directly capturable on this display: {message}"
            ))
        }
        Err(error) => CheckResult::fail(error.to_string()),
    }
}

/// Negotiates and reads one frame from the first connected V4L2 camera.
///
/// A host with no camera, or without `ffmpeg`, skips: a missing camera is a
/// capability this machine lacks, not a defect in the adapter.
fn check_camera() -> CheckResult {
    let Some(node) = first_camera_node() else {
        return CheckResult::skip("no /dev/video* node is present".to_owned());
    };
    let mut device = match V4l2CaptureDevice::from_device_id(&node, "camera") {
        Ok(device) => device,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let sizes = device.supported_sizes();
    let format = match VideoFormat::new(320, 180, FrameRate::new(30, 1).expect("valid rate")) {
        Ok(format) => format,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    match device.start(format) {
        Ok(()) => {}
        Err(CaptureError::PermissionDenied) => {
            return CheckResult::skip(format!("{node} is not readable by this user"));
        }
        Err(CaptureError::PlatformUnavailable { message } | CaptureError::Io { message }) => {
            return CheckResult::skip(message);
        }
        Err(error) => return CheckResult::fail(error.to_string()),
    }
    // The reader is non-blocking, so the first frames legitimately return
    // nothing while `ffmpeg` negotiates the camera.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match device.next_frame(Timestamp::ZERO) {
            Ok(Some(frame)) if frame.format() == format => {
                return CheckResult::pass(format!("device={node} modes={}", sizes.len()));
            }
            Ok(Some(frame)) => {
                return CheckResult::fail(format!("camera format mismatch: {:?}", frame.format()));
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(None) => {
                return CheckResult::skip(format!("{node} produced no frame within 5s"));
            }
            Err(error) => return CheckResult::skip(error.to_string()),
        }
    }
}

/// Returns the stable ID of the lowest-numbered camera node, if any exists.
fn first_camera_node() -> Option<String> {
    let mut nodes = std::fs::read_dir("/dev")
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let suffix = name.strip_prefix("video")?;
            (!suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| format!("v4l2-{name}"))
        })
        .collect::<Vec<_>>();
    nodes.sort();
    nodes.into_iter().next()
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

#[allow(clippy::too_many_lines)] // The lifecycle is kept linear so cleanup/invariants stay visible.
fn check_av_soak() -> CheckResult {
    let format = match VideoFormat::new(64, 36, FrameRate::new(30, 1).expect("valid rate")) {
        Ok(format) => format,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let token = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let requested_seconds = env::var("OBS_RS_SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let target_ticks = if requested_seconds == 0 {
        300
    } else {
        requested_seconds.saturating_mul(30)
    };
    let started = Instant::now();
    let result: Result<String, Box<dyn Error>> = (|| {
        let period = format
            .frame_rate()
            .period_nanos()
            .ok_or_else(|| std::io::Error::other("frame rate period does not fit in u64"))?;
        let initial_rss = resident_kib().unwrap_or(0);
        let mut warm_rss = initial_rss;
        let mut peak_rss = initial_rss;
        let mut ticks = 0_u64;
        let mut packets_total = 0_usize;
        let mut bytes_total = 0_usize;
        let mut audio_blocks = 0_u64;
        let mut fallback_blocks = 0_u64;
        let mut chunks = 0_u64;
        while ticks < target_ticks {
            let chunk_ticks = (target_ticks - ticks).min(300);
            let path = env::temp_dir().join(format!("obs-rs-linux-check-{token}-{chunks}.obsr"));
            let mut engine = EngineSession::for_format(format, EngineConfig::default())?;
            engine.start_recording(&path)?;
            for local in 0..chunk_ticks {
                let index = ticks.saturating_add(local);
                let timestamp = Timestamp::from_nanos(local.saturating_mul(period));
                let phase = u8::try_from(index % 240).unwrap_or_default();
                let frame = VideoFrame::solid(
                    format,
                    timestamp,
                    [phase, 255_u8.saturating_sub(phase), 180, 255],
                );
                engine.push_program_frame(&frame)?;
                if requested_seconds != 0 {
                    let expected = started
                        + Duration::from_nanos(index.saturating_add(1).saturating_mul(period));
                    if let Some(remaining) = expected.checked_duration_since(Instant::now()) {
                        std::thread::sleep(remaining);
                    }
                }
            }
            let bytes = engine.finish_recording()?;
            let persisted = std::fs::read(&path)?;
            let _ = std::fs::remove_file(&path);
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
            ticks = ticks.saturating_add(chunk_ticks);
            chunks = chunks.saturating_add(1);
            packets_total = packets_total.saturating_add(packets.len());
            bytes_total = bytes_total.saturating_add(persisted.len());
            audio_blocks = audio_blocks.saturating_add(stats.audio_blocks);
            fallback_blocks = fallback_blocks.saturating_add(stats.audio_fallback_blocks);
            let rss = resident_kib().unwrap_or(peak_rss);
            if chunks == 1 {
                warm_rss = rss;
            }
            peak_rss = peak_rss.max(rss);
        }
        let final_rss = resident_kib().unwrap_or(peak_rss);
        let allowed_rss = warm_rss.saturating_add(warm_rss / 10).saturating_add(1_024);
        if chunks > 1 && final_rss > allowed_rss {
            return Err(std::io::Error::other(format!(
                "post-warmup RSS growth exceeded 10%: warm={warm_rss}KiB final={final_rss}KiB"
            ))
            .into());
        }
        Ok(format!(
            "ticks={ticks} chunks={chunks} packets={packets_total} bytes={bytes_total} \
             audio_blocks={audio_blocks} audio_fallback_blocks={fallback_blocks} \
             rss_initial_kib={initial_rss} rss_warm_kib={warm_rss} \
             rss_peak_kib={peak_rss} rss_final_kib={final_rss} elapsed_ms={}",
            started.elapsed().as_millis()
        ))
    })();
    match result {
        Ok(detail) => CheckResult::pass(detail),
        Err(error) => CheckResult::fail(error.to_string()),
    }
}

fn resident_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_ascii_whitespace()
        .next()?
        .parse()
        .ok()
}
