//! The headless OBS-RS runtime and its reference scene compositor.

#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]

use std::{collections::BTreeMap, fmt, sync::Arc};

use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameTransform, FrameTransition, MediaError, VideoFrame};
use obs_rs_plugin_api::{
    Plugin, PluginApiVersion, PluginManifest, Source, SourceError, SourceFactory, VideoRequest,
};
use obs_rs_util::{Identifier, IdentifierError};

/// A stable runtime handle for an owned source instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceId(u64);

impl SourceId {
    /// Returns the numeric value for logs and deterministic fixtures.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

struct Registry {
    plugins: BTreeMap<Identifier, PluginManifest>,
    sources: BTreeMap<Identifier, Arc<dyn SourceFactory>>,
}

impl Registry {
    fn new() -> Self {
        Self {
            plugins: BTreeMap::new(),
            sources: BTreeMap::new(),
        }
    }
}

struct SourceInstance {
    kind: Identifier,
    name: String,
    source: Box<dyn Source>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Scene {
    sources: Vec<SourceId>,
    transforms: BTreeMap<SourceId, FrameTransform>,
    filters: BTreeMap<SourceId, Vec<FrameFilter>>,
}

/// Counters for CPU compositor work and source behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompositorMetrics {
    render_calls: u64,
    source_requests: u64,
    source_frames: u64,
    empty_sources: u64,
    transformed_frames: u64,
    filtered_frames: u64,
    blended_layers: u64,
}

impl CompositorMetrics {
    /// Returns the number of scene render calls.
    #[must_use]
    pub const fn render_calls(self) -> u64 {
        self.render_calls
    }

    /// Returns the number of source render requests.
    #[must_use]
    pub const fn source_requests(self) -> u64 {
        self.source_requests
    }

    /// Returns the number of source frames returned.
    #[must_use]
    pub const fn source_frames(self) -> u64 {
        self.source_frames
    }

    /// Returns the number of source requests that returned no frame.
    #[must_use]
    pub const fn empty_sources(self) -> u64 {
        self.empty_sources
    }

    /// Returns the number of frames sent through a non-identity transform.
    #[must_use]
    pub const fn transformed_frames(self) -> u64 {
        self.transformed_frames
    }

    /// Returns the number of in-place filter applications.
    #[must_use]
    pub const fn filtered_frames(self) -> u64 {
        self.filtered_frames
    }

