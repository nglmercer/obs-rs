use super::*;

#[test]
fn ticks_keep_audio_and_video_packets_monotonic() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let tick = engine.tick(None, Some("program")).expect("tick");
    assert_eq!(tick.audio_blocks.len(), 1);
    for _ in 0..4 {
        engine.tick(None, Some("program")).expect("tick");
    }
    assert_eq!(engine.stats().video_frames, 5);
    assert!(engine.stats().audio_blocks >= 10);
    assert_eq!(engine.reference_video_encode_calls, 0);
    assert_eq!(engine.reference_audio_encode_calls, 0);
    let sync = engine.stats().av_sync;
    assert_eq!(sync.observations(), 5);
    assert!(sync.max_abs_delta_nanos() > 0);
}

#[test]
fn microphone_sync_offset_delays_only_that_channel_and_is_bounded() {
    let config = EngineConfig::default().with_audio_input_sync_offset_millis(10);
    let mut engine = EngineSession::new(project(), config).expect("engine");

    let first = engine
        .drain_audio_until(Timestamp::ZERO)
        .expect("first audio block");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].sample(0, 0), Some(0.0));

    let second = engine
        .drain_audio_until(Timestamp::from_millis(10))
        .expect("second audio block");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].sample(0, 0), Some(0.12));

    engine
        .set_channel_sync_offset_millis(EngineAudioChannel::Microphone, 0)
        .expect("clear sync offset");
    let immediate = engine
        .drain_audio_until(Timestamp::from_millis(20))
        .expect("third audio block");
    assert_eq!(immediate[0].sample(0, 0), Some(0.12));

    let error = engine
        .set_channel_sync_offset_millis(
            EngineAudioChannel::Microphone,
            obs_rs_audio::MAX_AUDIO_SYNC_OFFSET_MILLISECONDS + 1,
        )
        .expect_err("offset must remain bounded");
    assert!(error.to_string().contains("sync offset"));
}

#[test]
fn fallback_audio_is_reported() {
    let engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let snapshot = engine.snapshot();
    assert!(!snapshot.audio_fallback);
    assert_eq!(snapshot.audio_backend, "Deterministic test signal");
}

#[test]
fn selected_audio_input_does_not_silently_switch_to_another_device() {
    let engine = EngineSession::new(
        project(),
        EngineConfig::default().with_audio_input_id("missing-input"),
    )
    .expect("engine");
    let snapshot = engine.snapshot();
    assert!(snapshot.audio_fallback);
    assert_eq!(snapshot.audio_backend, "simulated fallback");
}

#[test]
fn desktop_and_microphone_are_distinct_metered_mixer_sources() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine
        .set_channel_gain_milli(EngineAudioChannel::Desktop, 500)
        .expect("desktop gain");
    engine
        .set_channel_muted(EngineAudioChannel::Microphone, false)
        .expect("microphone mute");

    engine.tick(None, Some("program")).expect("tick");
    let stats = engine.stats();
    assert_eq!(
        stats.desktop_peak_milli, 0,
        "unavailable desktop capture is silence"
    );
    assert!(
        stats.microphone_peak_milli > 0,
        "the deterministic input drives only the microphone node"
    );
    assert_eq!(
        stats.microphone_peak_hold_milli, stats.microphone_peak_milli,
        "the live meter publishes its held peak from the same mixer source"
    );
    assert!(!stats.microphone_clipped);
}

#[test]
fn engine_rejects_gain_above_the_bounded_mixer_control() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    assert!(engine
        .set_channel_gain_milli(EngineAudioChannel::Microphone, MAX_GAIN_MILLI + 1)
        .is_err());
}

