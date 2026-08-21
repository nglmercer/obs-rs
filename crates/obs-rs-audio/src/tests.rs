use super::*;
use obs_rs_media::Timestamp;
use std::time::Instant;

struct FakeClock {
    now: Timestamp,
    requested_deadlines: Vec<Timestamp>,
}

impl AudioClock for FakeClock {
    fn now(&self) -> Timestamp {
        self.now
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        self.requested_deadlines.push(deadline);
        if deadline > self.now {
            self.now = deadline;
        }
    }
}

struct LateClock {
    now: Timestamp,
    delay_nanos: u64,
    requested_deadlines: Vec<Timestamp>,
}

impl AudioClock for LateClock {
    fn now(&self) -> Timestamp {
        self.now
    }

    fn sleep_until(&mut self, deadline: Timestamp) {
        self.requested_deadlines.push(deadline);
        self.now = Timestamp::from_nanos(deadline.as_nanos().saturating_add(self.delay_nanos));
    }
}

fn format() -> AudioFormat {
    AudioFormat::new(48_000, 2).expect("valid audio format")
}

fn buffer(values: &[f32]) -> AudioBuffer {
    AudioBuffer::new(format(), Timestamp::ZERO, values.to_vec()).expect("valid buffer")
}

#[test]
fn validates_interleaved_buffers() {
    assert_eq!(AudioFormat::new(0, 2), Err(AudioError::InvalidFormat));
    assert_eq!(
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.0]),
        Err(AudioError::SamplesNotInterleaved {
            samples: 1,
            channels: 2
        })
    );
    assert_eq!(
        AudioBuffer::new(format(), Timestamp::ZERO, vec![f32::NAN, 0.0]),
        Err(AudioError::NonFiniteSample)
    );
}

#[test]
fn simulated_monitor_sink_validates_format_and_lifecycle() {
    let provider = SimulatedAudioProvider::new();
    let devices = provider.discover_outputs().expect("monitor catalog");
    assert_eq!(devices[0].kind(), AudioDeviceKind::Output);
    let mut output = provider
        .open_output(devices[0].id(), format())
        .expect("monitor output");
    assert_eq!(output.state(), AudioOutputState::Stopped);
    output
        .write_block(&buffer(&[0.1, -0.1]))
        .expect("write block");
    assert_eq!(output.state(), AudioOutputState::Running);
    let other_format = AudioFormat::new(44_100, 2).expect("other format");
    let other = AudioBuffer::silence(other_format, Timestamp::ZERO, 1).expect("other buffer");
    assert!(matches!(
        output.write_block(&other),
        Err(AudioDeviceError::Audio(AudioError::FormatMismatch { .. }))
    ));
    output.stop();
    assert_eq!(output.state(), AudioOutputState::Stopped);
}

#[test]
fn callback_clock_tracks_device_edges_and_bounded_correction() {
    let mut clock = AudioCallbackClock::new(format());
    let first = clock
        .observe_callback(Timestamp::from_millis(10), 480)
        .expect("first callback");
    assert_eq!(first.drift_nanos(), 0);
    clock
        .set_correction_ppm(1_000)
        .expect("correction is within bounds");
    let second = clock
        .observe_callback(Timestamp::from_nanos(20_010_000), 480)
        .expect("second callback");
    assert_eq!(
        second.expected_timestamp(),
        Timestamp::from_nanos(20_010_000)
    );
    assert_eq!(second.drift_nanos(), 0);
    assert_eq!(clock.delivered_frames(), 960);
    assert_eq!(clock.correction_ppm(), 1_000);
    assert_eq!(
        clock.set_correction_ppm(MAX_CALLBACK_CORRECTION_PPM + 1),
        Err(AudioError::CallbackCorrectionOutOfRange {
            ppm: MAX_CALLBACK_CORRECTION_PPM + 1
        })
    );
    assert_eq!(
        clock.observe_callback(Timestamp::from_millis(19), 480),
        Err(AudioError::CallbackTimestampRegression {
            previous: Timestamp::from_nanos(20_010_000),
            actual: Timestamp::from_millis(19)
        })
    );
}

#[test]
fn queue_bounds_complete_buffers() {
    let mut queue = AudioQueue::new(format(), 3, AudioDropPolicy::DropOldest).expect("valid queue");
    queue
        .push(buffer(&[0.1, 0.1, 0.2, 0.2]))
        .expect("first buffer");
    queue.push(buffer(&[0.3, 0.3])).expect("second buffer");
    assert_eq!(queue.queued_frames(), 3);
    assert_eq!(
        queue.push(buffer(&[0.4, 0.4])).expect("third buffer"),
        AudioPushOutcome::DroppedOldest { frames: 2 }
    );
    assert_eq!(
        queue.pop().expect("remaining buffer").sample(0, 0),
        Some(0.3)
    );
    assert_eq!(queue.queued_frames(), 1);
}

