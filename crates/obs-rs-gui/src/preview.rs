use std::{
    collections::{HashMap, HashSet},
    error::Error,
    rc::Rc,
    sync::{Arc, Mutex},
};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_plugin_api::Plugin;
use obs_rs_core::{CompositorMetrics, Runtime, RuntimeLimits, RuntimeUsage, SourceId};
use obs_rs_engine::compile_filter;
#[cfg(test)]
use obs_rs_media::FrameTransition;
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_media::FrameTransform;
use obs_rs_project::{Profile, Project, SceneItemSpec, SourceSpec};
use obs_rs_render::{
    GpuFrameHandle, GpuPlaneHandle, RenderBackend, SceneLayer, TextureId, VideoSurface,
};
use obs_rs_render_wgpu::WgpuRenderBackend;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

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
    /// Scenes that currently exist in the runtime.
    scene_ids: HashSet<String>,
    /// Scenes made exclusively from solid color sources do not change between
    /// frames. Their first composed pixel buffer is reused while fresh frame
    /// timestamps are issued to the output encoder.
    static_scenes: HashSet<String>,
    static_frames: HashMap<String, Vec<u8>>,
    compositor: PreviewCompositor,
    gpu_scene: Option<String>,
    /// The canvas drag currently applied to the runtime, if any.
    applied_draft: Option<(String, SourceId)>,
}

/// A scene-item transform being dragged on the canvas.
///
/// A drag is not a project edit until the pointer is released, so it reaches
/// the compositor through this side channel instead of through a project
/// revision. That is what keeps a drag from churning the undo history — and,
/// before the runtime learned to update incrementally, from restarting every
/// capture device in the scene on every mouse move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TransformDraft {
    pub(crate) scene: String,
    pub(crate) item: String,
    pub(crate) transform: FrameTransform,
}

/// A snapshot of engine state the studio window can read without a runtime.
#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeDiagnostics {
    pub(crate) metrics: CompositorMetrics,
    pub(crate) usage: RuntimeUsage,
    pub(crate) limits: RuntimeLimits,
    /// One line per source that is currently failing.
    pub(crate) failures: Vec<String>,
}

/// The studio window's non-live view of the engine.
///
/// The window used to hold a second [`PreviewRenderer`], which meant a second
/// [`Runtime`], which meant every camera and screen-cast session in the project
/// was opened twice — once for the window that never rendered a frame from it,
/// and once for the worker that actually composites. Cameras in particular do
/// not survive being opened twice. The window needs the canvas format, the
/// revision it has observed, and engine counters, so that is all this carries;
/// the worker owns the only live runtime.
pub(crate) struct PreviewSurface {
    pub(crate) format: VideoFormat,
    revision: u64,
    diagnostics: Arc<Mutex<RuntimeDiagnostics>>,
}

impl PreviewSurface {
    /// Creates the window's view of `project` without opening any device.
    pub(crate) fn new(project: &Project, revision: u64) -> Result<Self, Box<dyn Error>> {
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        Ok(Self {
            format: profile.video_format(),
            revision,
            diagnostics: Arc::new(Mutex::new(RuntimeDiagnostics::default())),
        })
    }

    /// Returns the slot the preview worker publishes engine counters into.
    pub(crate) fn diagnostics_handle(&self) -> Arc<Mutex<RuntimeDiagnostics>> {
        Arc::clone(&self.diagnostics)
    }

    /// Returns the newest engine snapshot the worker published.
    pub(crate) fn diagnostics(&self) -> RuntimeDiagnostics {
        self.diagnostics
            .lock()
            .map_or_else(|_| RuntimeDiagnostics::default(), |value| value.clone())
    }

    pub(crate) const fn is_synced(&self, revision: u64) -> bool {
        self.revision == revision
    }

    /// Records a new project revision and the canvas it renders at.
    ///
    /// Nothing here touches a device: the worker's runtime is the only thing
    /// that opens capture hardware.
    pub(crate) fn sync_project(
        &mut self,
        project: &Project,
        revision: u64,
    ) -> Result<bool, Box<dyn Error>> {
        if revision == self.revision {
            return Ok(false);
        }
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        self.format = profile.video_format();
        self.revision = revision;
        Ok(true)
    }
}

enum PreviewCompositor {
    Wgpu {
        backend: Box<WgpuRenderBackend>,
        target: TextureId,
    },
    Cpu {
        reason: Option<String>,
    },
}

