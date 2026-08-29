use super::*;

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
fn mixer_monitor_modes_route_separate_buses() {
    let mut mixer = AudioMixer::new(format());
    let output_only = mixer.add_source(1.0).expect("output source");
    let monitor_only = mixer.add_source(1.0).expect("monitor source");
    let both = mixer.add_source(1.0).expect("both source");
    mixer
        .set_monitor_mode(monitor_only, AudioMonitorMode::MonitorOnly)
        .expect("monitor-only mode");
    mixer
        .set_monitor_mode(both, AudioMonitorMode::MonitorAndOutput)
        .expect("both mode");

    let input = buffer(&[0.1, 0.2]);
    let (output, monitor) = mixer
        .mix_buses(
            Timestamp::ZERO,
            1,
            &[
                (output_only, &input),
                (monitor_only, &input),
                (both, &input),
            ],
        )
        .expect("bus mix");

    assert_eq!(output.samples(), &[0.2, 0.4]);
    assert_eq!(monitor.samples(), &[0.2, 0.4]);
    assert_eq!(
        mixer.source_monitor_mode(output_only),
        Ok(AudioMonitorMode::Off)
    );
    assert_eq!(
        mixer.source_monitor_mode(AudioSourceId(99)),
        Err(AudioError::UnknownSource(AudioSourceId(99)))
    );
}