#[test]
fn mixer_applies_gain_mute_and_clamp() {
    let mut mixer = AudioMixer::new(format());
    let loud = mixer.add_source(2.0).expect("source");
    let muted = mixer.add_source(1.0).expect("source");
    mixer.set_muted(muted, true).expect("mute source");
    let loud_buffer = buffer(&[0.75, -0.75]);
    let muted_buffer = buffer(&[1.0, 1.0]);

    let output = mixer
        .mix(
            Timestamp::from_millis(10),
            1,
            &[(loud, &loud_buffer), (muted, &muted_buffer)],
        )
        .expect("mix succeeds");
    assert_eq!(output.timestamp(), Timestamp::from_millis(10));
    assert_eq!(output.samples(), &[1.0, -1.0]);
}

#[test]
fn mixer_applies_stereo_pan_and_rejects_invalid_values() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(1.0).expect("source");
    assert_eq!(mixer.set_pan(source, 2.0), Err(AudioError::InvalidPan));
    assert_eq!(mixer.set_pan(source, f32::NAN), Err(AudioError::InvalidPan));
    mixer.set_pan(source, 1.0).expect("right pan");

    let output = mixer
        .mix(Timestamp::ZERO, 1, &[(source, &buffer(&[0.75, 0.5]))])
        .expect("mix succeeds");
    assert_eq!(output.samples(), &[0.0, 0.5]);
}

#[test]
fn gain_filter_chain_matches_bounded_db_semantics() {
    let gain = AudioFilter::gain_db_milli(6_000).expect("valid gain");
    let mut chain = AudioFilterChain::new();
    chain.try_push(gain.clone()).expect("first filter");
    chain
        .try_push(AudioFilter::gain_db_milli(-6_000).expect("second gain"))
        .expect("second filter");
    assert_eq!(chain.len(), 2);
    assert_eq!(
        chain.filters(),
        &[gain, AudioFilter::gain_db_milli(-6_000).unwrap()]
    );

    let mut input = buffer(&[0.5, -0.5]);
    chain.apply(&mut input).expect("gain chain");
    assert!((input.samples()[0] - 0.5).abs() < 0.0001);
    assert!((input.samples()[1] + 0.5).abs() < 0.0001);
    assert_eq!(input.timestamp(), Timestamp::ZERO);
}

#[test]
fn gain_filter_rejects_unbounded_values_and_overflow_without_partial_write() {
    assert_eq!(
        AudioFilter::gain_db_milli(MIN_GAIN_DB_MILLI - 1),
        Err(AudioError::InvalidFilterGain {
            milli_db: MIN_GAIN_DB_MILLI - 1
        })
    );
    assert_eq!(
        AudioFilter::gain_db_milli(MAX_GAIN_DB_MILLI + 1),
        Err(AudioError::InvalidFilterGain {
            milli_db: MAX_GAIN_DB_MILLI + 1
        })
    );

    let mut input =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![f32::MAX, 0.25]).expect("finite input");
    let result =
        AudioFilter::Gain(AudioGain::new(MAX_GAIN_DB_MILLI).expect("max gain")).apply(&mut input);
    assert_eq!(result, Err(AudioError::FilterOverflow));
    assert_eq!(input.samples(), &[f32::MAX, 0.25]);
}

#[test]
fn invert_polarity_preserves_magnitude_and_composes_in_order() {
    let mut chain = AudioFilterChain::new();
    chain
        .try_push(AudioFilter::gain_db_milli(6_000).expect("gain"))
        .expect("gain filter");
    chain
        .try_push(AudioFilter::InvertPolarity)
        .expect("polarity filter");

    let mut input = buffer(&[0.5, -0.25]);
    chain.apply(&mut input).expect("audio chain");
    assert!((input.samples()[0] + 0.997_631).abs() < 0.0001);
    assert!((input.samples()[1] - 0.498_815).abs() < 0.0001);
}

#[test]
fn limiter_validates_obs_threshold_and_release_bounds() {
    assert_eq!(
        AudioLimiter::new(MIN_LIMITER_THRESHOLD_DB_MILLI - 1, 60),
        Err(AudioError::InvalidLimiterThreshold {
            milli_db: MIN_LIMITER_THRESHOLD_DB_MILLI - 1
        })
    );
    assert_eq!(
        AudioLimiter::new(MAX_LIMITER_THRESHOLD_DB_MILLI + 1, 60),
        Err(AudioError::InvalidLimiterThreshold {
            milli_db: MAX_LIMITER_THRESHOLD_DB_MILLI + 1
        })
    );
    assert_eq!(
        AudioLimiter::new(-6_000, MIN_LIMITER_RELEASE_MS - 1),
        Err(AudioError::InvalidLimiterRelease { milliseconds: 0 })
    );
    assert_eq!(
        AudioLimiter::new(-6_000, MAX_LIMITER_RELEASE_MS + 1),
        Err(AudioError::InvalidLimiterRelease {
            milliseconds: MAX_LIMITER_RELEASE_MS + 1
        })
    );
}

