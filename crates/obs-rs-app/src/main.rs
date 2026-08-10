//! End-to-end headless demonstration of the current OBS-RS vertical slice.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::error::Error;

use obs_rs_audio::{
    AudioBuffer, AudioCancellationToken, AudioDropPolicy, AudioFormat, AudioMixer, AudioPacer,
    AudioScheduler, AudioWorker, AudioWorkerReport, AvSyncController, MonotonicAudioClock,
};
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_capture::{encode_frame_packet, CaptureKind, StreamCaptureDevice, VideoCaptureDevice};
use obs_rs_clock::{
    IndependentMediaClock, MediaSession, MediaSessionReport, MediaTimeline,
    SessionCancellationToken,
};
use obs_rs_config::Config;
use obs_rs_core::{CompositorMetrics, Runtime};
use obs_rs_diagnostics::{AtomicDiagnosticFileWriter, DiagnosticBundle};
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{
    AtomicPacketFileWriter, AudioEncoder, MemoryMuxer, MemoryPacketTransport, PacketDropPolicy,
    PacketMuxer, PacketQueue, PngVideoEncoder, RawAudioEncoder, RawRecording, RawRecordingSession,
    ReconnectPolicy, RleVideoEncoder, StreamSession, VideoEncoder, WavRecording, Y4mRecording,
};
use obs_rs_plugin_api::{Plugin, PluginManifest, VideoRequest};
use obs_rs_project::{Profile, Project, ProjectCommand, SceneSpec, SourceSpec};
use obs_rs_render::{CpuRenderBackend, RenderBackend, RenderMetrics};
use obs_rs_sandbox::{SandboxedPlugin, SandboxedPluginManifest};
use obs_rs_ui::{DesktopState, UiCommand};
use obs_rs_util::Identifier;
use obs_rs_video::{DropPolicy, RenderOutcome, VideoMetrics, VideoPacer, VideoPipeline};

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn Error>> {
    let plugin = BuiltinPlugin::new()?;
    let capture_devices = plugin.discover_capture_devices()?.len();
    let sandbox_manifest = sandbox_manifest()?;
    let sandbox_plugin = SandboxedPlugin::new(
        &sandbox_manifest,
        "obs-rs-sandbox-source",
        vec!["--frames".to_owned()],
    )?;
    let sandbox_source_kinds = sandbox_plugin.source_factories().len();
    let mut runtime = Runtime::new();
    runtime.register_plugin(&plugin)?;
    runtime.register_plugin(&sandbox_plugin)?;
    runtime.create_scene("main")?;

    let background = runtime.create_source(
        "color_source",
        "background",
        &color_settings("640", "360", "#102030FF"),
    )?;
    let foreground = screen_source(&mut runtime)?;
    runtime.attach_source("main", background)?;
    runtime.attach_source("main", foreground)?;
    runtime.set_source_transform(
        "main",
        foreground,
        FrameTransform::new(1_000, 1_000, 0, 0, true, false, 220)?,
    )?;
    runtime.add_source_filter("main", foreground, FrameFilter::Grayscale)?;

    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1)?)?;
    let mut pipeline = VideoPipeline::new(format, 2, DropPolicy::DropOldest)?;
    let outcome = pipeline.render_next(|deadline, output_format| {
        let request = VideoRequest::new(deadline.timestamp(), output_format);
        runtime
            .render_scene("main", &request)
            .map_err(|error| error.to_string())
    })?;
    if matches!(outcome, RenderOutcome::Empty { .. }) {
        return Err("main scene produced no frame".into());
    }
    let frame = pipeline.take_next().ok_or("pipeline produced no frame")?;
    let pixel = frame
        .pixel(0, 0)
        .ok_or("rendered frame has no first pixel")?;
    let metrics = pipeline.metrics();
    let (renderer_checksum, renderer_metrics) = renderer_fixture(format, &frame)?;
    let capture_stream_bytes = capture_stream_fixture(format, &frame)?;
    let y4m_bytes = y4m_fixture(format, &frame)?;
    let mut recording = RawRecordingSession::new(format);
    recording.push(frame.clone())?;
    let recording_bytes = recording.finalize()?;
    let decoded_recording = RawRecording::decode(&recording_bytes)?;

    let mut encoder = RleVideoEncoder::new(format);
    let packet = encoder.encode(&frame)?;
    let mut png_encoder = PngVideoEncoder::new(format);
    let png_bytes = png_encoder.encode(&frame)?.byte_len();
    let packet_bytes_per_frame = packet.byte_len();
    let mut packet_queue = PacketQueue::new(packet_bytes_per_frame, PacketDropPolicy::DropOldest)?;
    packet_queue.push(packet.clone())?;
    let mut muxer = MemoryMuxer::new();
    while let Some(packet) = packet_queue.pop() {
        muxer.push(packet)?;
    }

    let audio_format = AudioFormat::new(48_000, 2)?;
    let timeline_in_sync = timeline_fixture(format.frame_rate(), audio_format)?;
    let clock_drift_nanos = independent_clock_fixture(format.frame_rate(), audio_format)?;
    let (audio_output, audio_worker, wav_bytes) = audio_fixture(audio_format)?;
    let session_report = media_session_fixture(format, audio_format)?;
    let sync = av_sync_state(frame.timestamp(), audio_output.timestamp());
    let mut audio_encoder = RawAudioEncoder::new(audio_format);
    muxer.push(audio_encoder.encode(&audio_output)?)?;
    let packet_bytes = muxer.finalize()?;
    let packet_count = MemoryMuxer::decode(&packet_bytes)?.len();
    let packet_file_bytes = atomic_packet_file_fixture(&packet_bytes)?;
    let mut stream = StreamSession::new(
        MemoryPacketTransport::new(),
        packet_bytes_per_frame,
        PacketDropPolicy::DropNewest,
        ReconnectPolicy::new(1),
    )?;
    stream.connect()?;
    stream.submit(packet)?;
    stream.flush()?;
    let stream_metrics = stream.metrics();

    let (project_bytes, project_profiles, ui_snapshot_bytes, diagnostic_bytes) =
        project_diagnostics_fixture(
            format,
            runtime.compositor_metrics(),
            metrics,
            frame.checksum(),
        )?;

    println!(
        "obs-rs demo: plugins={}, sandbox_source_kinds={}, capture_devices={}, scenes={}, sources={}, frame={}x{} outcome={outcome:?} pixel={pixel:?} checksum={} renderer_checksum={} renderer_created={} renderer_uploads={} renderer_compositions={} renderer_readbacks={} renderer_peak_bytes={} rendered={} dropped_oldest={} png_bytes={} capture_stream_bytes={} y4m_bytes={} audio={:?} sync={:?} timeline_in_sync={} clock_drift_ns={} session_ticks={} audio_worker_blocks={} audio_worker_missed={} wav_bytes={} packet_bytes={} packet_file_bytes={} packets={} stream_sent={} recording_bytes={} recording_frames={} project_bytes={} project_profiles={} ui_snapshot_bytes={} diagnostic_bytes={}",
        runtime.plugins().len(),
        sandbox_source_kinds,
        capture_devices,
        runtime.scene_count(),
        runtime.source_count(),
        frame.format().width(),
        frame.format().height(),
        frame.checksum(),
        renderer_checksum,
        renderer_metrics.textures_created(),
        renderer_metrics.uploads(),
        renderer_metrics.compositions(),
        renderer_metrics.readbacks(),
        renderer_metrics.peak_allocated_bytes(),
        metrics.produced_frames(),
        metrics.dropped_oldest(),
        png_bytes,
        capture_stream_bytes,
        y4m_bytes,
        audio_output.samples(),
        sync,
        timeline_in_sync,
        clock_drift_nanos,
        session_report.completed_ticks(),
        audio_worker.processed_blocks(),
        audio_worker.missed_deadlines(),
        wav_bytes,
        packet_bytes.len(),
        packet_file_bytes,
        packet_count,
        stream_metrics.sent_packets(),
        recording_bytes.len(),
        decoded_recording.len(),
        project_bytes,
        project_profiles,
        ui_snapshot_bytes,
        diagnostic_bytes
    );

    Ok(())
}

