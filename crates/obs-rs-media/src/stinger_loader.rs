//! Bounded asynchronous boundary for resolving persistent Stinger resources.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use super::{error::MediaError, format::VideoFormat, stinger::StingerClip, StingerSpec};

/// Maximum number of pending Stinger resource requests.
pub const STINGER_LOAD_QUEUE_CAPACITY: usize = 1;
/// Maximum number of completed Stinger results retained for polling.
pub const STINGER_LOAD_RESULT_CAPACITY: usize = 1;

/// Cancellation shared by a Stinger resource worker and its loader.
#[derive(Clone, Debug)]
pub struct StingerLoadCancellation {
    cancelled: Arc<AtomicBool>,
}

impl StingerLoadCancellation {
    /// Creates a token that is initially active.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests cancellation of the current load and any pending request.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Returns whether the loader should stop at its next safe checkpoint.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Default for StingerLoadCancellation {
    fn default() -> Self {
        Self::new()
    }
}

/// One worker-side request to resolve a persistent Stinger resource.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StingerLoadRequest {
    request_id: u64,
    spec: StingerSpec,
    target_format: VideoFormat,
}

impl StingerLoadRequest {
    /// Creates a request with a caller-owned identity for stale-result checks.
    #[must_use]
    pub const fn new(request_id: u64, spec: StingerSpec, target_format: VideoFormat) -> Self {
        Self {
            request_id,
            spec,
            target_format,
        }
    }

    /// Returns the caller-owned request identity.
    #[must_use]
    pub const fn request_id(&self) -> u64 {
        self.request_id
    }

    /// Returns the persistent resource metadata to resolve.
    #[must_use]
    pub fn spec(&self) -> &StingerSpec {
        &self.spec
    }

    /// Returns the bounded output format expected by the decoded clip.
    #[must_use]
    pub const fn target_format(&self) -> VideoFormat {
        self.target_format
    }
}

/// Result produced by the bounded Stinger resource worker.
#[derive(Debug, Eq, PartialEq)]
pub struct StingerLoadResult {
    request_id: u64,
    result: Result<StingerClip, MediaError>,
}

impl StingerLoadResult {
    /// Returns the request identity associated with this result.
    #[must_use]
    pub const fn request_id(&self) -> u64 {
        self.request_id
    }

    /// Borrows the decoded clip or typed media failure.
    ///
    /// # Errors
    ///
    /// Returns the media error produced by the loader when resolution failed.
    #[must_use = "inspect the loader result"]
    pub fn result(&self) -> Result<&StingerClip, &MediaError> {
        self.result.as_ref()
    }

    /// Consumes the result and returns the decoded clip or typed media failure.
    ///
    /// # Errors
    ///
    /// Returns the media error produced by the loader when resolution failed.
    #[must_use = "handle the loader result"]
    pub fn into_result(self) -> Result<StingerClip, MediaError> {
        self.result
    }
}

/// Errors returned when a request cannot enter the bounded loader queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StingerLoadQueueError {
    /// The worker is already processing a request and its one-slot queue is full.
    Full,
    /// Cancellation or worker teardown has closed the queue.
    Stopped,
}

/// Decoder-independent contract for loading a persistent Stinger resource.
///
/// Implementations run on the dedicated resource thread. They must check the
/// cancellation token between blocking/native operations and return a
/// preloaded [`StingerClip`] whose format matches the request target.
pub trait StingerResourceLoader: Send + 'static {
    /// Resolves one request without touching the caller's render or UI thread.
    ///
    /// # Errors
    ///
    /// Returns a typed media error when decoding, validation, or target-format
    /// conversion cannot produce a bounded [`StingerClip`].
    fn load(
        &mut self,
        request: &StingerLoadRequest,
        cancellation: &StingerLoadCancellation,
    ) -> Result<StingerClip, MediaError>;
}

impl<F> StingerResourceLoader for F
where
    F: FnMut(&StingerLoadRequest, &StingerLoadCancellation) -> Result<StingerClip, MediaError>
        + Send
        + 'static,
{
    fn load(
        &mut self,
        request: &StingerLoadRequest,
        cancellation: &StingerLoadCancellation,
    ) -> Result<StingerClip, MediaError> {
        self(request, cancellation)
    }
}