#[test]
fn limiter_applies_one_gain_to_each_channel_after_bounded_attack() {
    let limiter = AudioFilter::limiter_db_milli(-6_000, 60).expect("valid limiter");
    let mut chain = AudioFilterChain::new();
    chain.try_push(limiter).expect("limiter filter");
    let mut input =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0; 480 * 2]).expect("loud audio block");

    chain.apply(&mut input).expect("limiter block");

    assert!(
        input.samples()[0] > 0.9,
        "1 ms attack must not hard clip the first sample"
    );
    let final_left = input.samples()[(480 - 1) * 2];
    let final_right = input.samples()[(480 - 1) * 2 + 1];
    assert!(final_left < 0.9, "the sustained signal must be limited");
    assert!((final_left - final_right).abs() < f32::EPSILON);
}

#[test]
fn limiter_envelope_continues_across_audio_blocks_without_allocation() {
    let mut continuing = AudioFilterChain::new();
    continuing
        .try_push(AudioFilter::limiter_db_milli(-6_000, 60).expect("limiter"))
        .expect("limiter filter");
    let mut loud =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0; 480 * 2]).expect("loud audio block");
    continuing.apply(&mut loud).expect("loud block");

    let mut continued_impulse = buffer(&[1.0, 1.0]);
    continuing
        .apply(&mut continued_impulse)
        .expect("continued limiter state");

    let mut fresh = AudioFilterChain::new();
    fresh
        .try_push(AudioFilter::limiter_db_milli(-6_000, 60).expect("limiter"))
        .expect("limiter filter");
    let mut fresh_impulse = buffer(&[1.0, 1.0]);
    fresh
        .apply(&mut fresh_impulse)
        .expect("fresh limiter state");

    assert!(
        continued_impulse.samples()[0] < fresh_impulse.samples()[0],
        "the envelope must survive block boundaries"
    );
}

#[test]
fn limiter_release_setting_controls_envelope_decay_rate() {
    let mut short_release = AudioFilterChain::new();
    short_release
        .try_push(AudioFilter::limiter_db_milli(-6_000, 1).expect("short limiter"))
        .expect("short limiter filter");
    let mut long_release = AudioFilterChain::new();
    long_release
        .try_push(AudioFilter::limiter_db_milli(-6_000, 1_000).expect("long limiter"))
        .expect("long limiter filter");

    for chain in [&mut short_release, &mut long_release] {
        let mut loud =
            AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0; 480 * 2]).expect("loud block");
        chain.apply(&mut loud).expect("loud limiter block");
        let mut silence = AudioBuffer::silence(format(), Timestamp::ZERO, 48).expect("silence");
        chain.apply(&mut silence).expect("release block");
    }

    let mut short_impulse = buffer(&[1.0, 1.0]);
    short_release
        .apply(&mut short_impulse)
        .expect("short release impulse");
    let mut long_impulse = buffer(&[1.0, 1.0]);
    long_release
        .apply(&mut long_impulse)
        .expect("long release impulse");

    assert!(
        long_impulse.samples()[0] < short_impulse.samples()[0],
        "a longer release must retain more gain reduction"
    );
}

#[test]
fn compressor_validates_obs_control_bounds() {
    assert_eq!(
        AudioCompressor::new(MIN_COMPRESSOR_RATIO_MILLI - 1, -18_000, 6, 60, 0),
        Err(AudioError::InvalidCompressorRatio { milli_ratio: 999 })
    );
    assert_eq!(
        AudioCompressor::new(MAX_COMPRESSOR_RATIO_MILLI + 1, -18_000, 6, 60, 0),
        Err(AudioError::InvalidCompressorRatio {
            milli_ratio: MAX_COMPRESSOR_RATIO_MILLI + 1
        })
    );
    assert_eq!(
        AudioCompressor::new(10_000, MIN_COMPRESSOR_THRESHOLD_DB_MILLI - 1, 6, 60, 0),
        Err(AudioError::InvalidCompressorThreshold {
            milli_db: MIN_COMPRESSOR_THRESHOLD_DB_MILLI - 1
        })
    );
    assert_eq!(
        AudioCompressor::new(10_000, -18_000, MIN_COMPRESSOR_ATTACK_MS - 1, 60, 0),
        Err(AudioError::InvalidCompressorAttack { milliseconds: 0 })
    );
    assert_eq!(
        AudioCompressor::new(10_000, -18_000, 6, MIN_COMPRESSOR_RELEASE_MS - 1, 0),
        Err(AudioError::InvalidCompressorRelease { milliseconds: 0 })
    );
    assert_eq!(
        AudioCompressor::new(
            10_000,
            -18_000,
            6,
            60,
            MAX_COMPRESSOR_OUTPUT_GAIN_DB_MILLI + 1
        ),
        Err(AudioError::InvalidCompressorOutputGain {
            milli_db: MAX_COMPRESSOR_OUTPUT_GAIN_DB_MILLI + 1
        })
    );
}

