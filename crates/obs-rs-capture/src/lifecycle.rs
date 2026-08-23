use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
};

use obs_rs_media::{Timestamp, VideoFormat, VideoFrame};

use super::{CaptureError, VideoCaptureDevice};

/// Observable lifecycle of an asynchronous platform capture session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureLifecycleState {
    /// A native permission prompt, device open, or format negotiation is active.
    Opening,
    /// Frames can be polled without waiting for native setup.
    Ready,
    /// The device or native service disconnected.
    Lost,
    /// A replacement native session is being opened after loss.
    Retrying,
    /// Capture permission was denied and automatic retries are disabled.
    Denied,
}

type OpenResult = Result<Box<dyn VideoCaptureDevice>, CaptureError>;
type OpenDevice = dyn Fn(&CaptureCancellation) -> OpenResult + Send + Sync + 'static;

/// Cancellation shared by a source handle and its asynchronous opener.
///
/// Native APIs are not all interruptible, but portal requests can be closed
/// explicitly. Keeping this token at the capture boundary lets a backend stop
/// waiting as soon as its owning source is replaced or dropped.
#[derive(Clone, Debug)]
pub struct CaptureCancellation {
    cancelled: Arc<AtomicBool>,
}

impl CaptureCancellation {
    /// Creates a cancellation token that is initially active.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Marks the associated open attempt as cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Returns whether the owner has abandoned the open attempt.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Default for CaptureCancellation {
    fn default() -> Self {
        Self::new()
    }
}

/// Non-blocking boundary around permission prompts and native device opening.
///
/// The opener always runs on a dedicated worker. [`Self::poll_frame`] only
/// checks a channel and polls an already-ready device, so GUI/compositor callers
/// never wait for a portal dialog or native format negotiation.
pub struct AsyncCaptureDevice {
    format: VideoFormat,
    opener: Arc<OpenDevice>,
    receiver: mpsc::Receiver<OpenResult>,
    cancelled: CaptureCancellation,
    device: Option<Box<dyn VideoCaptureDevice>>,
    state: CaptureLifecycleState,
    last_error: Option<CaptureError>,
}

impl AsyncCaptureDevice {
    /// Starts opening a platform device on a background worker.
    #[must_use]
    pub fn open(
        format: VideoFormat,
        opener: impl Fn() -> OpenResult + Send + Sync + 'static,
    ) -> Self {
        Self::open_cancellable(format, move |_| opener())
    }

    /// Starts opening a platform device with a cancellation-aware opener.
    #[must_use]
    pub fn open_cancellable(
        format: VideoFormat,
        opener: impl Fn(&CaptureCancellation) -> OpenResult + Send + Sync + 'static,
    ) -> Self {
        let opener: Arc<OpenDevice> = Arc::new(opener);
        let cancelled = CaptureCancellation::new();
        let receiver = spawn_open(Arc::clone(&opener), format, cancelled.clone());
        Self {
            format,
            opener,
            receiver,
            cancelled,
            device: None,
            state: CaptureLifecycleState::Opening,
            last_error: None,
        }
    }

    /// Adopts an already-opened device, retaining `opener` for recovery after
    /// the adopted session is lost.
    pub fn ready(
        format: VideoFormat,
        mut device: Box<dyn VideoCaptureDevice>,
        opener: impl Fn(&CaptureCancellation) -> OpenResult + Send + Sync + 'static,
    ) -> Result<Self, CaptureError> {
        device.start(format)?;
        let (_sender, receiver) = mpsc::sync_channel(1);
        Ok(Self {
            format,
            opener: Arc::new(opener),
            receiver,
            cancelled: CaptureCancellation::new(),
            device: Some(device),
            state: CaptureLifecycleState::Ready,
            last_error: None,
        })
    }

    /// Returns the current observable lifecycle state.
    #[must_use]
    pub const fn state(&self) -> CaptureLifecycleState {
        self.state
    }

