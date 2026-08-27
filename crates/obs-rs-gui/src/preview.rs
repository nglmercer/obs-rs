use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error,
    sync::Arc,
};

use obs_rs_config::Config;
use obs_rs_core::{
    CompositorMetrics, RenderedSceneLayer, Runtime, RuntimeLimits, RuntimeUsage, SourceId,
};
use obs_rs_engine::{compile_filter_report, FilterCompilation, MAX_FILTER_DIAGNOSTICS};
use obs_rs_media::{FrameScaler, FrameTransform, FrameTransition, ScaleFilter};
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{Profile, Project, SourceSpec};
use obs_rs_render::{RenderBackend, RenderTarget, RenderTargetRole, SceneLayer};

mod compositor;
mod presenter;
#[path = "preview_render.rs"]
mod preview_render;
#[path = "preview_sync.rs"]
mod preview_sync;
mod support;
mod surface;

use compositor::PreviewCompositor;
pub(crate) use presenter::frame_to_image;
pub(crate) use support::builtin_source_kinds;
use support::{builtin_plugin, empty_project, static_scenes};
pub(crate) use surface::PreviewSurface;

pub(crate) struct PreviewRenderer {
    pub(crate) format: VideoFormat,
    pub(crate) runtime: Runtime,
    timestamp: Timestamp,
    /// Project revision this renderer was built from.
    ///
    /// Change detection compares this integer against the session's current
    /// revision. It used to serialize the whole project on every frame and
    /// compare the resulting strings.
    revision: u64,
    /// The project the runtime currently mirrors.
    ///
    /// Kept so the next revision can be applied as a diff. Without it, the only
    /// way to answer "what changed?" is to rebuild everything, which for a live
    /// studio means closing and reopening every camera and screen-cast session
    /// because someone nudged a source.
    applied: Project,
    /// Project source ID to the live runtime source that implements it.
    source_ids: HashMap<String, SourceId>,
    /// Bounded persisted filters unavailable in the preview runtime.
    filter_diagnostics: Vec<String>,
    /// Scenes that currently exist in the runtime.
    scene_ids: HashSet<String>,
    /// Scenes made exclusively from solid color sources do not change between
    /// frames. Their first composed pixel buffer is reused while fresh frame
    /// timestamps are issued to the output encoder.
    static_scenes: HashSet<String>,
    static_frames: HashMap<String, Arc<Vec<u8>>>,
    static_preview_frames: HashMap<(String, VideoFormat), Arc<Vec<u8>>>,
    /// Source-layer fan-out cache for the current render tick. The capacity
    /// is deliberately small and oldest-first; it is a sharing boundary, not
    /// a second unbounded frame queue.
    scene_layer_cache: VecDeque<CachedSceneLayers>,
    compositor: PreviewCompositor,
    gpu_program_scene: Option<String>,
    preview_scaler: Option<FrameScaler>,
    /// The canvas drag currently applied to the runtime, if any.
    applied_draft: Option<(String, Vec<(String, FrameTransform)>)>,
}

type VisibleItem = (String, SourceId, FrameTransform);

/// One bounded snapshot of a scene's source frames.
///
/// A render request can feed preview, program, projector, and multiview
/// targets at the same timestamp. Capturing a source once and reusing its
/// immutable frame for those targets avoids advancing a capture source and
/// acquiring/uploading a duplicate CPU frame for every consumer.
struct CachedSceneLayers {
    scene: String,
    timestamp: Timestamp,
    layers: Vec<RenderedSceneLayer>,
}

/// One scene-item transform being dragged on the canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TransformDraftItem {
    pub(crate) item: String,
    /// Effective profile-canvas transform used by the live overlay and
    /// renderer while the gesture is in progress.
    pub(crate) transform: FrameTransform,
    /// Transform contributed by enclosing groups. The commit boundary uses it
    /// to convert the effective draft back into local project coordinates.
    pub(crate) parent_transform: FrameTransform,
}

/// Scene-item transforms being dragged on the canvas.
///
/// A drag is not a project edit until the pointer is released, so it reaches
/// the compositor through this side channel instead of through a project
/// revision. That is what keeps a drag from churning the undo history — and,
/// before the runtime learned to update incrementally, from restarting every
/// capture device in the scene on every mouse move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TransformDraft {
    pub(crate) scene: String,
    pub(crate) items: Vec<TransformDraftItem>,
}

/// A snapshot of engine state the studio window can read without a runtime.
#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeDiagnostics {
    pub(crate) metrics: CompositorMetrics,
    pub(crate) usage: RuntimeUsage,
    pub(crate) limits: RuntimeLimits,
    /// GPU adapter selected by the compositor, or the CPU fallback label.
    pub(crate) gpu_adapter: String,
    /// WGPU backend name, or the CPU fallback reason.
    pub(crate) gpu_backend: String,
    /// One line per source that is currently failing.
    pub(crate) failures: Vec<String>,
    /// Persisted filters unavailable in the preview runtime.
    pub(crate) filter_diagnostics: Vec<String>,
}