#[test]
fn compressor_tracks_interleaved_peak_with_attack_and_ratio() {
    let mut chain = AudioFilterChain::new();
    chain
        .try_push(AudioFilter::compressor(10_000, -18_000, 1, 60, 0).expect("compressor"))
        .expect("compressor filter");
    let mut input =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0; 480 * 2]).expect("loud block");

    chain.apply(&mut input).expect("compressor block");

    assert!(
        input.samples()[0] > 0.99,
        "attack must not compress the first sample"
    );
    let final_left = input.samples()[(480 - 1) * 2];
    let final_right = input.samples()[(480 - 1) * 2 + 1];
    assert!(final_left < 0.9, "the sustained signal must be compressed");
    assert!((final_left - final_right).abs() < f32::EPSILON);
}

#[test]
fn compressor_state_continues_across_blocks_and_preserves_overflow_atomicity() {
    let mut continuing = AudioFilterChain::new();
    continuing
        .try_push(AudioFilter::compressor(10_000, -18_000, 1, 60, 0).expect("compressor"))
        .expect("compressor filter");
    let mut loud =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0; 480 * 2]).expect("loud block");
    continuing.apply(&mut loud).expect("loud compressor block");
    let mut continued_impulse = buffer(&[1.0, 1.0]);
    continuing
        .apply(&mut continued_impulse)
        .expect("continued compressor state");

    let mut fresh = AudioFilterChain::new();
    fresh
        .try_push(AudioFilter::compressor(10_000, -18_000, 1, 60, 0).expect("compressor"))
        .expect("compressor filter");
    let mut fresh_impulse = buffer(&[1.0, 1.0]);
    fresh
        .apply(&mut fresh_impulse)
        .expect("fresh compressor state");
    assert!(continued_impulse.samples()[0] < fresh_impulse.samples()[0]);

    let mut overflow =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![f32::MAX, 0.25]).expect("finite input");
    let result = AudioFilter::compressor(1_000, 0, 1, 60, 32_000)
        .expect("valid compressor")
        .apply(&mut overflow);
    assert_eq!(result, Err(AudioError::FilterOverflow));
    assert_eq!(overflow.samples(), &[f32::MAX, 0.25]);
}

#[test]
fn expander_validates_obs_control_bounds() {
    assert_eq!(
        AudioExpander::new(MIN_EXPANDER_RATIO_MILLI - 1, -40_000, 10, 50, 0),
        Err(AudioError::InvalidExpanderRatio { milli_ratio: 999 })
    );
    assert_eq!(
        AudioExpander::new(MAX_EXPANDER_RATIO_MILLI + 1, -40_000, 10, 50, 0),
        Err(AudioError::InvalidExpanderRatio {
            milli_ratio: MAX_EXPANDER_RATIO_MILLI + 1
        })
    );
    assert_eq!(
        AudioExpander::new(10_000, MIN_EXPANDER_THRESHOLD_DB_MILLI - 1, 10, 50, 0),
        Err(AudioError::InvalidExpanderThreshold {
            milli_db: MIN_EXPANDER_THRESHOLD_DB_MILLI - 1
        })
    );
    assert_eq!(
        AudioExpander::new(10_000, -40_000, MIN_EXPANDER_ATTACK_MS - 1, 50, 0),
        Err(AudioError::InvalidExpanderAttack { milliseconds: 0 })
    );
    assert_eq!(
        AudioExpander::new(10_000, -40_000, 10, MIN_EXPANDER_RELEASE_MS - 1, 0),
        Err(AudioError::InvalidExpanderRelease { milliseconds: 0 })
    );
    assert_eq!(
        AudioExpander::new(
            10_000,
            -40_000,
            10,
            50,
            MAX_EXPANDER_OUTPUT_GAIN_DB_MILLI + 1
        ),
        Err(AudioError::InvalidExpanderOutputGain {
            milli_db: MAX_EXPANDER_OUTPUT_GAIN_DB_MILLI + 1
        })
    );
}

#[test]
fn expander_attenuates_below_threshold_and_preserves_above_threshold_signal() {
    let mut quiet_chain = AudioFilterChain::new();
    quiet_chain
        .try_push(AudioFilter::expander(10_000, -18_000, 1, 60, 0).expect("expander"))
        .expect("expander filter");
    let mut quiet =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.01; 480 * 2]).expect("quiet block");
    quiet_chain.apply(&mut quiet).expect("quiet expander block");
    let final_quiet = quiet.samples()[(480 - 1) * 2];
    assert!(
        final_quiet < 0.005,
        "the quiet signal must be expanded downward"
    );

    let mut loud_chain = AudioFilterChain::new();
    loud_chain
        .try_push(AudioFilter::expander(10_000, -18_000, 1, 60, 0).expect("expander"))
        .expect("expander filter");
    let mut loud =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0; 480 * 2]).expect("loud block");
    loud_chain.apply(&mut loud).expect("loud expander block");
    assert!((loud.samples()[0] - 1.0).abs() < f32::EPSILON);
    assert!((loud.samples()[(480 - 1) * 2] - 1.0).abs() < f32::EPSILON);
}

