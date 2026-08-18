//! End-to-end headless demonstration of the current OBS-RS vertical slice.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::error::Error;

use obs_rs_audio::AudioFormat;
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_core::Runtime;
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, VideoFormat};
use obs_rs_output::{
    AudioEncoder, MemoryMuxer, MemoryPacketTransport, PacketDropPolicy, PacketMuxer, PacketQueue,
    PngVideoEncoder, RawAudioEncoder, RawRecording, RawRecordingSession, ReconnectPolicy,
    RleVideoEncoder, StreamSession, VideoEncoder,
};
use obs_rs_plugin_api::{Plugin, VideoRequest};
use obs_rs_sandbox::SandboxedPlugin;
use obs_rs_video::{DropPolicy, RenderOutcome, VideoPipeline};

mod diagnostics;
mod fixtures;
mod sandbox;

use diagnostics::project_diagnostics_fixture;
use fixtures::{
    atomic_packet_file_fixture, audio_fixture, av_sync_state, capture_stream_fixture,
    color_settings, independent_clock_fixture, media_session_fixture, renderer_fixture,
    screen_source, timeline_fixture, y4m_fixture,
};
use sandbox::{sandbox_manifest, sandbox_source_command};

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn Error>> {
    let plugin = BuiltinPlugin::new()?;
    let capture_devices = plugin.discover_capture_devices()?.len();
    let sandbox_manifest = sandbox_manifest()?;
    let sandbox_command = sandbox_source_command();
    let sandbox_discovered = sandbox_command.is_file();
    let sandbox_arguments = vec!["--frames".to_owned()];
    let sandbox_plugin = if sandbox_discovered {
        SandboxedPlugin::from_process(&sandbox_command, sandbox_arguments)?
    } else {
        SandboxedPlugin::new(&sandbox_manifest, sandbox_command, sandbox_arguments)?
    };
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
    runtime.add_source_filter(foreground, FrameFilter::Grayscale)?;

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
        "obs-rs demo: plugins={}, sandbox_discovered={sandbox_discovered}, sandbox_source_kinds={}, capture_devices={}, scenes={}, sources={}, frame={}x{} outcome={outcome:?} pixel={pixel:?} checksum={} renderer_checksum={} renderer_created={} renderer_uploads={} renderer_compositions={} renderer_readbacks={} renderer_peak_bytes={} rendered={} dropped_oldest={} png_bytes={} capture_stream_bytes={} y4m_bytes={} audio={:?} sync={:?} timeline_in_sync={} clock_drift_ns={} session_ticks={} audio_worker_blocks={} audio_worker_missed={} wav_bytes={} packet_bytes={} packet_file_bytes={} packets={} stream_sent={} recording_bytes={} recording_frames={} project_bytes={} project_profiles={} ui_snapshot_bytes={} diagnostic_bytes={}",
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
