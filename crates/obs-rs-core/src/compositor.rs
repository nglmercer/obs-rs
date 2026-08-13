use obs_rs_media::{FrameTransform, FrameTransition, MediaError, VideoFrame};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_util::Identifier;
use std::time::Instant;

use super::{
    error::{identifier, RuntimeError},
    ids::SourceId,
    metrics::CompositorMetrics,
    runtime::Runtime,
};

impl Runtime {
    /// Renders one scene in item order using the CPU reference compositor.
    ///
    /// # Concurrency
    ///
    /// Rendering is serialized by design, and the `&mut self` receiver states
    /// that guarantee rather than merely reflecting an implementation detail.
    /// A source is a stateful device — a capture node advances its buffer, a
    /// decoder its position — so `render` takes `&mut` on the source itself,
    /// and two scenes sharing a source could not be composited concurrently
    /// without changing what a source is. Scene compositing is therefore one
    /// scene at a time; within a scene, the per-frame pixel work is what
    /// parallelizes, across rayon blocks inside `obs-rs-media`.
    ///
    /// Rendering independent scenes in parallel would require per-source
    /// interior mutability and a defined answer for shared sources; that is a
    /// deliberate future change, not something a caller can assume today.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a scene or source is missing, a source
    /// rejects the request, or a frame violates the media format contract.
    pub fn render_scene(
        &mut self,
        scene: &str,
        request: &VideoRequest,
    ) -> Result<Option<VideoFrame>, RuntimeError> {
        self.metrics.render_calls = self.metrics.render_calls.saturating_add(1);
        let scene = identifier(scene, "scene")?;
        let (scenes, sources, metrics) = (&self.scenes, &mut self.sources, &mut self.metrics);
        let scene_state = scenes
            .get(&scene)
            .ok_or_else(|| RuntimeError::UnknownScene(scene.clone()))?;
        let mut result: Option<VideoFrame> = None;

        for source_id in &scene_state.sources {
            // One lookup resolves both the transform and the filter chain, and
            // yields a plain slice with no function-pointer indirection.
            let (transform, filters) = scene_state
                .items
                .get(source_id)
                .map_or((FrameTransform::IDENTITY, &[][..]), |item| {
                    (item.transform, item.filters.as_slice())
                });
            metrics.source_requests = metrics.source_requests.saturating_add(1);
            let instance = sources
                .get_mut(source_id)
                .ok_or(RuntimeError::UnknownSource(*source_id))?;
            let capture_started = Instant::now();
            let frame = instance
                .source
                .render(request)
                .map_err(RuntimeError::Source)?;
            metrics.capture_latency.record(capture_started.elapsed());
            let Some(frame) = frame else {
                metrics.empty_sources = metrics.empty_sources.saturating_add(1);
                continue;
            };
            metrics.source_frames = metrics.source_frames.saturating_add(1);
            if frame.format() != request.format() {
                return Err(RuntimeError::Media(MediaError::FormatMismatch {
                    expected: request.format(),
                    actual: frame.format(),
                }));
            }
            if transform != FrameTransform::IDENTITY {
                metrics.transformed_frames = metrics.transformed_frames.saturating_add(1);
            }
            let mut frame = if transform == FrameTransform::IDENTITY {
                frame
            } else {
                frame
                    .into_transformed(transform)
                    .map_err(RuntimeError::Media)?
            };
            metrics.filtered_frames = metrics
                .filtered_frames
                .saturating_add(u64::try_from(filters.len()).unwrap_or(u64::MAX));
            frame.apply_filters(filters);

            if let Some(composite) = result.take() {
                frame.blend_under(&composite).map_err(RuntimeError::Media)?;
                result = Some(frame);
                metrics.blended_layers = metrics.blended_layers.saturating_add(1);
            } else {
                frame.clear_transparent_rgb();
                result = Some(frame);
            }
        }

        Ok(result)
    }

    /// Renders a transition from one scene to another using the same source
    /// lifecycle and media validation as ordinary scene rendering.
    ///
    /// A cross-fade treats a missing scene frame as a transparent frame, which
    /// allows a scene to fade in or out without a special source implementation.
    /// A cut returns the destination frame directly.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when either scene or one of its sources fails, or
    /// when the transition violates the media format contract.
    pub fn render_scene_transition(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        request: &VideoRequest,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, RuntimeError> {
        let source = self.render_scene(source_scene, request)?;
        let destination = self.render_scene(destination_scene, request)?;

        if matches!(transition, FrameTransition::Cut) {
            return Ok(destination);
        }

        if source.is_none() && destination.is_none() {
            return Ok(None);
        }

        let source = source.unwrap_or_else(|| {
            VideoFrame::solid(request.format(), request.timestamp(), [0, 0, 0, 0])
        });
        let destination = destination.unwrap_or_else(|| {
            VideoFrame::solid(request.format(), request.timestamp(), [0, 0, 0, 0])
        });
        VideoFrame::transitioned(&source, destination, transition)
            .map(Some)
            .map_err(RuntimeError::Media)
    }

    #[must_use]
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Returns the number of named scenes.
    #[must_use]
    pub fn scene_count(&self) -> usize {
        self.scenes.len()
    }

    /// Returns accumulated compositor and source-work metrics.
    #[must_use]
    pub const fn compositor_metrics(&self) -> CompositorMetrics {
        self.metrics
    }

    /// Clears compositor counters without changing runtime-owned sources or scenes.
    pub fn reset_compositor_metrics(&mut self) {
        self.metrics = CompositorMetrics::default();
    }

    /// Returns source metadata for diagnostics.
    #[must_use]
    pub fn source_info(&self, source: SourceId) -> Option<(&str, &str)> {
        self.sources
            .get(&source)
            .map(|instance| (instance.kind.as_str(), instance.name.as_str()))
    }

    /// Returns the source IDs in scene order.
    #[must_use]
    pub fn scene_sources(&self, scene: &str) -> Option<&[SourceId]> {
        let scene = Identifier::new(scene).ok()?;
        self.scenes
            .get(&scene)
            .map(|value| value.sources.as_slice())
    }
}
