//! GUI-side preloading boundary for scene-owned Stinger resources.

use obs_rs_engine::GStreamerStingerLoader;
use obs_rs_media::{MediaError, StingerClip, StingerLoadQueueError, StingerSpec, VideoFormat};
use obs_rs_project::Project;
use obs_rs_ui::{StingerLoadSession, StingerLoadState};
use std::{fmt, sync::Arc};

#[cfg(test)]
use obs_rs_project::{Profile, SceneSpec};

/// One non-blocking event produced while the GUI keeps a scene Stinger warm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StingerLoadEvent {
    /// A new persistent resource was accepted by the bounded worker.
    Requested,
    /// The requested resource is ready for an explicit Take workflow.
    Ready,
    /// The worker returned a typed media failure.
    Failed(MediaError),
}

/// Failure reported when the explicit Take action cannot use a ready clip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StingerTakeError {
    /// The selected resource is not preloaded and ready yet.
    NotReady,
    /// The worker produced a typed media failure for the current resource.
    Failed(MediaError),
    /// The loader session was cancelled during shutdown.
    Stopped,
}

impl fmt::Display for StingerTakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady => formatter.write_str(
                "Stinger is not ready; request the resource and wait for it to finish loading",
            ),
            Self::Failed(error) => write!(formatter, "Stinger resource failed: {error}"),
            Self::Stopped => formatter.write_str("Stinger loader stopped"),
        }
    }
}

/// Owns the GUI-facing Stinger preload session without copying project state.
pub(crate) struct StingerLoadController {
    session: StingerLoadSession,
    pending_take: Option<PendingStingerTake>,
}

struct PendingStingerTake {
    spec: StingerSpec,
    duration_ms: u32,
}

impl StingerLoadController {
    /// Starts the native GStreamer-backed session used by the desktop build.
    ///
    /// No file or decoder work occurs until a preloaded scene resource is
    /// submitted by [`Self::sync`].
    pub(crate) fn native(target_format: VideoFormat) -> Result<Self, std::io::Error> {
        Self::with_loader(GStreamerStingerLoader, target_format)
    }

