use super::*;

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
