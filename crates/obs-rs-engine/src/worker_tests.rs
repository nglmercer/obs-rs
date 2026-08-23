
use std::time::Duration;

use obs_rs_audio::{AudioFormat, AudioMonitorMode};
use obs_rs_media::{FrameRate, Timestamp, VideoFormat, VideoFrame};

use super::worker_runtime::push_output_event;
use super::*;
use crate::EngineConfig;

fn worker(capacity: usize) -> EngineWorker {
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("valid frame rate"))
        .expect("valid video format");
    let audio = AudioFormat::new(48_000, 2).expect("valid audio format");
    worker_with_config(capacity, format, EngineConfig::new(audio))
}

fn worker_with_config(capacity: usize, format: VideoFormat, config: EngineConfig) -> EngineWorker {
    let session = EngineSession::for_format(format, config).expect("session");
    EngineWorker::spawn_with_capacity(session, capacity).expect("worker")
}

#[test]
fn stream_start_returns_without_waiting_for_worker_or_transport_setup() {
    let worker = Arc::new(worker(1));
    let (entered_send, entered_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    worker
        .sender
        .send(WorkerCommand::TestBlock(entered_send, release_receive))
        .expect("block command");
    entered_receive.recv().expect("worker entered barrier");

    let requesting_worker = Arc::clone(&worker);
    let (result_send, result_receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = result_send.send(requesting_worker.start_streaming("127.0.0.1:0"));
    });
    result_receive
        .recv_timeout(Duration::from_millis(250))
        .expect("start must return while the worker remains blocked")
        .expect("start request accepted");
    assert_eq!(
        worker.snapshot().engine.streaming_lifecycle,
        OutputLifecycle::Starting
    );
    assert_eq!(worker.take_output_events(), vec![OutputEvent::Starting]);

    release_send.send(()).expect("release worker");
    for _ in 0..100 {
        if worker.snapshot().engine.streaming_lifecycle == OutputLifecycle::Failed {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        worker.snapshot().engine.streaming_lifecycle,
        OutputLifecycle::Failed
    );
    assert!(matches!(
        worker.take_output_events().as_slice(),
        [OutputEvent::Failed { .. }]
    ));
}

#[test]
fn stream_stop_returns_without_waiting_for_worker_teardown() {
    let worker = Arc::new(worker(1));
    let (entered_send, entered_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    worker
        .sender
        .send(WorkerCommand::TestBlock(entered_send, release_receive))
        .expect("block command");
    entered_receive.recv().expect("worker entered barrier");

    let requesting_worker = Arc::clone(&worker);
    let (result_send, result_receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = result_send.send(requesting_worker.finish_streaming());
    });
    result_receive
        .recv_timeout(Duration::from_millis(250))
        .expect("stop must return while the worker remains blocked")
        .expect("stop request accepted");
    assert_eq!(
        worker.snapshot().engine.streaming_lifecycle,
        OutputLifecycle::Stopping
    );
    assert_eq!(worker.take_output_events(), vec![OutputEvent::Stopping]);

    release_send.send(()).expect("release worker");
    for _ in 0..100 {
        if worker.snapshot().engine.streaming_lifecycle == OutputLifecycle::Idle {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        worker.snapshot().engine.streaming_lifecycle,
        OutputLifecycle::Idle
    );
    assert_eq!(worker.take_output_events(), vec![OutputEvent::Stopped]);
}

#[test]
fn recording_requests_return_without_waiting_for_file_work() {
    let worker = Arc::new(worker(1));
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-engine-async-{token}.obsr"));

    let (entered_send, entered_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    worker
        .sender
        .send(WorkerCommand::TestBlock(entered_send, release_receive))
        .expect("block command");
    entered_receive.recv().expect("worker entered barrier");

    let requesting_worker = Arc::clone(&worker);
    let requesting_path = path.clone();
    let (result_send, result_receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = result_send.send(requesting_worker.try_start_recording(requesting_path, None));
    });
    result_receive
        .recv_timeout(Duration::from_millis(250))
        .expect("start must return while the worker remains blocked")
        .expect("start request accepted");
    assert_eq!(
        worker.snapshot().engine.recording_lifecycle,
        OutputLifecycle::Starting
    );

    release_send.send(()).expect("release worker");
    for _ in 0..100 {
        if worker.snapshot().engine.recording_lifecycle == OutputLifecycle::Running {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        worker.snapshot().engine.recording_lifecycle,
        OutputLifecycle::Running
    );

    let (entered_send, entered_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    worker
        .sender
        .send(WorkerCommand::TestBlock(entered_send, release_receive))
        .expect("block command");
    entered_receive.recv().expect("worker entered barrier");

    let requesting_worker = Arc::clone(&worker);
    let (result_send, result_receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = result_send.send(requesting_worker.try_finish_recording());
    });
    result_receive
        .recv_timeout(Duration::from_millis(250))
        .expect("finish must return while the worker remains blocked")
        .expect("finish request accepted");
    assert_eq!(
        worker.snapshot().engine.recording_lifecycle,
        OutputLifecycle::Stopping
    );

    release_send.send(()).expect("release worker");
    for _ in 0..100 {
        if worker.snapshot().engine.recording_lifecycle == OutputLifecycle::Idle {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        worker.snapshot().engine.recording_lifecycle,
        OutputLifecycle::Idle
    );
    assert!(path.exists(), "the worker finalized the requested path");
    std::fs::remove_file(path).expect("remove recording fixture");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn worker_accepts_remux_recording_and_finalizes_mp4() {
    let capabilities = obs_rs_output_gstreamer::GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::MatroskaH264Aac)
        || !capabilities.supports_remux()
    {
        return;
    }
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    let worker = EngineWorker::spawn_with_capacity(
        EngineSession::for_format(
            format,
            EngineConfig::new(AudioFormat::new(48_000, 2).expect("audio format")),
        )
        .expect("session"),
        2,
    )
    .expect("worker");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("obs-rs-worker-auto-remux-{token}.mp4"));
    let source_path = path.with_extension("mkv.part");
    worker
        .start_remux_recording(&path)
        .expect("start remux recording");
    assert!(worker.try_push_frame(VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 255],)));
    let bytes = worker.finish_recording().expect("finish remux recording");
    let persisted = std::fs::read(&path).expect("read remuxed recording");
    assert_eq!(persisted.len(), bytes);
    assert_eq!(persisted.get(4..8), Some(&b"ftyp"[..]));
    assert!(!source_path.exists());
    std::fs::remove_file(path).expect("remove remux recording");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn worker_recovers_interrupted_remux_on_the_worker_thread() {
    let capabilities = obs_rs_output_gstreamer::GStreamerCapabilitySnapshot::probe();
    if !capabilities
        .output_capabilities()
        .supports(obs_rs_output::OutputProfileKind::MatroskaH264Aac)
        || !capabilities.supports_remux()
    {
        return;
    }
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let final_path = std::env::temp_dir().join(format!("obs-rs-worker-recovery-{token}.mp4"));
    let completed_source = final_path.with_extension("mkv");
    let interrupted_source = final_path.with_extension("mkv.part");
    let mut source_session =
        EngineSession::for_format(format, EngineConfig::new(audio)).expect("source session");
    source_session
        .start_recording(&completed_source)
        .expect("Matroska source recording");
    for index in 0_u64..4 {
        let timestamp = Timestamp::from_nanos(index * 1_000_000_000 / 30);
        source_session
            .push_program_frame(&VideoFrame::solid(format, timestamp, [24, 96, 180, 255]))
            .expect("media frame");
    }
    source_session
        .finish_recording()
        .expect("close Matroska source");
    std::fs::rename(&completed_source, &interrupted_source).expect("hide source");
    obs_rs_output_gstreamer::write_interrupted_remux_manifest(&final_path)
        .expect("recovery manifest");

    let worker = worker(2);
    let receive = worker
        .try_recover_interrupted_remux_recording(&final_path)
        .expect("enqueue recovery");
    let recovered = receive
        .recv_timeout(Duration::from_mins(1))
        .expect("recovery result")
        .expect("recover remux");
    assert!(matches!(recovered, RemuxRecovery::Recovered { bytes } if bytes > 0));
    assert!(final_path.is_file());
    assert!(!interrupted_source.exists());
    std::fs::remove_file(final_path).expect("remove recovered MP4");
}

#[cfg(feature = "production-gstreamer")]
#[test]
fn worker_discovers_bounded_remux_candidates_on_the_worker_thread() {
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("obs-rs-worker-candidates-{token}"));
    std::fs::create_dir(&directory).expect("candidate directory");
    std::fs::write(directory.join("zeta.mkv.part"), [1_u8]).expect("zeta candidate");
    std::fs::write(directory.join("alpha.mkv.part"), [2_u8]).expect("alpha candidate");
    obs_rs_output_gstreamer::write_interrupted_remux_manifest(directory.join("zeta.mp4"))
        .expect("zeta manifest");
    obs_rs_output_gstreamer::write_interrupted_remux_manifest(directory.join("alpha.mp4"))
        .expect("alpha manifest");
    std::fs::write(directory.join("published.mkv.part"), [3_u8]).expect("published source");
    std::fs::write(directory.join("published.mp4"), [4_u8]).expect("published destination");

    let worker = worker(2);
    let receive = worker
        .try_discover_interrupted_remux_candidates(&directory)
        .expect("enqueue candidate discovery");
    let candidates = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("candidate discovery result")
        .expect("discover remux candidates");
    assert_eq!(
        candidates,
        vec![directory.join("alpha.mp4"), directory.join("zeta.mp4")]
    );

    std::fs::remove_dir_all(directory).expect("remove candidate directory");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the barrier test covers the complete replay request lifecycle"
)]
fn replay_requests_return_without_waiting_for_worker_or_file_work() {
    let worker = Arc::new(worker(1));
    let (entered_send, entered_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    worker
        .sender
        .send(WorkerCommand::TestBlock(entered_send, release_receive))
        .expect("block command");
    entered_receive.recv().expect("worker entered barrier");

    let requesting_worker = Arc::clone(&worker);
    let (result_send, result_receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = result_send
            .send(requesting_worker.try_start_replay_buffer(1024 * 1024, Duration::from_secs(5)));
    });
    result_receive
        .recv_timeout(Duration::from_millis(250))
        .expect("replay start must return while worker is blocked")
        .expect("replay start request accepted");
    assert_eq!(
        worker.snapshot().engine.replay_lifecycle,
        OutputLifecycle::Starting
    );
    release_send.send(()).expect("release worker");
    for _ in 0..100 {
        if worker.snapshot().engine.replay_lifecycle == OutputLifecycle::Running {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        worker.snapshot().engine.replay_lifecycle,
        OutputLifecycle::Running
    );

    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    assert!(worker.try_push_frame(VideoFrame::solid(
        format,
        Timestamp::ZERO,
        [0x20, 0x40, 0x80, 0xFF],
    )));
    for _ in 0..100 {
        if worker.snapshot().queued_frames == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    let path =
        std::env::temp_dir().join(format!("obs-rs-replay-async-{}.obsr", std::process::id()));
    let (entered_send, entered_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    worker
        .sender
        .send(WorkerCommand::TestBlock(entered_send, release_receive))
        .expect("block save command");
    entered_receive.recv().expect("worker entered save barrier");

    let requesting_worker = Arc::clone(&worker);
    let save_path = path.clone();
    let (result_send, result_receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = result_send.send(requesting_worker.try_save_replay_buffer(save_path));
    });
    result_receive
        .recv_timeout(Duration::from_millis(250))
        .expect("replay save must return while worker is blocked")
        .expect("replay save request accepted");
    assert!(matches!(
        worker.snapshot().engine.replay_save_status,
        ReplaySaveStatus::Saving
    ));
    release_send.send(()).expect("release save worker");
    for _ in 0..100 {
        if matches!(
            worker.snapshot().engine.replay_save_status,
            ReplaySaveStatus::Saved { .. }
        ) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        worker.snapshot().engine.replay_save_status,
        ReplaySaveStatus::Saved { .. }
    ));
    assert!(path.exists());

    let (entered_send, entered_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    worker
        .sender
        .send(WorkerCommand::TestBlock(entered_send, release_receive))
        .expect("block stop command");
    entered_receive.recv().expect("worker entered stop barrier");
    let requesting_worker = Arc::clone(&worker);
    let (result_send, result_receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = result_send.send(requesting_worker.try_stop_replay_buffer());
    });
    result_receive
        .recv_timeout(Duration::from_millis(250))
        .expect("replay stop must return while worker is blocked")
        .expect("replay stop request accepted");
    assert_eq!(
        worker.snapshot().engine.replay_lifecycle,
        OutputLifecycle::Stopping
    );
    release_send.send(()).expect("release stop worker");
    for _ in 0..100 {
        if worker.snapshot().engine.replay_lifecycle == OutputLifecycle::Idle {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        worker.snapshot().engine.replay_lifecycle,
        OutputLifecycle::Idle
    );
    std::fs::remove_file(path).expect("remove replay fixture");
}

#[test]
fn output_lifecycle_events_stay_bounded() {
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let attempts = u32::try_from(OUTPUT_EVENT_CAPACITY).expect("small event capacity") + 10;
    for attempt in 0..attempts {
        push_output_event(&events, OutputEvent::Reconnecting { attempt });
    }
    let events = events.lock().expect("event queue");
    assert_eq!(events.len(), OUTPUT_EVENT_CAPACITY);
    assert_eq!(
        events.front(),
        Some(&OutputEvent::Reconnecting { attempt: 10 })
    );
}

#[test]
fn gain_filter_updates_stay_on_the_worker_and_validate_bounds() {
    let worker = worker(1);
    worker
        .set_channel_gain_filter_db_milli(EngineAudioChannel::Microphone, -6_000)
        .expect("valid gain filter");
    worker
        .set_channel_pan_milli(EngineAudioChannel::Microphone, -1_000)
        .expect("valid pan");
    worker
        .set_channel_sync_offset_millis(EngineAudioChannel::Microphone, 100)
        .expect("valid sync offset");
    worker
        .set_channel_invert_polarity(EngineAudioChannel::Microphone)
        .expect("valid invert polarity filter");
    worker
        .set_channel_limiter(EngineAudioChannel::Microphone, -6_000, 60)
        .expect("valid limiter filter");
    worker
        .set_channel_compressor(EngineAudioChannel::Microphone, 10_000, -18_000, 6, 60, 0)
        .expect("valid compressor filter");
    worker
        .set_channel_expander(EngineAudioChannel::Microphone, 10_000, -40_000, 10, 50, 0)
        .expect("valid expander filter");
    worker
        .set_channel_noise_gate(
            EngineAudioChannel::Microphone,
            -26_000,
            -32_000,
            10,
            125,
            150,
        )
        .expect("valid noise gate filter");
    let error = worker
        .set_channel_gain_filter_db_milli(EngineAudioChannel::Microphone, 30_001)
        .expect_err("unbounded gain filter");
    assert!(matches!(error, EngineError::Worker(reason) if reason.contains("outside")));
    let error = worker
        .set_channel_sync_offset_millis(
            EngineAudioChannel::Microphone,
            obs_rs_audio::MAX_AUDIO_SYNC_OFFSET_MILLISECONDS + 1,
        )
        .expect_err("unbounded sync offset");
    assert!(matches!(error, EngineError::Worker(reason) if reason.contains("sync offset")));
    let error = worker
        .set_channel_pan_milli(EngineAudioChannel::Microphone, 1_001)
        .expect_err("unbounded pan");
    assert!(matches!(error, EngineError::Worker(reason) if reason.contains("pan")));
    let error = worker
        .set_channel_limiter(EngineAudioChannel::Microphone, -60_001, 60)
        .expect_err("unbounded limiter threshold");
    assert!(matches!(error, EngineError::Worker(reason) if reason.contains("threshold")));
    let error = worker
        .set_channel_compressor(EngineAudioChannel::Microphone, 32_001, -18_000, 6, 60, 0)
        .expect_err("unbounded compressor ratio");
    assert!(matches!(error, EngineError::Worker(reason) if reason.contains("ratio")));
    let error = worker
        .set_channel_expander(EngineAudioChannel::Microphone, 20_001, -40_000, 10, 50, 0)
        .expect_err("unbounded expander ratio");
    assert!(matches!(error, EngineError::Worker(reason) if reason.contains("ratio")));
    let error = worker
        .set_channel_noise_gate(
            EngineAudioChannel::Microphone,
            -97_000,
            -32_000,
            10,
            125,
            150,
        )
        .expect_err("unbounded noise gate open threshold");
    assert!(matches!(error, EngineError::Worker(reason) if reason.contains("open threshold")));
}

#[test]
fn monitor_controls_and_sink_selection_stay_on_the_worker() {
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let worker = worker_with_config(
        2,
        format,
        EngineConfig::new(audio).with_audio_input_monitor_mode(AudioMonitorMode::MonitorOnly),
    );

    worker
        .set_channel_monitor_mode(
            EngineAudioChannel::Microphone,
            AudioMonitorMode::MonitorOnly,
        )
        .expect("monitor mode");
    worker
        .set_monitor_output_id(Some("test-output"))
        .expect("monitor sink");
    assert!(worker.try_monitor_audio(Timestamp::ZERO));

    for _ in 0..100 {
        if worker.snapshot().engine.stats.monitor_blocks_submitted > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let snapshot = worker.snapshot();
    assert!(snapshot.engine.stats.monitor_blocks_submitted > 0);
    assert!(snapshot.engine.monitor_output.is_some());

    worker
        .set_monitor_output_id(None)
        .expect("clear monitor sink");
    for _ in 0..100 {
        if worker.snapshot().engine.monitor_output.is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(worker.snapshot().engine.monitor_output.is_none());
}

#[test]
fn audio_format_replacement_stays_on_the_worker_boundary() {
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    let audio = AudioFormat::new(48_000, 2).expect("audio format");
    let worker = worker_with_config(2, format, EngineConfig::new(audio));
    let next = AudioFormat::new(44_100, 1).expect("next audio format");

    worker.set_audio_format(next).expect("audio format change");
    assert!(worker.try_monitor_audio(Timestamp::ZERO));
    for _ in 0..100 {
        if worker.snapshot().engine.stats.audio_blocks > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(worker.snapshot().engine.stats.audio_blocks > 0);
}

#[test]
fn replay_buffer_controls_stay_on_the_worker() {
    let worker = worker(1);
    let path =
        std::env::temp_dir().join(format!("obs-rs-replay-worker-{}.obsr", std::process::id()));
    worker
        .start_replay_buffer(1024 * 1024, Duration::from_secs(5))
        .expect("start replay buffer");
    let format = VideoFormat::new(16, 16, FrameRate::new(30, 1).expect("rate")).expect("format");
    assert!(worker.try_push_frame(VideoFrame::solid(
        format,
        Timestamp::ZERO,
        [0x20, 0x40, 0x80, 0xFF],
    )));
    for _ in 0..100 {
        if worker.snapshot().queued_frames == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let bytes = worker
        .save_replay_buffer(&path)
        .expect("save replay buffer");
    assert!(bytes > 16);
    worker.stop_replay_buffer();
    std::fs::remove_file(path).expect("remove replay file");
}