#[test]
fn expander_state_continues_and_overflow_is_atomic() {
    let mut continuing = AudioFilterChain::new();
    continuing
        .try_push(AudioFilter::expander(10_000, -18_000, 1, 60, 0).expect("expander"))
        .expect("expander filter");
    let mut quiet =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.01; 480 * 2]).expect("quiet block");
    continuing.apply(&mut quiet).expect("quiet expander block");
    let mut continued_impulse = buffer(&[1.0, 1.0]);
    continuing
        .apply(&mut continued_impulse)
        .expect("continued expander state");

    let mut fresh = AudioFilterChain::new();
    fresh
        .try_push(AudioFilter::expander(10_000, -18_000, 1, 60, 0).expect("expander"))
        .expect("expander filter");
    let mut fresh_impulse = buffer(&[1.0, 1.0]);
    fresh
        .apply(&mut fresh_impulse)
        .expect("fresh expander state");
    assert!(continued_impulse.samples()[0] < fresh_impulse.samples()[0]);

    let mut overflow =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![f32::MAX, 0.25]).expect("finite input");
    let result = AudioFilter::expander(1_000, 0, 1, 60, 32_000)
        .expect("valid expander")
        .apply(&mut overflow);
    assert_eq!(result, Err(AudioError::FilterOverflow));
    assert_eq!(overflow.samples(), &[f32::MAX, 0.25]);
}

#[test]
fn noise_gate_matches_obs_peak_threshold_and_timing_model() {
    assert_eq!(
        AudioFilter::noise_gate(-97_000, -32_000, 25, 200, 150),
        Err(AudioError::InvalidNoiseGateOpenThreshold { milli_db: -97_000 })
    );
    assert_eq!(
        AudioFilter::noise_gate(-26_000, 1, 25, 200, 150),
        Err(AudioError::InvalidNoiseGateCloseThreshold { milli_db: 1 })
    );
    assert_eq!(
        AudioFilter::noise_gate(-26_000, -20_000, 25, 200, 150),
        Err(AudioError::InvalidNoiseGateThresholdOrder {
            open_milli_db: -26_000,
            close_milli_db: -20_000,
        })
    );
    assert_eq!(
        AudioFilter::noise_gate(-26_000, -32_000, 0, 200, 150),
        Err(AudioError::InvalidNoiseGateAttack { milliseconds: 0 })
    );
    assert_eq!(
        AudioFilter::noise_gate(-26_000, -32_000, 25, 200, 0),
        Err(AudioError::InvalidNoiseGateRelease { milliseconds: 0 })
    );

    let mut gate = AudioFilterChain::new();
    gate.try_push(AudioFilter::noise_gate(-26_000, -32_000, 25, 200, 150).expect("gate"))
        .expect("gate filter");

    let mut quiet_block =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.01; 480 * 2]).expect("quiet block");
    gate.apply(&mut quiet_block).expect("quiet gate block");
    assert!(quiet_block
        .samples()
        .iter()
        .all(|sample| sample.abs() < f32::EPSILON));

    let mut loud_block =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0; 480 * 2]).expect("loud block");
    gate.apply(&mut loud_block).expect("loud gate block");
    assert!(loud_block.samples()[0].abs() < f32::EPSILON);
    assert!(loud_block.samples()[2] > 0.0);
    assert!(loud_block.samples()[2] < loud_block.samples()[4]);
    assert!(loud_block.samples().last().copied().unwrap_or(1.0) < 1.0);
}

#[test]
fn noise_gate_honors_hold_and_release_after_level_decay() {
    let mut gate = AudioFilterChain::new();
    gate.try_push(AudioFilter::noise_gate(-6_000, -12_000, 1, 2, 4).expect("gate"))
        .expect("gate filter");

    let mut loud =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0; 480 * 2]).expect("loud block");
    gate.apply(&mut loud).expect("loud gate block");

    let mut quiet =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.1; 2_400 * 2]).expect("quiet block");
    gate.apply(&mut quiet).expect("quiet gate block");

    let before_release = quiet.samples()[1_900 * 2];
    let during_release = quiet.samples()[2_050 * 2];
    let after_release = quiet.samples()[2_250 * 2];
    assert!(before_release > during_release);
    assert!(during_release > after_release);
    assert!(after_release.abs() < f32::EPSILON);
}

#[test]
fn audio_filter_chain_has_a_fixed_capacity() {
    let mut chain = AudioFilterChain::new();
    let filter = AudioFilter::gain_db_milli(0).expect("zero gain");
    for _ in 0..MAX_AUDIO_FILTERS {
        chain.try_push(filter.clone()).expect("capacity slot");
    }
    assert_eq!(
        chain.try_push(filter),
        Err(AudioError::FilterChainFull {
            max: MAX_AUDIO_FILTERS
        })
    );
}

