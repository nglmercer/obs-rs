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

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::{
    error::Error,
    process::{Child, Command},
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
use obs_rs_builtins::BuiltinPlugin;
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
#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
use obs_rs_output::{
    AudioEncoderConfig, HlsConfig, OutputProfile, StreamTarget, VideoEncoderConfig,
};
#[cfg(target_os = "windows")]
use obs_rs_output::{MemoryMuxer, PacketKind, RleVideoEncoder};
#[cfg(target_os = "windows")]
use obs_rs_plugin_api::Plugin;
#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
use obs_rs_plugin_api::{Source, SourceFactory, VideoRequest};
#[cfg(target_os = "windows")]
use obs_rs_project::{
    Profile, Project, ProjectFileStore, ProjectSession, SceneItemSpec, SceneSpec, SourceSpec,
};

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
        ("builtin_capture_sources", check_builtin_capture_sources()),
        ("capture_helper", check_capture_helper()),
        ("discovery_stability", check_discovery_stability()),
        ("target_persistence", check_target_persistence()),
        ("display", check_display()),
        ("display_frame_rates", check_display_frame_rates()),
        ("window", check_window()),
        ("window_lifecycle", check_window_lifecycle()),
        ("reference_recording", check_reference_recording()),
        ("camera", check_camera()),
        ("audio_device_stability", check_audio_device_stability()),
        ("microphone", check_microphone()),
        ("desktop_loopback", check_desktop_loopback()),
        ("monitor_output", check_monitor_output()),
        ("av_soak", check_av_soak()),
        ("cleanup_restart", check_cleanup_restart()),
    ];
    checks.push(("production_output", check_production_output()));
    checks.push(("production_recording", check_production_recording()));
    checks.push(("media_source", check_media_source()));
    checks.push(("production_hls", check_production_hls()));
    checks.push(("production_streaming", check_production_streaming()));
    let mut failed = false;
    for (name, result) in checks {
        let result = if result.status == "skip" && required_check(name) {
            CheckResult::fail(format!("required check skipped: {}", result.detail))
        } else {
            result
        };
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
fn check_builtin_capture_sources() -> CheckResult {
    let plugin = match BuiltinPlugin::new() {
        Ok(plugin) => plugin,
        Err(error) => return CheckResult::fail(format!("create built-in plugin: {error}")),
    };
    let mut kinds = plugin
        .source_factories()
        .iter()
        .map(|factory| factory.kind().as_str())
        .collect::<Vec<_>>();
    kinds.sort_unstable();

    let required = ["camera_capture", "screen_capture", "window_capture"];
    let missing = required
        .iter()
        .filter(|kind| !kinds.contains(kind))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return CheckResult::fail(format!(
            "Windows built-in plugin is missing capture source kinds: {}",
            missing.join(",")
        ));
    }

    let legacy = kinds
        .iter()
        .filter(|kind| {
            matches!(
                **kind,
                "wayland_screen_capture"
                    | "wayland_window_capture"
                    | "x11_screen_capture"
                    | "x11_window_capture"
            )
        })
        .copied()
        .collect::<Vec<_>>();
    if !legacy.is_empty() {
        return CheckResult::fail(format!(
            "Windows built-in plugin registered Linux-only capture kinds: {}",
            legacy.join(",")
        ));
    }

    CheckResult::pass(format!(
        "screen_capture=true window_capture=true camera_capture=true total_source_kinds={}",
        kinds.len()
    ))
}

#[cfg(target_os = "windows")]
fn check_production_recording() -> CheckResult {
    #[cfg(not(feature = "production-gstreamer"))]
    {
        CheckResult::skip(
            "production recording is not compiled; use the production-gstreamer package",
        )
    }

    #[cfg(feature = "production-gstreamer")]
    {
        let capabilities = output_capabilities_snapshot();
        let profile = OutputProfile::matroska_h264_aac();
        if !capabilities.recording_formats().contains(&profile.kind()) {
            return CheckResult::skip(format!(
                "status={} {}; required_recording_profile={}",
                capabilities.production_status().id(),
                capabilities.production_status_detail(),
                profile.kind().id()
            ));
        }
        let devices = match discover_windows() {
            Ok(devices) => devices,
            Err(error) => return capture_check_result(&error),
        };
        let format = probe_video_format();
        let (display_id, frame) = match capture_first_working_display(
            &devices,
            format,
            Duration::from_secs(8),
            "production recording",
        ) {
            Ok(capture) => capture,
            Err(result) => return result,
        };
        let path = native_recording_path();
        let result = record_native_frames(&path, format, &frame);
        let artifact = if result.is_ok() {
            preserve_native_recording(&path)
        } else {
            Ok(None)
        };
        remove_recording_artifacts(&path);
        match (result, artifact) {
            (Ok((bytes, frames)), Ok(artifact)) => {
                let artifact =
                    artifact.map_or_else(String::new, |path| format!(" artifact={path}"));
                CheckResult::pass(format!(
                    "device={} profile={} frames={} bytes={} container=Matroska{artifact}",
                    display_id,
                    profile.kind().id(),
                    frames,
                    bytes
                ))
            }
            (Ok(_), Err(error)) => CheckResult::fail(error),
            (Err(error), _) => CheckResult::fail(error),
        }
    }
}

