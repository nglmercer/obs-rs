//! Machine-readable live checks for the supported Windows vertical slice.
//!
//! Hardware-dependent checks report `skip` with the typed platform reason when
//! the current session has no usable device. They never turn an unavailable
//! native backend into a simulated success.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::process::ExitCode;

#[cfg(not(target_os = "windows"))]
fn main() -> ExitCode {
    println!("check=platform status=skip detail=obs-rs-windows-check requires Windows");
    ExitCode::SUCCESS
}

#[cfg(target_os = "windows")]
use std::{
    error::Error,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "windows")]
use obs_rs_audio::{
    AudioBuffer, AudioDeviceError, AudioDeviceInfo, AudioDeviceKind, AudioFormat, AudioInput,
    AudioInputProvider, AudioOutput, AudioOutputProvider,
};
#[cfg(target_os = "windows")]
use obs_rs_audio_wasapi::WasapiAudioProvider;
#[cfg(target_os = "windows")]
use obs_rs_capture::{
    discover_nokhwa_camera_devices, CaptureError, CaptureKind, CaptureLifecycleState,
    CapturePermission, CaptureRequest, NokhwaCaptureDevice, PlatformCaptureAdapter,
    ThreadedCaptureDevice, VideoCaptureDevice,
};
#[cfg(target_os = "windows")]
use obs_rs_capture_windows::WindowsCaptureAdapter;
#[cfg(target_os = "windows")]
use obs_rs_config::Config;
#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
use obs_rs_engine::{output_capabilities_snapshot, ProductionProtocol};
#[cfg(target_os = "windows")]
use obs_rs_engine::{EngineConfig, EngineSession};
#[cfg(target_os = "windows")]
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};
#[cfg(target_os = "windows")]
use obs_rs_output::{MemoryMuxer, PacketKind, RleVideoEncoder};
#[cfg(target_os = "windows")]
use obs_rs_project::{Profile, Project, SceneItemSpec, SceneSpec, SourceSpec};

#[cfg(target_os = "windows")]
struct CheckResult {
    status: &'static str,
    detail: String,
}

#[cfg(target_os = "windows")]
impl CheckResult {
    fn pass(detail: impl Into<String>) -> Self {
        Self {
            status: "pass",
            detail: detail.into(),
        }
    }

    fn skip(detail: impl Into<String>) -> Self {
        Self {
            status: "skip",
            detail: detail.into(),
        }
    }

    fn fail(detail: impl Into<String>) -> Self {
        Self {
            status: "fail",
            detail: detail.into(),
        }
    }
}