#[test]
fn gain_filter_block_timing_report() {
    let mut chain = AudioFilterChain::new();
    chain
        .try_push(AudioFilter::gain_db_milli(6_000).expect("gain"))
        .expect("filter");
    let mut block =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.1; 480 * 2]).expect("audio block");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for _ in 0..200 {
        block.samples_mut().fill(0.1);
        chain.apply(&mut block).expect("gain block");
        checksum += block.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "gain filter: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
fn invert_polarity_block_timing_report() {
    let mut chain = AudioFilterChain::new();
    chain
        .try_push(AudioFilter::InvertPolarity)
        .expect("polarity filter");
    let mut block =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.1; 480 * 2]).expect("audio block");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for _ in 0..200 {
        block.samples_mut().fill(0.1);
        chain.apply(&mut block).expect("polarity block");
        checksum += block.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "invert polarity: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
fn limiter_block_timing_report() {
    let mut chain = AudioFilterChain::new();
    chain
        .try_push(AudioFilter::limiter_db_milli(-6_000, 60).expect("limiter"))
        .expect("filter");
    let mut block =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.1; 480 * 2]).expect("audio block");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for _ in 0..200 {
        block.samples_mut().fill(0.1);
        chain.apply(&mut block).expect("limiter block");
        checksum += block.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "limiter: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
fn compressor_block_timing_report() {
    let mut chain = AudioFilterChain::new();
    chain
        .try_push(AudioFilter::compressor(10_000, -18_000, 6, 60, 0).expect("compressor"))
        .expect("filter");
    let mut block =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.1; 480 * 2]).expect("audio block");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for _ in 0..200 {
        block.samples_mut().fill(0.1);
        chain.apply(&mut block).expect("compressor block");
        checksum += block.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "compressor: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
fn expander_block_timing_report() {
    let mut chain = AudioFilterChain::new();
    chain
        .try_push(AudioFilter::expander(2_000, -40_000, 10, 50, 0).expect("expander"))
        .expect("filter");
    let mut block =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.01; 480 * 2]).expect("audio block");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for _ in 0..200 {
        block.samples_mut().fill(0.01);
        chain.apply(&mut block).expect("expander block");
        checksum += block.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "expander: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
fn noise_gate_block_timing_report() {
    let mut chain = AudioFilterChain::new();
    chain
        .try_push(AudioFilter::noise_gate(-26_000, -32_000, 25, 200, 150).expect("noise gate"))
        .expect("filter");
    let mut block =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.01; 480 * 2]).expect("audio block");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for _ in 0..200 {
        block.samples_mut().fill(0.01);
        chain.apply(&mut block).expect("noise gate block");
        checksum += block.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "noise gate: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
fn mixer_monitor_taps_are_bounded_and_post_mix() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(1.0).expect("source");
    let tap = mixer.add_monitor_tap(1).expect("tap");
    let first = AudioBuffer::new(format(), Timestamp::ZERO, vec![0.25, 0.5]).expect("first input");
    let second = AudioBuffer::new(format(), Timestamp::from_millis(1), vec![0.75, 1.0])
        .expect("second input");

    mixer
        .mix(Timestamp::ZERO, 1, &[(source, &first)])
        .expect("first mix");
    mixer
        .mix(Timestamp::from_millis(1), 1, &[(source, &second)])
        .expect("second mix");

    assert_eq!(mixer.monitor_dropped_buffers(tap), Ok(1));
    let monitored = mixer
        .pop_monitor_buffer(tap)
        .expect("tap exists")
        .expect("latest buffer");
    assert_eq!(monitored.timestamp(), Timestamp::from_millis(1));
    assert_eq!(monitored.samples(), &[0.75, 1.0]);
    assert_eq!(mixer.pop_monitor_buffer(tap), Ok(None));
    assert_eq!(
        AudioMonitorTap::new(0).map(|_| ()),
        Err(AudioError::ZeroMonitorCapacity)
    );
}

#[test]
fn mixer_rejects_duplicate_and_unknown_inputs() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(1.0).expect("source");
    let input = buffer(&[0.1, 0.2]);

    assert_eq!(
        mixer.mix(Timestamp::ZERO, 1, &[(source, &input), (source, &input)]),
        Err(AudioError::DuplicateInput(source))
    );
    assert_eq!(
        mixer.mix(Timestamp::ZERO, 1, &[(AudioSourceId(99), &input)]),
        Err(AudioError::UnknownSource(AudioSourceId(99)))
    );
}

#[test]
fn resampler_changes_rate_and_preserves_channel_layout() {
    let input_format = AudioFormat::new(48_000, 1).expect("input format");
    let output_format = AudioFormat::new(24_000, 1).expect("output format");
    let input = AudioBuffer::new(
        input_format,
        Timestamp::from_millis(2),
        vec![0.0, 1.0, 0.0, -1.0],
    )
    .expect("input buffer");
    let resampler = AudioResampler::new(input_format, output_format).expect("resampler");
    let output = resampler.process(&input).expect("resample succeeds");

    assert_eq!(output.format(), output_format);
    assert_eq!(output.frames(), 2);
    assert_eq!(output.timestamp(), Timestamp::from_millis(2));
    assert_eq!(output.samples(), &[0.0, 0.0]);
}