#[test]
fn pan_reaches_the_mixed_audio_output_before_encoding() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    engine
        .set_channel_pan_milli(EngineAudioChannel::Microphone, -1_000)
        .expect("full-left pan");
    let tick = engine.tick(None, Some("program")).expect("panned tick");
    let block = tick.audio_blocks.first().expect("audio block");
    assert!(block
        .samples()
        .chunks_exact(2)
        .all(|frame| frame[1].abs() < f32::EPSILON));
    assert!(block
        .samples()
        .chunks_exact(2)
        .any(|frame| frame[0].abs() > f32::EPSILON));
}

#[test]
fn gain_filter_runs_on_a_live_channel_before_metering_and_mix() {
    let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    baseline.tick(None, Some("program")).expect("baseline tick");
    let baseline_peak = baseline.stats().microphone_peak_milli;

    let mut filtered = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    filtered
        .set_channel_gain_filter_db_milli(EngineAudioChannel::Microphone, -6_000)
        .expect("gain filter");
    let tick = filtered.tick(None, Some("program")).expect("filtered tick");

    assert!(
        baseline_peak > 0,
        "deterministic microphone must produce audio"
    );
    assert!(
        filtered.stats().microphone_peak_milli < baseline_peak,
        "the filtered channel meter must see gain-filtered audio"
    );
    assert!(
        tick.audio_blocks
            .iter()
            .flat_map(AudioBuffer::samples)
            .any(|sample| sample.abs() > 0.0),
        "the filter must preserve a non-silent live channel"
    );
}

#[test]
fn invert_polarity_runs_on_a_live_channel_without_changing_peak() {
    let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let baseline_tick = baseline.tick(None, Some("program")).expect("baseline tick");

    let mut inverted = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    inverted
        .set_channel_invert_polarity(EngineAudioChannel::Microphone)
        .expect("invert polarity");
    let inverted_tick = inverted.tick(None, Some("program")).expect("inverted tick");

    assert_eq!(
        baseline.stats().microphone_peak_milli,
        inverted.stats().microphone_peak_milli
    );
    let mut saw_signal = false;
    for (original, inverted) in baseline_tick.audio_blocks[0]
        .samples()
        .iter()
        .zip(inverted_tick.audio_blocks[0].samples())
    {
        assert!((original + inverted).abs() < 0.000_001);
        saw_signal |= original.abs() > 0.0;
    }
    assert!(saw_signal, "deterministic microphone must produce audio");
}

#[test]
fn limiter_runs_on_a_live_channel_before_metering_and_mix() {
    let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    baseline.tick(None, Some("program")).expect("baseline tick");
    let baseline_peak = baseline.stats().microphone_peak_milli;

    let mut limited = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    limited
        .set_channel_limiter(EngineAudioChannel::Microphone, -60_000, 60)
        .expect("limiter");
    let tick = limited.tick(None, Some("program")).expect("limited tick");

    assert!(
        baseline_peak > 0,
        "deterministic microphone must produce audio"
    );
    assert!(
        limited.stats().microphone_peak_milli < baseline_peak,
        "the channel meter must see limiter gain reduction"
    );
    assert!(
        tick.audio_blocks
            .iter()
            .flat_map(AudioBuffer::samples)
            .all(|sample| sample.is_finite()),
        "limiting must preserve the finite audio contract"
    );
}

#[test]
fn compressor_runs_on_a_live_channel_before_metering_and_mix() {
    let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    baseline.tick(None, Some("program")).expect("baseline tick");
    let baseline_peak = baseline.stats().microphone_peak_milli;

    let mut compressed = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    compressed
        .set_channel_compressor(EngineAudioChannel::Microphone, 32_000, -60_000, 1, 60, 0)
        .expect("compressor");
    let tick = compressed
        .tick(None, Some("program"))
        .expect("compressed tick");

    assert!(
        baseline_peak > 0,
        "deterministic microphone must produce audio"
    );
    assert!(
        compressed.stats().microphone_peak_milli < baseline_peak,
        "the channel meter must see compressor gain reduction"
    );
    assert!(
        tick.audio_blocks
            .iter()
            .flat_map(AudioBuffer::samples)
            .all(|sample| sample.is_finite()),
        "compression must preserve the finite audio contract"
    );
}