#[cfg(target_os = "windows")]
fn check_media_source() -> CheckResult {
    #[cfg(not(feature = "production-gstreamer"))]
    {
        CheckResult::skip(
            "native media playback is not compiled; use the production-gstreamer package",
        )
    }

    #[cfg(feature = "production-gstreamer")]
    {
        let capabilities = output_capabilities_snapshot();
        let profile = OutputProfile::matroska_h264_aac();
        if !capabilities.recording_formats().contains(&profile.kind()) {
            return CheckResult::skip(format!(
                "status={} {}; media fixture requires a native Matroska recording",
                capabilities.production_status().id(),
                capabilities.production_status_detail()
            ));
        }
        let devices = match discover_windows() {
            Ok(devices) => devices,
            Err(error) => return capture_check_result(&error),
        };
        let format = probe_video_format();
        let (display_id, frame) = match capture_first_working_display(
            &devices,
            format,
            Duration::from_secs(8),
            "media source",
        ) {
            Ok(capture) => capture,
            Err(result) => return result,
        };
        let path = native_media_fixture_path();
        let recording = record_native_frames(&path, format, &frame);
        let result = recording.and_then(|(_, _)| exercise_native_media_source(&path, format));
        remove_recording_artifacts(&path);
        match result {
            Ok(frames) => CheckResult::pass(format!(
                "device={display_id} source=media_source decoded_frames={frames} replacement=true"
            )),
            Err(error) => CheckResult::fail(error),
        }
    }
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn exercise_native_media_source(path: &Path, format: VideoFormat) -> Result<u64, String> {
    let plugin = BuiltinPlugin::new().map_err(|error| format!("create built-ins: {error}"))?;
    let factory = plugin
        .source_factories()
        .iter()
        .find(|factory| factory.kind().as_str() == obs_rs_builtins::MEDIA_SOURCE_KIND)
        .ok_or_else(|| "built-in media_source factory is missing".to_owned())?;
    let path = path
        .to_str()
        .ok_or_else(|| "native media fixture path is not valid UTF-8".to_owned())?;
    let mut settings = Config::new();
    settings
        .set("path", path)
        .map_err(|error| format!("configure media source path: {error}"))?;
    settings
        .set("loop", "true")
        .map_err(|error| format!("configure media source loop: {error}"))?;
    settings
        .set("width", &format.width().to_string())
        .map_err(|error| format!("configure media source width: {error}"))?;
    settings
        .set("height", &format.height().to_string())
        .map_err(|error| format!("configure media source height: {error}"))?;
    let mut source = factory
        .create("Windows media acceptance", &settings)
        .map_err(|error| format!("open media source: {error}"))?;
    let first = source
        .render(&VideoRequest::new(Timestamp::ZERO, format))
        .map_err(|error| format!("decode first media frame: {error}"))?
        .ok_or_else(|| "media source returned no first frame".to_owned())?;
    if first.format() != format {
        return Err(format!(
            "media source returned the wrong format: expected={format:?} actual={:?}",
            first.format()
        ));
    }

    // Replacing the same path must reopen the native playback graph cleanly;
    // this is the path used when a user changes media properties in the GUI.
    source
        .update(&settings)
        .map_err(|error| format!("replace media source: {error}"))?;
    let replacement = source
        .render(&VideoRequest::new(
            Timestamp::from_nanos(33_333_333),
            format,
        ))
        .map_err(|error| format!("decode replacement media frame: {error}"))?
        .ok_or_else(|| "media source returned no replacement frame".to_owned())?;
    if replacement.format() != format {
        return Err(format!(
            "replacement media source returned the wrong format: expected={format:?} actual={:?}",
            replacement.format()
        ));
    }
    Ok(2)
}

#[cfg(target_os = "windows")]
fn check_production_streaming() -> CheckResult {
    #[cfg(not(feature = "production-gstreamer"))]
    {
        CheckResult::skip(
            "production streaming is not compiled; use the production-gstreamer package",
        )
    }

    #[cfg(feature = "production-gstreamer")]
    {
        let Some(endpoint) = std::env::var("OBS_RS_PRODUCTION_STREAM_URL")
            .ok()
            .filter(|endpoint| !endpoint.trim().is_empty())
        else {
            return CheckResult::skip(
                "set OBS_RS_PRODUCTION_STREAM_URL to run the live production-stream acceptance check",
            );
        };
        let Some((scheme, _)) = endpoint.split_once("://") else {
            return CheckResult::fail(
                "OBS_RS_PRODUCTION_STREAM_URL must use rtmp://, rtmps://, srt://, rist://, whip://, or webrtc://",
            );
        };
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(
            scheme.as_str(),
            "rtmp" | "rtmps" | "srt" | "rist" | "whip" | "webrtc"
        ) {
            return CheckResult::fail(format!(
                "unsupported production stream scheme {scheme}; expected rtmp/rtmps/srt/rist/whip/webrtc"
            ));
        }
        let capability_protocol = if matches!(scheme.as_str(), "whip" | "webrtc") {
            "webrtc"
        } else {
            scheme.as_str()
        };
        let capabilities = output_capabilities_snapshot();
        if !capabilities.protocols().iter().any(|capability| {
            capability.available() && capability.protocol().id() == capability_protocol
        }) {
            return CheckResult::skip(format!(
                "status={} {}; required_protocol={capability_protocol}",
                capabilities.production_status().id(),
                capabilities.production_status_detail()
            ));
        }
        let devices = match discover_windows() {
            Ok(devices) => devices,
            Err(error) => return capture_check_result(&error),
        };
        let format = probe_video_format();
        let (display_id, frame) = match capture_first_working_display(
            &devices,
            format,
            Duration::from_secs(8),
            "production streaming",
        ) {
            Ok(capture) => capture,
            Err(result) => return result,
        };
        let mut engine = match acceptance_engine(format) {
            Ok(engine) => engine,
            Err(error) => return CheckResult::fail(error),
        };
        if let Err(error) = engine.start_streaming(&endpoint) {
            return CheckResult::fail(format!("start {scheme} stream: {error}"));
        }
        for index in 0..4_u64 {
            let timestamp = Timestamp::from_nanos(index.saturating_mul(33_333_333));
            let stamped = frame.at_timestamp(timestamp);
            if let Err(error) = engine.push_program_frame(&stamped) {
                engine.finish_streaming().ok();
                return CheckResult::fail(format!("push {scheme} stream media: {error}"));
            }
        }
        let metrics = engine.snapshot().production_stream_metrics;
        if let Err(error) = engine.finish_streaming() {
            return CheckResult::fail(format!("finish {scheme} stream: {error}"));
        }
        let Some(metrics) = metrics else {
            return CheckResult::fail(
                "production stream did not expose native submission telemetry",
            );
        };
        if metrics.video_submitted == 0 || metrics.audio_submitted == 0 {
            return CheckResult::fail(format!(
                "production stream submitted no complete media: video={} audio={} dropped={}",
                metrics.video_submitted, metrics.audio_submitted, metrics.dropped
            ));
        }
        CheckResult::pass(format!(
            "device={} protocol={} video_submitted={} audio_submitted={} dropped={}",
            display_id, scheme, metrics.video_submitted, metrics.audio_submitted, metrics.dropped
        ))
    }
}

#[cfg(target_os = "windows")]
fn check_production_hls() -> CheckResult {
    #[cfg(not(feature = "production-gstreamer"))]
    {
        CheckResult::skip(
            "production HLS output is not compiled; use the production-gstreamer package",
        )
    }

    #[cfg(feature = "production-gstreamer")]
    {
        let capabilities = output_capabilities_snapshot();
        if !capabilities
            .protocols()
            .iter()
            .any(|capability| capability.available() && capability.protocol().id() == "hls")
        {
            return CheckResult::skip(format!(
                "status={} {}; required_protocol=hls",
                capabilities.production_status().id(),
                capabilities.production_status_detail()
            ));
        }

        let devices = match discover_windows() {
            Ok(devices) => devices,
            Err(error) => return capture_check_result(&error),
        };
        let format = probe_video_format();
        let (display_id, frame) = match capture_first_working_display(
            &devices,
            format,
            Duration::from_secs(8),
            "production HLS",
        ) {
            Ok(capture) => capture,
            Err(result) => return result,
        };
        let directory = native_hls_directory();
        let result = (|| -> Result<(u64, u64), String> {
            let target = StreamTarget::Hls(HlsConfig {
                directory: directory.clone(),
                segment_duration_secs: 1,
                playlist_size: 3,
                low_latency: false,
            });
            let mut engine = acceptance_engine(format)?;
            if let Err(error) = engine.start_streaming_target_configured(
                &target,
                &VideoEncoderConfig::default(),
                &AudioEncoderConfig::default(),
            ) {
                let _ = engine.finish_streaming();
                return Err(format!("start native HLS output: {error}"));
            }
            for index in 0_u64..40 {
                let timestamp = Timestamp::from_nanos(index.saturating_mul(33_333_333));
                let stamped = frame.at_timestamp(timestamp);
                if let Err(error) = engine.push_program_frame(&stamped) {
                    let _ = engine.finish_streaming();
                    return Err(format!("push native HLS media: {error}"));
                }
            }
            engine
                .finish_streaming()
                .map_err(|error| format!("finish native HLS output: {error}"))?;

            let playlist_path = directory.join("playlist.m3u8");
            let playlist = std::fs::read_to_string(&playlist_path)
                .map_err(|error| format!("read native HLS playlist: {error}"))?;
            if !playlist.starts_with("#EXTM3U") || !playlist.contains("#EXTINF:") {
                return Err("native HLS playlist is missing media entries".to_owned());
            }
            let mut segments = 0_u64;
            let mut bytes = 0_u64;
            for entry in std::fs::read_dir(&directory)
                .map_err(|error| format!("read native HLS directory: {error}"))?
            {
                let entry = entry.map_err(|error| format!("read native HLS entry: {error}"))?;
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("ts"))
                {
                    let length = entry
                        .metadata()
                        .map_err(|error| format!("stat native HLS segment: {error}"))?
                        .len();
                    if length > 0 {
                        segments = segments.saturating_add(1);
                        bytes = bytes.saturating_add(length);
                    }
                }
            }
            if segments == 0 || bytes == 0 {
                return Err(format!(
                    "native HLS output has no non-empty transport segments: segments={segments} bytes={bytes}"
                ));
            }
            Ok((segments, bytes))
        })();
        let cleanup = remove_hls_directory(&directory);
        match (result, cleanup) {
            (Ok((segments, bytes)), Ok(())) => CheckResult::pass(format!(
                "device={display_id} protocol=hls segments={segments} bytes={bytes}"
            )),
            (Ok(_), Err(error)) => CheckResult::fail(error),
            (Err(error), Ok(())) => CheckResult::fail(error),
            (Err(error), Err(cleanup_error)) => {
                CheckResult::fail(format!("{error}; cleanup failed: {cleanup_error}"))
            }
        }
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
            return CheckResult::skip(format!(
                "status={} {}",
                capabilities.production_status().id(),
                capabilities.production_status_detail()
            ));
        }
        return CheckResult::pass(format!(
            "status={} protocols={} recording_formats={} video_encoders={} audio_encoders={} detail={}",
            capabilities.production_status().id(),
            protocols.join(","),
            capabilities.recording_formats().len(),
            capabilities.video_encoders().len(),
            capabilities.audio_encoders().len(),
            capabilities.production_status_detail(),
        ));
    }
}