    /// Returns the number of layer-over-layer blend operations.
    #[must_use]
    pub const fn blended_layers(self) -> u64 {
        self.blended_layers
    }
}

/// A single-threaded runtime that owns plugins, sources, and scenes.
pub struct Runtime {
    registry: Registry,
    sources: BTreeMap<SourceId, SourceInstance>,
    scenes: BTreeMap<Identifier, Scene>,
    next_source_id: u64,
    metrics: CompositorMetrics,
}

impl Runtime {
    /// Creates an empty runtime with no global state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
            sources: BTreeMap::new(),
            scenes: BTreeMap::new(),
            next_source_id: 1,
            metrics: CompositorMetrics::default(),
        }
    }

    /// Registers a plugin and all of its source factories atomically.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::DuplicatePlugin`] or
    /// [`RuntimeError::DuplicateSourceKind`] when registration would collide with
    /// existing runtime state.
    pub fn register_plugin(&mut self, plugin: &dyn Plugin) -> Result<(), RuntimeError> {
        let manifest = plugin.manifest().clone();
        let expected_api = PluginApiVersion::current();
        if manifest.api_version().major() != expected_api.major()
            || manifest.api_version().minor() > expected_api.minor()
        {
            return Err(RuntimeError::UnsupportedPluginApi {
                expected: expected_api,
                actual: manifest.api_version(),
            });
        }
        if self.registry.plugins.contains_key(manifest.id()) {
            return Err(RuntimeError::DuplicatePlugin(manifest.id().clone()));
        }

        let factories = plugin.source_factories();
        for factory in &factories {
            if self.registry.sources.contains_key(factory.kind()) {
                return Err(RuntimeError::DuplicateSourceKind(factory.kind().clone()));
            }
        }

        self.registry
            .plugins
            .insert(manifest.id().clone(), manifest);
        for factory in factories {
            self.registry
                .sources
                .insert(factory.kind().clone(), factory);
        }
        Ok(())
    }

    /// Returns the manifests of registered plugins in identifier order.
    #[must_use]
    pub fn plugins(&self) -> Vec<PluginManifest> {
        self.registry.plugins.values().cloned().collect()
    }

    /// Creates a named scene.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidIdentifier`] for an invalid name or
    /// [`RuntimeError::DuplicateScene`] when the name is already in use.
    pub fn create_scene(&mut self, name: &str) -> Result<(), RuntimeError> {
        let name = identifier(name, "scene")?;
        if self.scenes.contains_key(&name) {
            return Err(RuntimeError::DuplicateScene(name));
        }

        self.scenes.insert(
            name.clone(),
            Scene {
                sources: Vec::new(),
                transforms: BTreeMap::new(),
                filters: BTreeMap::new(),
            },
        );
        Ok(())
    }

    /// Creates a source through a registered factory.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the kind or name is invalid, no factory exists,
    /// source settings are rejected, or the source ID space is exhausted.
    pub fn create_source(
        &mut self,
        kind: &str,
        name: &str,
        settings: &Config,
    ) -> Result<SourceId, RuntimeError> {
        if name.trim().is_empty() {
            return Err(RuntimeError::InvalidName { kind: "source" });
        }

        let kind = identifier(kind, "source kind")?;
        let factory = self
            .registry
            .sources
            .get(&kind)
            .ok_or_else(|| RuntimeError::UnknownSourceKind(kind.clone()))?;
        let source = factory
            .create(name, settings)
            .map_err(RuntimeError::Source)?;
        let id = SourceId(self.next_source_id);
        self.next_source_id = self
            .next_source_id
            .checked_add(1)
            .ok_or(RuntimeError::IdExhausted)?;

        self.sources.insert(
            id,
            SourceInstance {
                kind,
                name: name.to_owned(),
                source,
            },
        );
        Ok(id)
    }

    /// Attaches a source to the end of a scene's ordered item list.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the scene or source does not exist, or when the
    /// source is already attached to that scene.
    pub fn attach_source(&mut self, scene: &str, source: SourceId) -> Result<(), RuntimeError> {
        let scene = identifier(scene, "scene")?;
        if !self.sources.contains_key(&source) {
            return Err(RuntimeError::UnknownSource(source));
        }
        let scene = self
            .scenes
            .get_mut(&scene)
            .ok_or_else(|| RuntimeError::UnknownScene(scene.clone()))?;
        if scene.sources.contains(&source) {
            return Err(RuntimeError::SourceAlreadyAttached(source));
        }
        scene.sources.push(source);
        scene.transforms.insert(source, FrameTransform::IDENTITY);
        scene.filters.insert(source, Vec::new());
        Ok(())
    }

    /// Removes a source from a scene while keeping the source instance alive.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the item is absent.
    pub fn detach_source(&mut self, scene: &str, source: SourceId) -> Result<(), RuntimeError> {
        let scene = identifier(scene, "scene")?;
        let scene = self
            .scenes
            .get_mut(&scene)
            .ok_or_else(|| RuntimeError::UnknownScene(scene.clone()))?;
        let Some(index) = scene
            .sources
            .iter()
            .position(|candidate| *candidate == source)
        else {
            return Err(RuntimeError::SourceNotAttached(source));
        };
        scene.sources.remove(index);
        scene.transforms.remove(&source);
        scene.filters.remove(&source);
        Ok(())
    }

    /// Sets the transform and opacity for one scene item.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the source is not an item in it.
    pub fn set_source_transform(
        &mut self,
        scene: &str,
        source: SourceId,
        transform: FrameTransform,
    ) -> Result<(), RuntimeError> {
        let scene = identifier(scene, "scene")?;
        let scene = self
            .scenes
            .get_mut(&scene)
            .ok_or_else(|| RuntimeError::UnknownScene(scene.clone()))?;
        if !scene.sources.contains(&source) {
            return Err(RuntimeError::SourceNotAttached(source));
        }
        scene.transforms.insert(source, transform);
        Ok(())
    }

    /// Returns a scene item's current transform.
    #[must_use]
    pub fn source_transform(&self, scene: &str, source: SourceId) -> Option<FrameTransform> {
        let scene = Identifier::new(scene).ok()?;
        self.scenes.get(&scene)?.transforms.get(&source).copied()
    }

    /// Adds a CPU filter to the end of one scene item's filter chain.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the source is not an item in it.
    pub fn add_source_filter(
        &mut self,
        scene: &str,
        source: SourceId,
        filter: FrameFilter,
    ) -> Result<(), RuntimeError> {
        let scene = identifier(scene, "scene")?;
        let scene = self
            .scenes
            .get_mut(&scene)
            .ok_or_else(|| RuntimeError::UnknownScene(scene.clone()))?;
        let filters = scene
            .filters
            .get_mut(&source)
            .ok_or(RuntimeError::SourceNotAttached(source))?;
        filters.push(filter);
        Ok(())
    }

    /// Removes every CPU filter from one scene item.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist or
    /// [`RuntimeError::SourceNotAttached`] when the source is not an item in it.
    pub fn clear_source_filters(
        &mut self,
        scene: &str,
        source: SourceId,
    ) -> Result<(), RuntimeError> {
        let scene = identifier(scene, "scene")?;
        let scene = self
            .scenes
            .get_mut(&scene)
            .ok_or_else(|| RuntimeError::UnknownScene(scene.clone()))?;
        let filters = scene
            .filters
            .get_mut(&source)
            .ok_or(RuntimeError::SourceNotAttached(source))?;
        filters.clear();
        Ok(())
    }

    /// Returns a copy of one scene item's filter chain.
    #[must_use]
    pub fn source_filters(&self, scene: &str, source: SourceId) -> Option<Vec<FrameFilter>> {
        let scene = Identifier::new(scene).ok()?;
        self.scenes.get(&scene)?.filters.get(&source).cloned()
    }

    /// Destroys a source that is not attached to any scene.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownSource`] for an unknown ID or
    /// [`RuntimeError::SourceInUse`] while a scene still references the source.
    pub fn destroy_source(&mut self, source: SourceId) -> Result<(), RuntimeError> {
        if !self.sources.contains_key(&source) {
            return Err(RuntimeError::UnknownSource(source));
        }
        if self
            .scenes
            .values()
            .any(|scene| scene.sources.contains(&source))
        {
            return Err(RuntimeError::SourceInUse(source));
        }
        self.sources.remove(&source);
        Ok(())
    }

    /// Destroys a scene and all of its item references.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownScene`] when the scene does not exist.
    pub fn destroy_scene(&mut self, scene: &str) -> Result<(), RuntimeError> {
        let scene = identifier(scene, "scene")?;
        self.scenes
            .remove(&scene)
            .map(|_| ())
            .ok_or(RuntimeError::UnknownScene(scene))
    }

    /// Applies new settings to a source instance.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::UnknownSource`] when the ID is not live or
    /// [`RuntimeError::Source`] when the source rejects the settings.
    pub fn update_source(
        &mut self,
        source: SourceId,
        settings: &Config,
    ) -> Result<(), RuntimeError> {
        let instance = self
            .sources
            .get_mut(&source)
            .ok_or(RuntimeError::UnknownSource(source))?;
        instance
            .source
            .update(settings)
            .map_err(RuntimeError::Source)
    }

    /// Renders one scene in item order using the CPU reference compositor.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when a scene or source is missing, a source rejects
    /// the request, or a frame violates the media format contract.
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
            let transform = scene_state
                .transforms
                .get(source_id)
                .copied()
                .unwrap_or(FrameTransform::IDENTITY);
            let filters = scene_state
                .filters
                .get(source_id)
                .map_or(&[][..], Vec::as_slice);
            metrics.source_requests = metrics.source_requests.saturating_add(1);
            let instance = sources
                .get_mut(source_id)
                .ok_or(RuntimeError::UnknownSource(*source_id))?;
            let frame = instance
                .source
                .render(request)
                .map_err(RuntimeError::Source)?;
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
                frame.transformed(transform).map_err(RuntimeError::Media)?
            };
            for filter in filters {
                metrics.filtered_frames = metrics.filtered_frames.saturating_add(1);
                frame.apply_filter(*filter);
            }

            if let Some(composite) = result.as_mut() {
                composite.blend_over(&frame).map_err(RuntimeError::Media)?;
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
        VideoFrame::transitioned(&source, &destination, transition)
            .map(Some)
            .map_err(RuntimeError::Media)
    }

    /// Returns the number of live source instances.
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

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