#[test]
fn resampler_maps_mono_to_stereo() {
    let input = AudioFormat::new(48_000, 1).expect("input format");
    let output = AudioFormat::new(48_000, 2).expect("output format");
    let buffer = AudioBuffer::new(input, Timestamp::ZERO, vec![0.25, -0.5]).expect("buffer");
    let converted = AudioResampler::new(input, output)
        .expect("converter")
        .process(&buffer)
        .expect("converted buffer");
    assert_eq!(converted.samples(), &[0.25, 0.25, -0.5, -0.5]);
}

#[test]
fn scheduler_and_buffer_end_use_sample_clock_timestamps() {
    let mono = AudioFormat::new(48_000, 1).expect("mono format");
    let mut scheduler = AudioScheduler::new(mono);
    assert_eq!(
        scheduler.next_deadline().expect("first deadline"),
        AudioDeadline {
            index: 0,
            timestamp: Timestamp::ZERO
        }
    );
    assert_eq!(
        scheduler
            .next_deadline()
            .expect("second deadline")
            .timestamp(),
        Timestamp::from_nanos(20_833)
    );
    assert_eq!(
        scheduler
            .next_deadline()
            .expect("third deadline")
            .timestamp(),
        Timestamp::from_nanos(41_666)
    );

    let buffer = AudioBuffer::silence(mono, Timestamp::from_millis(10), 48_000)
        .expect("one second of silence");
    assert_eq!(buffer.duration_nanos(), Some(1_000_000_000));
    assert_eq!(buffer.end_timestamp(), Some(Timestamp::from_millis(1_010)));
}

#[test]
fn audio_pacer_advances_by_blocks_with_an_injected_clock() {
    let mut clock = FakeClock {
        now: Timestamp::from_millis(5),
        requested_deadlines: Vec::new(),
    };
    let mut pacer = AudioPacer::new(format());
    assert_eq!(pacer.next(&mut clock, 0), Err(AudioError::ZeroBlock));

    let first = pacer.next(&mut clock, 480).expect("first block");
    assert_eq!(first.deadline().index(), 0);
    assert_eq!(first.frames(), 480);
    assert!(first.missed());
    assert_eq!(first.waited_nanos(), 0);

    let second = pacer.next(&mut clock, 480).expect("second block");
    assert_eq!(second.deadline().index(), 480);
    assert_eq!(second.deadline().timestamp(), Timestamp::from_millis(10));
    assert_eq!(second.observed_at(), Timestamp::from_millis(10));
    assert_eq!(second.waited_nanos(), Timestamp::from_millis(5).as_nanos());
    assert_eq!(
        clock.requested_deadlines,
        vec![Timestamp::ZERO, Timestamp::from_millis(10)]
    );
}

#[test]
fn audio_worker_reports_underflow_drop_pressure_and_lateness() {
    let mut clock = LateClock {
        now: Timestamp::ZERO,
        delay_nanos: 100,
        requested_deadlines: Vec::new(),
    };
    let token = AudioCancellationToken::new();
    let mut worker = AudioWorker::new(format(), 4, AudioDropPolicy::DropNewest).expect("worker");
    let report = worker
        .run(
            &mut clock,
            2,
            4,
            &token,
            |deadline, output_format, frames| {
                if deadline.index() == 2 {
                    return Ok::<_, std::convert::Infallible>(None);
                }
                Ok(Some(
                    AudioBuffer::silence(output_format, deadline.timestamp(), frames)
                        .expect("valid block"),
                ))
            },
        )
        .expect("worker run");

    assert_eq!(report.requested_blocks(), 4);
    assert_eq!(report.processed_blocks(), 4);
    assert!(!report.cancelled());
    assert_eq!(report.underflow_blocks(), 1);
    assert_eq!(report.produced_frames(), 6);
    assert_eq!(report.dropped_oldest_frames(), 0);
    assert_eq!(report.dropped_newest_frames(), 2);
    assert_eq!(report.missed_deadlines(), 4);
    assert_eq!(report.total_lateness_nanos(), 400);
    assert_eq!(report.remaining_queue_frames(), 4);
    assert_eq!(clock.requested_deadlines.len(), 4);

    assert_eq!(worker.take_next().expect("first block").frames(), 2);
    assert_eq!(worker.take_next().expect("second block").frames(), 2);
    assert_eq!(worker.take_next(), None);
}

#[test]
fn audio_worker_cancels_between_blocks_and_shares_token_state() {
    let mut clock = FakeClock {
        now: Timestamp::ZERO,
        requested_deadlines: Vec::new(),
    };
    let token = AudioCancellationToken::new();
    let callback_token = token.clone();
    let mut worker = AudioWorker::new(format(), 8, AudioDropPolicy::DropOldest).expect("worker");
    let report = worker
        .run(
            &mut clock,
            2,
            10,
            &token,
            |deadline, output_format, frames| {
                if deadline.index() == 2 {
                    callback_token.cancel();
                }
                Ok::<_, std::convert::Infallible>(Some(
                    AudioBuffer::silence(output_format, deadline.timestamp(), frames)
                        .expect("valid block"),
                ))
            },
        )
        .expect("worker run");

    assert_eq!(report.processed_blocks(), 2);
    assert!(report.cancelled());
    assert!(token.is_cancelled());
    assert_eq!(report.remaining_queue_frames(), 4);
    token.reset();
    assert!(!token.is_cancelled());
    worker.reset();
    assert_eq!(worker.queued_frames(), 0);
}