#[cfg(target_os = "windows")]
fn check_capture_helper() -> CheckResult {
    let adapter = WindowsCaptureAdapter::default();
    match adapter.helper_version() {
        Ok(version) => CheckResult::pass(format!(
            "protocol={} version={} path={}",
            obs_rs_capture_windows::WINDOWS_HELPER_PROTOCOL,
            version,
            adapter.helper().display()
        )),
        Err(error) => capture_check_result(&error),
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
fn capture_first_working_display(
    devices: &[obs_rs_capture::CaptureDeviceInfo],
    format: VideoFormat,
    timeout: Duration,
    context: &str,
) -> Result<(String, VideoFrame), CheckResult> {
    let displays = windows_display_candidates(devices);
    if displays.is_empty() {
        return Err(CheckResult::skip(format!(
            "{context} needs a connected Windows display target"
        )));
    }

    let mut failures = Vec::new();
    let mut unavailable = 0_usize;
    for display in displays.iter().copied() {
        match capture_one(display, format, timeout) {
            Ok(frame) => return Ok((display.id().to_string(), frame)),
            Err(error) if is_hardware_unavailable(&error) => {
                unavailable = unavailable.saturating_add(1);
                failures.push(format!("{}: unavailable: {error}", display.id()));
            }
            Err(error) => failures.push(format!("{}: {error}", display.id())),
        }
    }

    let detail = format!(
        "{context} could not capture any enumerated display: {}",
        failures.join("; ")
    );
    if unavailable == displays.len() {
        Err(CheckResult::skip(detail))
    } else {
        Err(CheckResult::fail(detail))
    }
}

#[cfg(target_os = "windows")]
fn windows_display_candidates(
    devices: &[obs_rs_capture::CaptureDeviceInfo],
) -> Vec<&obs_rs_capture::CaptureDeviceInfo> {
    devices
        .iter()
        .filter(|device| device.kind() == CaptureKind::Screen)
        .filter(|device| device.permission() == CapturePermission::Granted)
        .take(8)
        .collect()
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
fn wait_for_capture_frames(
    device: &mut ThreadedCaptureDevice,
    format: VideoFormat,
    target_frames: usize,
    timeout: Duration,
) -> Result<(VideoFrame, usize, Duration), CaptureError> {
    let target_frames = target_frames.max(1);
    let target_frames_u64 = u64::try_from(target_frames).unwrap_or(u64::MAX);
    let deadline = Instant::now() + timeout;
    let started = Instant::now();
    let mut first = None;
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
                first.get_or_insert(frame);
                let published = device.published_frames();
                if published >= target_frames_u64 {
                    let elapsed = started.elapsed();
                    return first.map_or_else(
                        || {
                            Err(CaptureError::Protocol {
                                message: "Windows capture returned no first frame".to_owned(),
                            })
                        },
                        |frame| {
                            Ok((
                                frame,
                                usize::try_from(published).unwrap_or(usize::MAX),
                                elapsed,
                            ))
                        },
                    );
                }
                if matches!(
                    device.state(),
                    CaptureLifecycleState::Lost | CaptureLifecycleState::Denied
                ) {
                    return Err(device.failure().unwrap_or(CaptureError::NotRunning));
                }
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
                    message: format!(
                        "Windows capture produced {}/{} published frames within {timeout:?}",
                        device.published_frames(),
                        target_frames
                    ),
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
    capture_frames(device, format, 1, timeout).map(|(frame, _, _)| frame)
}

#[cfg(target_os = "windows")]
fn capture_frames(
    device: &obs_rs_capture::CaptureDeviceInfo,
    format: VideoFormat,
    target_frames: usize,
    timeout: Duration,
) -> Result<(VideoFrame, usize, Duration), CaptureError> {
    let mut capture = open_windows_capture(device, format);
    let result = wait_for_capture_frames(&mut capture, format, target_frames, timeout);
    let stopped = capture.shutdown();
    if !stopped {
        return Err(CaptureError::Protocol {
            message: "Windows capture worker did not stop within its bounded grace period"
                .to_owned(),
        });
    }
    result
}

#[cfg(target_os = "windows")]
fn check_discovery_stability() -> CheckResult {
    let first = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let second = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let ids = |devices: &[obs_rs_capture::CaptureDeviceInfo], kind: CaptureKind| {
        let mut ids = devices
            .iter()
            .filter(|device| device.kind() == kind)
            .map(|device| device.id().to_string())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };
    let first_displays = ids(&first, CaptureKind::Screen);
    let second_displays = ids(&second, CaptureKind::Screen);
    if first_displays.is_empty() {
        return CheckResult::skip("Windows Graphics Capture reported no displays");
    }
    if first_displays != second_displays {
        return CheckResult::fail(format!(
            "display IDs changed between immediate discovery calls: first={first_displays:?} second={second_displays:?}"
        ));
    }
    let adapter = WindowsCaptureAdapter::default();
    let first_display_info = match adapter.discover_displays() {
        Ok(displays) => displays,
        Err(error) => return capture_check_result(&error),
    };
    let second_display_info = match adapter.discover_displays() {
        Ok(displays) => displays,
        Err(error) => return capture_check_result(&error),
    };
    if first_display_info != second_display_info {
        return CheckResult::fail(format!(
            "display metadata changed between immediate discovery calls: first={first_display_info:?} second={second_display_info:?}"
        ));
    }
    let primary_displays = first_display_info
        .iter()
        .filter(|display| display.primary)
        .count();
    if primary_displays != 1 {
        return CheckResult::fail(format!(
            "display discovery reported {primary_displays} primary displays"
        ));
    }
    if first_display_info
        .iter()
        .any(|display| display.width == 0 || display.height == 0)
    {
        return CheckResult::fail("display discovery reported a zero-sized display");
    }
    let negative_origin_displays = first_display_info
        .iter()
        .filter(|display| display.x < 0 || display.y < 0)
        .count();
    let first_windows = ids(&first, CaptureKind::Window);
    let second_windows = ids(&second, CaptureKind::Window);
    let duplicate_ids = |ids: &[String]| ids.windows(2).any(|pair| pair[0] == pair[1]);
    if duplicate_ids(&first_windows) || duplicate_ids(&second_windows) {
        return CheckResult::fail("window discovery returned duplicate stable IDs");
    }
    let shared_windows = first_windows
        .iter()
        .filter(|id| second_windows.binary_search(id).is_ok())
        .count();
    if !first_windows.is_empty() && !second_windows.is_empty() && shared_windows == 0 {
        return CheckResult::fail(
            "window IDs were not stable across immediate discovery calls; target persistence is unsafe",
        );
    }
    CheckResult::pass(format!(
        "displays={} primary_displays={primary_displays} negative_origin_displays={negative_origin_displays} display_metadata_stable=true windows_first={} windows_second={} shared_windows={shared_windows}",
        first_display_info.len(),
        first_windows.len(),
        second_windows.len()
    ))
}

#[cfg(target_os = "windows")]
fn check_target_persistence() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let Some(display) = first_windows_device(&devices, CaptureKind::Screen) else {
        return CheckResult::skip("target persistence needs a connected Windows display");
    };
    let format = probe_video_format();
    // Discovery can include a window that disappears or rejects conversion to
    // GraphicsCaptureItem between enumeration and open (for example a short
    // lived popup or a protected surface). Select the first target that really
    // produces a frame so this check tests persistence rather than the order
    // of the helper's catalog.
    let window = {
        let candidates = devices
            .iter()
            .filter(|device| device.kind() == CaptureKind::Window)
            .filter(|device| device.permission() == CapturePermission::Granted)
            .take(8)
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        let mut unavailable = 0_usize;
        let mut selected = None;
        for candidate in &candidates {
            match capture_one(candidate, format, Duration::from_secs(8)) {
                Ok(_) => {
                    selected = Some(*candidate);
                    break;
                }
                Err(error) if is_hardware_unavailable(&error) => {
                    unavailable = unavailable.saturating_add(1);
                    failures.push(format!("{}: unavailable: {error}", candidate.id()));
                }
                Err(error) => failures.push(format!("{}: {error}", candidate.id())),
            }
        }
        if let Some(selected) = selected {
            Some(selected)
        } else if candidates.is_empty() {
            None
        } else if unavailable == candidates.len() {
            return CheckResult::skip(format!(
                "target persistence found no capturable window: {}",
                failures.join("; ")
            ));
        } else {
            return CheckResult::fail(format!(
                "target persistence found no capturable window: {}",
                failures.join("; ")
            ));
        }
    };
    let mut project = match Project::new("OBS-RS Windows target persistence") {
        Ok(project) => project,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let mut profile = match Profile::new("windows-check", "Windows acceptance", format) {
        Ok(profile) => profile,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let mut scene = match SceneSpec::new("program", "Program") {
        Ok(scene) => scene,
        Err(error) => return CheckResult::fail(error.to_string()),
    };
    let mut expected = Vec::new();
    let mut add_target = |id: &str, kind: &str, device_id: &str, monitor: Option<&str>| {
        let mut settings = Config::new();
        settings.set("width", &format.width().to_string())?;
        settings.set("height", &format.height().to_string())?;
        settings.set("device_id", device_id)?;
        settings.set("capture_cursor", "true")?;
        settings.set("capture_border", "false")?;
        if let Some(monitor) = monitor {
            settings.set("monitor", monitor)?;
        }
        profile.add_source(SourceSpec::new(id, kind, id, settings)?)?;
        scene.add_item(SceneItemSpec::for_source(id)?)?;
        expected.push((
            id.to_owned(),
            kind.to_owned(),
            device_id.to_owned(),
            monitor.map(str::to_owned),
        ));
        Ok::<(), Box<dyn Error>>(())
    };
    if let Err(error) = add_target(
        "display",
        "screen_capture",
        display.id().as_str(),
        Some(display.id().as_str()),
    ) {
        return CheckResult::fail(error.to_string());
    }
    if let Some(window) = window {
        if let Err(error) = add_target("window", "window_capture", window.id().as_str(), None) {
            return CheckResult::fail(error.to_string());
        }
    }
    if let Err(error) = profile
        .add_scene(scene)
        .and_then(|()| project.add_profile(profile))
    {
        return CheckResult::fail(error.to_string());
    }
    let final_path = target_persistence_path();
    let temp_path = final_path.with_extension("obsr.tmp");
    let store = match ProjectFileStore::new(&final_path, &temp_path) {
        Ok(store) => store,
        Err(error) => return CheckResult::fail(format!("create target project store: {error}")),
    };
    let persisted = (|| {
        let mut session = ProjectSession::new(project);
        let bytes = store
            .save(&mut session)
            .map_err(|error| format!("save target project: {error}"))?;
        let decoded = store
            .load()
            .map_err(|error| format!("load target project: {error}"))?;
        Ok::<_, String>((decoded, bytes))
    })();
    let _ = std::fs::remove_file(&final_path);
    let _ = std::fs::remove_file(&temp_path);
    let (decoded, bytes) = match persisted {
        Ok(persisted) => persisted,
        Err(error) => return CheckResult::fail(error),
    };
    let Some(profile) = decoded.profile("windows-check") else {
        return CheckResult::fail("persisted target project lost its profile");
    };
    for (id, kind, device_id, monitor) in &expected {
        let Some(source) = profile.source(id.as_str()) else {
            return CheckResult::fail(format!("persisted target {id} is missing"));
        };
        if source.kind().as_str() != kind
            || source.settings().get("device_id") != Some(device_id.as_str())
            || source.settings().get("monitor") != monitor.as_deref()
        {
            return CheckResult::fail(format!(
                "persisted target {id} changed: kind={} device_id={:?} monitor={:?}",
                source.kind().as_str(),
                source.settings().get("device_id"),
                source.settings().get("monitor")
            ));
        }
    }
    let persisted_display = devices.iter().find(|device| {
        device.kind() == CaptureKind::Screen && device.id().as_str() == display.id().as_str()
    });
    let Some(persisted_display) = persisted_display else {
        return CheckResult::fail(format!(
            "display target {} disappeared after project reload",
            display.id()
        ));
    };
    let (_, display_frames, _) =
        match capture_frames(persisted_display, format, 1, Duration::from_secs(8)) {
            Ok(result) => result,
            Err(error) => {
                return capture_check_result_with_context(
                    &error,
                    "capture persisted display target after project reload",
                )
            }
        };
    let mut captured_targets = 1_usize;
    if let Some(expected_window) = expected
        .iter()
        .find(|(_, kind, _, _)| kind == "window_capture")
    {
        let Some(persisted_window) = devices.iter().find(|device| {
            device.kind() == CaptureKind::Window
                && device.id().as_str() == expected_window.2.as_str()
        }) else {
            return CheckResult::fail(format!(
                "window target {} disappeared after project reload",
                expected_window.2
            ));
        };
        match capture_frames(persisted_window, format, 1, Duration::from_secs(8)) {
            Ok(_) => captured_targets = captured_targets.saturating_add(1),
            Err(error) => {
                return capture_check_result_with_context(
                    &error,
                    "capture persisted window target after project reload",
                )
            }
        }
    }
    CheckResult::pass(format!(
        "persisted_targets={} captured_after_reload={} display_frames={} bytes={} file_round_trip=true",
        expected.len(),
        captured_targets,
        display_frames,
        bytes
    ))
}

#[cfg(target_os = "windows")]
fn check_display_frame_rates() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let displays = windows_display_candidates(&devices);
    if displays.is_empty() {
        return CheckResult::skip("frame-rate acceptance needs a connected display target");
    }
    let mut failures = Vec::new();
    let mut unavailable = 0_usize;
    for display in &displays {
        let mut results = Vec::new();
        let mut candidate_unavailable = false;
        for fps in [30_u32, 60_u32] {
            let format = match VideoFormat::new(320, 180, FrameRate::new(fps, 1).expect("fps")) {
                Ok(format) => format,
                Err(error) => return CheckResult::fail(error.to_string()),
            };
            match capture_frames(display, format, 4, Duration::from_secs(8)) {
                Ok((_, frames, elapsed)) => results.push(format!(
                    "{fps}fps_frames={frames}_elapsed_ms={}",
                    elapsed.as_millis()
                )),
                Err(error) if is_hardware_unavailable(&error) => {
                    candidate_unavailable = true;
                    failures.push(format!("{}: {fps} FPS unavailable: {error}", display.id()));
                    break;
                }
                Err(error) => {
                    failures.push(format!("{}: {fps} FPS: {error}", display.id()));
                    break;
                }
            }
        }
        if results.len() == 2 {
            return CheckResult::pass(format!("device={} {}", display.id(), results.join(" ")));
        }
        if candidate_unavailable {
            unavailable = unavailable.saturating_add(1);
        }
    }
    let detail = format!(
        "no enumerated display passed 30/60 FPS capture: {}",
        failures.join("; ")
    );
    if unavailable == displays.len() {
        CheckResult::skip(detail)
    } else {
        CheckResult::fail(detail)
    }
}

#[cfg(target_os = "windows")]
fn check_display() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let displays = windows_display_candidates(&devices);
    if displays.is_empty() {
        return CheckResult::skip("Windows Graphics Capture reported no connected display");
    }
    let mut failures = Vec::new();
    let mut unavailable = 0_usize;
    for display in &displays {
        match capture_one(display, probe_video_format(), Duration::from_secs(8)) {
            Ok(frame) => {
                return CheckResult::pass(format!(
                    "device={} size={}x{} timestamp_ns={}",
                    display.id(),
                    frame.format().width(),
                    frame.format().height(),
                    frame.timestamp().as_nanos()
                ));
            }
            Err(error) if is_hardware_unavailable(&error) => {
                unavailable = unavailable.saturating_add(1);
                failures.push(format!("{}: unavailable: {error}", display.id()));
            }
            Err(error) => failures.push(format!("{}: {error}", display.id())),
        }
    }
    let detail = format!(
        "no enumerated display could be captured: {}",
        failures.join("; ")
    );
    if unavailable == displays.len() {
        CheckResult::skip(detail)
    } else {
        CheckResult::fail(detail)
    }
}

#[cfg(target_os = "windows")]
fn check_reference_recording() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let format = probe_video_format();
    let (display_id, frame) = match capture_first_working_display(
        &devices,
        format,
        Duration::from_secs(8),
        "reference recording",
    ) {
        Ok(capture) => capture,
        Err(result) => return result,
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
            "device={display_id} bytes={bytes} packets={packets} container=OBSRPKT1"
        )),
        Err(error) => CheckResult::fail(error),
    }
}