    /// Builds a controller with an injected loader for deterministic tests and
    /// non-native frontends.
    pub(crate) fn with_loader(
        loader: impl obs_rs_media::StingerResourceLoader,
        target_format: VideoFormat,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            session: StingerLoadSession::spawn(loader, target_format)?,
            pending_take: None,
        })
    }

    /// Reconciles the selected scene's persisted Stinger reference.
    ///
    /// Only `preload=true` resources are submitted here. The operation never
    /// waits for disk, `GStreamer`, or the worker; a later explicit Take packet
    /// will decide how a non-preloaded resource is resolved.
    pub(crate) fn sync(
        &mut self,
        project: &Project,
        preview_scene: Option<&str>,
    ) -> Result<Option<StingerLoadEvent>, String> {
        let Some(profile) = project.active_profile_spec() else {
            self.session.clear();
            self.pending_take = None;
            return Ok(None);
        };
        let spec = preview_scene
            .and_then(|scene| profile.scene(scene))
            .and_then(|scene| scene.stinger_override())
            .cloned();
        self.sync_spec(spec.as_ref(), profile.video_format())
    }

    /// Reconciles one already-selected resource and format.
    pub(crate) fn sync_spec(
        &mut self,
        spec: Option<&StingerSpec>,
        target_format: VideoFormat,
    ) -> Result<Option<StingerLoadEvent>, String> {
        self.session.set_target_format(target_format);
        let Some(spec) = spec else {
            self.session.clear();
            self.pending_take = None;
            return Ok(None);
        };
        if self
            .pending_take
            .as_ref()
            .is_some_and(|pending| pending.spec != *spec)
        {
            self.pending_take = None;
        }
        if !self.matches_current_spec(spec) {
            self.session.clear();
            self.pending_take = None;
            if spec.preload() {
                match self.session.try_request(spec.clone()) {
                    Ok(_) => return Ok(Some(StingerLoadEvent::Requested)),
                    Err(StingerLoadQueueError::Full) => return Ok(None),
                    Err(StingerLoadQueueError::Stopped) => {
                        return Err("Stinger preload worker stopped".to_owned())
                    }
                }
            }
        }
        if !self.session.poll() {
            return Ok(None);
        }
        let event = match self.session.state() {
            StingerLoadState::Ready { .. } => StingerLoadEvent::Ready,
            StingerLoadState::Failed { error, .. } => {
                self.pending_take = None;
                StingerLoadEvent::Failed(*error)
            }
            StingerLoadState::Idle
            | StingerLoadState::Loading { .. }
            | StingerLoadState::Stopped => {
                self.pending_take = None;
                return Ok(None);
            }
        };
        Ok(Some(event))
    }

    /// Starts a bounded load for an explicit Take and remembers its validated
    /// duration for automatic dispatch when the worker publishes the clip.
    /// Resources whose persisted `preload` flag is false use the same worker;
    /// the caller remains on the UI thread.
    pub(crate) fn request_on_demand_take(
        &mut self,
        project: &Project,
        preview_scene: Option<&str>,
        duration_ms: u32,
    ) -> Result<StingerLoadEvent, String> {
        let Some(profile) = project.active_profile_spec() else {
            self.session.clear();
            self.pending_take = None;
            return Err("Stinger is not ready; no active profile is configured".to_owned());
        };
        let Some(spec) = preview_scene
            .and_then(|scene| profile.scene(scene))
            .and_then(|scene| scene.stinger_override())
            .cloned()
        else {
            self.session.clear();
            self.pending_take = None;
            return Err("Stinger is not ready; no resource is configured".to_owned());
        };
        let event = self.request_spec(spec.clone(), profile.video_format())?;
        self.pending_take = Some(PendingStingerTake { spec, duration_ms });
        Ok(event)
    }

    /// Returns the immutable clip already published by the preload worker.
    ///
    /// This is deliberately a state lookup only. Explicit Take never opens a
    /// file, polls a decoder, or waits for the worker; callers surface
    /// [`StingerTakeError::NotReady`] and submit a bounded request through the
    /// same worker when a persisted resource is available.
    pub(crate) fn ready_clip(&self) -> Result<Arc<StingerClip>, StingerTakeError> {
        match self.session.state() {
            StingerLoadState::Ready { clip, .. } => Ok(Arc::clone(clip)),
            StingerLoadState::Failed { error, .. } => Err(StingerTakeError::Failed(*error)),
            StingerLoadState::Stopped => Err(StingerTakeError::Stopped),
            StingerLoadState::Idle | StingerLoadState::Loading { .. } => {
                Err(StingerTakeError::NotReady)
            }
        }
    }

    /// Takes the one-shot intent once its matching worker result is ready.
    ///
    /// The intent is transient and is consumed before dispatch, so a failed
    /// state command cannot cause an automatic retry loop on every refresh.
    pub(crate) fn take_ready_pending(&mut self) -> Option<(Arc<StingerClip>, u32)> {
        let (clip, duration_ms) = {
            let pending = self.pending_take.as_ref()?;
            let clip = match self.session.state() {
                StingerLoadState::Ready { spec, clip, .. } if spec == &pending.spec => {
                    Arc::clone(clip)
                }
                _ => return None,
            };
            (clip, pending.duration_ms)
        };
        self.pending_take.take();
        Some((clip, duration_ms))
    }

    /// Returns the current transient session state for diagnostics/tests.
    #[cfg(test)]
    pub(crate) const fn state(&self) -> &StingerLoadState {
        self.session.state()
    }

    fn matches_current_spec(&self, spec: &StingerSpec) -> bool {
        match self.session.state() {
            StingerLoadState::Loading { spec: current, .. }
            | StingerLoadState::Ready { spec: current, .. }
            | StingerLoadState::Failed { spec: current, .. } => current == spec,
            StingerLoadState::Idle | StingerLoadState::Stopped => false,
        }
    }

    fn request_spec(
        &mut self,
        spec: StingerSpec,
        target_format: VideoFormat,
    ) -> Result<StingerLoadEvent, String> {
        self.session.set_target_format(target_format);
        if self.matches_current_spec(&spec) {
            return Ok(StingerLoadEvent::Requested);
        }
        self.session.clear();
        self.pending_take = None;
        match self.session.try_request(spec) {
            Ok(_) => Ok(StingerLoadEvent::Requested),
            Err(StingerLoadQueueError::Full) => {
                Err("Stinger load queue is full; try Take again shortly".to_owned())
            }
            Err(StingerLoadQueueError::Stopped) => Err("Stinger preload worker stopped".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_media::{
        FrameRate, StingerClip, StingerLoadCancellation, StingerLoadRequest,
        StingerResourceFailure, Timestamp, VideoFrame,
    };
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    fn format() -> VideoFormat {
        VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format")
    }

    fn spec(path: &str, preload: bool) -> StingerSpec {
        StingerSpec::new(path, 500, preload, false).expect("spec")
    }

    fn clip(format: VideoFormat) -> StingerClip {
        StingerClip::new(
            vec![VideoFrame::solid(format, Timestamp::ZERO, [0, 255, 0, 255])],
            vec![100_000_000],
            500,
        )
        .expect("clip")
    }

    fn wait_for_event(
        controller: &mut StingerLoadController,
        spec: &StingerSpec,
        format: VideoFormat,
    ) -> StingerLoadEvent {
        for _ in 0..200 {
            if let Some(event) = controller.sync_spec(Some(spec), format).expect("sync") {
                if event != StingerLoadEvent::Requested {
                    return event;
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("stinger controller did not publish an event");
    }

    #[test]
    fn controller_preloads_only_the_current_scene_resource_and_exposes_ready_clip() {
        let format = format();
        let expected = clip(format);
        let expected_for_loader = expected.clone();
        let mut controller = StingerLoadController::with_loader(
            move |request: &StingerLoadRequest, cancellation: &StingerLoadCancellation| {
                assert_eq!(request.target_format(), format);
                assert!(!cancellation.is_cancelled());
                Ok(expected_for_loader.clone())
            },
            format,
        )
        .expect("controller");
        let resource = spec("assets/intro.webm", true);
        assert_eq!(
            controller
                .sync_spec(Some(&resource), format)
                .expect("request"),
            Some(StingerLoadEvent::Requested)
        );
        assert_eq!(
            wait_for_event(&mut controller, &resource, format),
            StingerLoadEvent::Ready
        );
        assert!(matches!(controller.state(), StingerLoadState::Ready { .. }));
        assert!(controller.state().clone().eq(&StingerLoadState::Ready {
            request_id: 1,
            spec: resource,
            clip: Arc::new(expected.clone()),
        }));
        assert_eq!(expected.frame_count(), 1);
    }

    #[test]
    fn controller_does_not_submit_non_preloaded_resources() {
        let format = format();
        let mut controller = StingerLoadController::with_loader(
            |_request: &StingerLoadRequest, _cancellation: &StingerLoadCancellation| {
                panic!("non-preloaded resource must not reach the loader")
            },
            format,
        )
        .expect("controller");
        assert_eq!(
            controller
                .sync_spec(Some(&spec("assets/on-demand.webm", false)), format)
                .expect("sync"),
            None
        );
        assert_eq!(controller.state(), &StingerLoadState::Idle);
    }

    #[test]
    fn controller_can_request_a_non_preloaded_resource_on_demand() {
        let format = format();
        let expected = clip(format);
        let expected_for_loader = expected.clone();
        let mut controller = StingerLoadController::with_loader(
            move |_request: &StingerLoadRequest, _cancellation: &StingerLoadCancellation| {
                Ok(expected_for_loader.clone())
            },
            format,
        )
        .expect("controller");
        let resource = spec("assets/on-demand.webm", false);
        let mut project = Project::new("On-demand Stinger").expect("project");
        let mut profile = Profile::new("live", "Live profile", format).expect("profile");
        let mut scene = SceneSpec::new("main", "Main scene").expect("scene");
        scene.set_stinger_override(Some(resource.clone()));
        profile.add_scene(scene).expect("scene attach");
        project.add_profile(profile).expect("profile attach");
        assert_eq!(
            controller
                .request_on_demand_take(&project, Some("main"), 450)
                .expect("on-demand request"),
            StingerLoadEvent::Requested
        );
        assert_eq!(
            wait_for_event(&mut controller, &resource, format),
            StingerLoadEvent::Ready
        );
        let (pending_clip, duration_ms) = controller
            .take_ready_pending()
            .expect("pending Take should complete");
        assert_eq!(pending_clip, Arc::new(expected));
        assert_eq!(duration_ms, 450);
        assert!(controller.take_ready_pending().is_none());
    }

    #[test]
    fn controller_replaces_a_changed_resource_and_keeps_failures_typed() {
        let format = format();
        let mut controller = StingerLoadController::with_loader(
            |_request: &StingerLoadRequest, _cancellation: &StingerLoadCancellation| {
                Err(MediaError::StingerResource {
                    failure: StingerResourceFailure::DecoderUnavailable,
                })
            },
            format,
        )
        .expect("controller");
        let first = spec("assets/first.webm", true);
        assert_eq!(
            controller.sync_spec(Some(&first), format).expect("sync"),
            Some(StingerLoadEvent::Requested)
        );
        assert_eq!(
            wait_for_event(&mut controller, &first, format),
            StingerLoadEvent::Failed(MediaError::StingerResource {
                failure: StingerResourceFailure::DecoderUnavailable,
            })
        );
        let second = spec("assets/second.webm", true);
        assert_eq!(
            controller.sync_spec(Some(&second), format).expect("sync"),
            Some(StingerLoadEvent::Requested)
        );
        assert!(matches!(
            controller.state(),
            StingerLoadState::Loading { spec, .. } if spec == &second
        ));
    }
}