fn renderer_fixture(
    format: VideoFormat,
    frame: &obs_rs_media::VideoFrame,
) -> Result<(u64, RenderMetrics), Box<dyn Error>> {
    let mut renderer = CpuRenderBackend::new(2)?;
    let source_texture = renderer.create_texture(format)?;
    let target_texture = renderer.create_texture(format)?;
    renderer.upload(source_texture, frame)?;
    renderer.composite(target_texture, &[source_texture])?;
    let checksum = renderer.readback(target_texture)?.checksum();
    Ok((checksum, renderer.metrics()))
}

fn capture_stream_fixture(
    format: VideoFormat,
    frame: &VideoFrame,
) -> Result<usize, Box<dyn Error>> {
    let packet = encode_frame_packet(frame)?;
    let mut device = StreamCaptureDevice::new(
        "demo-stream",
        "Demo Rust frame stream",
        CaptureKind::Screen,
        std::io::Cursor::new(packet.clone()),
    )?;
    device.start(format)?;
    let received = device
        .next_frame(Timestamp::ZERO)?
        .ok_or("frame stream ended before one frame")?;
    if received != *frame {
        return Err("frame stream changed the captured frame".into());
    }
    Ok(packet.len())
}

fn y4m_fixture(format: VideoFormat, frame: &VideoFrame) -> Result<usize, Box<dyn Error>> {
    let mut recording = Y4mRecording::new(format);
    recording.push(frame.clone())?;
    Ok(recording.encode()?.len())
}

