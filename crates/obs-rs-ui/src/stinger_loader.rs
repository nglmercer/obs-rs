//! Toolkit-neutral ownership of one bounded Stinger resource load.

use std::sync::Arc;

use obs_rs_media::{
    MediaError, StingerClip, StingerLoadQueueError, StingerLoadRequest, StingerLoadWorker,
    StingerResourceLoader, StingerSpec, VideoFormat,
};

/// Current transient state of one Stinger resource request.
///
/// The persistent [`StingerSpec`] remains owned by the project model. This
/// state only describes the request and resolved clip currently presented to
/// a toolkit adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StingerLoadState {
    /// No request is active or a previous request was invalidated.
    Idle,
    /// The resource is being resolved on the dedicated worker.
    Loading {
        /// Caller-visible identity of the in-flight request.
        request_id: u64,
        /// Persistent metadata associated with the request.
        spec: StingerSpec,
    },
    /// The resource resolved to a clip matching the current canvas format.
    Ready {
        /// Identity of the request that produced the clip.
        request_id: u64,
        /// Persistent metadata associated with the resolved clip.
        spec: StingerSpec,
        /// Immutable, bounded clip safe to pass to the render command.
        clip: Arc<StingerClip>,
    },
    /// The current request failed before it produced a renderable clip.
    Failed {
        /// Identity of the request that failed.
        request_id: u64,
        /// Persistent metadata associated with the failed request.
        spec: StingerSpec,
        /// Bounded, typed failure suitable for a UI notice.
        error: MediaError,
    },
    /// The session was cancelled and cannot accept more requests.
    Stopped,
}

/// Toolkit-neutral controller for one bounded asynchronous Stinger load.
///
/// The session owns no project data and performs no file or decoder I/O. A
/// frontend injects a [`StingerResourceLoader`], submits validated metadata,
/// and polls this object from its ordinary UI cadence. At most one completed
/// result is retained by the underlying worker, and a new accepted request
/// invalidates the previous renderable clip in this session.
pub struct StingerLoadSession {
    worker: StingerLoadWorker,
    target_format: VideoFormat,
    next_request_id: u64,
    state: StingerLoadState,
}

impl StingerLoadSession {
    /// Starts a toolkit-neutral Stinger load session.
    ///
    /// # Errors
    ///
    /// Returns the operating-system thread-spawn error when the bounded
    /// resource worker cannot be created.
    pub fn spawn(
        loader: impl StingerResourceLoader,
        target_format: VideoFormat,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            worker: StingerLoadWorker::spawn(loader)?,
            target_format,
            next_request_id: 1,
            state: StingerLoadState::Idle,
        })
    }

    /// Returns the canvas format required from the next decoded clip.
    #[must_use]
    pub const fn target_format(&self) -> VideoFormat {
        self.target_format
    }

    /// Returns the current transient request state.
    #[must_use]
    pub const fn state(&self) -> &StingerLoadState {
        &self.state
    }

    /// Attempts to submit one validated resource reference without waiting.
    ///
    /// The previous `Ready` clip is cleared only after the request enters the
    /// worker queue. A full queue therefore leaves the caller's current state
    /// intact and can be retried on a later UI tick.
    ///
    /// # Errors
    ///
    /// Returns [`StingerLoadQueueError::Full`] when the bounded worker cannot
    /// accept this request, or [`StingerLoadQueueError::Stopped`] after
    /// cancellation.
    pub fn try_request(&mut self, spec: StingerSpec) -> Result<u64, StingerLoadQueueError> {
        let request_id = self.next_request_id;
        let request = StingerLoadRequest::new(request_id, spec.clone(), self.target_format);
        self.worker.try_submit(request)?;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.state = StingerLoadState::Loading { request_id, spec };
        Ok(request_id)
    }

    /// Polls one completed worker result without waiting.
    ///
    /// Returns `true` when a result was consumed. Results that do not belong
    /// to the current request are discarded, keeping a late completion from
    /// replacing a newer UI selection.
    pub fn poll(&mut self) -> bool {
        let Some(result) = self.worker.try_take_result() else {
            return false;
        };
        let request_id = result.request_id();
        let Some((current_id, spec)) = self.current_request() else {
            return true;
        };
        if current_id != request_id {
            return true;
        }
        self.state = match result.into_result() {
            Ok(clip) if clip.format() == self.target_format => StingerLoadState::Ready {
                request_id,
                spec,
                clip: Arc::new(clip),
            },
            Ok(clip) => StingerLoadState::Failed {
                request_id,
                spec,
                error: MediaError::FormatMismatch {
                    expected: self.target_format,
                    actual: clip.format(),
                },
            },
            Err(error) => StingerLoadState::Failed {
                request_id,
                spec,
                error,
            },
        };
        true
    }

    /// Changes the target format and invalidates any pending or ready clip.
    ///
    /// An old worker completion is ignored because there is no longer a
    /// current request state. The existing worker remains usable for a new
    /// request at the new format.
    pub fn set_target_format(&mut self, target_format: VideoFormat) {
        if self.target_format == target_format {
            return;
        }
        self.target_format = target_format;
        self.state = StingerLoadState::Idle;
    }

    /// Requests cancellation and marks the session permanently stopped.
    pub fn cancel(&mut self) {
        self.worker.cancel();
        self.state = StingerLoadState::Stopped;
    }

    /// Returns whether the underlying worker has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.worker.is_cancelled()
    }

    /// Clones the immutable clip when the current request is ready to render.
    #[must_use]
    pub fn ready_clip(&self) -> Option<Arc<StingerClip>> {
        match &self.state {
            StingerLoadState::Ready { clip, .. } => Some(Arc::clone(clip)),
            _ => None,
        }
    }

    fn current_request(&self) -> Option<(u64, StingerSpec)> {
        match &self.state {
            StingerLoadState::Loading { request_id, spec }
            | StingerLoadState::Ready {
                request_id, spec, ..
            }
            | StingerLoadState::Failed {
                request_id, spec, ..
            } => Some((*request_id, spec.clone())),
            StingerLoadState::Idle | StingerLoadState::Stopped => None,
        }
    }
}