impl PreviewCompositor {
    fn new(format: VideoFormat) -> Self {
        let texture_budget = format.rgba_bytes().saturating_mul(12);
        match WgpuRenderBackend::new(12, texture_budget) {
            Ok(mut backend) => match backend.create_texture(format) {
                Ok(target) => Self::Wgpu {
                    backend: Box::new(backend),
                    target,
                },
                Err(error) => Self::Cpu {
                    reason: Some(format!("target allocation failed: {error}")),
                },
            },
            Err(error) => Self::Cpu {
                reason: Some(error.to_string()),
            },
        }
    }
}

thread_local! {
    /// The builtin plugin, constructed once per thread.
    ///
    /// Rebuilding the renderer used to recreate the plugin and all of its
    /// factory objects; the plugin is immutable, so one instance is shared.
    static BUILTIN_PLUGIN: Rc<BuiltinPlugin> = Rc::new(
        BuiltinPlugin::new().unwrap_or_else(|error| {
            unreachable!("builtin plugin manifest is valid: {error}")
        }),
    );
}

impl PreviewRenderer {
    pub(crate) fn new(project: &Project, revision: u64) -> Result<Self, Box<dyn Error>> {
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        let format = profile.video_format();
        let mut runtime = Runtime::new();
        let plugin = BUILTIN_PLUGIN.with(Rc::clone);
        runtime.register_plugin(plugin.as_ref())?;

        let mut renderer = Self {
            format,
            runtime,
            timestamp: Timestamp::ZERO,
            revision,
            applied: empty_project(project),
            source_ids: HashMap::new(),
            scene_ids: HashSet::new(),
            static_scenes: HashSet::new(),
            static_frames: HashMap::new(),
            compositor: PreviewCompositor::new(format),
            gpu_scene: None,
            applied_draft: None,
        };
        // Building from an empty mirror of the same profile makes the first
        // build and every later update the same code path, so there is exactly
        // one description of how a project becomes runtime state.
        renderer.apply_profile(project)?;
        renderer.applied = project.clone();
        Ok(renderer)
    }

    /// Brings the runtime in line with `project` without recreating sources.
    ///
    /// Returns whether anything was applied, so the caller can skip the UI work
    /// that depends on project content.
    ///
    /// Moving, hiding, reordering, renaming, or filtering a source is a scene
    /// graph edit. Only a changed canvas, a changed profile, or changed source
    /// settings can reach the capture devices, and even then only the sources
    /// that actually changed.
    pub(crate) fn sync_project(
        &mut self,
        project: &Project,
        revision: u64,
    ) -> Result<bool, Box<dyn Error>> {
        if revision == self.revision {
            return Ok(false);
        }
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        // A different canvas means every source renegotiates its output shape,
        // and a different profile is a different scene collection entirely.
        // Those are the two cases a diff cannot express.
        let rebuild = project.active_profile() != self.applied.active_profile()
            || profile.video_format() != self.format
            || self.kind_changed(profile);
        if rebuild {
            *self = Self::new(project, revision)?;
            return Ok(true);
        }
        self.apply_profile(project)?;
        self.applied = project.clone();
        self.revision = revision;
        // The draft is expressed against project state that has just moved, so
        // it is re-applied from scratch on the next render.
        self.applied_draft = None;
        Ok(true)
    }

    /// Returns whether a live source changed its kind, which a diff cannot do.
    fn kind_changed(&self, profile: &Profile) -> bool {
        let Some(applied) = self.applied.active_profile_spec() else {
            return true;
        };
        profile.sources().any(|source| {
            applied
                .source(source.id())
                .is_some_and(|previous| previous.kind() != source.kind())
        })
    }