#[test]
fn expander_runs_on_a_live_channel_before_metering_and_mix() {
    let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    baseline.tick(None, Some("program")).expect("baseline tick");
    let baseline_peak = baseline.stats().microphone_peak_milli;

    let mut expanded = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    expanded
        .set_channel_expander(EngineAudioChannel::Microphone, 10_000, 0, 1, 60, 0)
        .expect("expander");
    expanded
        .tick(None, Some("program"))
        .expect("first expanded tick");
    let tick = expanded.tick(None, Some("program")).expect("expanded tick");

    assert!(
        baseline_peak > 0,
        "deterministic microphone must produce audio"
    );
    assert!(
        expanded.stats().microphone_peak_milli < baseline_peak,
        "the channel meter must see expander attenuation"
    );
    assert!(
        tick.audio_blocks
            .iter()
            .flat_map(AudioBuffer::samples)
            .all(|sample| sample.is_finite()),
        "expansion must preserve the finite audio contract"
    );
}

#[test]
fn noise_gate_runs_on_a_live_channel_before_metering_and_mix() {
    let mut baseline = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    baseline.tick(None, Some("program")).expect("baseline tick");
    let baseline_peak = baseline.stats().microphone_peak_milli;

    let mut gated = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    gated
        .set_channel_noise_gate(EngineAudioChannel::Microphone, 0, -32_000, 1, 125, 150)
        .expect("noise gate");
    gated.tick(None, Some("program")).expect("first gated tick");
    let tick = gated.tick(None, Some("program")).expect("gated tick");

    assert!(
        baseline_peak > 0,
        "deterministic microphone must produce audio"
    );
    assert!(
        gated.stats().microphone_peak_milli < baseline_peak,
        "the channel meter must see gate attenuation"
    );
    assert!(
        tick.audio_blocks
            .iter()
            .flat_map(AudioBuffer::samples)
            .all(|sample| sample.is_finite()),
        "gating must preserve the finite audio contract"
    );
}

/// Provider exposing one playback route whose monitor is readable, which is
/// the shape a real desktop capture takes on Linux.
#[derive(Debug)]
struct MonitorProvider;

impl AudioInputProvider for MonitorProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
        Ok(vec![AudioDeviceInfo::new(
            "speakers",
            "Speakers",
            AudioDeviceKind::Output,
        )?])
    }

    fn open_input(
        &self,
        _device_id: &str,
        _format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
        Err(obs_rs_audio::AudioDeviceError::Unavailable(
            "Speakers are playback-only; use loopback".to_owned(),
        ))
    }

    fn open_loopback(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
        if device_id != "speakers" {
            return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                device_id.to_owned(),
            ));
        }
        SimulatedAudioProvider::new().open_input("test-audio", format)
    }
}

/// Provider whose default routes are deliberately not first in discovery
/// order, proving automatic selection does not depend on vector order.
#[derive(Debug)]
struct DefaultRouteProvider;

impl AudioInputProvider for DefaultRouteProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
        let mut default_input = AudioDeviceInfo::new(
            "default-input",
            "Default microphone",
            AudioDeviceKind::Input,
        )?;
        default_input.set_default(true);
        let mut default_output = AudioDeviceInfo::new(
            "default-output",
            "Default speakers",
            AudioDeviceKind::Output,
        )?;
        default_output.set_default(true);
        Ok(vec![
            AudioDeviceInfo::new("other-input", "Other microphone", AudioDeviceKind::Input)?,
            AudioDeviceInfo::new("other-output", "Other speakers", AudioDeviceKind::Output)?,
            default_input,
            default_output,
        ])
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
        if !matches!(
            device_id,
            "default-input" | "other-input" | "default-output" | "other-output"
        ) {
            return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                device_id.to_owned(),
            ));
        }
        SimulatedAudioProvider::new().open_input("test-audio", format)
    }
}

