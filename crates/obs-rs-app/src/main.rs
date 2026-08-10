//! End-to-end headless demonstration of the current OBS-RS vertical slice.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::error::Error;

use obs_rs_audio::{AudioBuffer, AudioFormat, AudioMixer, AudioScheduler, AvSyncController};
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_core::Runtime;
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, Timestamp, VideoFormat};
use obs_rs_output::{
    AudioEncoder, MemoryMuxer, MemoryPacketTransport, PacketDropPolicy, PacketMuxer, PacketQueue,
    RawAudioEncoder, RawRecording, RawRecordingSession, ReconnectPolicy, RleVideoEncoder,
    StreamSession, VideoEncoder, WavRecording,
};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{Profile, Project, ProjectCommand, SceneSpec, SourceSpec};
use obs_rs_render::{CpuRenderBackend, RenderBackend};
use obs_rs_ui::{DesktopState, UiCommand};
use obs_rs_video::{DropPolicy, RenderOutcome, VideoPipeline};

fn main() -> Result<(), Box<dyn Error>> {
    let plugin = BuiltinPlugin::new()?;
    let mut runtime = Runtime::new();
    runtime.register_plugin(&plugin)?;
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
    let renderer_checksum = renderer_fixture(format, &frame)?;
    let mut recording = RawRecordingSession::new(format);
    recording.push(frame.clone())?;
    let recording_bytes = recording.finalize()?;
    let decoded_recording = RawRecording::decode(&recording_bytes)?;

    let mut encoder = RleVideoEncoder::new(format);
    let packet = encoder.encode(&frame)?;
    let packet_bytes_per_frame = packet.byte_len();
    let mut packet_queue = PacketQueue::new(packet_bytes_per_frame, PacketDropPolicy::DropOldest)?;
    packet_queue.push(packet.clone())?;
    let mut muxer = MemoryMuxer::new();
    while let Some(packet) = packet_queue.pop() {
        muxer.push(packet)?;
    }

    let audio_format = AudioFormat::new(48_000, 2)?;
    let mut audio_scheduler = AudioScheduler::new(audio_format);
    let audio_deadline = audio_scheduler.next_deadline()?;
    let mut audio_mixer = AudioMixer::new(audio_format);
    let audio_source = audio_mixer.add_source(0.5)?;
    audio_mixer.set_pan(audio_source, 0.25)?;
    let audio_input = AudioBuffer::new(audio_format, audio_deadline.timestamp(), vec![0.8, -0.8])?;
    let audio_output = audio_mixer.mix(
        audio_deadline.timestamp(),
        1,
        &[(audio_source, &audio_input)],
    )?;
    let sync = av_sync_state(frame.timestamp(), audio_output.timestamp());
    let wav_bytes = wav_fixture(audio_format, &audio_output)?;
    let mut audio_encoder = RawAudioEncoder::new(audio_format);
    muxer.push(audio_encoder.encode(&audio_output)?)?;
    let packet_bytes = muxer.finalize()?;
    let packet_count = MemoryMuxer::decode(&packet_bytes)?.len();
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

    let (project_bytes, project_profiles) = project_fixture(format)?;

    println!(
        "obs-rs demo: plugins={}, scenes={}, sources={}, frame={}x{} outcome={outcome:?} pixel={pixel:?} checksum={} renderer_checksum={} rendered={} dropped_oldest={} audio={:?} sync={:?} wav_bytes={} packet_bytes={} packets={} stream_sent={} recording_bytes={} recording_frames={} project_bytes={} project_profiles={}",
        runtime.plugins().len(),
        runtime.scene_count(),
        runtime.source_count(),
        frame.format().width(),
        frame.format().height(),
        frame.checksum(),
        renderer_checksum,
        metrics.produced_frames(),
        metrics.dropped_oldest(),
        audio_output.samples(),
        sync,
        wav_bytes,
        packet_bytes.len(),
        packet_count,
        stream_metrics.sent_packets(),
        recording_bytes.len(),
        decoded_recording.len(),
        project_bytes,
        project_profiles
    );

    Ok(())
}

fn renderer_fixture(
    format: VideoFormat,
    frame: &obs_rs_media::VideoFrame,
) -> Result<u64, Box<dyn Error>> {
    let mut renderer = CpuRenderBackend::new(2)?;
    let source_texture = renderer.create_texture(format)?;
    let target_texture = renderer.create_texture(format)?;
    renderer.upload(source_texture, frame)?;
    renderer.composite(target_texture, &[source_texture])?;
    Ok(renderer.readback(target_texture)?.checksum())
}

fn wav_fixture(format: AudioFormat, buffer: &AudioBuffer) -> Result<usize, Box<dyn Error>> {
    let mut recording = WavRecording::new(format);
    recording.push(buffer.clone())?;
    Ok(recording.encode()?.len())
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

fn project_fixture(format: VideoFormat) -> Result<(usize, usize), Box<dyn Error>> {
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
    let project_document = desktop.project_document();
    let restored_project = Project::parse(&project_document)?;
    Ok((project_document.len(), restored_project.profiles().count()))
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