fn wav_fixture(format: AudioFormat, buffer: &AudioBuffer) -> Result<usize, Box<dyn Error>> {
    let mut recording = WavRecording::new(format);
    recording.push(buffer.clone())?;
    Ok(recording.encode()?.len())
}

fn audio_fixture(
    format: AudioFormat,
) -> Result<(AudioBuffer, AudioWorkerReport, usize), Box<dyn Error>> {
    let mut audio_scheduler = AudioScheduler::new(format);
    let audio_deadline = audio_scheduler.next_deadline()?;
    let mut audio_mixer = AudioMixer::new(format);
    let audio_source = audio_mixer.add_source(0.5)?;
    audio_mixer.set_pan(audio_source, 0.25)?;
    let audio_input = AudioBuffer::new(format, audio_deadline.timestamp(), vec![0.8, -0.8])?;
    let audio_output = audio_mixer.mix(
        audio_deadline.timestamp(),
        1,
        &[(audio_source, &audio_input)],
    )?;
    let audio_worker = audio_worker_fixture(format)?;
    let wav_bytes = wav_fixture(format, &audio_output)?;
    Ok((audio_output, audio_worker, wav_bytes))
}

fn timeline_fixture(
    video_rate: FrameRate,
    audio_format: AudioFormat,
) -> Result<u64, Box<dyn Error>> {
    let mut timeline = MediaTimeline::new(video_rate, audio_format, 1_000);
    let audio_frames = usize::try_from(
        u64::from(audio_format.sample_rate()) * u64::from(video_rate.denominator())
            / u64::from(video_rate.numerator()),
    )?;
    for _ in 0..2 {
        let video = timeline.next_video_frame()?;
        let audio = timeline.next_audio_block(audio_frames)?;
        let _ = timeline.observe(video.timestamp(), audio.timestamp());
    }
    Ok(timeline.metrics().in_sync())
}

fn independent_clock_fixture(
    video_rate: FrameRate,
    audio_format: AudioFormat,
) -> Result<i64, Box<dyn Error>> {
    let mut clock = IndependentMediaClock::new(1_000, -1_000)?;
    let mut audio_pacer = AudioPacer::new(audio_format);
    let mut video_pacer = VideoPacer::new(video_rate);
    let controller = AvSyncController::new(1_000_000);
    let audio_frames = usize::try_from(
        u64::from(audio_format.sample_rate()) * u64::from(video_rate.denominator())
            / u64::from(video_rate.numerator()),
    )?;
    for _ in 0..300 {
        audio_pacer.next(&mut clock, audio_frames)?;
        video_pacer.next(&mut clock)?;
    }
    let observation = controller.observe(clock.video_now(), clock.audio_now());
    if observation.state() != obs_rs_audio::SyncState::AudioAhead {
        return Err("independent clock fixture did not observe positive audio drift".into());
    }
    Ok(observation.delta_nanos())
}