#[cfg(target_os = "windows")]
fn acceptance_engine(format: VideoFormat) -> Result<EngineSession, String> {
    let project = acceptance_project(format).map_err(|error| error.to_string())?;
    let audio_format = AudioFormat::new(48_000, 2).map_err(|error| error.to_string())?;
    let config =
        EngineConfig::new(audio_format).with_video_encoder(Box::new(RleVideoEncoder::new(format)));
    EngineSession::new(project, config).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn record_captured_frame(
    path: &std::path::Path,
    format: VideoFormat,
    frame: &VideoFrame,
) -> Result<(usize, usize), String> {
    let mut engine = acceptance_engine(format)?;
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

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn record_native_frames(
    path: &Path,
    format: VideoFormat,
    frame: &VideoFrame,
) -> Result<(usize, u64), String> {
    let mut engine = acceptance_engine(format)?;
    engine
        .start_recording_profile(path, OutputProfile::matroska_h264_aac())
        .map_err(|error| format!("start native recording: {error}"))?;
    const ACCEPTANCE_FRAMES: u64 = 40;
    for index in 0..ACCEPTANCE_FRAMES {
        let timestamp = Timestamp::from_nanos(index.saturating_mul(33_333_333));
        let stamped = frame.at_timestamp(timestamp);
        if let Err(error) = engine.push_program_frame(&stamped) {
            engine.abort_recording();
            return Err(format!("push native recording media: {error}"));
        }
    }
    let bytes = match engine.finish_recording() {
        Ok(bytes) => bytes,
        Err(error) => {
            engine.abort_recording();
            return Err(format!("finish native recording: {error}"));
        }
    };
    let persisted = std::fs::read(path).map_err(|error| error.to_string())?;
    if bytes == 0 || persisted.len() != bytes {
        return Err(format!(
            "native recording byte count is invalid: engine={bytes} file={}",
            persisted.len()
        ));
    }
    if persisted.len() < 4 || persisted[..4] != [0x1A, 0x45, 0xDF, 0xA3] {
        return Err("native recording is not an EBML/Matroska file".to_owned());
    }
    validate_native_recording(path)?;
    Ok((bytes, ACCEPTANCE_FRAMES))
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
fn target_persistence_path() -> std::path::PathBuf {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "obs-rs-windows-check-{}-{token}-targets.obsr",
        std::process::id()
    ))
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn native_recording_path() -> PathBuf {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "obs-rs-windows-check-{}-{token}.mkv",
        std::process::id()
    ))
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn native_media_fixture_path() -> PathBuf {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "obs-rs-windows-check-{}-{token}-media.mkv",
        std::process::id()
    ))
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn native_hls_directory() -> PathBuf {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "obs-rs-windows-check-{}-{token}-hls",
        std::process::id()
    ))
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn remove_hls_directory(directory: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "remove native HLS directory {}: {error}",
            directory.display()
        )),
    }
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn remove_recording_artifacts(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("mkv.part"));
    let _ = std::fs::remove_file(path.with_extension("mp4.part"));
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn gst_discoverer_command() -> Command {
    if let Some(path) = std::env::var_os("OBSR_GST_DISCOVERER").filter(|path| !path.is_empty()) {
        return Command::new(path);
    }
    let executable_name = "gst-discoverer-1.0.exe";
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            for bundled in [
                parent.join(executable_name),
                parent.join("gstreamer").join("bin").join(executable_name),
            ] {
                if bundled.is_file() {
                    return Command::new(bundled);
                }
            }
        }
    }
    Command::new(executable_name)
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn validate_native_recording(path: &Path) -> Result<(), String> {
    let output = gst_discoverer_command()
        .arg("-v")
        .arg(path)
        .output()
        .map_err(|error| format!("start gst-discoverer-1.0: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    if !output.status.success() {
        return Err(format!(
            "gst-discoverer-1.0 rejected the production recording: {}",
            bounded_external_output(&combined)
        ));
    }
    let lower = combined.to_ascii_lowercase();
    let has_video = lower
        .lines()
        .any(|line| line.contains("video:") || line.contains("video/x-"));
    let has_audio = lower
        .lines()
        .any(|line| line.contains("audio:") || line.contains("audio/x-"));
    if !has_video || !has_audio {
        return Err(format!(
            "gst-discoverer-1.0 did not report both video and audio streams: video={has_video} audio={has_audio}"
        ));
    }
    Ok(())
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn bounded_external_output(value: &str) -> String {
    value
        .chars()
        .take(512)
        .map(|character| match character {
            '\r' | '\n' | '\t' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(all(target_os = "windows", feature = "production-gstreamer"))]
fn preserve_native_recording(path: &Path) -> Result<Option<String>, String> {
    let Some(directory) = std::env::var_os("OBS_RS_ACCEPTANCE_ARTIFACTS") else {
        return Ok(None);
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "create production recording artifact directory {}: {error}",
            directory.display()
        )
    })?;
    let destination = directory.join("production-recording.mkv");
    std::fs::copy(path, &destination).map_err(|error| {
        format!(
            "preserve production recording artifact {}: {error}",
            destination.display()
        )
    })?;
    Ok(Some(destination.display().to_string()))
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
    let mut unavailable = 0_usize;
    let candidates = windows.into_iter().take(8).collect::<Vec<_>>();
    for window in &candidates {
        match capture_frames(window, probe_video_format(), 4, Duration::from_secs(8)) {
            Ok((frame, frames, elapsed)) => {
                return CheckResult::pass(format!(
                    "device={} size={}x{} frames={} elapsed_ms={}",
                    window.id(),
                    frame.format().width(),
                    frame.format().height(),
                    frames,
                    elapsed.as_millis()
                ));
            }
            Err(error) if is_hardware_unavailable(&error) => {
                unavailable = unavailable.saturating_add(1);
                failures.push(format!("{}: unavailable: {error}", window.id()));
            }
            Err(error) => failures.push(format!("{}: {error}", window.id())),
        }
    }
    if unavailable == candidates.len() {
        return CheckResult::skip(format!(
            "no enumerated window could be captured: {}",
            failures.join("; ")
        ));
    }
    CheckResult::fail(format!(
        "no enumerated window could be captured: {}",
        failures.join("; ")
    ))
}

/// Opens a real top-level Windows window, captures its PID/HWND target, closes
/// it, and verifies that the old target is rejected instead of falling back to
/// whichever window happens to be in the foreground. This is intentionally a
/// separate check from ordinary window capture: discovery stability alone does
/// not prove that a persisted target remains authoritative after destruction.
#[cfg(target_os = "windows")]
fn check_window_lifecycle() -> CheckResult {
    let mut process = match Command::new("notepad.exe").spawn() {
        Ok(process) => process,
        Err(error) => {
            return CheckResult::skip(format!(
                "could not start a test window for lifecycle acceptance: {error}"
            ))
        }
    };
    let result = check_window_lifecycle_process(&mut process);
    // The process may have been closed by the test already. Only perform the
    // fallback cleanup when the lifecycle check returned early; the normal
    // path already waited for the child after closing its window.
    if !matches!(process.try_wait(), Ok(Some(_))) {
        let _ = process.kill();
        let _ = process.wait();
    }
    result
}

#[cfg(target_os = "windows")]
fn check_window_lifecycle_process(process: &mut Child) -> CheckResult {
    let pid = process.id();
    let prefix = format!("wgc-window-{pid:08x}-");
    let deadline = Instant::now() + Duration::from_secs(8);
    let target = loop {
        let devices = match discover_windows() {
            Ok(devices) => devices,
            Err(error) => {
                return capture_check_result_with_context(&error, "window lifecycle discovery")
            }
        };
        if let Some(target) = devices.into_iter().find(|device| {
            device.kind() == CaptureKind::Window
                && device.permission() == CapturePermission::Granted
                && device.id().as_str().starts_with(&prefix)
        }) {
            break target;
        }
        if Instant::now() >= deadline {
            return CheckResult::skip(format!(
                "test window PID {pid} was not exposed as a capturable WGC target"
            ));
        }
        thread::sleep(Duration::from_millis(100));
    };

    let format = probe_video_format();
    let (frame, frames, elapsed) = match capture_frames(&target, format, 4, Duration::from_secs(8))
    {
        Ok(result) => result,
        Err(error) => {
            return capture_check_result_with_context(&error, "window lifecycle initial capture")
        }
    };

    if let Err(error) = process.kill() {
        // A test window can close itself during startup. If it is already gone,
        // `wait` below still gives us the normal cleanup path.
        if process.try_wait().ok().flatten().is_none() {
            return CheckResult::fail(format!("close test window PID {pid}: {error}"));
        }
    }
    if let Err(error) = process.wait() {
        return CheckResult::fail(format!("wait for test window PID {pid}: {error}"));
    }

    let disappeared_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let devices = match discover_windows() {
            Ok(devices) => devices,
            Err(error) => {
                return CheckResult::fail(format!(
                    "rediscover closed test window PID {pid}: {error}"
                ))
            }
        };
        if !devices.iter().any(|device| device.id() == target.id()) {
            break;
        }
        if Instant::now() >= disappeared_deadline {
            return CheckResult::fail(format!(
                "closed test window target {} remained in discovery",
                target.id()
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Re-opening the exact persisted ID must fail with the target-specific
    // error. `capture_frames` also exercises the bounded worker shutdown path,
    // so a helper that hangs after the close becomes a failed check rather than
    // hanging the acceptance process indefinitely.
    match capture_frames(&target, format, 1, Duration::from_secs(8)) {
        Ok(_) => CheckResult::fail(format!(
            "closed window target {} captured frames after PID {pid} exited",
            target.id()
        )),
        Err(error) if is_closed_window_error(&error) => CheckResult::pass(format!(
            "pid={} target={} initial_size={}x{} frames={} elapsed_ms={} closed_target_rejected=true",
            pid,
            target.id(),
            frame.format().width(),
            frame.format().height(),
            frames,
            elapsed.as_millis()
        )),
        Err(error) => CheckResult::fail(format!(
            "closed window target {} returned an unexpected error: {error}",
            target.id()
        )),
    }
}

#[cfg(target_os = "windows")]
fn is_closed_window_error(error: &CaptureError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("selected window is no longer available")
        || message.contains("graphics capture target closed")
}

#[cfg(target_os = "windows")]
fn check_camera() -> CheckResult {
    let cameras = match discover_nokhwa_camera_devices() {
        Ok(cameras) => cameras,
        Err(error) => return capture_check_result(&error),
    };
    if cameras.is_empty() {
        return CheckResult::skip("no native Nokhwa camera is present");
    }
    let mut failures = Vec::new();
    let mut unavailable = 0_usize;
    let candidates = cameras.into_iter().take(8).collect::<Vec<_>>();
    for camera in &candidates {
        match probe_camera(camera) {
            Ok(report) => {
                let mode = report.native_mode.map_or_else(
                    || "auto".to_owned(),
                    |mode| {
                        format!(
                            "{}x{}@{}/{} {}",
                            mode.width(),
                            mode.height(),
                            mode.frame_rate().numerator(),
                            mode.frame_rate().denominator(),
                            mode.pixel_format()
                        )
                    },
                );
                return CheckResult::pass(format!(
                    "device={} modes={} selected_mode={} size={}x{} cycles=2 frames={} elapsed_ms={}",
                    camera.id(),
                    report.modes.len(),
                    mode,
                    report.frame.format().width(),
                    report.frame.format().height(),
                    report.frames,
                    report.elapsed.as_millis()
                ));
            }
            Err(error) if is_hardware_unavailable(&error) => {
                unavailable = unavailable.saturating_add(1);
                failures.push(format!("{}: unavailable: {error}", camera.id()));
            }
            Err(error) => failures.push(format!("{}: {error}", camera.id())),
        }
    }
    if unavailable == candidates.len() {
        return CheckResult::skip(format!(
            "no enumerated camera could be captured: {}",
            failures.join("; ")
        ));
    }
    CheckResult::fail(format!(
        "no enumerated camera could be captured: {}",
        failures.join("; ")
    ))
}

#[cfg(target_os = "windows")]
struct CameraProbeReport {
    modes: Vec<obs_rs_capture::CameraMode>,
    native_mode: Option<obs_rs_capture::CameraMode>,
    frame: VideoFrame,
    frames: u64,
    elapsed: Duration,
}

#[cfg(target_os = "windows")]
fn probe_camera(camera: &obs_rs_capture::CameraDevice) -> Result<CameraProbeReport, CaptureError> {
    let id = camera.id().to_string();
    let name = camera.name().to_owned();
    // The picker catalog intentionally skips native mode probing because
    // Media Foundation drivers can make that query expensive. Acceptance
    // must perform the explicit selected-device query so a successful camera
    // result proves more than an automatic fallback.
    let modes = obs_rs_capture::discover_nokhwa_camera_modes(&id)?;
    let native_mode = modes.first().copied();
    let format = probe_video_format();
    let mut last_frame = None;
    let mut total_frames = 0_u64;
    let mut total_elapsed = Duration::ZERO;
    for _ in 0..2 {
        let (frame, frames, elapsed) = capture_camera_cycle(&id, &name, format, native_mode)?;
        last_frame = Some(frame);
        total_frames = total_frames.saturating_add(u64::try_from(frames).unwrap_or(0));
        total_elapsed = total_elapsed.saturating_add(elapsed);
    }
    let frame = last_frame.ok_or_else(|| CaptureError::Protocol {
        message: "camera cycles completed without a frame".to_owned(),
    })?;
    Ok(CameraProbeReport {
        modes,
        native_mode,
        frame,
        frames: total_frames,
        elapsed: total_elapsed,
    })
}

#[cfg(target_os = "windows")]
fn capture_camera_cycle(
    id: &str,
    name: &str,
    format: VideoFormat,
    native_mode: Option<obs_rs_capture::CameraMode>,
) -> Result<(VideoFrame, usize, Duration), CaptureError> {
    let opener_id = id.to_owned();
    let opener_name = name.to_owned();
    let request = native_mode.map_or_else(
        || CaptureRequest::output(format),
        |mode| CaptureRequest::camera(format, mode),
    );
    let mut capture = ThreadedCaptureDevice::open(request, name, move || {
        NokhwaCaptureDevice::from_device_id(&opener_id, &opener_name)
            .map(|device| Box::new(device) as Box<dyn VideoCaptureDevice>)
    });
    let frames = wait_for_capture_frames(&mut capture, format, 4, Duration::from_secs(8));
    if !capture.shutdown() {
        return Err(CaptureError::Protocol {
            message: "Nokhwa camera worker did not stop within its bounded grace period".to_owned(),
        });
    }
    frames
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
fn check_audio_device_stability() -> CheckResult {
    let provider = WasapiAudioProvider::new();
    let first = match discover_audio(provider) {
        Ok(devices) => devices,
        Err(result) => return result,
    };
    let second = match discover_audio(provider) {
        Ok(devices) => devices,
        Err(result) => return result,
    };
    let snapshot = |devices: &[AudioDeviceInfo]| {
        let mut entries = devices
            .iter()
            .map(|device| {
                (
                    device.kind(),
                    device.id().to_owned(),
                    device.name().to_owned(),
                    device.available(),
                    device.is_default(),
                )
            })
            .collect::<Vec<_>>();
        entries
            .sort_by(|left, right| (left.0, &left.1, &left.2).cmp(&(right.0, &right.1, &right.2)));
        entries
    };
    let first_snapshot = snapshot(&first);
    let second_snapshot = snapshot(&second);
    if first_snapshot != second_snapshot {
        return CheckResult::fail(format!(
            "WASAPI device snapshot changed between immediate calls: first={first_snapshot:?} second={second_snapshot:?}"
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for device in &first {
        if !seen.insert((device.kind(), device.id().to_owned())) {
            return CheckResult::fail(format!(
                "WASAPI discovery returned duplicate {} endpoint {}",
                audio_kind_name(device.kind()),
                device.id()
            ));
        }
    }
    let defaults = [AudioDeviceKind::Input, AudioDeviceKind::Output]
        .into_iter()
        .map(|kind| {
            first
                .iter()
                .filter(|device| device.kind() == kind && device.available() && device.is_default())
                .count()
        })
        .collect::<Vec<_>>();
    if defaults.iter().any(|count| *count > 1) {
        return CheckResult::fail(format!(
            "WASAPI discovery marked multiple default endpoints: input={} output={}",
            defaults[0], defaults[1]
        ));
    }
    if first.is_empty() {
        return CheckResult::skip("WASAPI returned no input or output endpoints");
    }
    let inputs = first
        .iter()
        .filter(|device| device.kind() == AudioDeviceKind::Input)
        .count();
    let outputs = first
        .iter()
        .filter(|device| device.kind() == AudioDeviceKind::Output)
        .count();
    CheckResult::pass(format!(
        "inputs={inputs} outputs={outputs} default_inputs={} default_outputs={}",
        defaults[0], defaults[1]
    ))
}

#[cfg(target_os = "windows")]
fn audio_kind_name(kind: AudioDeviceKind) -> &'static str {
    match kind {
        AudioDeviceKind::Input => "input",
        AudioDeviceKind::Output => "output",
    }
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
            Ok(input) => {
                // WASAPI shared mode may negotiate a fixed endpoint format
                // (for example, a mono microphone when stereo was requested).
                // The returned input contract is authoritative for both the
                // block assertion and the timestamp/format used by the soak.
                let actual_format = input.format();
                return Ok((input, actual_format));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AudioDeviceError::Unavailable("no probe audio format was accepted".to_owned())
    }))
}

#[cfg(target_os = "windows")]
fn open_probe_loopback(
    provider: WasapiAudioProvider,
    device: &AudioDeviceInfo,
) -> Result<(Box<dyn AudioInput>, AudioFormat), AudioDeviceError> {
    let mut last_error = None;
    for (sample_rate, channels) in PROBE_AUDIO_FORMATS {
        let format = AudioFormat::new(sample_rate, channels).map_err(AudioDeviceError::from)?;
        match provider.open_loopback(device.id(), format) {
            Ok(input) => {
                let actual_format = input.format();
                return Ok((input, actual_format));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AudioDeviceError::Unavailable("no probe loopback format was accepted".to_owned())
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
            Ok(output) => {
                let actual_format = output.format();
                return Ok((output, actual_format));
            }
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
    let (mut loopback, format) = match open_probe_loopback(provider, device) {
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
    let silence = match AudioBuffer::silence(render.format(), Timestamp::ZERO, 480) {
        Ok(buffer) => buffer,
        Err(error) => {
            render.stop();
            return CheckResult::fail(error.to_string());
        }
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
        Ok(block) => {
            render.stop();
            CheckResult::fail(format!(
                "WASAPI loopback returned invalid block: format={:?} frames={}",
                block.format(),
                block.frames()
            ))
        }
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
#[allow(
    clippy::too_many_lines,
    reason = "the soak keeps capture, audio, and bounded cleanup assertions in one acceptance check"
)]
fn check_av_soak() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
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
    // Probe displays through the same candidate policy as the ordinary
    // capture checks. The soak then reopens the selected stable target for
    // its longer run, instead of assuming the first enumerated monitor is
    // capturable.
    let (display_id, _) =
        match capture_first_working_display(&devices, format, Duration::from_secs(8), "A/V soak") {
            Ok(capture) => capture,
            Err(result) => return result,
        };
    let Some(display) = devices
        .iter()
        .find(|device| device.id().to_string() == display_id)
    else {
        return CheckResult::fail(format!(
            "A/V soak selected display {display_id}, but it disappeared before reopen"
        ));
    };
    let mut capture = open_windows_capture(display, format);
    let (mut audio, audio_format) = match open_probe_input(provider, audio_device) {
        Ok(input) => input,
        Err(error) => return CheckResult::skip(error.to_string()),
    };
    let seconds = std::env::var("OBS_RS_SOAK_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(2, |value| value.clamp(1, 8 * 60 * 60));
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
        "device={display_id} seconds={seconds} frames={frames} audio_blocks={audio_blocks} audio_device={}",
        audio_device.id()
    ))
}

#[cfg(target_os = "windows")]
fn check_cleanup_restart() -> CheckResult {
    let devices = match discover_windows() {
        Ok(devices) => devices,
        Err(error) => return capture_check_result(&error),
    };
    let displays = windows_display_candidates(&devices);
    if displays.is_empty() {
        return CheckResult::skip("cleanup/restart needs a connected Windows display target");
    }
    let format = probe_video_format();
    let mut failures = Vec::new();
    let mut unavailable = 0_usize;
    for display in displays.iter().copied() {
        let mut candidate_failed = false;
        let mut candidate_unavailable = false;
        for cycle in 0..3 {
            match capture_one(display, format, Duration::from_secs(8)) {
                Ok(_) => {}
                Err(error) if is_hardware_unavailable(&error) => {
                    candidate_failed = true;
                    candidate_unavailable = true;
                    failures.push(format!(
                        "{}: cycle {cycle} unavailable: {error}",
                        display.id()
                    ));
                    break;
                }
                Err(error) => {
                    candidate_failed = true;
                    failures.push(format!("{}: cycle {cycle}: {error}", display.id()));
                    break;
                }
            }
        }
        if !candidate_failed {
            return CheckResult::pass(format!(
                "device={} three capture start/stop cycles joined cleanly",
                display.id()
            ));
        }
        if candidate_unavailable {
            unavailable = unavailable.saturating_add(1);
        }
    }
    let detail = format!(
        "no enumerated display passed three capture start/stop cycles: {}",
        failures.join("; ")
    );
    if unavailable == displays.len() {
        CheckResult::skip(detail)
    } else {
        CheckResult::fail(detail)
    }
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

#[cfg(target_os = "windows")]
fn required_check(name: &str) -> bool {
    let individual = format!("OBS_RS_REQUIRE_{}", name.to_ascii_uppercase());
    environment_flag(&individual)
        || (name.starts_with("production_") && environment_flag("OBS_RS_REQUIRE_PRODUCTION"))
}

#[cfg(target_os = "windows")]
fn environment_flag(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}
