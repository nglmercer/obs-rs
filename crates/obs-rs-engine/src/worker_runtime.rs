use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{Receiver, TrySendError},
        Arc, Mutex,
    },
};

#[cfg(feature = "production-gstreamer")]
use obs_rs_output::OutputProfile;
use obs_rs_output::{AudioEncoderConfig, StreamTarget, VideoEncoderConfig};

use super::{
    EngineError, EngineSession, EngineWorkerSnapshot, OutputEvent, OutputLifecycle,
    ReplaySaveStatus, WorkerCommand, OUTPUT_EVENT_CAPACITY,
};

#[allow(
    clippy::needless_pass_by_value,
    reason = "the worker thread takes ownership of its receive and status handles"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive command loop keeps worker state transitions serialized"
)]
pub(super) fn worker_loop(
    mut session: EngineSession,
    receiver: Receiver<WorkerCommand>,
    snapshot: Arc<Mutex<EngineWorkerSnapshot>>,
    dropped_frames: Arc<AtomicU64>,
    queued_frames: Arc<AtomicUsize>,
    output_events: Arc<Mutex<VecDeque<OutputEvent>>>,
) {
    while let Ok(command) = receiver.recv() {
        let shutdown = match command {
            WorkerCommand::StartRecording(path, encoder_config, reply) => {
                let result = start_recording(&mut session, path, encoder_config);
                let _ = reply.send(result);
                false
            }
            #[cfg(feature = "production-gstreamer")]
            WorkerCommand::StartRemuxRecording(path, encoder_config, reply) => {
                let result = start_remux_recording(&mut session, path, encoder_config);
                let _ = reply.send(result);
                false
            }
            #[cfg(feature = "production-gstreamer")]
            WorkerCommand::RecoverInterruptedRemux(path, reply) => {
                let result = session
                    .recover_interrupted_remux_recording(path)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            #[cfg(feature = "production-gstreamer")]
            WorkerCommand::DiscoverInterruptedRemux(directory, reply) => {
                let result = session
                    .discover_interrupted_remux_candidates(directory)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            #[cfg(feature = "production-gstreamer")]
            WorkerCommand::StartRecordingProfile(path, profile, encoder_config, reply) => {
                let result = start_recording_profile(&mut session, path, profile, encoder_config);
                let _ = reply.send(result);
                false
            }
            WorkerCommand::StartSegmentedRecording(path, policy, encoder_config, reply) => {
                let result = match encoder_config {
                    Some((video, audio)) => {
                        session.start_segmented_recording_configured(path, policy, video, audio)
                    }
                    None => session.start_segmented_recording(path, policy),
                }
                .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::FinishRecording(reply) => {
                let result = session
                    .finish_recording()
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::AbortRecording => abort_recording(&mut session),
            WorkerCommand::StartReplayBuffer(capacity_bytes, duration, reply) => {
                let result = session
                    .start_replay_buffer(capacity_bytes, duration)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::StopReplayBuffer => {
                session.stop_replay_buffer();
                false
            }
            WorkerCommand::SaveReplayBuffer(path, reply) => {
                let result = session
                    .save_replay_buffer(path)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::StartStreaming(address, encoder_config) => {
                start_stream(
                    &mut session,
                    &address,
                    encoder_config.as_ref(),
                    &output_events,
                );
                false
            }
            WorkerCommand::StartStreamingTarget(target, video, audio) => {
                start_stream_target(&mut session, &target, &video, &audio, &output_events);
                false
            }
            WorkerCommand::FinishStreaming => {
                finish_streaming(&mut session, &output_events);
                false
            }
            WorkerCommand::PushFrame(frame) => {
                queued_frames.fetch_sub(1, Ordering::Relaxed);
                if let Err(error) = session.push_program_frame(&frame) {
                    session.last_error = Some(error.to_string());
                }
                false
            }
            WorkerCommand::PushRawFrame(frame) => {
                queued_frames.fetch_sub(1, Ordering::Relaxed);
                if let Err(error) = session.push_program_raw_frame(&frame) {
                    session.last_error = Some(error.to_string());
                }
                false
            }
            WorkerCommand::MonitorAudio(timestamp) => {
                if let Err(error) = session.monitor_audio_until(timestamp) {
                    session.last_error = Some(error.to_string());
                }
                false
            }
            WorkerCommand::SetGain(channel, gain, reply) => {
                let result = session
                    .set_channel_gain_milli(channel, gain)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetPan(channel, pan_milli, reply) => {
                let result = session
                    .set_channel_pan_milli(channel, pan_milli)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetSyncOffset(channel, milliseconds, reply) => {
                let result = session
                    .set_channel_sync_offset_millis(channel, milliseconds)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetGainFilter(channel, milli_db, reply) => {
                let result = session
                    .set_channel_gain_filter_db_milli(channel, milli_db)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetInvertPolarity(channel, reply) => {
                let result = session
                    .set_channel_invert_polarity(channel)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetLimiter(channel, threshold_db_milli, release_ms, reply) => {
                let result = session
                    .set_channel_limiter(channel, threshold_db_milli, release_ms)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetCompressor(
                channel,
                ratio_milli,
                threshold_db_milli,
                attack_ms,
                release_ms,
                output_gain_db_milli,
                reply,
            ) => {
                let result = session
                    .set_channel_compressor(
                        channel,
                        ratio_milli,
                        threshold_db_milli,
                        attack_ms,
                        release_ms,
                        output_gain_db_milli,
                    )
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetExpander(
                channel,
                ratio_milli,
                threshold_db_milli,
                attack_ms,
                release_ms,
                output_gain_db_milli,
                reply,
            ) => {
                let result = session
                    .set_channel_expander(
                        channel,
                        ratio_milli,
                        threshold_db_milli,
                        attack_ms,
                        release_ms,
                        output_gain_db_milli,
                    )
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetNoiseGate(
                channel,
                open_threshold_db_milli,
                close_threshold_db_milli,
                attack_ms,
                hold_ms,
                release_ms,
                reply,
            ) => {
                let result = session
                    .set_channel_noise_gate(
                        channel,
                        open_threshold_db_milli,
                        close_threshold_db_milli,
                        attack_ms,
                        hold_ms,
                        release_ms,
                    )
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetMuted(channel, muted, reply) => {
                let result = session
                    .set_channel_muted(channel, muted)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetAudioInput(device_id, reply) => {
                session.set_audio_input_id(device_id.as_deref());
                let _ = reply.send(Ok(()));
                false
            }
            WorkerCommand::SetDesktopAudio(device_id, reply) => {
                session.set_desktop_audio_id(device_id.as_deref());
                let _ = reply.send(Ok(()));
                false
            }
            WorkerCommand::SetMonitorMode(channel, mode, reply) => {
                let result = session
                    .set_channel_monitor_mode(channel, mode)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetMonitorOutput(device_id, reply) => {
                let result = session
                    .set_monitor_output_id(device_id.as_deref())
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SetAudioFormat(format, reply) => {
                let result = session
                    .set_audio_format(format)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
                false
            }
            WorkerCommand::SyncProject(project, reply) => {
                let result = session
                    .sync_project(project)
                    .map_err(|error| error.to_string());
                if let Err(error) = &result {
                    session.last_error = Some(error.clone());
                }
                let _ = reply.send(result);
                false
            }
            #[cfg(test)]
            WorkerCommand::TestBlock(entered, release) => {
                let _ = entered.send(());
                let _ = release.recv();
                false
            }
            WorkerCommand::Shutdown => true,
        };

        pump_stream(&mut session);
        publish_snapshot(
            &session,
            &snapshot,
            &dropped_frames,
            &queued_frames,
            !shutdown,
        );
        if shutdown {
            break;
        }
    }
    let _ = session.finish_streaming();
    session.abort_recording();
    publish_snapshot(&session, &snapshot, &dropped_frames, &queued_frames, false);
}

fn pump_stream(session: &mut EngineSession) {
    if session.is_streaming() {
        if let Err(error) = session.pump_stream() {
            session.last_error = Some(error.to_string());
        }
    }
}

fn publish_snapshot(
    session: &EngineSession,
    snapshot: &Arc<Mutex<EngineWorkerSnapshot>>,
    dropped_frames: &Arc<AtomicU64>,
    queued_frames: &Arc<AtomicUsize>,
    alive: bool,
) {
    if let Ok(mut target) = snapshot.lock() {
        *target = EngineWorkerSnapshot {
            engine: session.snapshot(),
            queued_frames: queued_frames.load(Ordering::Relaxed),
            dropped_frames: dropped_frames.load(Ordering::Relaxed),
            alive,
        };
    }
}

pub(super) fn worker_closed() -> EngineError {
    EngineError::Worker("engine worker is closed".to_owned())
}

fn abort_recording(session: &mut EngineSession) -> bool {
    session.abort_recording();
    false
}

fn start_recording(
    session: &mut EngineSession,
    path: PathBuf,
    encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
) -> Result<(), String> {
    match encoder_config {
        Some((video, audio)) => session.start_recording_configured(path, video, audio),
        None => session.start_recording(path),
    }
    .map_err(|error| error.to_string())
}

#[cfg(feature = "production-gstreamer")]
fn start_remux_recording(
    session: &mut EngineSession,
    path: PathBuf,
    encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
) -> Result<(), String> {
    match encoder_config {
        Some((video, audio)) => session.start_remux_recording_configured(path, video, audio),
        None => session.start_remux_recording(path),
    }
    .map_err(|error| error.to_string())
}

#[cfg(feature = "production-gstreamer")]
fn start_recording_profile(
    session: &mut EngineSession,
    path: PathBuf,
    profile: OutputProfile,
    encoder_config: Option<(VideoEncoderConfig, AudioEncoderConfig)>,
) -> Result<(), String> {
    match encoder_config {
        Some((video, audio)) => {
            session.start_recording_profile_configured(path, profile, video, audio)
        }
        None => session.start_recording_profile(path, profile),
    }
    .map_err(|error| error.to_string())
}

fn start_stream(
    session: &mut EngineSession,
    address: &str,
    encoder_config: Option<&(VideoEncoderConfig, AudioEncoderConfig)>,
    output_events: &Arc<Mutex<VecDeque<OutputEvent>>>,
) {
    let result = match encoder_config {
        Some((video, audio)) => session.start_streaming_configured(address, video, audio),
        None => session.start_streaming(address),
    };
    match result {
        Ok(()) => push_output_event(output_events, OutputEvent::Running),
        Err(error) => push_output_event(
            output_events,
            OutputEvent::Failed {
                reason: error.to_string(),
            },
        ),
    }
}

fn start_stream_target(
    session: &mut EngineSession,
    target: &StreamTarget,
    video: &VideoEncoderConfig,
    audio: &AudioEncoderConfig,
    output_events: &Arc<Mutex<VecDeque<OutputEvent>>>,
) {
    match session.start_streaming_target_configured(target, video, audio) {
        Ok(()) => push_output_event(output_events, OutputEvent::Running),
        Err(error) => push_output_event(
            output_events,
            OutputEvent::Failed {
                reason: error.to_string(),
            },
        ),
    }
}

fn finish_streaming(
    session: &mut EngineSession,
    output_events: &Arc<Mutex<VecDeque<OutputEvent>>>,
) {
    match session.finish_streaming() {
        Ok(()) => push_output_event(output_events, OutputEvent::Stopped),
        Err(error) => push_output_event(
            output_events,
            OutputEvent::Failed {
                reason: error.to_string(),
            },
        ),
    }
}

pub(super) fn command_enqueue_error(error: &TrySendError<WorkerCommand>) -> EngineError {
    match error {
        TrySendError::Full(_) => EngineError::Worker("engine worker queue is full".to_owned()),
        TrySendError::Disconnected(_) => worker_closed(),
    }
}

pub(super) fn set_streaming_lifecycle(
    snapshot: &Arc<Mutex<EngineWorkerSnapshot>>,
    lifecycle: OutputLifecycle,
) {
    if let Ok(mut snapshot) = snapshot.lock() {
        snapshot.engine.streaming_lifecycle = lifecycle;
    }
}

pub(super) fn set_recording_lifecycle(
    snapshot: &Arc<Mutex<EngineWorkerSnapshot>>,
    lifecycle: OutputLifecycle,
) {
    if let Ok(mut snapshot) = snapshot.lock() {
        snapshot.engine.recording_lifecycle = lifecycle;
    }
}

pub(super) fn set_replay_lifecycle(
    snapshot: &Arc<Mutex<EngineWorkerSnapshot>>,
    lifecycle: OutputLifecycle,
) {
    if let Ok(mut snapshot) = snapshot.lock() {
        snapshot.engine.replay_lifecycle = lifecycle;
    }
}

pub(super) fn set_replay_save_status(
    snapshot: &Arc<Mutex<EngineWorkerSnapshot>>,
    status: ReplaySaveStatus,
) {
    if let Ok(mut snapshot) = snapshot.lock() {
        snapshot.engine.replay_save_status = status;
    }
}

pub(super) fn push_output_event(events: &Arc<Mutex<VecDeque<OutputEvent>>>, event: OutputEvent) {
    if let Ok(mut events) = events.lock() {
        if events.len() == OUTPUT_EVENT_CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }
}