fn identifier(input: &str, kind: &'static str) -> Result<Identifier, RuntimeError> {
    Identifier::new(input).map_err(|error| RuntimeError::InvalidIdentifier { kind, error })
}

/// Errors raised by runtime lifecycle and rendering operations.
#[derive(Debug, Eq, PartialEq)]
pub enum RuntimeError {
    /// A name or kind failed identifier validation.
    InvalidIdentifier {
        /// The logical value being validated.
        kind: &'static str,
        /// The validation failure.
        error: IdentifierError,
    },
    /// A user-facing source or scene name is empty.
    InvalidName {
        /// The logical value being named.
        kind: &'static str,
    },
    /// The plugin is already registered.
    DuplicatePlugin(Identifier),
    /// A source kind is already owned by another factory.
    DuplicateSourceKind(Identifier),
    /// A scene name is already in use.
    DuplicateScene(Identifier),
    /// No factory is registered for a source kind.
    UnknownSourceKind(Identifier),
    /// No source exists for an ID.
    UnknownSource(SourceId),
    /// No scene exists for a name.
    UnknownScene(Identifier),
    /// A source is already present in a scene.
    SourceAlreadyAttached(SourceId),
    /// A source was not present in a scene.
    SourceNotAttached(SourceId),
    /// A source cannot be destroyed while a scene references it.
    SourceInUse(SourceId),
    /// Source IDs are exhausted.
    IdExhausted,
    /// A plugin requires an API version this runtime cannot provide.
    UnsupportedPluginApi {
        /// Runtime API version.
        expected: PluginApiVersion,
        /// Plugin API version.
        actual: PluginApiVersion,
    },
    /// A source rejected creation, update, or rendering.
    Source(SourceError),
    /// A media invariant failed during composition.
    Media(MediaError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { kind, error } => {
                write!(formatter, "invalid {kind} identifier: {error}")
            }
            Self::InvalidName { kind } => write!(formatter, "{kind} name is empty"),
            Self::DuplicatePlugin(id) => write!(formatter, "plugin {id} is already registered"),
            Self::DuplicateSourceKind(kind) => {
                write!(formatter, "source kind {kind} is already registered")
            }
            Self::DuplicateScene(name) => write!(formatter, "scene {name} already exists"),
            Self::UnknownSourceKind(kind) => {
                write!(formatter, "source kind {kind} is not registered")
            }
            Self::UnknownSource(source) => {
                write!(formatter, "source {} does not exist", source.value())
            }
            Self::UnknownScene(scene) => write!(formatter, "scene {scene} does not exist"),
            Self::SourceAlreadyAttached(source) => {
                write!(formatter, "source {} is already attached", source.value())
            }
            Self::SourceNotAttached(source) => {
                write!(formatter, "source {} is not attached", source.value())
            }
            Self::SourceInUse(source) => {
                write!(
                    formatter,
                    "source {} is still used by a scene",
                    source.value()
                )
            }
            Self::IdExhausted => formatter.write_str("source ID space is exhausted"),
            Self::UnsupportedPluginApi { expected, actual } => write!(
                formatter,
                "plugin API {actual:?} is incompatible with runtime API {expected:?}"
            ),
            Self::Source(error) => error.fmt(formatter),
            Self::Media(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RuntimeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use obs_rs_builtins::BuiltinPlugin;
    use obs_rs_media::{FrameRate, Timestamp, VideoFormat};
    use std::sync::Arc;

    fn settings(width: u32, height: u32, color: &str) -> Config {
        let mut config = Config::new();
        config
            .set("width", &width.to_string())
            .expect("valid width");
        config
            .set("height", &height.to_string())
            .expect("valid height");
        config.set("color", color).expect("valid color");
        config
    }

    fn format() -> VideoFormat {
        VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("valid rate")).expect("valid format")
    }

    struct FutureApiPlugin {
        manifest: PluginManifest,
    }

    impl Plugin for FutureApiPlugin {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn source_factories(&self) -> Vec<Arc<dyn SourceFactory>> {
            Vec::new()
        }
    }

    #[test]
    fn registers_plugin_creates_scene_and_composites_sources() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let mut runtime = Runtime::new();
        runtime
            .register_plugin(&plugin)
            .expect("registration succeeds");
        runtime.create_scene("main").expect("scene is new");
        let background = runtime
            .create_source("color_source", "background", &settings(2, 2, "#0000FFFF"))
            .expect("background is valid");
        let foreground = runtime
            .create_source("color_source", "foreground", &settings(2, 2, "#FF000080"))
            .expect("foreground is valid");
        runtime
            .attach_source("main", background)
            .expect("attach background");
        runtime
            .attach_source("main", foreground)
            .expect("attach foreground");

        let request = VideoRequest::new(Timestamp::ZERO, format());
        let frame = runtime
            .render_scene("main", &request)
            .expect("render succeeds")
            .expect("scene has frames");

        assert_eq!(runtime.plugins().len(), 1);
        assert_eq!(runtime.source_count(), 2);
        assert_eq!(runtime.scene_count(), 1);
        assert_eq!(
            runtime.scene_sources("main"),
            Some(&[background, foreground][..])
        );
        assert_eq!(
            runtime.source_info(background),
            Some(("color_source", "background"))
        );
        assert_eq!(frame.pixel(0, 0), Some([128, 0, 127, 255]));
    }