#[test]
fn audio_worker_rejects_wrong_timestamp_before_queueing() {
    let mut clock = FakeClock {
        now: Timestamp::ZERO,
        requested_deadlines: Vec::new(),
    };
    let token = AudioCancellationToken::new();
    let mut worker = AudioWorker::new(format(), 8, AudioDropPolicy::DropOldest).expect("worker");
    let result = worker.run(&mut clock, 2, 1, &token, |_, output_format, frames| {
        Ok::<_, std::convert::Infallible>(Some(
            AudioBuffer::silence(output_format, Timestamp::from_nanos(1), frames)
                .expect("valid block"),
        ))
    });

    assert_eq!(
        result,
        Err(AudioWorkerError::Submit(
            AudioError::BufferTimestampMismatch {
                expected: Timestamp::ZERO,
                actual: Timestamp::from_nanos(1),
            }
        ))
    );
    assert_eq!(worker.queued_frames(), 0);
}

#[test]
fn av_sync_reports_signed_drift_and_safe_actions() {
    let controller = AvSyncController::new(5_000_000);
    let aligned = controller.observe(Timestamp::from_millis(10), Timestamp::from_millis(12));
    assert_eq!(aligned.state(), SyncState::InSync);
    assert_eq!(aligned.action(), SyncAction::Keep);
    assert_eq!(aligned.delta_nanos(), 2_000_000);

    let audio_behind = controller.observe(Timestamp::from_millis(20), Timestamp::from_millis(1));
    assert_eq!(audio_behind.state(), SyncState::AudioBehind);
    assert_eq!(audio_behind.action(), SyncAction::DropEarlyAudio);
    assert_eq!(audio_behind.delta_nanos(), -19_000_000);

    let audio_ahead = controller.observe(Timestamp::from_millis(1), Timestamp::from_millis(20));
    assert_eq!(audio_ahead.state(), SyncState::AudioAhead);
    assert_eq!(audio_ahead.action(), SyncAction::WaitForAudio);
    assert_eq!(audio_ahead.delta_nanos(), 19_000_000);
}

#[test]
fn av_sync_reconciles_early_late_and_obsolete_audio() {
    let controller = AvSyncController::new(1_000);
    let early = AudioBuffer::silence(format(), Timestamp::ZERO, 100).expect("early buffer");
    let trimmed = controller
        .reconcile(Timestamp::from_millis(1), &early)
        .expect("trim succeeds")
        .expect("some audio remains");
    assert_eq!(trimmed.timestamp(), Timestamp::from_millis(1));
    assert_eq!(trimmed.frames(), 52);

    let late = AudioBuffer::silence(format(), Timestamp::from_millis(10), 2).expect("late buffer");
    let prefixed = controller
        .reconcile(Timestamp::ZERO, &late)
        .expect("prefix succeeds")
        .expect("late audio remains");
    assert_eq!(prefixed.timestamp(), Timestamp::ZERO);
    assert_eq!(prefixed.frames(), 482);
    assert_eq!(prefixed.sample(0, 0), Some(0.0));
    assert_eq!(prefixed.sample(480, 0), Some(0.0));

    let obsolete = AudioBuffer::silence(format(), Timestamp::ZERO, 2).expect("buffer");
    assert_eq!(
        controller
            .reconcile(Timestamp::from_millis(100), &obsolete)
            .expect("drop succeeds"),
        None
    );
}

#[test]
fn av_sync_monitor_accumulates_long_run_diagnostics() {
    let mut monitor = AvSyncMonitor::new(100);
    for index in 1..=10_000_u64 {
        let video = Timestamp::from_nanos(index * 1_000_000);
        let audio = match index % 3 {
            0 => Timestamp::from_nanos(video.as_nanos() - 200),
            1 => Timestamp::from_nanos(video.as_nanos() + 50),
            _ => Timestamp::from_nanos(video.as_nanos() + 1_000),
        };
        let _ = monitor.observe(video, audio);
    }

    let metrics = monitor.metrics();
    assert_eq!(metrics.observations(), 10_000);
    assert_eq!(metrics.in_sync(), 3_334);
    assert_eq!(metrics.audio_behind(), 3_333);
    assert_eq!(metrics.audio_ahead(), 3_333);
    assert_eq!(metrics.max_abs_delta_nanos(), 1_000);
    assert!(metrics.total_abs_delta_nanos() > metrics.max_abs_delta_nanos());
    monitor.reset();
    assert_eq!(monitor.metrics(), AvSyncMetrics::default());
}