fn media_session_fixture(
    video_format: VideoFormat,
    audio_format: AudioFormat,
) -> Result<MediaSessionReport, Box<dyn Error>> {
    let mut session = MediaSession::new(
        video_format,
        2,
        DropPolicy::DropOldest,
        audio_format,
        3_200,
        AudioDropPolicy::DropOldest,
    )?;
    let cancellation = SessionCancellationToken::new();
    let audio_frames = usize::try_from(
        u64::from(audio_format.sample_rate()) * u64::from(video_format.frame_rate().denominator())
            / u64::from(video_format.frame_rate().numerator()),
    )?;
    Ok(session.run(
        audio_frames,
        2,
        &cancellation,
        |deadline, format| {
            Ok::<_, std::convert::Infallible>(Some(VideoFrame::solid(
                format,
                deadline.timestamp(),
                [0, 0, 0, 255],
            )))
        },
        |deadline, format, frames| {
            AudioBuffer::silence(format, deadline.timestamp(), frames).map(Some)
        },
    )?)
}

fn atomic_packet_file_fixture(bytes: &[u8]) -> Result<usize, Box<dyn Error>> {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let root = std::env::temp_dir();
    let final_path = root.join(format!(
        "obs-rs-demo-{}-{token}.obsrpacket",
        std::process::id()
    ));
    let temp_path = root.join(format!("obs-rs-demo-{}-{token}.part", std::process::id()));
    let mut writer = AtomicPacketFileWriter::new(&final_path, &temp_path)?;
    for packet in MemoryMuxer::decode(bytes)? {
        writer.push(packet)?;
    }
    let committed = writer.finalize()?;
    let persisted = std::fs::read(writer.final_path())?;
    if persisted.len() != committed {
        return Err("packet file size changed after atomic commit".into());
    }
    std::fs::remove_file(writer.final_path())?;
    Ok(committed)
}

fn audio_worker_fixture(format: AudioFormat) -> Result<AudioWorkerReport, Box<dyn Error>> {
    let mut worker = AudioWorker::new(format, 960, AudioDropPolicy::DropOldest)?;
    let cancellation = AudioCancellationToken::new();
    let mut clock = MonotonicAudioClock::start();
    let report = worker.run(
        &mut clock,
        480,
        2,
        &cancellation,
        |deadline, output_format, frames| {
            AudioBuffer::silence(output_format, deadline.timestamp(), frames).map(Some)
        },
    )?;
    while worker.take_next().is_some() {}
    Ok(report)
}

fn av_sync_state(video: Timestamp, audio: Timestamp) -> obs_rs_audio::SyncState {
    AvSyncController::new(5_000_000)
        .observe(video, audio)
        .state()
}

fn screen_source(runtime: &mut Runtime) -> Result<obs_rs_core::SourceId, Box<dyn Error>> {
    Ok(runtime.create_source(
        "screen_capture",
        "foreground",
        &video_settings("640", "360"),
    )?)
}