    /// Returns the most recent native failure, if any.
    #[must_use]
    pub const fn last_error(&self) -> Option<&CaptureError> {
        self.last_error.as_ref()
    }

    /// Returns a backend-provided restore token from the ready device.
    #[must_use]
    pub fn restore_token(&self) -> Option<&str> {
        self.device.as_ref()?.restore_token()
    }

    /// Starts another asynchronous open after a lost session.
    ///
    /// Denied sessions intentionally cannot retry without constructing a new
    /// instance after an explicit permission action.
    ///
    /// # Errors
    ///
    /// Returns [`CaptureError::AlreadyRunning`] unless the state is `Lost`.
    pub fn retry(&mut self) -> Result<(), CaptureError> {
        if self.state != CaptureLifecycleState::Lost {
            return Err(CaptureError::AlreadyRunning);
        }
        self.cancelled.cancel();
        self.device = None;
        self.cancelled = CaptureCancellation::new();
        self.receiver = spawn_open(
            Arc::clone(&self.opener),
            self.format,
            self.cancelled.clone(),
        );
        self.state = CaptureLifecycleState::Retrying;
        self.last_error = None;
        Ok(())
    }

    /// Polls native opening and then the ready device without blocking on setup.
    ///
    /// `Ok(None)` means setup is still active or no newest frame is available.
    /// A running device failure changes the lifecycle to `Lost`; permission
    /// failures change it to `Denied`.
    ///
    /// # Errors
    ///
    /// Returns the native error that moved the session to `Lost` or `Denied`.
    pub fn poll_frame(&mut self, timestamp: Timestamp) -> Result<Option<VideoFrame>, CaptureError> {
        if matches!(
            self.state,
            CaptureLifecycleState::Opening | CaptureLifecycleState::Retrying
        ) {
            match self.receiver.try_recv() {
                Ok(Ok(device)) => {
                    self.device = Some(device);
                    self.state = CaptureLifecycleState::Ready;
                    self.last_error = None;
                }
                Ok(Err(error)) => return self.fail(error),
                Err(mpsc::TryRecvError::Empty) => return Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return self.fail(CaptureError::Protocol {
                        message: "capture opener stopped without a result".to_owned(),
                    });
                }
            }
        }
        match self.state {
            CaptureLifecycleState::Ready => {
                let result = self
                    .device
                    .as_mut()
                    .ok_or(CaptureError::NotRunning)?
                    .next_frame(timestamp);
                match result {
                    Ok(frame) => Ok(frame),
                    Err(error) => self.fail(error),
                }
            }
            CaptureLifecycleState::Opening | CaptureLifecycleState::Retrying => Ok(None),
            CaptureLifecycleState::Lost | CaptureLifecycleState::Denied => {
                Err(self.last_error.clone().unwrap_or(CaptureError::NotRunning))
            }
        }
    }

    fn fail<T>(&mut self, error: CaptureError) -> Result<T, CaptureError> {
        self.device = None;
        self.state = if is_permission_denial(&error) {
            CaptureLifecycleState::Denied
        } else {
            CaptureLifecycleState::Lost
        };
        self.last_error = Some(error.clone());
        Err(error)
    }
}

fn spawn_open(
    opener: Arc<OpenDevice>,
    format: VideoFormat,
    cancelled: CaptureCancellation,
) -> mpsc::Receiver<OpenResult> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = opener(&cancelled).and_then(|mut device| {
            if cancelled.is_cancelled() {
                return Err(CaptureError::NotRunning);
            }
            device.start(format)?;
            if cancelled.is_cancelled() {
                device.stop();
                return Err(CaptureError::NotRunning);
            }
            Ok(device)
        });
        if cancelled.is_cancelled() {
            return;
        }
        let _ = sender.send(result);
    });
    receiver
}

const fn is_permission_denial(error: &CaptureError) -> bool {
    matches!(
        error,
        CaptureError::PermissionDenied | CaptureError::PermissionRequired
    )
}

impl Drop for AsyncCaptureDevice {
    fn drop(&mut self) {
        self.cancelled.cancel();
    }
}
