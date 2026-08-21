use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use obs_rs_media::{FrameFilter, FrameTransform, VideoFrame};
use obs_rs_plugin_api::{PluginManifest, Source, SourceFactory};
use obs_rs_util::Identifier;

pub(crate) struct Registry {
    /// Ordered because [`Runtime::plugins`] documents identifier order.
    pub(crate) plugins: BTreeMap<Identifier, PluginManifest>,
    /// Ordered because [`Runtime::source_kinds`] documents identifier order.
    pub(crate) sources: BTreeMap<Identifier, Arc<dyn SourceFactory>>,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            plugins: BTreeMap::new(),
            sources: BTreeMap::new(),
        }
    }
}

pub(crate) struct SourceInstance {
    pub(crate) kind: Identifier,
    pub(crate) name: String,
    pub(crate) source: Box<dyn Source>,
    /// Filters belonging to the shared source definition rather than one
    /// scene item. Every scene reference sees the same compiled chain.
    pub(crate) filters: Vec<FrameFilter>,
    /// The newest frame this source produced.
    ///
    /// A live device drops frames — a camera that is reconnecting, a portal
    /// stream that stalls for a moment. Holding the last good frame lets the
    /// compositor keep drawing the layer instead of making it disappear and
    /// reappear. Frame storage is reference-counted, so this costs a pointer.
    pub(crate) last_frame: Option<VideoFrame>,
    /// Why this source's last render failed, cleared by the next good frame.
    pub(crate) failure: Option<String>,
}

/// The per-scene compositing state of one attached source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SceneItem {
    pub(crate) source: super::SourceId,
    pub(crate) transform: FrameTransform,
}

impl SceneItem {
    fn new(source: super::SourceId) -> Self {
        Self {
            source,
            transform: FrameTransform::IDENTITY,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Scene {
    /// Composition order. The compositor walks this and nothing else, which is
    /// what keeps rendering deterministic while the lookups below are hashed.
    pub(crate) sources: Vec<super::SourceId>,
    /// O(1) membership mirror of `sources`, kept in step by [`Scene::attach`]
    /// and [`Scene::detach`].
    pub(crate) attached: HashSet<super::SourceId>,
    /// Scene-item transform in the same order as `sources`. Filters live on
    /// the shared source instance above and are intentionally absent here.
    /// Keeping this as an ordered vector allows two items to reference one
    /// capture source while retaining independent transforms.
    pub(crate) items: Vec<SceneItem>,
}

impl Scene {
    pub(crate) fn new() -> Self {
        Self {
            sources: Vec::new(),
            attached: HashSet::new(),
            items: Vec::new(),
        }
    }

    /// Appends `source` to the composition order.
    ///
    /// Returns `false` when the source is already attached.
    pub(crate) fn attach(&mut self, source: super::SourceId) -> bool {
        if !self.attached.insert(source) {
            return false;
        }
        self.sources.push(source);
        self.items.push(SceneItem::new(source));
        true
    }

    /// Appends a source reference even when that source is already present.
    /// The source device remains shared; only the scene-item transform is new.
    pub(crate) fn attach_instance(&mut self, source: super::SourceId) -> usize {
        self.attached.insert(source);
        self.sources.push(source);
        self.items.push(SceneItem::new(source));
        self.sources.len() - 1
    }

    /// Removes `source` while keeping the shared source definition alive.
    ///
    /// Returns `None` when the source is not attached. The ordered `sources`
    /// vector is shifted rather than swap-removed because composition order is
    /// part of the rendering contract.
    pub(crate) fn detach(&mut self, source: super::SourceId) -> Option<()> {
        let index = self
            .sources
            .iter()
            .position(|candidate| *candidate == source)?;
        self.sources.remove(index);
        self.items.remove(index);
        if !self.sources.contains(&source) {
            self.attached.remove(&source);
        }
        Some(())
    }
}
