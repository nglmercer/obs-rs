//! A capture device that never blocks its caller.
//!
//! [`AsyncCaptureDevice`](super::AsyncCaptureDevice) moves the *opening* of a
//! device off the calling thread but still pulls each frame inline, so a driver
//! that stalls for 100 ms stalls the whole compositor — and with it every other
//! source in the scene. This wrapper moves both halves off: a worker owns the
//! device, writes the newest frame into a one-slot mailbox, and
//! [`ThreadedCaptureDevice::poll_frame`] only reads that slot.
//!
//! Frames are dropped rather than queued. A compositor asking for "now" wants
//! the newest frame, not the oldest one a device happened to produce.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{
    device::{CaptureRequest, VideoCaptureDevice},
    error::CaptureError,
    lifecycle::CaptureLifecycleState,
};

/// Shortest pause between device polls when a device reports no new frame.
///
/// A device that returns `Ok(None)` immediately would otherwise spin a core;
/// one millisecond is well below any capture cadence and keeps latency at the
/// noise floor.
const IDLE_POLL: Duration = Duration::from_millis(1);

/// How long shutdown waits for the worker to leave the native driver.
///
/// The worker can only notice the stop flag between calls to `next_frame`, and
/// a native frame acquisition is not interruptible: a camera whose driver has
/// wedged inside `frame()` never returns. Waiting for it unconditionally would
/// hang the compositor on every settings change, source removal, and quit, so
/// shutdown waits this long and then walks away. A normal frame completes in
/// well under a frame period, so a healthy device always joins cleanly.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

type OpenResult = Result<Box<dyn VideoCaptureDevice>, CaptureError>;

struct Mailbox {
    /// The newest frame the worker produced, or `None` before the first one.
    latest: Mutex<Option<VideoFrame>>,
    /// The failure that ended the worker, if it ended in one.
    failure: Mutex<Option<CaptureError>>,
    /// Set once the device has opened and started.
    ready: AtomicBool,
    /// Set when the worker has stopped for any reason.
    finished: AtomicBool,
    /// Asks the worker to stop; read between frames.
    stop: AtomicBool,
    /// Frames the worker has published, for diagnostics and tests.
    frames: AtomicU64,
}

impl Mailbox {
    fn new() -> Self {
        Self {
            latest: Mutex::new(None),
            failure: Mutex::new(None),
            ready: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            frames: AtomicU64::new(0),
        }
    }
}

/// A [`VideoCaptureDevice`] driven by its own thread, polled without blocking.
///
/// Dropping the handle stops the worker and joins it, so the underlying device
/// is closed exactly once and never outlives the source that owns it.
pub struct ThreadedCaptureDevice {
    mailbox: Arc<Mailbox>,
    join: Option<JoinHandle<()>>,
    format: VideoFormat,
}

impl ThreadedCaptureDevice {
    /// Starts a worker that opens `opener`'s device and keeps it running.
    ///
    /// Neither opening nor the first frame blocks the caller: until the device
    /// is ready, [`Self::poll_frame`] simply returns `Ok(None)`.
    #[must_use]
    pub fn open(
        request: CaptureRequest,
        name: &str,
        opener: impl FnOnce() -> OpenResult + Send + 'static,
    ) -> Self {
        let format = request.output_format();
        let mailbox = Arc::new(Mailbox::new());
        let worker = Arc::clone(&mailbox);
        let join = thread::Builder::new()
            .name(format!("obs-rs-capture-{name}"))
            .spawn(move || capture_loop(&worker, request, opener))
            .ok();
        if join.is_none() {
            mailbox.finished.store(true, Ordering::Release);
            if let Ok(mut failure) = mailbox.failure.lock() {
                *failure = Some(CaptureError::Protocol {
                    message: "capture worker could not be started".to_owned(),
                });
            }
        }
        Self {
            mailbox,
            join,
            format,
        }
    }

    /// Returns the negotiated output format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the number of frames the worker has published.
    #[must_use]
    pub fn published_frames(&self) -> u64 {
        self.mailbox.frames.load(Ordering::Relaxed)
    }

    /// Returns the observable lifecycle state without touching the device.
    #[must_use]
    pub fn state(&self) -> CaptureLifecycleState {
        if self.mailbox.finished.load(Ordering::Acquire) {
            return match self.failure() {
                Some(error) if is_permission_denial(&error) => CaptureLifecycleState::Denied,
                _ => CaptureLifecycleState::Lost,
            };
        }
        if self.mailbox.ready.load(Ordering::Acquire) {
            CaptureLifecycleState::Ready
        } else {
            CaptureLifecycleState::Opening
        }
    }

    /// Returns the failure that stopped the worker, if it has stopped.
    #[must_use]
    pub fn failure(&self) -> Option<CaptureError> {
        self.mailbox.failure.lock().ok()?.clone()
    }