#[test]
fn automatic_audio_routes_prefer_provider_defaults_over_discovery_order() {
    let config = EngineConfig::default().with_audio_provider(Arc::new(DefaultRouteProvider));
    let engine = EngineSession::new(project(), config).expect("engine");

    assert_eq!(
        engine.audio_active_device_id.as_deref(),
        Some("default-input")
    );
    assert_eq!(
        engine.desktop_audio_active_device_id.as_deref(),
        Some("default-output")
    );
    assert_eq!(
        engine.snapshot().desktop_audio,
        DesktopAudioSource::Monitor("Default speakers".to_owned())
    );
}

#[derive(Debug)]
struct ChangingDefaultProvider {
    phase: Arc<AtomicUsize>,
    opens: Arc<AtomicUsize>,
}

impl AudioInputProvider for ChangingDefaultProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
        let input_id = if self.phase.load(Ordering::Acquire) == 0 {
            "first-input"
        } else {
            "second-input"
        };
        let input_name = if input_id == "first-input" {
            "First microphone"
        } else {
            "Second microphone"
        };
        let mut input = AudioDeviceInfo::new(input_id, input_name, AudioDeviceKind::Input)?;
        input.set_default(true);
        let mut output =
            AudioDeviceInfo::new("stable-output", "Stable speakers", AudioDeviceKind::Output)?;
        output.set_default(true);
        Ok(vec![input, output])
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
        if !matches!(device_id, "first-input" | "second-input" | "stable-output") {
            return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                device_id.to_owned(),
            ));
        }
        self.opens.fetch_add(1, Ordering::AcqRel);
        SimulatedAudioProvider::new().open_input("test-audio", format)
    }
}

#[test]
fn automatic_audio_routes_reconcile_a_live_default_change_off_tick() {
    let phase = Arc::new(AtomicUsize::new(0));
    let opens = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(ChangingDefaultProvider {
        phase: Arc::clone(&phase),
        opens: Arc::clone(&opens),
    });
    let config = EngineConfig::default().with_audio_provider(provider);
    let mut engine = EngineSession::new(project(), config).expect("engine");
    assert_eq!(
        engine.audio_active_device_id.as_deref(),
        Some("first-input")
    );
    engine
        .drain_audio_until(Timestamp::ZERO)
        .expect("initial route refresh");
    phase.store(1, Ordering::Release);

    for attempt in 0..100_u64 {
        engine
            .drain_audio_until(Timestamp::from_millis(500 + attempt * 10))
            .expect("route-refresh audio");
        if engine.audio_active_device_id.as_deref() == Some("second-input") {
            break;
        }
        std::thread::yield_now();
    }

    assert_eq!(
        engine.audio_active_device_id.as_deref(),
        Some("second-input")
    );
    assert!(!engine.snapshot().audio_fallback);
    assert!(opens.load(Ordering::Acquire) >= 3);
}

#[test]
fn explicit_audio_selection_ignores_a_live_default_change() {
    let phase = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(ChangingDefaultProvider {
        phase: Arc::clone(&phase),
        opens: Arc::new(AtomicUsize::new(0)),
    });
    let config = EngineConfig::default()
        .with_audio_provider(provider)
        .with_audio_input_id("first-input");
    let mut engine = EngineSession::new(project(), config).expect("engine");
    engine
        .drain_audio_until(Timestamp::ZERO)
        .expect("initial route refresh");
    phase.store(1, Ordering::Release);

    for attempt in 0..40_u64 {
        engine
            .drain_audio_until(Timestamp::from_millis(500 + attempt * 10))
            .expect("explicit route audio");
        std::thread::yield_now();
    }

    assert_eq!(
        engine.audio_active_device_id.as_deref(),
        Some("first-input")
    );
}

#[derive(Debug)]
struct NativeFormatProvider;

