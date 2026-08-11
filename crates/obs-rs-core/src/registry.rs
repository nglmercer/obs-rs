use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use obs_rs_media::{FrameFilter, FrameTransform};
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
}

/// The per-scene compositing state of one attached source.
///
/// The transform and the filter chain live together so the compositor resolves
/// both with a single map lookup per source per frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SceneItem {
    pub(crate) transform: FrameTransform,
    pub(crate) filters: Vec<FrameFilter>,
}

impl SceneItem {
    fn new() -> Self {
        Self {
            transform: FrameTransform::IDENTITY,
            // `Vec::new` does not allocate; the first filter push does.
            filters: Vec::new(),
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
    /// Transform and filter chain per attached source.
    pub(crate) items: HashMap<super::SourceId, SceneItem>,
}

impl Scene {
    pub(crate) fn new() -> Self {
        Self {
            sources: Vec::new(),
            attached: HashSet::new(),
            items: HashMap::new(),
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
        self.items.insert(source, SceneItem::new());
        true
    }

    /// Removes `source`, returning the number of filters it carried.
    ///
    /// Returns `None` when the source is not attached. The ordered `sources`
    /// vector is shifted rather than swap-removed because composition order is
    /// part of the rendering contract.
    pub(crate) fn detach(&mut self, source: super::SourceId) -> Option<usize> {
        if !self.attached.remove(&source) {
            return None;
        }
        if let Some(index) = self
            .sources
            .iter()
            .position(|candidate| *candidate == source)
        {
            self.sources.remove(index);
        }
        Some(self.items.remove(&source).map_or(0, |item| {
            item.filters.len()
        }))
    }

    /// Returns the total number of filters across every item in this scene.
    pub(crate) fn filter_count(&self) -> usize {
        self.items
            .values()
            .fold(0_usize, |total, item| total.saturating_add(item.filters.len()))
    }
}