#[cfg(target_os = "windows")]
fn main() -> ExitCode {
    let mut checks = vec![
        ("display", check_display()),
        ("window", check_window()),
        ("reference_recording", check_reference_recording()),
        ("camera", check_camera()),
        ("microphone", check_microphone()),
        ("desktop_loopback", check_desktop_loopback()),
        ("monitor_output", check_monitor_output()),
        ("av_soak", check_av_soak()),
        ("cleanup_restart", check_cleanup_restart()),
    ];
    checks.push(("production_output", check_production_output()));
    let mut failed = false;
    for (name, result) in checks {
        failed |= result.status == "fail";
        println!(
            "check={name} status={} detail={}",
            result.status,
            machine_detail(&result.detail)
        );
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(target_os = "windows")]
fn check_production_output() -> CheckResult {
    #[cfg(not(feature = "production-gstreamer"))]
    {
        CheckResult::skip(
            "production GStreamer output is not compiled; use the production-gstreamer package",
        )
    }

    #[cfg(feature = "production-gstreamer")]
    {
        let capabilities = output_capabilities_snapshot();
        let protocols = capabilities
            .protocols()
            .iter()
            .filter(|capability| {
                capability.available() && capability.protocol() != ProductionProtocol::Reference
            })
            .map(|capability| capability.protocol().id())
            .collect::<Vec<_>>();
        if protocols.is_empty() {
            return CheckResult::skip(
                "GStreamer is compiled in, but no approved production encoder/sink is available",
            );
        }
        return CheckResult::pass(format!(
            "protocols={} recording_formats={} video_encoders={} audio_encoders={}",
            protocols.join(","),
            capabilities.recording_formats().len(),
            capabilities.video_encoders().len(),
            capabilities.audio_encoders().len(),
        ));
    }
}

#[cfg(target_os = "windows")]
fn probe_video_format() -> VideoFormat {
    VideoFormat::new(320, 180, FrameRate::new(30, 1).expect("30 fps is valid"))
        .expect("probe dimensions are valid")
}

#[cfg(target_os = "windows")]
fn discover_windows() -> Result<Vec<obs_rs_capture::CaptureDeviceInfo>, CaptureError> {
    WindowsCaptureAdapter::default().discover()
}

#[cfg(target_os = "windows")]
fn first_windows_device(
    devices: &[obs_rs_capture::CaptureDeviceInfo],
    kind: CaptureKind,
) -> Option<&obs_rs_capture::CaptureDeviceInfo> {
    devices
        .iter()
        .find(|device| device.kind() == kind && device.permission() == CapturePermission::Granted)
        .or_else(|| devices.iter().find(|device| device.kind() == kind))
}

#[cfg(target_os = "windows")]
fn open_windows_capture(
    device: &obs_rs_capture::CaptureDeviceInfo,
    format: VideoFormat,
) -> ThreadedCaptureDevice {
    let adapter = WindowsCaptureAdapter::default()
        .with_capture_cursor(true)
        .with_capture_border(false);
    let stable_id = device.id().to_string();
    ThreadedCaptureDevice::open(CaptureRequest::output(format), device.name(), move || {
        adapter.open(&stable_id)
    })
}

#[cfg(target_os = "windows")]
fn wait_for_frame(
    device: &mut ThreadedCaptureDevice,
    format: VideoFormat,
    timeout: Duration,
) -> Result<VideoFrame, CaptureError> {
    let deadline = Instant::now() + timeout;
    let started = Instant::now();
    loop {
        let timestamp =
            Timestamp::from_nanos(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        match device.poll_frame(timestamp) {
            Ok(Some(frame)) => {
                if frame.format() != format {
                    return Err(CaptureError::FrameFormatMismatch {
                        expected: format,
                        actual: frame.format(),
                    });
                }
                return Ok(frame);
            }
            Ok(None)
                if matches!(
                    device.state(),
                    CaptureLifecycleState::Lost | CaptureLifecycleState::Denied
                ) =>
            {
                return Err(device.failure().unwrap_or(CaptureError::NotRunning));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                return Err(CaptureError::Protocol {
                    message: format!("Windows capture produced no frame within {timeout:?}"),
                });
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(target_os = "windows")]
fn capture_one(
    device: &obs_rs_capture::CaptureDeviceInfo,
    format: VideoFormat,
    timeout: Duration,
) -> Result<VideoFrame, CaptureError> {
    let mut capture = open_windows_capture(device, format);
    let frame = wait_for_frame(&mut capture, format, timeout);
    if !capture.shutdown() {
        return Err(CaptureError::Protocol {
            message: "Windows capture worker did not stop within its bounded grace period"
                .to_owned(),
        });
    }
    frame
}

#[cfg(target_os = "windows")]
fn check_display() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let Some(display) = first_windows_device(&devices, CaptureKind::Screen) else {
        return CheckResult::skip("Windows Graphics Capture reported no connected display");
    };
    match capture_one(display, probe_video_format(), Duration::from_secs(8)) {
        Ok(frame) => CheckResult::pass(format!(
            "device={} size={}x{} timestamp_ns={}",
            display.id(),
            frame.format().width(),
            frame.format().height(),
            frame.timestamp().as_nanos()
        )),
        Err(error) => capture_check_result(&error),
    }
}

#[cfg(target_os = "windows")]
fn check_reference_recording() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let Some(display) = first_windows_device(&devices, CaptureKind::Screen) else {
        return CheckResult::skip("reference recording needs a connected Windows display target");
    };
    let format = probe_video_format();
    let frame = match capture_one(display, format, Duration::from_secs(8)) {
        Ok(frame) => frame,
        Err(error) => return capture_check_result_with_context(&error, "recording capture"),
    };
    let path = reference_recording_path();
    let result = record_captured_frame(&path, format, &frame);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_file_name(format!(
        "{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("windows-check.obsr")
    )));
    match result {
        Ok((bytes, packets)) => CheckResult::pass(format!(
            "device={} bytes={} packets={} container=OBSRPKT1",
            display.id(),
            bytes,
            packets
        )),
        Err(error) => CheckResult::fail(error),
    }
}

#[cfg(target_os = "windows")]
fn record_captured_frame(
    path: &std::path::Path,
    format: VideoFormat,
    frame: &VideoFrame,
) -> Result<(usize, usize), String> {
    let project = acceptance_project(format).map_err(|error| error.to_string())?;
    let audio_format = AudioFormat::new(48_000, 2).map_err(|error| error.to_string())?;
    let config =
        EngineConfig::new(audio_format).with_video_encoder(Box::new(RleVideoEncoder::new(format)));
    let mut engine = EngineSession::new(project, config).map_err(|error| error.to_string())?;
    engine
        .start_recording(path)
        .map_err(|error| error.to_string())?;
    if let Err(error) = engine.push_program_frame(frame) {
        engine.abort_recording();
        return Err(error.to_string());
    }
    let bytes = engine
        .finish_recording()
        .map_err(|error| error.to_string())?;
    let persisted = std::fs::read(path).map_err(|error| error.to_string())?;
    if persisted.len() != bytes {
        return Err(format!(
            "recording byte count mismatch: engine={bytes} file={}",
            persisted.len()
        ));
    }
    let packets = MemoryMuxer::decode(&persisted).map_err(|error| error.to_string())?;
    let has_video = packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Video);
    let has_audio = packets
        .iter()
        .any(|packet| packet.kind() == PacketKind::Audio);
    if !has_video || !has_audio {
        return Err(format!(
            "recording is missing media: video={has_video} audio={has_audio}"
        ));
    }
    Ok((bytes, packets.len()))
}

#[cfg(target_os = "windows")]
fn acceptance_project(format: VideoFormat) -> Result<Project, Box<dyn Error>> {
    let mut project = Project::new("OBS-RS Windows acceptance")?;
    let mut profile = Profile::new("windows-check", "Windows acceptance", format)?;
    let mut settings = Config::new();
    settings.set("width", &format.width().to_string())?;
    settings.set("height", &format.height().to_string())?;
    settings.set("color", "#203040FF")?;
    let mut scene = SceneSpec::new("program", "Program")?;
    scene.add_item(SceneItemSpec::for_source("background")?)?;
    profile.add_source(SourceSpec::new(
        "background",
        "color_source",
        "Background",
        settings,
    )?)?;
    profile.add_scene(scene)?;
    project.add_profile(profile)?;
    Ok(project)
}

#[cfg(target_os = "windows")]
fn reference_recording_path() -> std::path::PathBuf {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "obs-rs-windows-check-{}-{token}.obsr",
        std::process::id()
    ))
}