impl AudioInputProvider for NativeFormatProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Ok(vec![AudioDeviceInfo::new(
            "native-mono",
            "Native mono device",
            AudioDeviceKind::Input,
        )?])
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, AudioDeviceError> {
        let native = AudioFormat::new(44_100, 1)?;
        if device_id != "native-mono" || format != native {
            return Err(AudioDeviceError::Unavailable(
                "device only accepts its native 44.1 kHz mono format".to_owned(),
            ));
        }
        SimulatedAudioProvider::new().open_input("test-audio", native)
    }
}

#[test]
fn device_native_audio_is_mapped_and_resampled_to_the_mix_format() {
    let provider: Arc<dyn AudioInputProvider> = Arc::new(NativeFormatProvider);
    let mix = AudioFormat::new(48_000, 2).expect("mix format");
    let (mut input, name, fallback, active_id) =
        open_audio_input(&provider, mix, Some("native-mono"));
    assert_eq!(name, "Native mono device");
    assert!(!fallback);
    assert_eq!(active_id.as_deref(), Some("native-mono"));
    assert_eq!(input.format(), mix);
    let block = input
        .read_block(Timestamp::ZERO, 480)
        .expect("converted block");
    assert_eq!(block.format(), mix);
    assert_eq!(block.frames(), 480);
    assert!(block
        .samples()
        .chunks_exact(2)
        .all(|frame| (frame[0] - frame[1]).abs() < f32::EPSILON));
}

/// An input that serves `healthy_blocks` and then fails permanently.
struct FailingAudioInput {
    format: AudioFormat,
    inner: Box<dyn AudioInput>,
    healthy_blocks: usize,
}

impl AudioInput for FailingAudioInput {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn state(&self) -> obs_rs_audio::AudioInputState {
        self.inner.state()
    }

    fn read_block(
        &mut self,
        timestamp: Timestamp,
        frames: usize,
    ) -> Result<obs_rs_audio::AudioBuffer, obs_rs_audio::AudioDeviceError> {
        if self.healthy_blocks == 0 {
            return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                "unplugged".to_owned(),
            ));
        }
        self.healthy_blocks -= 1;
        self.inner.read_block(timestamp, frames)
    }

    fn stop(&mut self) {
        self.inner.stop();
    }
}

struct FailingProvider;

impl AudioInputProvider for FailingProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
        Ok(vec![AudioDeviceInfo::new(
            "failing-input",
            "Failing input",
            AudioDeviceKind::Input,
        )?])
    }

    fn open_input(
        &self,
        _device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
        Ok(Box::new(FailingAudioInput {
            format,
            inner: SimulatedAudioProvider::new().open_input("test-audio", format)?,
            healthy_blocks: 2,
        }))
    }
}

struct ReconnectingProvider {
    opens: Arc<AtomicUsize>,
}

impl AudioInputProvider for ReconnectingProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
        Ok(vec![AudioDeviceInfo::new(
            "reconnecting-input",
            "Reconnecting input",
            AudioDeviceKind::Input,
        )?])
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
        if device_id != "reconnecting-input" {
            return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                device_id.to_owned(),
            ));
        }
        let attempt = self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FailingAudioInput {
            format,
            inner: SimulatedAudioProvider::new().open_input("test-audio", format)?,
            healthy_blocks: if attempt == 0 { 2 } else { usize::MAX },
        }))
    }
}

struct ReconnectingMonitorProvider {
    opens: Arc<AtomicUsize>,
}

impl AudioInputProvider for ReconnectingMonitorProvider {
    fn discover(&self) -> Result<Vec<AudioDeviceInfo>, obs_rs_audio::AudioDeviceError> {
        Ok(vec![AudioDeviceInfo::new(
            "reconnecting-monitor",
            "Reconnecting monitor",
            AudioDeviceKind::Output,
        )?])
    }