    #[test]
    fn scene_item_transform_is_applied_before_composition() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let mut runtime = Runtime::new();
        runtime
            .register_plugin(&plugin)
            .expect("registration succeeds");
        runtime.create_scene("main").expect("scene is new");
        let source = runtime
            .create_source("color_source", "red", &settings(2, 2, "#FF0000FF"))
            .expect("source is valid");
        runtime
            .attach_source("main", source)
            .expect("attach source");
        let transform =
            FrameTransform::new(1_000, 1_000, 0, 0, false, false, 128).expect("transform is valid");
        runtime
            .set_source_transform("main", source, transform)
            .expect("set transform");
        runtime
            .add_source_filter("main", source, FrameFilter::Grayscale)
            .expect("add filter");

        let request = VideoRequest::new(Timestamp::ZERO, format());
        let frame = runtime
            .render_scene("main", &request)
            .expect("render succeeds")
            .expect("scene has a frame");

        assert_eq!(runtime.source_transform("main", source), Some(transform));
        assert_eq!(
            runtime.source_filters("main", source),
            Some(vec![FrameFilter::Grayscale])
        );
        assert_eq!(frame.pixel(0, 0), Some([76, 76, 76, 128]));
    }

    #[test]
    fn compositor_metrics_report_work_and_reset() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let mut runtime = Runtime::new();
        runtime
            .register_plugin(&plugin)
            .expect("registration succeeds");
        runtime.create_scene("main").expect("scene is new");
        let source = runtime
            .create_source("color_source", "red", &settings(2, 2, "#FF0000FF"))
            .expect("source is valid");
        runtime
            .attach_source("main", source)
            .expect("attach source");
        runtime
            .set_source_transform(
                "main",
                source,
                FrameTransform::new(1_000, 1_000, 0, 0, false, false, 128)
                    .expect("transform is valid"),
            )
            .expect("set transform");
        runtime
            .add_source_filter("main", source, FrameFilter::Grayscale)
            .expect("add filter");

        let request = VideoRequest::new(Timestamp::ZERO, format());
        runtime
            .render_scene("main", &request)
            .expect("render succeeds")
            .expect("scene has a frame");

        let metrics = runtime.compositor_metrics();
        assert_eq!(metrics.render_calls(), 1);
        assert_eq!(metrics.source_requests(), 1);
        assert_eq!(metrics.source_frames(), 1);
        assert_eq!(metrics.empty_sources(), 0);
        assert_eq!(metrics.transformed_frames(), 1);
        assert_eq!(metrics.filtered_frames(), 1);
        assert_eq!(metrics.blended_layers(), 0);

        runtime.reset_compositor_metrics();
        assert_eq!(runtime.compositor_metrics(), CompositorMetrics::default());
    }

    #[test]
    fn first_transparent_layer_has_canonical_rgb_values() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let mut runtime = Runtime::new();
        runtime
            .register_plugin(&plugin)
            .expect("registration succeeds");
        runtime.create_scene("main").expect("scene is new");
        let source = runtime
            .create_source("color_source", "transparent", &settings(2, 2, "#FF000000"))
            .expect("source is valid");
        runtime
            .attach_source("main", source)
            .expect("attach source");

        let frame = runtime
            .render_scene("main", &VideoRequest::new(Timestamp::ZERO, format()))
            .expect("render succeeds")
            .expect("scene has a frame");

        assert_eq!(frame.pixel(0, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn scene_transition_renders_cut_and_cross_fade() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let mut runtime = Runtime::new();
        runtime
            .register_plugin(&plugin)
            .expect("registration succeeds");
        runtime.create_scene("from").expect("source scene");
        runtime.create_scene("to").expect("destination scene");
        let from = runtime
            .create_source("color_source", "from-color", &settings(2, 2, "#FF0000FF"))
            .expect("source is valid");
        let to = runtime
            .create_source("color_source", "to-color", &settings(2, 2, "#0000FFFF"))
            .expect("destination is valid");
        runtime.attach_source("from", from).expect("attach source");
        runtime.attach_source("to", to).expect("attach destination");
        let request = VideoRequest::new(Timestamp::ZERO, format());

        let cut = runtime
            .render_scene_transition("from", "to", &request, FrameTransition::Cut)
            .expect("cut succeeds")
            .expect("destination has a frame");
        let fade = runtime
            .render_scene_transition(
                "from",
                "to",
                &request,
                FrameTransition::cross_fade(500).expect("valid progress"),
            )
            .expect("fade succeeds")
            .expect("both scenes have frames");

        assert_eq!(cut.pixel(0, 0), Some([0, 0, 255, 255]));
        assert_eq!(fade.pixel(0, 0), Some([128, 0, 128, 255]));
    }

    #[test]
    fn rejects_duplicate_registration_and_unknown_values() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let mut runtime = Runtime::new();
        runtime
            .register_plugin(&plugin)
            .expect("first registration");
        assert_eq!(
            runtime.register_plugin(&plugin),
            Err(RuntimeError::DuplicatePlugin(
                Identifier::new("obs_rs_builtins").expect("valid id")
            ))
        );
        assert!(matches!(
            runtime.create_source("missing", "source", &Config::new()),
            Err(RuntimeError::UnknownSourceKind(_))
        ));
    }

    #[test]
    fn rejects_plugins_from_a_newer_api_version() {
        let plugin = FutureApiPlugin {
            manifest: PluginManifest::with_api_version(
                "future_plugin",
                "Future plugin",
                "1.0.0",
                PluginApiVersion::new(2, 0),
            )
            .expect("manifest"),
        };
        let mut runtime = Runtime::new();
        assert_eq!(
            runtime.register_plugin(&plugin),
            Err(RuntimeError::UnsupportedPluginApi {
                expected: PluginApiVersion::current(),
                actual: PluginApiVersion::new(2, 0),
            })
        );
    }

    #[test]
    fn empty_scene_renders_no_frame() {
        let mut runtime = Runtime::new();
        runtime.create_scene("empty").expect("scene is new");
        let request = VideoRequest::new(Timestamp::ZERO, format());

        assert_eq!(
            runtime
                .render_scene("empty", &request)
                .expect("render succeeds"),
            None
        );
    }

    #[test]
    fn lifecycle_requires_detach_before_source_destruction() {
        let plugin = BuiltinPlugin::new().expect("builtins are valid");
        let mut runtime = Runtime::new();
        runtime
            .register_plugin(&plugin)
            .expect("registration succeeds");
        runtime.create_scene("main").expect("scene is new");
        let source = runtime
            .create_source("color_source", "background", &settings(2, 2, "#000000FF"))
            .expect("source is valid");
        runtime
            .attach_source("main", source)
            .expect("attach source");
        assert_eq!(
            runtime.destroy_source(source),
            Err(RuntimeError::SourceInUse(source))
        );
        runtime
            .detach_source("main", source)
            .expect("detach source");
        runtime.destroy_source(source).expect("destroy source");
        runtime.destroy_scene("main").expect("destroy scene");
        assert_eq!(runtime.source_count(), 0);
        assert_eq!(runtime.scene_count(), 0);
    }
}
