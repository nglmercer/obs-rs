use super::*;

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
    assert_eq!(mixer.source_peak_milli(loud), Ok(1_000));
    assert_eq!(mixer.source_peak_hold_milli(loud), Ok(1_000));
    assert_eq!(mixer.source_clipped(loud), Ok(true));
    assert_eq!(mixer.source_peak_milli(muted), Ok(0));
    assert_eq!(mixer.source_clipped(muted), Ok(false));
}

#[test]
fn mixer_rejects_unbounded_fixed_point_gain_controls() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(1.0).expect("source");
    assert_eq!(
        mixer.set_gain_milli(source, MAX_GAIN_MILLI + 1),
        Err(AudioError::InvalidGain)
    );
    mixer
        .set_gain_milli(source, MAX_GAIN_MILLI)
        .expect("maximum gain");
}

#[test]
fn mixer_applies_stereo_pan_and_rejects_invalid_values() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(1.0).expect("source");
    assert_eq!(mixer.set_pan(source, 2.0), Err(AudioError::InvalidPan));
    assert_eq!(mixer.set_pan(source, f32::NAN), Err(AudioError::InvalidPan));
    assert_eq!(
        mixer.set_pan_milli(source, MAX_PAN_MILLI + 1),
        Err(AudioError::InvalidPan)
    );
    mixer
        .set_pan_milli(source, MAX_PAN_MILLI)
        .expect("right pan");

    let output = mixer
        .mix(Timestamp::ZERO, 1, &[(source, &buffer(&[0.75, 0.5]))])
        .expect("mix succeeds");
    assert_eq!(output.samples(), &[0.0, 0.5]);
}

#[test]
fn mixer_format_reconfiguration_preserves_controls_and_resets_meter_state() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(1.0).expect("source");
    mixer.set_gain_milli(source, 1_500).expect("gain");
    mixer.set_pan_milli(source, -500).expect("pan");
    mixer
        .set_monitor_mode(source, AudioMonitorMode::MonitorAndOutput)
        .expect("monitor mode");
    let loud = AudioBuffer::new(format(), Timestamp::ZERO, vec![1.0, 1.0]).expect("loud buffer");
    mixer
        .mix(Timestamp::ZERO, 1, &[(source, &loud)])
        .expect("meter mix");
    assert_eq!(mixer.source_peak_milli(source).expect("peak"), 1_000);
    mixer.set_muted(source, true).expect("mute");

    let next_format = AudioFormat::new(44_100, 1).expect("next format");
    mixer.set_format(next_format);

    assert_eq!(mixer.format(), next_format);
    assert_eq!(mixer.source_count(), 1);
    assert_eq!(mixer.source_peak_milli(source).expect("reset peak"), 0);
    assert_eq!(mixer.source_peak_hold_milli(source).expect("reset hold"), 0);
    assert!(!mixer.source_clipped(source).expect("reset clip"));
    assert_eq!(
        mixer.source_monitor_mode(source),
        Ok(AudioMonitorMode::MonitorAndOutput)
    );
    let input = AudioBuffer::new(next_format, Timestamp::ZERO, vec![0.25]).expect("mono input");
    let output = mixer
        .mix(Timestamp::ZERO, 1, &[(source, &input)])
        .expect("reconfigured mix");
    assert_eq!(output.format(), next_format);
    assert_eq!(output.samples(), &[0.0]);
}

#[test]
fn mixer_block_timing_report() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(1.0).expect("source");
    let input =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.1; 480 * 2]).expect("audio input");
    let mut output = AudioBuffer::silence(format(), Timestamp::ZERO, 480).expect("audio output");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for block in 0_u64..200 {
        mixer
            .mix_into(
                Timestamp::from_millis(block * 10),
                &mut output,
                &[(source, &input)],
            )
            .expect("mix block");
        checksum += output.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "mixer: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
fn mixer_buses_block_timing_report() {
    let mut mixer = AudioMixer::new(format());
    let output_source = mixer.add_source(1.0).expect("output source");
    let monitor_source = mixer.add_source(1.0).expect("monitor source");
    mixer
        .set_monitor_mode(monitor_source, AudioMonitorMode::MonitorAndOutput)
        .expect("both-bus source");
    let input =
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.01; 480 * 2]).expect("audio input");
    let mut output = AudioBuffer::silence(format(), Timestamp::ZERO, 480).expect("output");
    let mut monitor = AudioBuffer::silence(format(), Timestamp::ZERO, 480).expect("monitor");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for block in 0_u64..200 {
        mixer
            .mix_buses_into(
                Timestamp::from_millis(block * 10),
                &mut output,
                &mut monitor,
                &[(output_source, &input), (monitor_source, &input)],
            )
            .expect("mix buses");
        checksum += output.samples()[0] + monitor.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "mixer buses: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
fn mixer_meter_hold_and_clip_indicators_expire_on_bounded_timers() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(2.0).expect("source");
    let loud = buffer(&[0.75, 0.75]);
    let quiet = buffer(&[0.1, 0.1]);

    mixer
        .mix(Timestamp::ZERO, 1, &[(source, &loud)])
        .expect("loud mix");
    assert_eq!(mixer.source_peak_hold_milli(source), Ok(1_000));
    assert_eq!(mixer.source_clipped(source), Ok(true));

    mixer
        .mix(Timestamp::from_millis(500), 1, &[(source, &quiet)])
        .expect("quiet mix");
    assert_eq!(mixer.source_peak_milli(source), Ok(200));
    assert_eq!(mixer.source_peak_hold_milli(source), Ok(1_000));
    assert_eq!(mixer.source_clipped(source), Ok(true));

    mixer
        .mix(Timestamp::from_millis(1_500), 1, &[(source, &quiet)])
        .expect("clip timeout mix");
    assert_eq!(mixer.source_clipped(source), Ok(false));
    assert_eq!(mixer.source_peak_hold_milli(source), Ok(1_000));

    mixer
        .mix(
            Timestamp::from_nanos(20_000_000_001),
            1,
            &[(source, &quiet)],
        )
        .expect("hold timeout mix");
    assert_eq!(mixer.source_peak_hold_milli(source), Ok(200));
}
