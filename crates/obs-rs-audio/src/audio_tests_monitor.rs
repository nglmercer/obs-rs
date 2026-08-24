use super::*;

struct CountingOutput {
    format: AudioFormat,
    writes: Arc<AtomicUsize>,
}

impl AudioOutput for CountingOutput {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn state(&self) -> AudioOutputState {
        AudioOutputState::Running
    }

    fn write_block(&mut self, buffer: &AudioBuffer) -> Result<(), AudioDeviceError> {
        if buffer.format() != self.format {
            return Err(AudioDeviceError::Audio(AudioError::FormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            }));
        }
        self.writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn stop(&mut self) {}
}

struct CountingOutputProvider {
    writes: Arc<AtomicUsize>,
    fail_open: bool,
}

struct GateOutput {
    format: AudioFormat,
    started: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    writes: Arc<AtomicUsize>,
}

impl AudioOutput for GateOutput {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn state(&self) -> AudioOutputState {
        AudioOutputState::Running
    }

    fn write_block(&mut self, buffer: &AudioBuffer) -> Result<(), AudioDeviceError> {
        if buffer.format() != self.format {
            return Err(AudioDeviceError::Audio(AudioError::FormatMismatch {
                expected: self.format,
                actual: buffer.format(),
            }));
        }
        self.started.store(true, Ordering::Release);
        while !self.release.load(Ordering::Acquire) {
            thread::yield_now();
        }
        self.writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn stop(&mut self) {}
}

struct GateOutputProvider {
    started: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    writes: Arc<AtomicUsize>,
}

impl AudioOutputProvider for GateOutputProvider {
    fn discover_outputs(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Ok(Vec::new())
    }

    fn open_output(
        &self,
        _device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioOutput>, AudioDeviceError> {
        Ok(Box::new(GateOutput {
            format,
            started: Arc::clone(&self.started),
            release: Arc::clone(&self.release),
            writes: Arc::clone(&self.writes),
        }))
    }
}

impl AudioOutputProvider for CountingOutputProvider {
    fn discover_outputs(&self) -> Result<Vec<AudioDeviceInfo>, AudioDeviceError> {
        Ok(Vec::new())
    }

    fn open_output(
        &self,
        _device_id: &str,
        format: AudioFormat,
    ) -> Result<Box<dyn AudioOutput>, AudioDeviceError> {
        if self.fail_open {
            return Err(AudioDeviceError::Unavailable(
                "test monitor unavailable".to_owned(),
            ));
        }
        Ok(Box::new(CountingOutput {
            format,
            writes: Arc::clone(&self.writes),
        }))
    }
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    condition()
}

#[test]
fn asynchronous_monitor_output_is_bounded_and_non_blocking() {
    let writes = Arc::new(AtomicUsize::new(0));
    let worker = AudioOutputWorker::spawn(
        Arc::new(CountingOutputProvider {
            writes: Arc::clone(&writes),
            fail_open: false,
        }),
        "test-output",
        format(),
        2,
    )
    .expect("output worker");
    let handle = worker.handle();
    assert!(handle.try_write(buffer(&[0.1, 0.1])));
    assert!(handle.try_write(buffer(&[0.2, 0.2])));
    assert!(wait_until(Duration::from_secs(1), || {
        writes.load(Ordering::Relaxed) == 2
    }));
    assert_eq!(handle.snapshot().dropped_blocks, 0);
    drop(worker);
    assert!(wait_until(Duration::from_secs(1), || {
        matches!(handle.snapshot().state, AudioOutputWorkerState::Stopped)
    }));
}

#[test]
fn asynchronous_monitor_output_reports_open_failure_and_drops_after_close() {
    let worker = AudioOutputWorker::spawn(
        Arc::new(CountingOutputProvider {
            writes: Arc::new(AtomicUsize::new(0)),
            fail_open: true,
        }),
        "missing-output",
        format(),
        1,
    )
    .expect("output worker");
    let handle = worker.handle();
    assert!(wait_until(Duration::from_secs(1), || {
        matches!(handle.snapshot().state, AudioOutputWorkerState::Failed)
    }));
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.queued_blocks, 0);
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("audio device unavailable: test monitor unavailable")
    );
    assert!(!handle.try_write(buffer(&[0.1, 0.1])));
    assert_eq!(handle.snapshot().dropped_blocks, 1);
}

#[test]
fn asynchronous_monitor_output_drops_when_the_bounded_queue_is_full() {
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicUsize::new(0));
    let worker = AudioOutputWorker::spawn(
        Arc::new(GateOutputProvider {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
            writes: Arc::clone(&writes),
        }),
        "test-output",
        format(),
        1,
    )
    .expect("output worker");
    let handle = worker.handle();
    assert!(handle.try_write(buffer(&[0.1, 0.1])));
    assert!(wait_until(Duration::from_secs(1), || {
        started.load(Ordering::Acquire)
    }));
    assert!(handle.try_write(buffer(&[0.2, 0.2])));
    assert!(!handle.try_write(buffer(&[0.3, 0.3])));
    assert_eq!(handle.snapshot().dropped_blocks, 1);

    release.store(true, Ordering::Release);
    assert!(wait_until(Duration::from_secs(1), || {
        writes.load(Ordering::Relaxed) == 2
    }));
    drop(worker);
    assert!(wait_until(Duration::from_secs(1), || {
        matches!(handle.snapshot().state, AudioOutputWorkerState::Stopped)
    }));
}

#[test]
fn asynchronous_monitor_output_rejects_zero_capacity() {
    let result = AudioOutputWorker::spawn(
        Arc::new(CountingOutputProvider {
            writes: Arc::new(AtomicUsize::new(0)),
            fail_open: false,
        }),
        "test-output",
        format(),
        0,
    );
    assert!(matches!(result, Err(AudioOutputWorkerError::ZeroCapacity)));
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