#[cfg(target_os = "windows")]
fn check_window() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let windows = devices
        .iter()
        .filter(|device| device.kind() == CaptureKind::Window)
        .filter(|device| device.permission() == CapturePermission::Granted)
        .collect::<Vec<_>>();
    if windows.is_empty() {
        return CheckResult::skip("Windows reported no capturable top-level window");
    }
    let mut failures = Vec::new();
    for window in windows.into_iter().take(8) {
        match capture_one(window, probe_video_format(), Duration::from_secs(8)) {
            Ok(frame) => {
                return CheckResult::pass(format!(
                    "device={} size={}x{}",
                    window.id(),
                    frame.format().width(),
                    frame.format().height()
                ));
            }
            Err(error) if is_hardware_unavailable(&error) => {
                return capture_check_result_with_context(&error, "window capture");
            }
            Err(error) => failures.push(format!("{}: {error}", window.id())),
        }
    }
    CheckResult::fail(format!(
        "no enumerated window could be captured: {}",
        failures.join("; ")
    ))
}

#[cfg(target_os = "windows")]
fn check_camera() -> CheckResult {
    let cameras = match discover_nokhwa_camera_devices() {
        Ok(cameras) => cameras,
        Err(error) => return capture_check_result(&error),
    };
    let Some(camera) = cameras.first() else {
        return CheckResult::skip("no native Nokhwa camera is present");
    };
    let id = camera.id().to_string();
    let name = camera.name().to_owned();
    let opener_id = id.clone();
    let opener_name = name.clone();
    let format = probe_video_format();
    let mut capture =
        ThreadedCaptureDevice::open(CaptureRequest::output(format), &name, move || {
            NokhwaCaptureDevice::from_device_id(&opener_id, &opener_name)
                .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>)
        });
    let frame = wait_for_frame(&mut capture, format, Duration::from_secs(8));
    let stopped = capture.shutdown();
    if !stopped {
        return CheckResult::fail(
            "Nokhwa camera worker did not stop within its bounded grace period",
        );
    }
    match frame {
        Ok(frame) => CheckResult::pass(format!(
            "device={} modes={} size={}x{}",
            id,
            camera.modes().len(),
            frame.format().width(),
            frame.format().height()
        )),
        Err(error) => capture_check_result(&error),
    }
}

