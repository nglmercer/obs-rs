use std::error::Error;

use obs_rs_audio::{
    AudioBuffer, AudioCancellationToken, AudioDropPolicy, AudioFormat, AudioMixer, AudioPacer,
    AudioScheduler, AudioWorker, AudioWorkerReport, AvSyncController, MonotonicAudioClock,
};
use obs_rs_capture::{encode_frame_packet, CaptureKind, StreamCaptureDevice, VideoCaptureDevice};
use obs_rs_clock::{
    IndependentMediaClock, MediaSession, MediaSessionReport, MediaTimeline,
    SessionCancellationToken,
};
use obs_rs_config::Config;
use obs_rs_core::{CompositorMetrics, Runtime};
use obs_rs_diagnostics::DiagnosticBundle;
use obs_rs_media::{FrameRate, FrameTransform, Timestamp, VideoFormat, VideoFrame};
use obs_rs_output::{AtomicPacketFileWriter, MemoryMuxer, WavRecording, Y4mRecording};
use obs_rs_project::{
    Profile, Project, ProjectCommand, SceneItemSpec, SceneSpec, SourceFilterSpec, SourceSpec,
};
use obs_rs_render::{CpuRenderBackend, RenderBackend, RenderMetrics};
use obs_rs_ui::{DesktopState, UiCommand};
use obs_rs_video::{DropPolicy, VideoMetrics, VideoPacer};

pub(crate) fn renderer_fixture(
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

pub(crate) fn capture_stream_fixture(
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

pub(crate) fn y4m_fixture(
    format: VideoFormat,
    frame: &VideoFrame,
) -> Result<usize, Box<dyn Error>> {
    let mut recording = Y4mRecording::new(format);
    recording.push(frame.clone())?;
    Ok(recording.encode()?.len())
}

pub(crate) fn wav_fixture(
    format: AudioFormat,
    buffer: &AudioBuffer,
) -> Result<usize, Box<dyn Error>> {
    let mut recording = WavRecording::new(format);
    recording.push(buffer.clone())?;
    Ok(recording.encode()?.len())
}

pub(crate) fn audio_fixture(
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

pub(crate) fn timeline_fixture(
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

pub(crate) fn independent_clock_fixture(
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

pub(crate) fn media_session_fixture(
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

pub(crate) fn atomic_packet_file_fixture(bytes: &[u8]) -> Result<usize, Box<dyn Error>> {
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

pub(crate) fn audio_worker_fixture(
    format: AudioFormat,
) -> Result<AudioWorkerReport, Box<dyn Error>> {
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

pub(crate) fn av_sync_state(video: Timestamp, audio: Timestamp) -> obs_rs_audio::SyncState {
    AvSyncController::new(5_000_000)
        .observe(video, audio)
        .state()
}

pub(crate) fn screen_source(
    runtime: &mut Runtime,
    format: VideoFormat,
) -> Result<obs_rs_core::SourceId, Box<dyn Error>> {
    Ok(runtime.create_source(
        "screen_capture",
        "foreground",
        &video_settings(&format.width().to_string(), &format.height().to_string()),
    )?)
}

pub(crate) fn project_fixture(
    format: VideoFormat,
    compositor_metrics: CompositorMetrics,
    video_metrics: VideoMetrics,
    checksum: u64,
) -> Result<(usize, usize, usize, DiagnosticBundle), Box<dyn Error>> {
    let mut project = Project::new("obs-rs demo")?;
    let mut profile = Profile::new("live", "Live profile", format)?;
    let mut scene = SceneSpec::new("main", "Main scene")?;
    let background = SourceSpec::new(
        "background",
        "color_source",
        "background",
        color_settings(
            &format.width().to_string(),
            &format.height().to_string(),
            "#102030FF",
        ),
    )?;
    scene.add_item(SceneItemSpec::for_source("background")?)?;
    profile.add_source(background)?;
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
            video_settings(&format.width().to_string(), &format.height().to_string()),
        )?,
    }))?;
    desktop.dispatch(UiCommand::Project(ProjectCommand::SetSceneItemTransform {
        profile: "live".to_owned(),
        scene: "main".to_owned(),
        item: "foreground".to_owned(),
        transform: FrameTransform::new(1_000, 1_000, 0, 0, true, false, 220)?,
    }))?;
    desktop.dispatch(UiCommand::Project(ProjectCommand::AddSourceFilter {
        profile: "live".to_owned(),
        source: "foreground".to_owned(),
        filter: SourceFilterSpec::new("grayscale", "Grayscale", "grayscale", Config::new())?,
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

pub(crate) fn color_settings(width: &str, height: &str, color: &str) -> Config {
    let mut settings = Config::new();
    settings.set("width", width).expect("demo width is valid");
    settings
        .set("height", height)
        .expect("demo height is valid");
    settings.set("color", color).expect("demo color is valid");
    settings
}

pub(crate) fn video_settings(width: &str, height: &str) -> Config {
    let mut settings = Config::new();
    settings.set("width", width).expect("demo width is valid");
    settings
        .set("height", height)
        .expect("demo height is valid");
    settings
}