fn project_fixture(
    format: VideoFormat,
    compositor_metrics: CompositorMetrics,
    video_metrics: VideoMetrics,
    checksum: u64,
) -> Result<(usize, usize, usize, DiagnosticBundle), Box<dyn Error>> {
    let mut project = Project::new("obs-rs demo")?;
    let mut profile = Profile::new("live", "Live profile", format)?;
    let mut scene = SceneSpec::new("main", "Main scene")?;
    scene.add_source(SourceSpec::new(
        "background",
        "color_source",
        "background",
        color_settings("640", "360", "#102030FF"),
    )?)?;
    profile.add_scene(scene)?;
    project.add_profile(profile)?;
    let mut desktop = DesktopState::new(project);
    desktop.dispatch(UiCommand::Project(ProjectCommand::AddSource {
        profile: "live".to_owned(),
        scene: "main".to_owned(),
        source: SourceSpec::new(
            "foreground",
            "screen_capture",
            "foreground",
            video_settings("640", "360"),
        )?,
    }))?;
    desktop.dispatch(UiCommand::Project(ProjectCommand::SetSourceTransform {
        profile: "live".to_owned(),
        scene: "main".to_owned(),
        source: "foreground".to_owned(),
        transform: FrameTransform::new(1_000, 1_000, 0, 0, true, false, 220)?,
    }))?;
    desktop.dispatch(UiCommand::Project(ProjectCommand::AddSourceFilter {
        profile: "live".to_owned(),
        scene: "main".to_owned(),
        source: "foreground".to_owned(),
        filter: FrameFilter::Grayscale,
    }))?;
    let ui_snapshot = desktop.accessible_snapshot();
    let ui_snapshot_bytes = ui_snapshot.len();
    let project_document = desktop.project_document();
    let restored_project = Project::parse(&project_document)?;
    let mut diagnostics = DiagnosticBundle::new();
    diagnostics.insert_text(
        "application",
        &format!(
            "format={}x{}\nframe_checksum={checksum}\n",
            format.width(),
            format.height()
        ),
    )?;
    diagnostics.insert_text("project", &project_document)?;
    diagnostics.insert_text("ui", &ui_snapshot)?;
    diagnostics.insert_text(
        "runtime",
        &format!(
            "renders={}\nsource_requests={}\nsource_frames={}\ntransformed={}\nfiltered={}\nblends={}\nvideo_produced={}\nvideo_missed={}\nvideo_lateness_ns={}\n",
            compositor_metrics.render_calls(),
            compositor_metrics.source_requests(),
            compositor_metrics.source_frames(),
            compositor_metrics.transformed_frames(),
            compositor_metrics.filtered_frames(),
            compositor_metrics.blended_layers(),
            video_metrics.produced_frames(),
            video_metrics.missed_deadlines(),
            video_metrics.total_lateness_nanos(),
        ),
    )?;
    Ok((
        project_document.len(),
        restored_project.profiles().count(),
        ui_snapshot_bytes,
        diagnostics,
    ))
}

fn project_diagnostics_fixture(
    format: VideoFormat,
    compositor_metrics: CompositorMetrics,
    video_metrics: VideoMetrics,
    checksum: u64,
) -> Result<(usize, usize, usize, usize), Box<dyn Error>> {
    let (project_bytes, project_profiles, ui_snapshot_bytes, bundle) =
        project_fixture(format, compositor_metrics, video_metrics, checksum)?;
    let diagnostic_bytes = diagnostic_file_fixture(&bundle)?;
    Ok((
        project_bytes,
        project_profiles,
        ui_snapshot_bytes,
        diagnostic_bytes,
    ))
}

fn diagnostic_file_fixture(bundle: &DiagnosticBundle) -> Result<usize, Box<dyn Error>> {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let root = std::env::temp_dir();
    let final_path = root.join(format!("obs-rs-demo-{}-{token}.diag", std::process::id()));
    let temp_path = root.join(format!(
        "obs-rs-demo-{}-{token}.diag.part",
        std::process::id()
    ));
    let mut writer = AtomicDiagnosticFileWriter::new(&final_path, &temp_path)?;
    let committed = writer.finalize(bundle)?;
    let persisted = std::fs::read(writer.final_path())?;
    let restored = DiagnosticBundle::decode(&persisted)?;
    if &restored != bundle || persisted.len() != committed {
        return Err("diagnostic bundle changed after atomic commit".into());
    }
    std::fs::remove_file(writer.final_path())?;
    Ok(committed)
}

fn sandbox_manifest() -> Result<SandboxedPluginManifest, Box<dyn Error>> {
    let manifest =
        PluginManifest::new("obs_rs_sandbox_demo", "OBS-RS sandbox demo source", "0.1.0")?;
    Ok(SandboxedPluginManifest::new(
        manifest,
        [Identifier::new("sandbox_pattern")?],
    )?)
}

fn color_settings(width: &str, height: &str, color: &str) -> Config {
    let mut settings = Config::new();
    settings.set("width", width).expect("demo width is valid");
    settings
        .set("height", height)
        .expect("demo height is valid");
    settings.set("color", color).expect("demo color is valid");
    settings
}

fn video_settings(width: &str, height: &str) -> Config {
    let mut settings = Config::new();
    settings.set("width", width).expect("demo width is valid");
    settings
        .set("height", height)
        .expect("demo height is valid");
    settings
}