    /// Returns the newest captured frame, restamped to `timestamp`.
    ///
    /// `Ok(None)` means "nothing new yet" — the device may still be opening, or
    /// simply not have produced a frame since the last poll. This never waits
    /// for the device.
    ///
    /// # Errors
    ///
    /// Returns the worker's failure once it has stopped and no frame is left to
    /// serve. A stopped worker with a frame still in its mailbox keeps serving
    /// that frame, so a momentary loss shows the last good picture rather than
    /// a hole in the scene.
    pub fn poll_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        let latest = self
            .mailbox
            .latest
            .lock()
            .ok()
            .and_then(|frame| frame.as_ref().map(|frame| frame.at_timestamp(timestamp)));
        match latest {
            Some(frame) => Ok(Some(frame)),
            None if self.mailbox.finished.load(Ordering::Acquire) => {
                Err(self.failure().unwrap_or(CaptureError::NotRunning))
            }
            None => Ok(None),
        }
    }

    /// Requests a clean worker shutdown and reports whether the worker joined.
    ///
    /// A caller that is about to replace a camera must not start the
    /// replacement when this returns `false`: the old worker may still be
    /// inside an uninterruptible native driver call and therefore still owns
    /// the physical device.
    pub fn shutdown(&mut self) -> bool {
        self.mailbox.stop.store(true, Ordering::Release);
        if self.join.is_none() {
            return true;
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while !self.mailbox.finished.load(Ordering::Acquire)
            && !self.join.as_ref().is_some_and(|join| join.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(IDLE_POLL);
        }
        let finished = self.mailbox.finished.load(Ordering::Acquire)
            || self.join.as_ref().is_some_and(|join| join.is_finished());
        if !finished {
            return false;
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        true
    }
}

impl Drop for ThreadedCaptureDevice {
    /// Stops the worker, waiting only as long as [`SHUTDOWN_GRACE`] for it.
    ///
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Opens the device and republishes its newest frame until asked to stop.
fn capture_loop(
    mailbox: &Arc<Mailbox>,
    request: CaptureRequest,
    opener: impl FnOnce() -> OpenResult,
) {
    let mut device = match opener() {
        Ok(device) => device,
        Err(error) => {
            finish(mailbox, Some(error));
            return;
        }
    };
    // A source can be dropped while native opening is in progress. Do not
    // start a device that the owner has already abandoned.
    if mailbox.stop.load(Ordering::Acquire) {
        device.stop();
        finish(mailbox, None);
        return;
    }
    if let Err(error) = device.start_capture(request) {
        finish(mailbox, Some(error));
        return;
    }
    if mailbox.stop.load(Ordering::Acquire) {
        device.stop();
        finish(mailbox, None);
        return;
    }
    mailbox.ready.store(true, Ordering::Release);

    // The worker stamps its own frames; the poller restamps to the scene clock,
    // so this only has to advance monotonically. The deadline is still
    // important: X11 GetImage returns as soon as the server replies, and
    // without pacing that path would run at the maximum server/CPU rate rather
    // than at the source's configured frame rate.
    let period_nanos = request
        .output_format()
        .frame_rate()
        .period_nanos()
        .unwrap_or(33_333_333)
        .max(1);
    let period = Duration::from_nanos(period_nanos);
    let mut next_deadline = Instant::now();
    let mut timestamp = Timestamp::ZERO;
    while !mailbox.stop.load(Ordering::Acquire) {
        wait_until(&mailbox.stop, next_deadline);
        if mailbox.stop.load(Ordering::Acquire) {
            break;
        }
        match device.next_frame(timestamp) {
            Ok(Some(frame)) => {
                if let Ok(mut latest) = mailbox.latest.lock() {
                    *latest = Some(frame);
                }
                mailbox.frames.fetch_add(1, Ordering::Relaxed);
                timestamp = timestamp
                    .checked_add(period_nanos)
                    .unwrap_or(Timestamp::ZERO);
                let now = Instant::now();
                next_deadline = next_deadline
                    .checked_add(period)
                    .filter(|deadline| *deadline > now)
                    .unwrap_or(now);
            }
            Ok(None) => {
                // A mailbox-backed reader can legitimately have no new frame
                // yet. Poll it often enough to keep latency low, but do not
                // let an empty source spin a core.
                next_deadline = Instant::now() + IDLE_POLL;
            }
            Err(error) => {
                finish(mailbox, Some(error));
                return;
            }
        }
    }
    device.stop();
    finish(mailbox, None);
}

/// Sleeps in short slices so a stop request is noticed promptly even when the
/// configured frame period is long. Native frame acquisition itself remains
/// the only uninterruptible operation in the worker.
fn wait_until(stop: &AtomicBool, deadline: Instant) {
    while !stop.load(Ordering::Acquire) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        thread::sleep(remaining.min(Duration::from_millis(5)));
    }
}

fn finish(mailbox: &Arc<Mailbox>, error: Option<CaptureError>) {
    if let (Some(error), Ok(mut failure)) = (error, mailbox.failure.lock()) {
        *failure = Some(error);
    }
    mailbox.finished.store(true, Ordering::Release);
}

const fn is_permission_denial(error: &CaptureError) -> bool {
    matches!(
        error,
        CaptureError::PermissionDenied | CaptureError::PermissionRequired
    )
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::{
        simulated::SimulatedCaptureDevice,
        types::{CaptureDeviceInfo, CaptureKind},
    };

    fn format() -> VideoFormat {
        VideoFormat::new(64, 32, obs_rs_media::FrameRate::new(30, 1).expect("rate"))
            .expect("format")
    }

    fn wait_for_frame(device: &mut ThreadedCaptureDevice) -> Option<VideoFrame> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match device.poll_frame(Timestamp::ZERO) {
                Ok(Some(frame)) => return Some(frame),
                Ok(None) => thread::sleep(IDLE_POLL),
                Err(_) => return None,
            }
        }
        None
    }

    #[test]
    fn frames_arrive_without_the_caller_ever_blocking() {
        let format = format();
        let mut device =
            ThreadedCaptureDevice::open(CaptureRequest::output(format), "test", move || {
                let device =
                    SimulatedCaptureDevice::new("threaded-test", "threaded", CaptureKind::Camera)?;
                Ok(Box::new(device) as Box<dyn VideoCaptureDevice>)
            });

        // The very first poll must return immediately, before the worker can
        // possibly have opened the device.
        let started = Instant::now();
        let _ = device.poll_frame(Timestamp::ZERO);
        assert!(started.elapsed() < Duration::from_millis(50));

        let frame = wait_for_frame(&mut device).expect("a frame");
        assert_eq!(frame.format(), format);
        assert!(device.published_frames() > 0);
        assert_eq!(device.state(), CaptureLifecycleState::Ready);
    }

    #[test]
    fn a_device_that_cannot_open_reports_its_failure_instead_of_hanging() {
        let mut device =
            ThreadedCaptureDevice::open(CaptureRequest::output(format()), "test", move || {
                Err(CaptureError::NotRunning)
            });

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut result = Ok(None);
        while Instant::now() < deadline {
            result = device.poll_frame(Timestamp::ZERO);
            if result.is_err() {
                break;
            }
            thread::sleep(IDLE_POLL);
        }
        assert!(matches!(result, Err(CaptureError::NotRunning)));
        assert_eq!(device.state(), CaptureLifecycleState::Lost);
    }

    /// A device whose `next_frame` never returns, like a wedged camera driver.
    struct WedgedDevice {
        info: CaptureDeviceInfo,
        release: Arc<AtomicBool>,
    }

    impl VideoCaptureDevice for WedgedDevice {
        fn info(&self) -> &CaptureDeviceInfo {
            &self.info
        }

        fn start(&mut self, _format: VideoFormat) -> Result<(), CaptureError> {
            Ok(())
        }

        fn stop(&mut self) {}

        fn is_running(&self) -> bool {
            true
        }

        fn next_frame(
            &mut self,
            _timestamp: Timestamp,
        ) -> Result<Option<VideoFrame>, CaptureError> {
            while !self.release.load(Ordering::Acquire) {
                thread::sleep(IDLE_POLL);
            }
            Err(CaptureError::NotRunning)
        }
    }

    #[test]
    fn a_wedged_driver_does_not_hang_shutdown() {
        let release = Arc::new(AtomicBool::new(false));
        let opener_release = Arc::clone(&release);
        let device =
            ThreadedCaptureDevice::open(CaptureRequest::output(format()), "wedged", move || {
                Ok(Box::new(WedgedDevice {
                    info: CaptureDeviceInfo::new("wedged", "wedged", CaptureKind::Camera)?,
                    release: opener_release,
                }) as Box<dyn VideoCaptureDevice>)
            });
        // Let the worker reach the call it will not come back from.
        thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        drop(device);
        let elapsed = started.elapsed();

        // Freeing the wedged thread keeps the test from leaking it.
        release.store(true, Ordering::Release);
        assert!(
            elapsed < SHUTDOWN_GRACE + Duration::from_secs(1),
            "shutdown waited {elapsed:?}, which is not bounded"
        );
    }

    #[test]
    fn a_restamped_frame_carries_the_requested_timestamp() {
        let mut device =
            ThreadedCaptureDevice::open(CaptureRequest::output(format()), "test", move || {
                let device =
                    SimulatedCaptureDevice::new("threaded-stamp", "threaded", CaptureKind::Camera)?;
                Ok(Box::new(device) as Box<dyn VideoCaptureDevice>)
            });
        assert!(wait_for_frame(&mut device).is_some());

        let stamped = device
            .poll_frame(Timestamp::from_nanos(1_234))
            .expect("poll")
            .expect("frame");

        assert_eq!(stamped.timestamp(), Timestamp::from_nanos(1_234));
    }
}