#[test]
fn monitor_only_mode_is_applied_to_the_legacy_output_mix() {
    let mut mixer = AudioMixer::new(format());
    let source = mixer.add_source(1.0).expect("source");
    mixer
        .set_monitor_mode(source, AudioMonitorMode::MonitorOnly)
        .expect("monitor-only mode");

    let output = mixer
        .mix(Timestamp::ZERO, 1, &[(source, &buffer(&[0.4, -0.4]))])
        .expect("output mix");
    assert_eq!(output.samples(), &[0.0, 0.0]);
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
fn resampler_maps_standard_layouts_by_speaker_role() {
    let input =
        AudioFormat::with_layout(48_000, AudioChannelLayout::FivePointOne).expect("5.1 input");
    let quad = AudioFormat::with_layout(48_000, AudioChannelLayout::Quad).expect("quad output");
    let buffer = AudioBuffer::new(
        input,
        Timestamp::ZERO,
        vec![1.0, -1.0, 0.5, 0.25, 0.2, -0.2],
    )
    .expect("5.1 buffer");
    let converted = AudioResampler::new(input, quad)
        .expect("quad converter")
        .process(&buffer)
        .expect("quad conversion");
    assert_eq!(converted.samples(), &[1.0, -1.0, 0.2, -0.2]);

    let stereo =
        AudioFormat::with_layout(48_000, AudioChannelLayout::Stereo).expect("stereo input");
    let surround =
        AudioFormat::with_layout(48_000, AudioChannelLayout::SevenPointOne).expect("7.1 output");
    let stereo_buffer =
        AudioBuffer::new(stereo, Timestamp::ZERO, vec![0.4, -0.2]).expect("stereo buffer");
    let expanded = AudioResampler::new(stereo, surround)
        .expect("surround converter")
        .process(&stereo_buffer)
        .expect("surround conversion");
    assert_eq!(
        expanded.samples(),
        &[0.4, -0.2, 0.1, 0.0, 0.4, -0.2, 0.4, -0.2]
    );
}

#[test]
fn resampler_block_timing_report() {
    let input_format =
        AudioFormat::with_layout(48_000, AudioChannelLayout::FivePointOne).expect("5.1 input");
    let output_format =
        AudioFormat::with_layout(48_000, AudioChannelLayout::Stereo).expect("stereo output");
    let input = AudioBuffer::new(
        input_format,
        Timestamp::ZERO,
        (0..(480 * usize::from(input_format.channels())))
            .map(|sample| f32::from(u16::try_from(sample % 17).expect("bounded sample")) / 17.0)
            .collect(),
    )
    .expect("input buffer");
    let resampler = AudioResampler::new(input_format, output_format).expect("resampler");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for _ in 0..200 {
        let output = resampler.process(&input).expect("resample block");
        checksum += output.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "resampler: 200 blocks x 480 5.1 frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn streaming_resampler_preserves_phase_across_rate_converted_blocks() {
    let input_format = AudioFormat::new(44_100, 1).expect("input format");
    let output_format = AudioFormat::new(48_000, 1).expect("output format");
    let mut resampler =
        StreamingAudioResampler::new(input_format, output_format).expect("streaming resampler");
    let mut next_output = 0_u64;
    let mut emitted = 0_usize;

    for block in 0..8_u64 {
        let start = usize::try_from(block * 441).expect("bounded test frame index");
        let input = AudioBuffer::new(
            input_format,
            Timestamp::from_nanos(block * 10_000_000),
            (start..start + 441).map(|sample| sample as f32).collect(),
        )
        .expect("ramp input");
        let converted = resampler.process(&input).expect("convert block");

        assert_eq!(
            converted.timestamp(),
            Timestamp::from_nanos(next_output * 1_000_000_000 / 48_000)
        );
        for sample in converted.samples() {
            let expected = next_output as f64 * 44_100.0 / 48_000.0;
            assert!(
                (f64::from(*sample) - expected).abs() < 0.001,
                "sample {next_output} was {sample}, expected {expected}"
            );
            next_output += 1;
            emitted += 1;
        }
    }

    // One look-ahead sample is intentionally held until another block arrives;
    // all emitted samples still follow one continuous source position without
    // a reset or duplicate at each 441-frame boundary.
    assert_eq!(emitted, 8 * 479 + 7);
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn streaming_resampler_keeps_downsampled_blocks_on_one_sample_clock() {
    let input_format = AudioFormat::new(48_000, 1).expect("input format");
    let output_format = AudioFormat::new(44_100, 1).expect("output format");
    let mut resampler =
        StreamingAudioResampler::new(input_format, output_format).expect("streaming resampler");
    let mut next_output = 0_u64;
    let mut emitted = 0_usize;

    for block in 0..8_u64 {
        let start = usize::try_from(block * 480).expect("bounded test frame index");
        let input = AudioBuffer::new(
            input_format,
            Timestamp::from_nanos(block * 10_000_000),
            (start..start + 480).map(|sample| sample as f32).collect(),
        )
        .expect("ramp input");
        let converted = resampler.process(&input).expect("convert block");

        assert_eq!(
            converted.timestamp(),
            Timestamp::from_nanos(next_output * 1_000_000_000 / 44_100)
        );
        for sample in converted.samples() {
            let expected = next_output as f64 * 48_000.0 / 44_100.0;
            assert!(
                (f64::from(*sample) - expected).abs() < 0.001,
                "sample {next_output} was {sample}, expected {expected}"
            );
            next_output += 1;
            emitted += 1;
        }
    }

    assert_eq!(emitted, 8 * 441);
}

#[test]
fn delay_line_preserves_order_and_reuses_the_input_shape() {
    let mono = AudioFormat::new(1_000, 1).expect("mono format");
    let mut delay = AudioDelayLine::with_block_frames(mono, 3, 2).expect("delay line");
    assert_eq!(delay.delay_frames(), 3);
    assert!(!delay.is_passthrough());

    let first = AudioBuffer::new(mono, Timestamp::ZERO, vec![1.0, 2.0]).expect("first block");
    let second =
        AudioBuffer::new(mono, Timestamp::from_millis(2), vec![3.0, 4.0]).expect("second block");
    let third =
        AudioBuffer::new(mono, Timestamp::from_millis(4), vec![5.0, 6.0]).expect("third block");

    assert_eq!(
        delay.process(first).expect("first output").samples(),
        &[0.0, 0.0]
    );
    assert_eq!(
        delay.process(second).expect("second output").samples(),
        &[0.0, 1.0]
    );
    assert_eq!(
        delay.process(third).expect("third output").samples(),
        &[2.0, 3.0]
    );
}

#[test]
fn delay_line_rejects_unbounded_offsets_and_wrong_formats() {
    let mono = AudioFormat::new(48_000, 1).expect("mono format");
    let too_large = MAX_AUDIO_SYNC_OFFSET_MILLISECONDS + 1;
    assert!(matches!(
        AudioDelayLine::new(mono, too_large),
        Err(AudioError::InvalidSyncOffset {
            milliseconds
        }) if milliseconds == too_large
    ));

    let mut delay = AudioDelayLine::new(mono, 10).expect("delay line");
    let stereo = AudioFormat::new(48_000, 2).expect("stereo format");
    let input = AudioBuffer::silence(stereo, Timestamp::ZERO, 1).expect("stereo buffer");
    assert!(matches!(
        delay.process(input),
        Err(AudioError::FormatMismatch {
            expected,
            actual
        }) if expected == mono && actual == stereo
    ));
}

#[test]
fn delay_line_block_timing_report() {
    let mut delay = AudioDelayLine::with_block_frames(format(), 500, 480).expect("delay line");
    let mut block = AudioBuffer::silence(format(), Timestamp::ZERO, 480).expect("audio block");
    let started = Instant::now();
    let mut checksum = 0.0_f32;
    for index in 0_u64..200 {
        block.set_timestamp(Timestamp::from_millis(index * 10));
        block = delay.process(block).expect("delay block");
        checksum += block.samples()[0];
    }
    let elapsed = started.elapsed();
    assert!(elapsed.as_nanos() > 0);
    assert!(checksum.is_finite());
    std::hint::black_box(checksum);
    println!(
        "audio delay: 200 blocks x 480 stereo frames = {:?} ({:?}/block)",
        elapsed,
        elapsed / 200
    );
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