    fn open_input(
        &self,
        device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioInput>, obs_rs_audio::AudioDeviceError> {
        if device_id != "reconnecting-monitor" {
            return Err(obs_rs_audio::AudioDeviceError::Unavailable(
                device_id.to_owned(),
            ));
        }
        let attempt = self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FailingAudioInput {
            format,
            inner: SimulatedAudioProvider::new().open_input("test-audio", format)?,
            healthy_blocks: if attempt == 0 { 2 } else { usize::MAX },
        }))
    }
}

#[test]
fn falling_back_after_a_device_failure_keeps_the_audio_timeline_continuous() {
    // The timeline, not the device, issues block timestamps, and the
    // fallback stamps the block it is handed. Swapping providers mid-session
    // must therefore leave no gap, overlap, or repeat in the emitted
    // timestamps — that continuity is what keeps A/V in sync afterwards.
    let config = EngineConfig::default().with_audio_provider(Arc::new(FailingProvider));
    let mut engine = EngineSession::new(project(), config).expect("engine");

    let mut timestamps = Vec::new();
    for index in 0..6_u64 {
        let blocks = engine
            .drain_audio_until(Timestamp::from_millis(index * 100))
            .expect("audio blocks");
        timestamps.extend(blocks.iter().map(|block| block.timestamp().as_nanos()));
    }

    assert!(
        engine.snapshot().audio_fallback,
        "the failing device should have been replaced"
    );
    assert!(
        timestamps.len() > 3,
        "the fallback kept producing blocks: {timestamps:?}"
    );
    assert_eq!(timestamps.first(), Some(&0));
    let steps = timestamps
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    let block_nanos = steps[0];
    assert!(block_nanos > 0, "blocks must advance the timeline");
    assert!(
        steps.iter().all(|step| *step == block_nanos),
        "block spacing changed across the fallback: {steps:?}"
    );
}

#[test]
fn selected_audio_input_reconnects_after_a_bounded_media_interval() {
    let opens = Arc::new(AtomicUsize::new(0));
    let config = EngineConfig::default()
        .with_audio_provider(Arc::new(ReconnectingProvider {
            opens: Arc::clone(&opens),
        }))
        .with_audio_input_id("reconnecting-input");
    let mut engine = EngineSession::new(project(), config).expect("engine");

    engine
        .drain_audio_until(Timestamp::from_millis(900))
        .expect("fallback audio blocks");
    assert!(engine.snapshot().audio_fallback);
    assert_eq!(opens.load(Ordering::SeqCst), 1);

    engine
        .drain_audio_until(Timestamp::from_millis(1_100))
        .expect("reconnected audio blocks");
    let snapshot = engine.snapshot();
    assert!(!snapshot.audio_fallback);
    assert_eq!(snapshot.audio_backend, "Reconnecting input");
    assert_eq!(opens.load(Ordering::SeqCst), 2);
    assert!(snapshot.last_error.is_none());
}

#[test]
fn automatic_audio_input_reconnects_after_a_bounded_media_interval() {
    let opens = Arc::new(AtomicUsize::new(0));
    let config = EngineConfig::default().with_audio_provider(Arc::new(ReconnectingProvider {
        opens: Arc::clone(&opens),
    }));
    let mut engine = EngineSession::new(project(), config).expect("engine");
    assert_eq!(
        engine.audio_active_device_id.as_deref(),
        Some("reconnecting-input")
    );

    engine
        .drain_audio_until(Timestamp::from_millis(900))
        .expect("fallback audio blocks");
    assert!(engine.snapshot().audio_fallback);
    assert!(engine.audio_active_device_id.is_none());
    assert_eq!(opens.load(Ordering::SeqCst), 1);

    engine
        .drain_audio_until(Timestamp::from_millis(1_100))
        .expect("reconnected audio blocks");
    let snapshot = engine.snapshot();
    assert!(!snapshot.audio_fallback);
    assert_eq!(
        engine.audio_active_device_id.as_deref(),
        Some("reconnecting-input")
    );
    assert_eq!(opens.load(Ordering::SeqCst), 2);
}