    /// Applies the difference between the mirrored project and `project`.
    fn apply_profile(&mut self, project: &Project) -> Result<(), Box<dyn Error>> {
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        // The mirrored profile is cloned out first: `sync_source` needs `&mut
        // self`, and the borrow checker will not hold a reference into
        // `self.applied` across it.
        let previous = self
            .applied
            .active_profile_spec()
            .filter(|_| self.applied.active_profile() == project.active_profile())
            .map(|profile| {
                profile
                    .sources()
                    .map(|source| (source.id().as_str().to_owned(), source.clone()))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        for source in profile.sources() {
            self.sync_source(source, previous.get(source.id().as_str()))?;
        }
        self.sync_scenes(profile)?;
        self.retire_sources(profile)?;

        self.static_scenes = static_scenes(profile);
        // Cached still frames describe scene content that may have just moved.
        self.static_frames.clear();
        Ok(())
    }

    /// Creates or updates one source without disturbing the others.
    fn sync_source(
        &mut self,
        source: &SourceSpec,
        previous: Option<&SourceSpec>,
    ) -> Result<(), Box<dyn Error>> {
        let Some(&id) = self.source_ids.get(source.id().as_str()) else {
            let id =
                self.runtime
                    .create_source(source.kind().as_str(), source.name(), source.settings())?;
            self.apply_filters(id, source)?;
            self.source_ids.insert(source.id().as_str().to_owned(), id);
            return Ok(());
        };
        let previous = previous.ok_or_else(|| {
            std::io::Error::other(format!("source {} has no mirrored state", source.id()))
        })?;
        // Only settings can reach the device, so only settings restart it.
        if previous.settings() != source.settings() {
            self.runtime.update_source(id, source.settings())?;
        }
        if previous.name() != source.name() {
            self.runtime.rename_source(id, source.name())?;
        }
        if previous.filters() != source.filters() {
            self.runtime.clear_source_filters(id)?;
            self.apply_filters(id, source)?;
        }
        Ok(())
    }

    fn apply_filters(&mut self, id: SourceId, source: &SourceSpec) -> Result<(), Box<dyn Error>> {
        for filter in source.filters() {
            if let Some(runtime_filter) = compile_filter(filter) {
                self.runtime.add_source_filter(id, runtime_filter)?;
            }
        }
        Ok(())
    }

    /// Rebuilds every scene's composition order in place.
    fn sync_scenes(&mut self, profile: &Profile) -> Result<(), Box<dyn Error>> {
        let live = profile
            .scenes()
            .map(|scene| scene.id().as_str().to_owned())
            .collect::<HashSet<_>>();
        for scene in self.scene_ids.clone() {
            if !live.contains(&scene) {
                self.runtime.destroy_scene(&scene)?;
                self.scene_ids.remove(&scene);
            }
        }

        for scene in profile.scenes() {
            let name = scene.id().as_str();
            if self.scene_ids.insert(name.to_owned()) {
                self.runtime.create_scene(name)?;
            }
            let order = self.visible_order(scene.items())?;
            let attached = self
                .runtime
                .scene_sources(name)
                .map(<[SourceId]>::to_vec)
                .unwrap_or_default();
            for source in &attached {
                if !order.contains(source) {
                    self.runtime.detach_source(name, *source)?;
                }
            }
            for source in &order {
                if !attached.contains(source) {
                    self.runtime.attach_source(name, *source)?;
                }
            }
            self.runtime.set_scene_order(name, &order)?;
            for item in scene.items() {
                if let Some(&source) = self.source_ids.get(item.source_id().as_str()) {
                    if order.contains(&source) {
                        self.runtime
                            .set_source_transform(name, source, item.transform())?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Resolves a scene's visible items to runtime sources, in draw order.
    ///
    /// A runtime scene attaches each source once, so two items pointing at the
    /// same source collapse to the first of them rather than failing the sync.
    fn visible_order(&self, items: &[SceneItemSpec]) -> Result<Vec<SourceId>, Box<dyn Error>> {
        let mut order = Vec::with_capacity(items.len());
        for item in items.iter().filter(|item| item.visible()) {
            let source = self
                .source_ids
                .get(item.source_id().as_str())
                .copied()
                .ok_or_else(|| {
                    std::io::Error::other(format!(
                        "scene item {} references unknown source {}",
                        item.id(),
                        item.source_id()
                    ))
                })?;
            if !order.contains(&source) {
                order.push(source);
            }
        }
        Ok(order)
    }

    /// Destroys sources the project no longer defines.
    fn retire_sources(&mut self, profile: &Profile) -> Result<(), Box<dyn Error>> {
        let removed = self
            .source_ids
            .iter()
            .filter(|(id, _)| !profile.has_source(id.as_str()))
            .map(|(id, source)| (id.clone(), *source))
            .collect::<Vec<_>>();
        for (project_id, source) in removed {
            self.runtime.destroy_source(source)?;
            self.source_ids.remove(&project_id);
        }
        Ok(())
    }

    /// Applies, replaces, or withdraws the canvas drag's transform.
    ///
    /// The runtime holds the dragged transform only while the pointer is down;
    /// letting go restores whatever the project says, which is either the
    /// committed drag or the untouched original if the drag was abandoned.
    pub(crate) fn set_transform_draft(&mut self, draft: Option<&TransformDraft>) {
        let target = draft.and_then(|draft| {
            let source = self
                .applied
                .active_profile_spec()?
                .scene(draft.scene.as_str())?
                .item(draft.item.as_str())?
                .source_id()
                .as_str()
                .to_owned();
            Some((draft.scene.clone(), *self.source_ids.get(&source)?))
        });
        if let Some((scene, source)) = self.applied_draft.clone() {
            if target.as_ref().map(|(scene, source)| (scene.as_str(), *source))
                != Some((scene.as_str(), source))
            {
                let committed = self
                    .applied
                    .active_profile_spec()
                    .and_then(|profile| profile.scene(scene.as_str()))
                    .and_then(|scene| {
                        scene
                            .items()
                            .iter()
                            .find(|item| {
                                self.source_ids.get(item.source_id().as_str()) == Some(&source)
                            })
                            .map(SceneItemSpec::transform)
                    })
                    .unwrap_or(FrameTransform::IDENTITY);
                let _ = self
                    .runtime
                    .set_source_transform(&scene, source, committed);
                // A scene composed only of still sources caches its picture, so
                // the cache has to go when the drag stops moving it.
                self.static_frames.remove(&scene);
                self.applied_draft = None;
            }
        }
        let (Some(draft), Some((scene, source))) = (draft, target) else {
            return;
        };
        if self
            .runtime
            .set_source_transform(&scene, source, draft.transform)
            .is_ok()
        {
            self.static_frames.remove(&scene);
            self.applied_draft = Some((scene, source));
        }
    }

    /// Returns the engine snapshot the studio window shows.
    pub(crate) fn diagnostics(&self) -> RuntimeDiagnostics {
        RuntimeDiagnostics {
            metrics: self.runtime.compositor_metrics(),
            usage: self.runtime.usage(),
            limits: self.runtime.limits(),
            failures: self
                .runtime
                .source_failures()
                .into_iter()
                .map(|(source, failure)| {
                    let name = self
                        .runtime
                        .source_info(source)
                        .map_or_else(String::new, |(_, name)| name.to_owned());
                    format!("{name}: {failure}")
                })
                .collect(),
        }
    }

    pub(crate) fn render(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let frame = if let Some(pixels) = self.static_frames.get(scene) {
            self.gpu_scene = None;
            Some(VideoFrame::new(
                self.format,
                self.timestamp,
                pixels.clone(),
            )?)
        } else {
            let frame = self.render_live_scene(scene)?;
            if self.static_scenes.contains(scene) {
                if let Some(frame) = frame.as_ref() {
                    self.static_frames
                        .insert(scene.to_owned(), frame.pixels().to_vec());
                }
            }
            frame
        };
        self.advance_timestamp();
        Ok(frame)
    }

    fn render_live_scene(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        match &mut self.compositor {
            PreviewCompositor::Wgpu { backend, target } => {
                let layers = self.runtime.render_scene_layers(scene, &request)?;
                if layers.is_empty() {
                    return Ok(None);
                }
                let submitted = layers
                    .iter()
                    .map(|layer| {
                        SceneLayer::frame(layer.frame(), layer.transform(), layer.filters())
                    })
                    .collect::<Vec<_>>();
                if let Err(error) = backend.submit_layers(*target, &submitted) {
                    self.compositor = PreviewCompositor::Cpu {
                        reason: Some(format!("GPU composition failed: {error}")),
                    };
                    return Err(error.into());
                }
                self.gpu_scene = Some(scene.to_owned());
                let timestamp = layers
                    .last()
                    .map_or(self.timestamp, |layer| layer.frame().timestamp());
                let handle = GpuFrameHandle::new(
                    "obs-rs-wgpu",
                    self.format,
                    obs_rs_media::PixelFormat::Rgba8,
                    timestamp,
                    vec![GpuPlaneHandle::new(
                        *target,
                        self.format.width(),
                        self.format.height(),
                    )],
                )
                .ok_or_else(|| std::io::Error::other("invalid GPU surface descriptor"))?;
                let surface = VideoSurface::Gpu(handle);
                match surface {
                    VideoSurface::Gpu(handle) => backend
                        .readback(handle.planes()[0].texture())
                        .map(Some)
                        .map_err(Into::into),
                    VideoSurface::Cpu(frame) => Ok(Some(frame)),
                }
            }
            PreviewCompositor::Cpu { .. } => {
                self.gpu_scene = None;
                self.runtime
                    .render_scene(scene, &request)
                    .map_err(Into::into)
            }
        }
    }

    /// Produces an encoder-oriented NV12 frame from the current GPU target.
    /// Static/cached frames are uploaded once when the target no longer names
    /// this scene; live GPU composition proceeds directly to conversion.
    pub(crate) fn encoder_frame(
        &mut self,
        scene: &str,
        frame: &VideoFrame,
    ) -> Result<Option<RawVideoFrame>, Box<dyn Error>> {
        let PreviewCompositor::Wgpu { backend, target } = &mut self.compositor else {
            return Ok(None);
        };
        if self.gpu_scene.as_deref() != Some(scene) {
            backend.upload(*target, frame)?;
            self.gpu_scene = Some(scene.to_owned());
        }
        backend.readback_nv12(*target).map(Some).map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn render_transition(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        let frame = self.runtime.render_scene_transition(
            source_scene,
            destination_scene,
            &request,
            transition,
        )?;
        self.advance_timestamp();
        Ok(frame)
    }

    fn advance_timestamp(&mut self) {
        let period = self
            .format
            .frame_rate()
            .period_nanos()
            .unwrap_or(33_333_333);
        self.timestamp = self
            .timestamp
            .checked_add(period)
            .unwrap_or(Timestamp::ZERO);
    }

    pub(crate) fn metrics_summary(&self) -> String {
        let metrics = self.runtime.compositor_metrics();
        let capture = metrics.capture_latency();
        let backend = match &self.compositor {
            PreviewCompositor::Wgpu { backend, .. } => {
                let render = backend.metrics();
                format!(
                    "WGPU uploads={} compositions={} conversions={} readbacks={} gpu={} MiB",
                    render.uploads(),
                    render.compositions(),
                    render.color_conversions(),
                    render.readbacks(),
                    backend.estimated_gpu_bytes() / (1024 * 1024),
                )
            }
            PreviewCompositor::Cpu { reason } => reason.as_ref().map_or_else(
                || "CPU fallback".to_owned(),
                |reason| format!("CPU fallback ({reason})"),
            ),
        };
        format!(
            "Preview work: {backend} · renders={} · source requests={} · frames={} · empty={} · failed={} · transforms={} · filters={} · blends={} · capture p50/p95/p99/max={}/{}/{}/{} µs",
            metrics.render_calls(),
            metrics.source_requests(),
            metrics.source_frames(),
            metrics.empty_sources(),
            metrics.failed_sources(),
            metrics.transformed_frames(),
            metrics.filtered_frames(),
            metrics.blended_layers(),
            capture.percentile_nanos(50) / 1_000,
            capture.percentile_nanos(95) / 1_000,
            capture.percentile_nanos(99) / 1_000,
            capture.max_nanos() / 1_000,
        )
    }
}

/// Returns a project holding only `project`'s active profile identity.
///
/// [`PreviewRenderer::new`] starts from this so the first build runs through
/// the same diff as every later update.
fn empty_project(project: &Project) -> Project {
    let mut empty = Project::new(project.title()).unwrap_or_else(|_| {
        Project::new("obs-rs").unwrap_or_else(|error| unreachable!("default title: {error}"))
    });
    if let Some(profile) = project.active_profile_spec() {
        if let Ok(bare) = Profile::new(
            profile.id().as_str(),
            profile.name(),
            profile.video_format(),
        ) {
            let _ = empty.add_profile(bare);
            let _ = empty.set_active_profile(profile.id().as_str());
        }
    }
    empty
}

/// Returns the scenes whose composed picture cannot change between frames.
fn static_scenes(profile: &Profile) -> HashSet<String> {
    profile
        .scenes()
        .filter(|scene| {
            scene.items().iter().any(SceneItemSpec::visible)
                && scene
                    .items()
                    .iter()
                    .filter(|item| item.visible())
                    .all(|item| {
                        profile
                            .source(item.source_id())
                            .is_some_and(|source| source.kind().as_str() == "color_source")
                    })
        })
        .map(|scene| scene.id().as_str().to_owned())
        .collect()
}

/// Returns the source kinds the builtin plugin registers, in identifier order.
///
/// The Add Source window needs the catalogue, not a live engine; reading it
/// from the plugin keeps the window from having to hold a runtime open.
pub(crate) fn builtin_source_kinds() -> Vec<String> {
    BUILTIN_PLUGIN.with(|plugin| {
        let mut kinds = plugin
            .source_factories()
            .iter()
            .map(|factory| factory.kind().as_str().to_owned())
            .collect::<Vec<_>>();
        kinds.sort();
        kinds
    })
}

pub(crate) fn frame_to_image(frame: &VideoFrame) -> Image {
    let format = frame.format();
    // Slint owns its pixel storage, so one copy out of the engine frame is
    // unavoidable here; `clone_from_slice` performs it as a single block copy.
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        frame.pixels(),
        format.width(),
        format.height(),
    );
    Image::from_rgba8(buffer)
}
