use std::{
    collections::{HashMap, HashSet},
    error::Error,
    rc::Rc,
    sync::{Arc, Mutex},
};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_core::{CompositorMetrics, Runtime, RuntimeLimits, RuntimeUsage, SourceId};
use obs_rs_engine::compile_filter;
use obs_rs_media::{FrameScaler, FrameTransform, FrameTransition, ScaleFilter};
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::Plugin;
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{Profile, Project, SceneItemSpec, SourceSpec};
use obs_rs_render::{RenderBackend, RenderTarget, RenderTargetRole, SceneLayer, TextureId};
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
    static_frames: HashMap<String, Arc<Vec<u8>>>,
    static_preview_frames: HashMap<(String, VideoFormat), Arc<Vec<u8>>>,
    compositor: PreviewCompositor,
    gpu_program_scene: Option<String>,
    preview_scaler: Option<FrameScaler>,
    /// The canvas drag currently applied to the runtime, if any.
    applied_draft: Option<(String, Vec<(String, FrameTransform)>)>,
}

type VisibleItem = (String, SourceId, FrameTransform);

/// One scene-item transform being dragged on the canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TransformDraftItem {
    pub(crate) item: String,
    pub(crate) transform: FrameTransform,
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

struct GpuTarget {
    target: RenderTarget,
    texture: TextureId,
}

struct WgpuCompositor {
    backend: Box<WgpuRenderBackend>,
    targets: HashMap<RenderTargetRole, GpuTarget>,
}

impl WgpuCompositor {
    fn target(&mut self, target: RenderTarget) -> Result<TextureId, Box<dyn Error>> {
        if let Some(existing) = self.targets.get(&target.role()) {
            if existing.target.format() == target.format() {
                return Ok(existing.texture);
            }
        }
        if let Some(previous) = self.targets.remove(&target.role()) {
            self.backend.destroy_texture(previous.texture)?;
        }
        let texture = self.backend.create_texture(target.format())?;
        self.targets
            .insert(target.role(), GpuTarget { target, texture });
        Ok(texture)
    }
}

enum PreviewCompositor {
    Wgpu(WgpuCompositor),
    Cpu { reason: Option<String> },
}