#[test]
fn selected_desktop_monitor_reconnects_after_a_bounded_media_interval() {
    let opens = Arc::new(AtomicUsize::new(0));
    let config = EngineConfig::default()
        .with_audio_provider(Arc::new(ReconnectingMonitorProvider {
            opens: Arc::clone(&opens),
        }))
        .with_desktop_audio_id("reconnecting-monitor");
    let mut engine = EngineSession::new(project(), config).expect("engine");

    engine
        .drain_audio_until(Timestamp::from_millis(900))
        .expect("silent desktop blocks");
    assert_eq!(
        engine.snapshot().desktop_audio,
        DesktopAudioSource::Silent("unavailable (audio device unavailable: unplugged)".to_owned())
    );
    assert_eq!(opens.load(Ordering::SeqCst), 1);

    engine
        .drain_audio_until(Timestamp::from_millis(1_100))
        .expect("reconnected desktop blocks");
    let snapshot = engine.snapshot();
    assert_eq!(
        snapshot.desktop_audio,
        DesktopAudioSource::Monitor("Reconnecting monitor".to_owned())
    );
    assert_eq!(opens.load(Ordering::SeqCst), 2);
}

#[test]
fn automatic_desktop_monitor_reconnects_after_a_bounded_media_interval() {
    let opens = Arc::new(AtomicUsize::new(0));
    let config =
        EngineConfig::default().with_audio_provider(Arc::new(ReconnectingMonitorProvider {
            opens: Arc::clone(&opens),
        }));
    let mut engine = EngineSession::new(project(), config).expect("engine");
    assert_eq!(
        engine.desktop_audio_active_device_id.as_deref(),
        Some("reconnecting-monitor")
    );

    engine
        .drain_audio_until(Timestamp::from_millis(900))
        .expect("silent desktop blocks");
    assert!(engine.desktop_audio_active_device_id.is_none());
    assert_eq!(opens.load(Ordering::SeqCst), 1);

    engine
        .drain_audio_until(Timestamp::from_millis(1_100))
        .expect("reconnected desktop blocks");
    let snapshot = engine.snapshot();
    assert_eq!(
        snapshot.desktop_audio,
        DesktopAudioSource::Monitor("Reconnecting monitor".to_owned())
    );
    assert_eq!(
        engine.desktop_audio_active_device_id.as_deref(),
        Some("reconnecting-monitor")
    );
    assert_eq!(opens.load(Ordering::SeqCst), 2);
}

#[test]
fn selected_desktop_monitor_does_not_silently_switch_to_another_route() {
    let config = EngineConfig::default()
        .with_audio_provider(Arc::new(MonitorProvider))
        .with_desktop_audio_id("missing-monitor");
    let engine = EngineSession::new(project(), config).expect("engine");

    assert_eq!(
        engine.snapshot().desktop_audio,
        DesktopAudioSource::Silent("no playback monitor".to_owned())
    );
}

#[test]
fn a_playback_monitor_feeds_the_desktop_channel() {
    let config = EngineConfig::default().with_audio_provider(Arc::new(MonitorProvider));
    let mut engine = EngineSession::new(project(), config).expect("engine");

    assert_eq!(
        engine.snapshot().desktop_audio,
        DesktopAudioSource::Monitor("Speakers".to_owned())
    );

    engine.tick(None, Some("program")).expect("tick");

    assert!(
        engine.stats().desktop_peak_milli > 0,
        "the opened monitor drives the desktop meter"
    );
}

#[test]
fn a_session_without_a_playback_route_keeps_the_desktop_channel_silent() {
    let engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");
    let snapshot = engine.snapshot();

    assert!(!snapshot.desktop_audio.is_capturing());
    assert_eq!(snapshot.desktop_audio.label(), "no playback monitor");
}