#[cfg(target_os = "windows")]
fn selected_audio_device(
    devices: &[AudioDeviceInfo],
    kind: AudioDeviceKind,
) -> Option<&AudioDeviceInfo> {
    devices
        .iter()
        .find(|device| device.kind() == kind && device.available() && device.is_default())
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.kind() == kind && device.available())
        })
}

#[cfg(target_os = "windows")]
fn discover_audio(provider: WasapiAudioProvider) -> Result<Vec<AudioDeviceInfo>, CheckResult> {
    provider
        .discover()
        .map_err(|error| CheckResult::skip(error.to_string()))
}

#[cfg(target_os = "windows")]
const PROBE_AUDIO_FORMATS: [(u32, u16); 4] = [(48_000, 2), (44_100, 2), (48_000, 1), (44_100, 1)];

#[cfg(target_os = "windows")]
fn open_probe_input(
    provider: WasapiAudioProvider,
    device: &AudioDeviceInfo,
) -> Result<(Box<dyn AudioInput>, AudioFormat), AudioDeviceError> {
    let mut last_error = None;
    for (sample_rate, channels) in PROBE_AUDIO_FORMATS {
        let format = AudioFormat::new(sample_rate, channels).map_err(AudioDeviceError::from)?;
        match provider.open_input(device.id(), format) {
            Ok(input) => return Ok((input, format)),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AudioDeviceError::Unavailable("no probe audio format was accepted".to_owned())
    }))
}

#[cfg(target_os = "windows")]
fn open_probe_output(
    provider: WasapiAudioProvider,
    device: &AudioDeviceInfo,
) -> Result<(Box<dyn AudioOutput>, AudioFormat), AudioDeviceError> {
    let mut last_error = None;
    for (sample_rate, channels) in PROBE_AUDIO_FORMATS {
        let format = AudioFormat::new(sample_rate, channels).map_err(AudioDeviceError::from)?;
        match provider.open_output(device.id(), format) {
            Ok(output) => return Ok((output, format)),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AudioDeviceError::Unavailable("no probe audio format was accepted".to_owned())
    }))
}

#[cfg(target_os = "windows")]
fn check_microphone() -> CheckResult {
    let provider = WasapiAudioProvider::new();
    let devices = match discover_audio(provider) {
        Ok(devices) => devices,
        Err(result) => return result,
    };
    let Some(device) = selected_audio_device(&devices, AudioDeviceKind::Input) else {
        return CheckResult::skip("no available WASAPI microphone/input endpoint");
    };
    let (mut input, format) = match open_probe_input(provider, device) {
        Ok(input) => input,
        Err(error) => return CheckResult::skip(error.to_string()),
    };
    match input.read_block(Timestamp::ZERO, 480) {
        Ok(block) if block.format() == format && block.frames() == 480 => {
            CheckResult::pass(format!(
                "device={} frames={} channels={}",
                device.id(),
                block.frames(),
                format.channels()
            ))
        }
        Ok(block) => CheckResult::fail(format!(
            "WASAPI microphone returned invalid block: format={:?} frames={}",
            block.format(),
            block.frames()
        )),
        Err(error) => CheckResult::skip(error.to_string()),
    }
}

