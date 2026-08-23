use std::{
    collections::{HashMap, HashSet},
    error::Error,
    sync::Arc,
};

use obs_rs_config::Config;
use obs_rs_core::{CompositorMetrics, Runtime, RuntimeLimits, RuntimeUsage, SourceId};
use obs_rs_engine::{compile_filter_report, FilterCompilation, MAX_FILTER_DIAGNOSTICS};
use obs_rs_media::{FrameScaler, FrameTransform, FrameTransition, ScaleFilter};
use obs_rs_media::{RawVideoFrame, Timestamp, VideoFormat, VideoFrame};
use obs_rs_plugin_api::VideoRequest;
use obs_rs_project::{Profile, Project, SceneItemSpec, SourceSpec};
use obs_rs_render::{RenderBackend, RenderTarget, RenderTargetRole, SceneLayer};

mod compositor;
mod presenter;
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
    /// Persisted filters unavailable in the preview runtime.
    pub(crate) filter_diagnostics: Vec<String>,
}

impl PreviewRenderer {
    pub(crate) fn new(project: &Project, revision: u64) -> Result<Self, Box<dyn Error>> {
        let profile = project
            .active_profile_spec()
            .ok_or_else(|| std::io::Error::other("active profile is missing"))?;
        let format = profile.video_format();
        let mut runtime = Runtime::new();
        let plugin = builtin_plugin();
        runtime.register_plugin(plugin.as_ref())?;

        let mut renderer = Self {
            format,
            runtime,
            timestamp: Timestamp::ZERO,
            revision,
            applied: empty_project(project),
            source_ids: HashMap::new(),
            filter_diagnostics: Vec::new(),
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
        // A different canvas means every source renegotiates its output shape.
        // Profile changes and source-kind changes are diffed below so sources
        // shared by two profiles can stay open while the scene graph moves.
        let rebuild = profile.video_format() != self.format;
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

    /// Returns whether a live source changed its kind and therefore needs its
    /// scene references detached before the old factory instance is replaced.
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
        self.filter_diagnostics = Self::collect_filter_diagnostics(profile);
        // The mirrored profile is cloned out first: `sync_source` needs `&mut
        // self`, and the borrow checker will not hold a reference into
        // `self.applied` across it.
        let previous = self
            .applied
            .active_profile_spec()
            .map(|profile| {
                profile
                    .sources()
                    .map(|source| (source.id().as_str().to_owned(), source.clone()))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();

        if self.kind_changed(profile) {
            for scene in self.scene_ids.clone() {
                self.runtime.clear_scene_sources(&scene)?;
            }
        }

        let active_sources = self.active_source_ids(profile)?;
        for source in profile
            .sources()
            .filter(|source| active_sources.contains(source.id().as_str()))
        {
            self.sync_source(source, previous.get(source.id().as_str()))?;
        }
        self.sync_scenes(profile)?;
        self.retire_sources(profile, &active_sources)?;

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
        if previous.kind() != source.kind() {
            // Runtime source instances cannot change factory kind in place.
            // Scene references were cleared by `apply_profile` before this
            // path, so destroying this one source does not disturb any other
            // capture device.
            self.runtime.destroy_source(id)?;
            let id = self.runtime.create_source(
                source.kind().as_str(),
                source.name(),
                source.settings(),
            )?;
            self.apply_filters(id, source)?;
            self.source_ids.insert(source.id().as_str().to_owned(), id);
            return Ok(());
        }
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
            if let FilterCompilation::Applied(runtime_filter) = compile_filter_report(filter) {
                self.runtime.add_source_filter(id, runtime_filter)?;
            }
        }
        Ok(())
    }

    /// Collects unavailable persisted filters without adding them to the
    /// renderer. The list is capped so a malformed project cannot inflate
    /// every diagnostics snapshot.
    fn collect_filter_diagnostics(profile: &Profile) -> Vec<String> {
        let mut diagnostics = Vec::new();
        for source in profile.sources() {
            for filter in source.filters() {
                let FilterCompilation::Unavailable(diagnostic) = compile_filter_report(filter)
                else {
                    continue;
                };
                if diagnostics.len() + 1 < MAX_FILTER_DIAGNOSTICS {
                    diagnostics.push(format!(
                        "source '{}' filter '{}': {diagnostic}",
                        source.name(),
                        filter.name()
                    ));
                } else if diagnostics.len() + 1 == MAX_FILTER_DIAGNOSTICS {
                    diagnostics.push(format!(
                        "additional filter diagnostics omitted after {MAX_FILTER_DIAGNOSTICS} entries"
                    ));
                }
            }
        }
        diagnostics
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

    /// Resolves the source instances that have at least one visible scene
    /// consumer. A source definition can exist in the project without being
    /// live in the preview runtime; keeping that distinction prevents hidden
    /// cameras and screen-cast sessions from claiming hardware.
    fn active_source_ids(&self, profile: &Profile) -> Result<HashSet<String>, Box<dyn Error>> {
        let mut active = HashSet::new();
        for scene in profile.scenes() {
            for item in profile.flatten_scene_items(scene.id().as_str())? {
                active.insert(item.source_id().as_str().to_owned());
            }
        }
        Ok(active)
    }

    /// Destroys sources the project no longer defines or no longer displays.
    fn retire_sources(
        &mut self,
        profile: &Profile,
        active_sources: &HashSet<String>,
    ) -> Result<(), Box<dyn Error>> {
        let removed = self
            .source_ids
            .iter()
            .filter(|(id, _)| {
                !profile.has_source(id.as_str()) || !active_sources.contains(id.as_str())
            })
            .map(|(id, source)| (id.clone(), *source))
            .collect::<Vec<_>>();
        for (project_id, source) in removed {
            self.runtime.destroy_source(source)?;
            self.source_ids.remove(&project_id);
        }
        Ok(())
    }

    /// Takes backend-generated settings that became available after an
    /// asynchronous source open, such as a fresh Wayland restore token.
    pub(crate) fn take_source_settings_updates(&mut self) -> Vec<(String, String, Config)> {
        let profile = self.applied.active_profile().as_str().to_owned();
        let sources = self
            .source_ids
            .iter()
            .map(|(project_id, source_id)| (project_id.clone(), *source_id))
            .collect::<Vec<_>>();
        sources
            .into_iter()
            .filter_map(|(project_id, source_id)| {
                self.runtime
                    .take_source_settings_update(source_id)
                    .map(|settings| (profile.clone(), project_id, settings))
            })
            .collect()
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
            filter_diagnostics: self.filter_diagnostics.clone(),
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

    /// Renders one selected source item without applying its scene-item
    /// transform or compositing the rest of the scene.
    ///
    /// A source projector follows the source's own output and source-level
    /// filters. Its target is separate from the preview/program targets so a
    /// projector cannot overwrite a GUI feed or force a full-canvas readback.
    pub(crate) fn render_source(
        &mut self,
        scene: &str,
        item: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        let target = RenderTarget::new(RenderTargetRole::Projector, format);
        if matches!(self.compositor, PreviewCompositor::Wgpu(_)) {
            if !self.submit_source_layer(scene, item, target)? {
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

        let request = VideoRequest::new(self.timestamp, self.format);
        let layers = self.runtime.render_scene_layers(scene, &request)?;
        let Some(layer) = layers.iter().find(|layer| layer.item_id() == item) else {
            return Ok(None);
        };
        let mut frame = layer.frame().clone();
        frame.apply_filters(layer.filters());
        if frame.format() != target.format() {
            frame = self.scale_frame(&frame, target.format())?;
        }
        Ok(Some(frame))
    }

    /// Renders one complete scene for a scene projector without opening a
    /// second runtime or changing the scene's persisted geometry.
    pub(crate) fn render_scene_projector(
        &mut self,
        scene: &str,
        format: VideoFormat,
    ) -> Result<Option<VideoFrame>, Box<dyn Error>> {
        self.render_target(
            scene,
            RenderTarget::new(RenderTargetRole::Projector, format),
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

    /// Submits one selected source item to a projector target without opening
    /// another runtime or applying scene-item geometry.
    fn submit_source_layer(
        &mut self,
        scene: &str,
        item: &str,
        target: RenderTarget,
    ) -> Result<bool, Box<dyn Error>> {
        let request = VideoRequest::new(self.timestamp, self.format);
        let layers = self.runtime.render_scene_layers(scene, &request)?;
        let Some(layer) = layers.iter().find(|layer| layer.item_id() == item) else {
            return Ok(false);
        };
        let submitted = SceneLayer::frame(layer.frame(), FrameTransform::IDENTITY, layer.filters());
        let submit_result = {
            let PreviewCompositor::Wgpu(compositor) = &mut self.compositor else {
                return Ok(false);
            };
            let texture = compositor.target(target)?;
            compositor
                .backend
                .submit_layers(texture, std::slice::from_ref(&submitted))
        };
        if let Err(error) = submit_result {
            self.compositor = PreviewCompositor::Cpu {
                reason: Some(format!("GPU composition failed: {error}")),
            };
            self.gpu_program_scene = None;
            return Err(error.into());
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