#[test]
fn monitor_audio_updates_levels_without_encoding_video() {
    let mut engine = EngineSession::new(project(), EngineConfig::default()).expect("engine");

    engine
        .monitor_audio_until(Timestamp::ZERO)
        .expect("monitor tick");

    assert!(engine.stats().microphone_peak_milli > 0);
    assert_eq!(engine.stats().video_frames, 0);
    assert_eq!(engine.stats().audio_blocks, 1);
}

#[test]
fn monitor_modes_route_engine_audio_to_the_bounded_output_worker() {
    let config = EngineConfig::default()
        .with_audio_input_monitor_mode(AudioMonitorMode::MonitorOnly)
        .with_monitor_output_id("test-output");
    let mut engine = EngineSession::new(project(), config).expect("engine");

    let output = engine
        .drain_audio_until(Timestamp::ZERO)
        .expect("audio block");
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].sample(0, 0), Some(0.0));
    assert_eq!(engine.stats().monitor_blocks_submitted, 1);
    assert_eq!(engine.stats().monitor_blocks_dropped, 0);
    assert!(engine.snapshot().monitor_output.is_some());

    engine
        .set_channel_monitor_mode(EngineAudioChannel::Microphone, AudioMonitorMode::Off)
        .expect("switch monitor mode");
    assert_eq!(
        engine
            .mixer
            .source_monitor_mode(engine.microphone_audio_source)
            .expect("microphone source"),
        AudioMonitorMode::Off
    );
    engine
        .set_monitor_output_id(None)
        .expect("clear monitor output");
    assert!(engine.snapshot().monitor_output.is_none());
}

#[test]
fn monitor_output_worker_failure_is_visible_without_failing_the_engine_tick() {
    let config = EngineConfig::default().with_monitor_output_id("missing-output");
    let mut engine = EngineSession::new(project(), config).expect("engine");
    engine
        .monitor_audio_until(Timestamp::ZERO)
        .expect("monitor tick remains independent of sink failure");

    for _ in 0..100 {
        if engine
            .snapshot()
            .monitor_output
            .as_ref()
            .is_some_and(|output| output.state == AudioOutputWorkerState::Failed)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let output = engine
        .snapshot()
        .monitor_output
        .expect("configured monitor worker");
    assert_eq!(output.state, AudioOutputWorkerState::Failed);
    assert!(output
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("unavailable")));
}

#[test]
fn idle_audio_format_rebuild_preserves_routing_and_restarts_the_timeline() {
    let config = EngineConfig::default()
        .with_audio_input_monitor_mode(AudioMonitorMode::MonitorOnly)
        .with_monitor_output_id("test-output");
    let mut engine = EngineSession::new(project(), config).expect("engine");
    let next_format = AudioFormat::new(44_100, 1).expect("next format");

    engine
        .set_audio_format(next_format)
        .expect("idle format change");
    assert_eq!(engine.config.audio_format, next_format);
    assert_eq!(engine.timeline.audio_format(), next_format);
    assert_eq!(
        engine
            .mixer
            .source_monitor_mode(engine.microphone_audio_source)
            .expect("microphone source"),
        AudioMonitorMode::MonitorOnly
    );
    assert!(engine.snapshot().monitor_output.is_some());

    let tick = engine.tick(None, None).expect("reconfigured tick");
    assert!(!tick.audio_blocks.is_empty());
    assert!(tick
        .audio_blocks
        .iter()
        .all(|buffer| buffer.format() == next_format));

    engine
        .start_replay_buffer(1_024 * 1_024, Duration::from_secs(5))
        .expect("replay buffer");
    let error = engine
        .set_audio_format(AudioFormat::new(48_000, 2).expect("other format"))
        .expect_err("active replay must block format replacement");
    assert!(matches!(
        error,
        EngineError::Busy("change the audio format")
    ));
    engine.stop_replay_buffer();
}