/// A dedicated, cancellation-aware worker with capacity-one request/result
/// queues.
pub struct StingerLoadWorker {
    sender: SyncSender<StingerLoadRequest>,
    result: Arc<Mutex<StingerLoadResultSlot>>,
    cancellation: StingerLoadCancellation,
    _join: thread::JoinHandle<()>,
}

#[derive(Default)]
struct StingerLoadResultSlot {
    latest_request_id: Option<u64>,
    result: Option<StingerLoadResult>,
}

impl StingerLoadWorker {
    /// Starts a decoder-independent Stinger resource worker.
    ///
    /// # Errors
    ///
    /// Returns the operating-system thread-spawn error when the dedicated
    /// worker cannot be created.
    pub fn spawn(loader: impl StingerResourceLoader) -> Result<Self, std::io::Error> {
        let (sender, requests) = mpsc::sync_channel(STINGER_LOAD_QUEUE_CAPACITY);
        let result = Arc::new(Mutex::new(StingerLoadResultSlot::default()));
        let thread_result = Arc::clone(&result);
        let cancellation = StingerLoadCancellation::new();
        let thread_cancellation = cancellation.clone();
        let join = thread::Builder::new()
            .name("obs-rs-stinger-loader".to_owned())
            .spawn(move || run_loader(loader, requests, thread_result, thread_cancellation))?;
        Ok(Self {
            sender,
            result,
            cancellation,
            _join: join,
        })
    }

    /// Attempts to enqueue one load request without waiting for I/O or decode.
    ///
    /// # Errors
    ///
    /// Returns [`StingerLoadQueueError::Full`] when the worker already has a
    /// request in flight and one pending, or
    /// [`StingerLoadQueueError::Stopped`] after cancellation/teardown.
    pub fn try_submit(&self, request: StingerLoadRequest) -> Result<(), StingerLoadQueueError> {
        if self.cancellation.is_cancelled() {
            return Err(StingerLoadQueueError::Stopped);
        }
        let request_id = request.request_id();
        let Ok(mut slot) = self.result.try_lock() else {
            return Err(StingerLoadQueueError::Full);
        };
        let previous_request_id = slot.latest_request_id;
        let previous_result = slot.result.take();
        slot.latest_request_id = Some(request_id);
        match self.sender.try_send(request) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                // The new request was not accepted, so restore the previous
                // freshness state and result slot. The lock is held while the
                // non-blocking send runs, preventing an in-flight completion
                // from observing a half-updated request identity.
                slot.latest_request_id = previous_request_id;
                slot.result = previous_result;
                Err(if self.cancellation.is_cancelled() {
                    StingerLoadQueueError::Stopped
                } else {
                    StingerLoadQueueError::Full
                })
            }
        }
    }

    /// Polls one completed result without waiting for the worker or its slot.
    #[must_use]
    pub fn try_take_result(&self) -> Option<StingerLoadResult> {
        self.result.try_lock().ok()?.result.take()
    }

    /// Requests cancellation without joining or blocking the caller.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Returns whether this worker has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

impl Drop for StingerLoadWorker {
    fn drop(&mut self) {
        self.cancel();
        // Dropping the join handle detaches the dedicated resource thread. A
        // native decoder must not make UI or render teardown wait indefinitely.
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the worker loop takes ownership of its bounded channels and cancellation token"
)]
fn run_loader(
    mut loader: impl StingerResourceLoader,
    requests: Receiver<StingerLoadRequest>,
    result: Arc<Mutex<StingerLoadResultSlot>>,
    cancellation: StingerLoadCancellation,
) {
    while !cancellation.is_cancelled() {
        let request = match requests.recv_timeout(Duration::from_millis(50)) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if cancellation.is_cancelled() {
            break;
        }
        let request_id = request.request_id();
        let outcome = loader.load(&request, &cancellation);
        if cancellation.is_cancelled() {
            break;
        }
        if let Ok(mut slot) = result.lock() {
            if slot.latest_request_id == Some(request_id) {
                slot.result = Some(StingerLoadResult {
                    request_id,
                    result: outcome,
                });
            }
        }
    }
}