impl PreviewCompositor {
    fn new(format: VideoFormat) -> Self {
        let texture_budget = format.rgba_bytes().saturating_mul(12);
        match WgpuRenderBackend::new(12, texture_budget) {
            Ok(backend) => Self::Wgpu(WgpuCompositor {
                backend: Box::new(backend),
                targets: HashMap::new(),
            }),
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
            static_preview_frames: HashMap::new(),
            compositor: PreviewCompositor::new(format),
            gpu_program_scene: None,
            preview_scaler: None,
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
        self.static_preview_frames.clear();
        // The full program target is retained by the compositor between GUI
        // requests. Any project diff invalidates that GPU-side snapshot too;
        // otherwise an output request for the same scene ID could reuse pixels
        // from before the edit.
        self.gpu_program_scene = None;
        Ok(())
    }

    /// Creates or updates one source without disturbing the others.
    fn sync_source(
        &mut self,
        source: &SourceSpec,
        previous: Option<&SourceSpec>,
    ) -> Result<(), Box<dyn Error>> {
        let Some(&id) = self.source_ids.get(source.id().as_str()) else {
            let id = self.runtime.create_source(
                source.kind().as_str(),
                source.name(),
                source.settings(),
            )?;
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
            let flattened = self.visible_items(profile, name)?;
            let order = flattened
                .iter()
                .map(|(_, source, _)| *source)
                .collect::<Vec<_>>();
            let item_ids = flattened
                .iter()
                .map(|(item_id, _, _)| item_id.clone())
                .collect::<Vec<_>>();
            let attached = self
                .runtime
                .scene_sources(name)
                .map(<[SourceId]>::to_vec)
                .unwrap_or_default();
            let attached_item_ids = self.runtime.scene_item_ids(name).unwrap_or_default();
            if attached != order || attached_item_ids != item_ids {
                // Rebuild only the scene-item references. The shared runtime
                // source instances stay alive, so changing visibility/order or
                // adding a second reference never reopens a capture device.
                self.runtime.clear_scene_sources(name)?;
                for (item_id, source, _) in &flattened {
                    self.runtime
                        .attach_source_instance_with_id(name, *source, item_id)?;
                }
            }
            for (item_id, _, transform) in &flattened {
                self.runtime
                    .set_scene_item_transform_by_id(name, item_id, *transform)?;
            }
        }
        Ok(())
    }

    /// Resolves a scene's visible items, including nested scene references, to
    /// runtime sources and composed transforms in draw order.
    ///
    /// Keeps every visible scene item in draw order, including repeated
    /// references to one shared runtime source.
    fn visible_items(
        &self,
        profile: &Profile,
        scene_id: &str,
    ) -> Result<Vec<VisibleItem>, Box<dyn Error>> {
        profile
            .flatten_scene_items(scene_id)?
            .into_iter()
            .map(|item| {
                let source = self
                    .source_ids
                    .get(item.source_id().as_str())
                    .copied()
                    .ok_or_else(|| {
                        std::io::Error::other(format!(
                            "scene item references unknown source {}",
                            item.source_id()
                        ))
                    })?;
                Ok((item.item_id().to_owned(), source, item.transform()))
            })
            .collect()
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
            let scene = self
                .applied
                .active_profile_spec()?
                .scene(draft.scene.as_str())?;
            let mut targets = Vec::with_capacity(draft.items.len());
            for item in &draft.items {
                if scene
                    .item(item.item.as_str())
                    .is_some_and(|item| item.is_scene_reference() || item.is_group())
                {
                    return None;
                }
                let is_visible = scene
                    .items()
                    .iter()
                    .find(|candidate| candidate.id().as_str() == item.item)
                    .is_some_and(SceneItemSpec::visible);
                if !is_visible {
                    return None;
                }
                targets.push((item.item.clone(), item.transform));
            }
            Some((draft.scene.clone(), targets))
        });
        if let Some((scene, sources)) = self.applied_draft.clone() {
            let same_targets = target.as_ref().is_some_and(|(next_scene, next)| {
                next_scene == &scene
                    && next
                        .iter()
                        .map(|(item_id, _)| item_id)
                        .eq(sources.iter().map(|(item_id, _)| item_id))
            });
            if !same_targets {
                for (item_id, committed) in sources {
                    let _ = self
                        .runtime
                        .set_scene_item_transform_by_id(&scene, &item_id, committed);
                }
                // A scene composed only of still sources caches its picture, so
                // the cache has to go when the drag stops moving it.
                self.invalidate_static_scene_cache(&scene);
                self.applied_draft = None;
            }
        }
        let (Some(_draft), Some((scene, targets))) = (draft, target) else {
            return;
        };
        let mut applied = Vec::with_capacity(targets.len());
        for (item_id, transform) in targets {
            if self
                .runtime
                .set_scene_item_transform_by_id(&scene, &item_id, transform)
                .is_err()
            {
                return;
            }
            applied.push((item_id, transform));
        }
        if !applied.is_empty() {
            self.invalidate_static_scene_cache(&scene);
            self.applied_draft = Some((scene, applied));
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

    /// Returns the bounded viewport format used for GUI preview rendering.
    ///
    /// The preview is deliberately independent from the profile's program
    /// format: a 4K canvas should not force a 4K CPU readback for a roughly
    /// 1,050-pixel-wide window.
    #[must_use]
    pub(crate) fn preview_format_for_canvas(canvas: VideoFormat) -> VideoFormat {
        const MAX_WIDTH: u64 = 1_050;
        const MAX_HEIGHT: u64 = 590;
        let canvas_width = u64::from(canvas.width());
        let canvas_height = u64::from(canvas.height());
        let (width, height) =
            if canvas_width.saturating_mul(MAX_HEIGHT) <= canvas_height.saturating_mul(MAX_WIDTH) {
                (
                    canvas_width.saturating_mul(MAX_HEIGHT) / canvas_height,
                    MAX_HEIGHT,
                )
            } else {
                (
                    MAX_WIDTH,
                    canvas_height.saturating_mul(MAX_WIDTH) / canvas_width,
                )
            };
        let width = width.max(1).min(canvas_width);
        let height = height.max(1).min(canvas_height);
        VideoFormat::new(
            u32::try_from(width).unwrap_or(u32::MAX),
            u32::try_from(height).unwrap_or(u32::MAX),
            canvas.frame_rate(),
        )
        .expect("bounded preview dimensions are valid")
    }

    /// Returns the bounded thumbnail format used by the multiview compositor.
    /// One tile is deliberately much smaller than the interactive preview so a
    /// collection of scenes cannot turn the GUI into a full-resolution fan-out.
    #[must_use]
    pub(crate) fn multiview_tile_format(canvas: VideoFormat) -> VideoFormat {
        const MAX_TILE_WIDTH: u64 = 256;
        const MAX_TILE_HEIGHT: u64 = 256;
        let canvas_width = u64::from(canvas.width()).max(1);
        let canvas_height = u64::from(canvas.height()).max(1);
        let width = u64::from(canvas.width()).clamp(1, MAX_TILE_WIDTH);
        let height = canvas_height
            .saturating_mul(width)
            .checked_div(canvas_width)
            .unwrap_or(1)
            .clamp(1, MAX_TILE_HEIGHT);
        VideoFormat::new(
            u32::try_from(width).unwrap_or(u32::MAX),
            u32::try_from(height).unwrap_or(u32::MAX),
            canvas.frame_rate(),
        )
        .expect("bounded multiview tile dimensions are valid")
    }

    /// Returns the composite dimensions for a bounded scene count.
    #[must_use]
    pub(crate) fn multiview_format_for_canvas(
        canvas: VideoFormat,
        scene_count: usize,
    ) -> VideoFormat {
        let tile = Self::multiview_tile_format(canvas);
        let (columns, rows) = crate::preview_worker::multiview_grid_dimensions(scene_count);
        VideoFormat::new(
            tile.width()
                .saturating_mul(u32::try_from(columns).unwrap_or(u32::MAX)),
            tile.height()
                .saturating_mul(u32::try_from(rows).unwrap_or(u32::MAX)),
            canvas.frame_rate(),
        )
        .expect("bounded multiview composite dimensions are valid")
    }

    /// Returns the timestamp shared by all targets in the current render tick.
    #[must_use]
    pub(crate) const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    fn invalidate_static_scene_cache(&mut self, scene: &str) {
        self.static_frames.remove(scene);
        self.static_preview_frames
            .retain(|(cached_scene, _), _| cached_scene != scene);
        self.gpu_program_scene = None;
    }

    #[allow(
        dead_code,
        reason = "kept as the single-frame program render API for diagnostics"
    )]
    pub(crate) fn render(&mut self, scene: &str) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let frame = self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::Program, self.format),
        )?;
        self.advance_timestamp();
        Ok(frame)
    }

    /// Renders the current scene into the viewport-sized preview target.
    /// The caller can render a matching program target at the same timestamp
    /// and advance the clock once after the complete request.
    pub(crate) fn render_preview(
        &mut self,
        scene: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(scene, RenderTarget::new(RenderTargetRole::Preview, format))
    }

    pub(crate) fn render_program(
        &mut self,
        scene: &str,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::Program, self.format),
        )
    }

    /// Renders the program feed at the bounded size used by the desktop
    /// program view. The full [`RenderTargetRole::Program`] target remains
    /// reserved for output and encoder consumers.
    pub(crate) fn render_program_preview(
        &mut self,
        scene: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::ProgramPreview, format),
        )
    }

    /// Renders one transition into the full program target.
    pub(crate) fn render_transition(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_transition_target(
            source_scene,
            destination_scene,
            RenderTarget::new(RenderTargetRole::Program, self.format),
            transition,
        )
    }

    /// Renders one transition into a bounded GUI program target.
    pub(crate) fn render_transition_preview(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        format: VideoFormat,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_transition_target(
            source_scene,
            destination_scene,
            RenderTarget::new(RenderTargetRole::ProgramPreview, format),
            transition,
        )
    }

    fn render_transition_target(
        &mut self,
        source_scene: &str,
        destination_scene: &str,
        target: RenderTarget,
        transition: FrameTransition,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let source = self.render_target(source_scene, target)?;
        let destination = self.render_target(destination_scene, target)?;
        match (source, destination) {
            (None, None) => Ok(None),
            (source, destination) => {
                let source = source.unwrap_or_else(|| {
                    VideoFrame::solid(target.format(), self.timestamp, [0, 0, 0, 0])
                });
                let destination = destination.unwrap_or_else(|| {
                    VideoFrame::solid(target.format(), self.timestamp, [0, 0, 0, 0])
                });
                VideoFrame::transitioned(&source, destination, transition)
                    .map(Some)
                    .map_err(Into::into)
            }
        }
    }

    fn render_target(
        &mut self,
        scene: &str,
        target: RenderTarget,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let cached = match target.role() {
            RenderTargetRole::Program => self
                .static_frames
                .get(scene)
                .map(|pixels| (self.format, Arc::clone(pixels))),
            RenderTargetRole::Preview | RenderTargetRole::ProgramPreview => self
                .static_preview_frames
                .get(&(scene.to_owned(), target.format()))
                .map(|pixels| (target.format(), Arc::clone(pixels))),
            RenderTargetRole::Projector | RenderTargetRole::Encoder => None,
        };
        let frame = if let Some((cached_format, pixels)) = cached.as_ref() {
            Some(VideoFrame::from_shared(
                *cached_format,
                self.timestamp,
                Arc::clone(pixels),
            )?)
        } else {
            self.render_live_scene(scene, target)?
        };
        let frame = match frame {
            Some(frame) if frame.format() != target.format() => {
                self.scale_frame(&frame, target.format())?
            }
            Some(frame) => frame,
            None => return Ok(None),
        };
        if self.static_scenes.contains(scene) && cached.is_none() {
            match target.role() {
                RenderTargetRole::Program => {
                    self.static_frames
                        .insert(scene.to_owned(), Arc::new(frame.pixels().to_vec()));
                }
                RenderTargetRole::Preview | RenderTargetRole::ProgramPreview => {
                    self.static_preview_frames.insert(
                        (scene.to_owned(), target.format()),
                        Arc::new(frame.pixels().to_vec()),
                    );
                }
                RenderTargetRole::Projector | RenderTargetRole::Encoder => {}
            }
        }
        Ok(Some(frame))
    }

    fn render_live_scene(
        &mut self,
        scene: &str,
        target: RenderTarget,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        if matches!(self.compositor, PreviewCompositor::Wgpu(_)) {
            if !self.submit_live_scene(scene, target)? {
                return Ok(None);
            }
            let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
                return Ok(None);
            };
            let texture = compositor.target(target)?;
            return compositor
                .backend
                .readback(texture)
                .map(Some)
                .map_err(Into::into);
        }
        if target.role() == RenderTargetRole::Program {
            self.gpu_program_scene = None;
        }
        let request = VideoRequest::new(self.timestamp, self.format);
        self.runtime
            .render_scene(scene, &request)
            .map_err(Into::into)
    }

    /// Submits one scene to a WGPU target without reading its pixels back.
    ///
    /// Keeping submission separate lets the encoder consume the full program
    /// texture as NV12 while the GUI consumes a different, viewport-sized
    /// target. A `false` result means the scene has no visible layers.
    fn submit_live_scene(
        &mut self,
        scene: &str,
        target: RenderTarget,
    ) -> Result<bool, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        let layers = self.runtime.render_scene_layers(scene, &request)?;
        if layers.is_empty() {
            return Ok(false);
        }
        let submitted = layers
            .iter()
            .map(|layer| SceneLayer::frame(layer.frame(), layer.transform(), layer.filters()))
            .collect::<Vec<_>>();
        let submit_result = {
            let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
                return Ok(false);
            };
            let texture = compositor.target(target)?;
            compositor.backend.submit_layers(texture, &submitted)
        };
        if let Err(error) = submit_result {
            self.compositor = PreviewCompositor::Cpu {
                reason: Some(format!("GPU composition failed: {error}")),
            };
            self.gpu_program_scene = None;
            return Err(error.into());
        }
        if target.role() == RenderTargetRole::Program {
            self.gpu_program_scene = Some(scene.to_owned());
        }
        Ok(true)
    }

    fn scale_frame(
        &mut self,
        frame: &VideoFrame,
        target: VideoFormat,
    ) -> Result<VideoFrame, Box<dyn Error>> {
        let scaler = self
            .preview_scaler
            .get_or_insert_with(|| FrameScaler::new(frame.format(), target, ScaleFilter::Bilinear));
        scaler.reconfigure(frame.format(), target, ScaleFilter::Bilinear);
        scaler.scale(frame).map_err(Into::into)
    }

    /// Produces an encoder-oriented NV12 frame from the full program target.
    /// The target is submitted directly when its scene snapshot is stale, so
    /// the normal accelerated path never needs an intermediate full RGBA
    /// frame.
    pub(crate) fn encoder_frame(
        &mut self,
        scene: &str,
    ) -> Result<Option<RawVideoFrame>, Box<dyn Error>> {
        if !matches!(self.compositor, PreviewCompositor::Wgpu(_)) {
            return Ok(None);
        }
        if self.gpu_program_scene.as_deref() != Some(scene)
            && !self.submit_live_scene(
                scene,
                RenderTarget::new(RenderTargetRole::Program, self.format),
            )?
        {
            return Ok(None);
        }
        let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
            return Ok(None);
        };
        let target =
            compositor.target(RenderTarget::new(RenderTargetRole::Program, self.format))?;
        compositor
            .backend
            .readback_nv12(target)
            .map(Some)
            .map_err(Into::into)
    }

    pub(crate) fn advance_timestamp(&mut self) {
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
            PreviewCompositor::Wgpu(compositor) => {
                let backend = &compositor.backend;
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
            "Preview work: {backend} · renders={} · source requests={} · frames={} · empty={} · failed={} · contract={} · transforms={} · filters={} · blends={} · capture p50/p95/p99/max={}/{}/{}/{} µs",
            metrics.render_calls(),
            metrics.source_requests(),
            metrics.source_frames(),
            metrics.empty_sources(),
            metrics.failed_sources(),
            metrics.contract_violations(),
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
                        item.is_source()
                            && profile
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

pub(crate) trait PreviewPresenter {
    fn present(&mut self, frame: &VideoFrame) -> Image;
}

struct SlintPreviewPresenter;

impl PreviewPresenter for SlintPreviewPresenter {
    fn present(&mut self, frame: &VideoFrame) -> Image {
        let format = frame.format();
        // Slint owns its pixel storage, so one copy out of the engine frame is
        // unavoidable here; `clone_from_slice` performs it as a single block
        // copy. The worker supplies a viewport-sized frame, not the full
        // program canvas.
        let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            frame.pixels(),
            format.width(),
            format.height(),
        );
        Image::from_rgba8(buffer)
    }
}

pub(crate) fn frame_to_image(frame: &VideoFrame) -> Image {
    SlintPreviewPresenter.present(frame)
}
