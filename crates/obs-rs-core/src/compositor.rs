use obs_rs_media::{FrameFilter, FrameTransform, FrameTransition, MediaError, VideoFrame};
use obs_rs_plugin_api::{SourceError, VideoRequest};
use obs_rs_util::Identifier;
use std::time::Instant;

use super::{
    error::{identifier, RuntimeError},
    ids::SourceId,
    metrics::CompositorMetrics,
    registry::SourceInstance,
    runtime::Runtime,
};

/// Returns whether a source failure means a broken contract, not a broken device.
const fn is_contract_violation(error: &SourceError) -> bool {
    matches!(
        error,
        SourceError::InvalidSetting { .. } | SourceError::UnsupportedFormat { .. }
    )
}

/// Captures one shared source and resolves its last-good-frame fallback.
///
/// Source failures are deliberately isolated here rather than returned to the
/// scene loop: one disconnected device must not blank healthy layers. The
/// caller owns the scene-item metrics because a cached frame can serve several
/// items while this function records one actual source render.
fn render_source_frame(
    instance: &mut SourceInstance,
    request: &VideoRequest,
    metrics: &mut CompositorMetrics,
) -> (Option<VideoFrame>, Vec<FrameFilter>) {
    let configured_filters = instance.filters.clone();
    let mut filters = Vec::with_capacity(configured_filters.len());
    let mut render_delay = None;
    for filter in configured_filters {
        match filter {
            FrameFilter::RenderDelay(delay) => {
                if render_delay.is_some() {
                    instance.failure = Some(
                        "multiple Render Delay filters on one source are not supported".into(),
                    );
                    metrics.failed_sources = metrics.failed_sources.saturating_add(1);
                    return (None, filters);
                }
                render_delay = Some(delay.milliseconds);
            }
            filter => filters.push(filter),
        }
    }
    let render_delay = render_delay.unwrap_or_default();
    if let Err(error) = instance.render_delay.set_milliseconds(render_delay) {
        instance.failure = Some(format!("Render Delay: {error}"));
        metrics.failed_sources = metrics.failed_sources.saturating_add(1);
        return (None, filters);
    }
    let capture_started = Instant::now();
    let rendered = instance.source.render(request);
    metrics.capture_latency.record(capture_started.elapsed());
    // One failing source must not erase the rest of the scene. A camera that
    // was unplugged mid-stream, a portal session the compositor closed —
    // neither is a reason to stop compositing a healthy layer beside it.
    let frame = match rendered {
        Ok(frame) => {
            if frame.is_some() {
                instance.failure = None;
            }
            frame
        }
        Err(error) => {
            metrics.failed_sources = metrics.failed_sources.saturating_add(1);
            if is_contract_violation(&error) {
                metrics.contract_violations = metrics.contract_violations.saturating_add(1);
            }
            instance.failure = Some(error.to_string());
            None
        }
    };
    let frame = match frame {
        Some(frame) if frame.format() == request.format() => {
            instance.last_frame = Some(frame.clone());
            Some(frame)
        }
        // A frame in the wrong shape cannot be composited, and it is the
        // source's contract that was broken, not the scene's.
        Some(frame) => {
            metrics.failed_sources = metrics.failed_sources.saturating_add(1);
            metrics.contract_violations = metrics.contract_violations.saturating_add(1);
            instance.failure = Some(
                RuntimeError::Media(MediaError::FormatMismatch {
                    expected: request.format(),
                    actual: frame.format(),
                })
                .to_string(),
            );
            None
        }
        None => None,
    };
    let frame = frame.or_else(|| {
        instance
            .last_frame
            .as_ref()
            .filter(|frame| frame.format() == request.format())
            .map(|frame| frame.at_timestamp(request.timestamp()))
    });
    let Some(frame) = frame else {
        return (None, filters);
    };
    if render_delay == 0 {
        return (Some(frame), filters);
    }
    match instance.render_delay.push(frame) {
        Ok(frame) => (frame, filters),
        Err(error) => {
            instance.failure = Some(format!("Render Delay: {error}"));
            metrics.failed_sources = metrics.failed_sources.saturating_add(1);
            (None, filters)
        }
    }
}

/// One captured scene layer before compositor-specific pixel processing.
///
/// This is the portable handoff used by accelerated compositors: sources stay
/// owned by [`Runtime`], while the returned RGBA frame and its scene metadata
/// can be uploaded without first performing CPU transforms, filters, or blends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedSceneLayer {
    item_id: std::sync::Arc<str>,
    source: SourceId,
    frame: VideoFrame,
    transform: FrameTransform,
    filters: Vec<FrameFilter>,
}

impl RenderedSceneLayer {
    /// Returns the stable scene-item path that produced this layer.
    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    /// Returns the captured source frame.
    #[must_use]
    pub const fn frame(&self) -> &VideoFrame {
        &self.frame
    }

    /// Returns the scene transform associated with the frame.
    #[must_use]
    pub const fn transform(&self) -> FrameTransform {
        self.transform
    }