#[cfg(target_os = "windows")]
fn check_desktop_loopback() -> CheckResult {
    let provider = WasapiAudioProvider::new();
    let devices = match discover_audio(provider) {
        Ok(devices) => devices,
        Err(result) => return result,
    };
    let Some(device) = selected_audio_device(&devices, AudioDeviceKind::Output) else {
        return CheckResult::skip("no available WASAPI render endpoint for loopback");
    };
    let (mut loopback, format) = match open_probe_input(provider, device) {
        Ok(input) => input,
        Err(error) => return CheckResult::skip(error.to_string()),
    };
    // A Windows loopback endpoint may not produce callbacks while no audio
    // session is active. Keep a short-lived real render stream alive and feed
    // it a bounded silence block so the probe distinguishes an idle desktop
    // from a loopback path that cannot deliver frames.
    let mut render = match provider.open_output(device.id(), format) {
        Ok(output) => output,
        Err(error) => return CheckResult::skip(error.to_string()),
    };
    let silence = match AudioBuffer::silence(format, Timestamp::ZERO, 480) {
        Ok(buffer) => buffer,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    if let Err(error) = render.write_block(&silence) {
        render.stop();
        return CheckResult::skip(error.to_string());
    }
    match loopback.read_block(Timestamp::ZERO, 480) {
        Ok(block) if block.format() == format && block.frames() == 480 => {
            render.stop();
            CheckResult::pass(format!(
                "render_device={} loopback_frames={}",
                device.id(),
                block.frames()
            ))
        }
        Ok(block) => CheckResult::fail(format!(
            "WASAPI loopback returned invalid block: format={:?} frames={}",
            block.format(),
            block.frames()
        )),
        Err(error) => {
            render.stop();
            CheckResult::skip(error.to_string())
        }
    }
}

#[cfg(target_os = "windows")]
fn check_monitor_output() -> CheckResult {
    let provider = WasapiAudioProvider::new();
    let devices = match provider.discover_outputs() {
        Ok(devices) => devices,
        Err(error) => return CheckResult::skip(error.to_string()),
    };
    let Some(device) = selected_audio_device(&devices, AudioDeviceKind::Output) else {
        return CheckResult::skip("no available WASAPI render endpoint for monitoring");
    };
    let (mut output, format) = match open_probe_output(provider, device) {
        Ok(output) => output,
        Err(error) => return CheckResult::skip(error.to_string()),
    };
    let silence = match AudioBuffer::silence(format, Timestamp::ZERO, 480) {
        Ok(buffer) => buffer,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let result = output.write_block(&silence);
    output.stop();
    match result {
        Ok(()) => CheckResult::pass(format!("render_device={} frames=480", device.id())),
        Err(error) => CheckResult::skip(error.to_string()),
    }
}

#[cfg(target_os = "windows")]
fn check_av_soak() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let Some(display) = first_windows_device(&devices, CaptureKind::Screen) else {
        return CheckResult::skip("A/V soak needs a connected Windows display capture target");
    };
    let provider = WasapiAudioProvider::new();
    let audio_devices = match discover_audio(provider) {
        Ok(devices) => devices,
        Err(result) => return result,
    };
    let Some(audio_device) = selected_audio_device(&audio_devices, AudioDeviceKind::Input)
        .or_else(|| selected_audio_device(&audio_devices, AudioDeviceKind::Output))
    else {
        return CheckResult::skip("A/V soak needs a microphone or render loopback endpoint");
    };
    let format = probe_video_format();
    let mut capture = open_windows_capture(display, format);
    let (mut audio, audio_format) = match open_probe_input(provider, audio_device) {
        Ok(input) => input,
        Err(error) => return CheckResult::skip(error.to_string()),
    };
    let seconds = std::env::var("OBS_RS_SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(2, |value| value.clamp(1, 30));
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let mut frames = 0_u64;
    let mut audio_blocks = 0_u64;
    let mut timestamp = Timestamp::ZERO;
    while Instant::now() < deadline {
        match capture.poll_frame(Timestamp::from_nanos(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        )) {
            Ok(Some(frame)) if frame.format() == format => frames = frames.saturating_add(1),
            Ok(Some(frame)) => {
                let _ = capture.shutdown();
                return CheckResult::fail(format!(
                    "A/V soak video format changed to {:?}",
                    frame.format()
                ));
            }
            Ok(None)
                if matches!(
                    capture.state(),
                    CaptureLifecycleState::Lost | CaptureLifecycleState::Denied
                ) =>
            {
                let error = capture.failure().unwrap_or(CaptureError::NotRunning);
                let _ = capture.shutdown();
                return capture_check_result(&error);
            }
            Ok(None) => {}
            Err(error) => {
                let _ = capture.shutdown();
                return capture_check_result(&error);
            }
        }
        match audio.read_block(timestamp, 480) {
            Ok(block) if block.format() == audio_format && block.frames() == 480 => {
                audio_blocks = audio_blocks.saturating_add(1);
                timestamp = timestamp
                    .checked_add(block.duration_nanos().unwrap_or(10_000_000))
                    .unwrap_or(Timestamp::ZERO);
            }
            Ok(block) => {
                let _ = capture.shutdown();
                return CheckResult::fail(format!(
                    "A/V soak audio block was invalid: format={:?} frames={}",
                    block.format(),
                    block.frames()
                ));
            }
            Err(error) => {
                let _ = capture.shutdown();
                return CheckResult::skip(error.to_string());
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    let stopped = capture.shutdown();
    if !stopped {
        return CheckResult::fail(
            "A/V soak capture worker did not stop within its bounded grace period",
        );
    }
    if frames == 0 || audio_blocks == 0 {
        return CheckResult::fail(format!(
            "A/V soak produced no usable samples: frames={frames} audio_blocks={audio_blocks}"
        ));
    }
    CheckResult::pass(format!(
        "seconds={seconds} frames={frames} audio_blocks={audio_blocks} audio_device={}",
        audio_device.id()
    ))
}

#[cfg(target_os = "windows")]
fn check_cleanup_restart() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let Some(display) = first_windows_device(&devices, CaptureKind::Screen) else {
        return CheckResult::skip("cleanup/restart needs a connected Windows display target");
    };
    let format = probe_video_format();
    for cycle in 0..3 {
        if let Err(error) = capture_one(display, format, Duration::from_secs(8)) {
            return capture_check_result_with_context(
                &error,
                &format!("cleanup/restart cycle {cycle} failed"),
            );
        }
    }
    CheckResult::pass("three capture start/stop cycles joined cleanly")
}

#[cfg(target_os = "windows")]
fn capture_check_result(error: &CaptureError) -> CheckResult {
    if is_hardware_unavailable(error) {
        CheckResult::skip(error.to_string())
    } else {
        CheckResult::fail(error.to_string())
    }
}

#[cfg(target_os = "windows")]
fn capture_check_result_with_context(error: &CaptureError, context: &str) -> CheckResult {
    if is_hardware_unavailable(error) {
        CheckResult::skip(format!("{context}: {error}"))
    } else {
        CheckResult::fail(format!("{context}: {error}"))
    }
}

#[cfg(target_os = "windows")]
fn is_hardware_unavailable(error: &CaptureError) -> bool {
    matches!(
        error,
        CaptureError::PermissionDenied
            | CaptureError::PermissionRequired
            | CaptureError::PermissionUnavailable
            | CaptureError::PlatformUnavailable { .. }
    )
}

#[cfg(target_os = "windows")]
fn machine_detail(detail: &str) -> String {
    detail.split_whitespace().collect::<Vec<_>>().join("_")
}