    /// Returns the ordered filter chain associated with the frame.
    #[must_use]
    pub fn filters(&self) -> &[FrameFilter] {
        &self.filters
    }
}

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
        let layers = self.render_scene_layers(scene, request)?;
        let mut result: Option<VideoFrame> = None;

        for layer in layers {
            let mut frame = if layer.transform == FrameTransform::IDENTITY {
                layer.frame
            } else {
                layer
                    .frame
                    .into_transformed(layer.transform)
                    .map_err(RuntimeError::Media)?
            };
            frame.apply_filters(&layer.filters);

            if let Some(composite) = result.take() {
                frame.blend_under(&composite).map_err(RuntimeError::Media)?;
                result = Some(frame);
                self.metrics.blended_layers = self.metrics.blended_layers.saturating_add(1);
            } else {
                frame.clear_transparent_rgb();
                result = Some(frame);
            }
        }

        Ok(result)
    }

    /// Captures the ordered source frames and scene metadata without applying
    /// CPU transforms, filters, or alpha blending.
    ///
    /// Accelerated adapters use this boundary to upload source frames once and
    /// perform composition on their selected device. Calling this method counts
    /// as one scene render just like [`Self::render_scene`].
    ///
    /// # Source failures
    ///
    /// A source that fails is skipped, not fatal. Its error is recorded on the
    /// instance — readable through [`Runtime::source_failures`] — counted in
    /// [`CompositorMetrics::failed_sources`], and the layer falls back to that
    /// source's last good frame if it has one. A live compositor that stopped
    /// because one of its inputs did would take a whole broadcast off the air
    /// for one unplugged webcam.
    ///
    /// Failures that mean a broken contract rather than an absent device are
    /// additionally counted in [`CompositorMetrics::contract_violations`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the scene itself is missing or names a
    /// source this runtime does not have. Source-reported errors do not appear
    /// here; see above.
    pub fn render_scene_layers(
        &mut self,
        scene: &str,
        request: &VideoRequest,
    ) -> Result<Vec<RenderedSceneLayer>, RuntimeError> {
        self.metrics.render_calls = self.metrics.render_calls.saturating_add(1);
        let scene = identifier(scene, "scene")?;
        let (scenes, sources, metrics) = (&self.scenes, &mut self.sources, &mut self.metrics);
        let scene_state = scenes
            .get(&scene)
            .ok_or_else(|| RuntimeError::UnknownScene(scene.clone()))?;
        let mut result: Vec<RenderedSceneLayer> = Vec::with_capacity(scene_state.sources.len());
        // A successful layer is also the per-render cache for later scene
        // items that reference the same source. This reuses the existing
        // bounded result storage and avoids a second hot-path allocation.
        // Empty sources are tracked only when a failure actually occurs.
        let mut empty_sources = Vec::new();

        for (item_index, source_id) in scene_state.sources.iter().enumerate() {
            // The scene lookup resolves item-only state. Source filters are
            // read from the shared source instance below, so every scene item
            // referencing that source observes the same filter chain.
            let Some(item) = scene_state.items.get(item_index) else {
                continue;
            };
            let transform = item.transform;
            metrics.source_requests = metrics.source_requests.saturating_add(1);
            if let Some(previous) = result.iter().find(|layer| layer.source == *source_id) {
                let frame = previous.frame.clone();
                let filters = previous.filters.clone();
                metrics.source_frames = metrics.source_frames.saturating_add(1);
                if transform != FrameTransform::IDENTITY {
                    metrics.transformed_frames = metrics.transformed_frames.saturating_add(1);
                }
                metrics.filtered_frames = metrics
                    .filtered_frames
                    .saturating_add(u64::try_from(filters.len()).unwrap_or(u64::MAX));
                result.push(RenderedSceneLayer {
                    item_id: std::sync::Arc::clone(&item.item_id),
                    source: *source_id,
                    frame,
                    transform,
                    filters,
                });
                continue;
            }
            if empty_sources.contains(source_id) {
                metrics.empty_sources = metrics.empty_sources.saturating_add(1);
                continue;
            }
            let instance = sources
                .get_mut(source_id)
                .ok_or(RuntimeError::UnknownSource(*source_id))?;
            let (frame, filters) = render_source_frame(instance, request, metrics);
            let Some(frame) = frame else {
                empty_sources.push(*source_id);
                metrics.empty_sources = metrics.empty_sources.saturating_add(1);
                continue;
            };
            metrics.source_frames = metrics.source_frames.saturating_add(1);
            if transform != FrameTransform::IDENTITY {
                metrics.transformed_frames = metrics.transformed_frames.saturating_add(1);
            }
            metrics.filtered_frames = metrics
                .filtered_frames
                .saturating_add(u64::try_from(filters.len()).unwrap_or(u64::MAX));
            result.push(RenderedSceneLayer {
                item_id: std::sync::Arc::clone(&item.item_id),
                source: *source_id,
                frame,
                transform,
                filters,
            });
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

    /// Returns the current failure reported by each source, in no order.
    ///
    /// Source failures are isolated by the compositor, so this is how a caller
    /// learns that a layer is stale: the scene still renders, but one of its
    /// sources is not delivering.
    #[must_use]
    pub fn source_failures(&self) -> Vec<(SourceId, &str)> {
        self.sources
            .iter()
            .filter_map(|(id, instance)| instance.failure.as_deref().map(|failure| (*id, failure)))
            .collect()
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
