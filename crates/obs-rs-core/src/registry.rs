use std::{collections::BTreeMap, sync::Arc};

use obs_rs_media::{FrameFilter, FrameTransform};
use obs_rs_plugin_api::{PluginManifest, Source, SourceFactory};
use obs_rs_util::Identifier;

pub(crate) struct Registry {
    pub(crate) plugins: BTreeMap<Identifier, PluginManifest>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Scene {
    pub(crate) sources: Vec<super::SourceId>,
    pub(crate) transforms: BTreeMap<super::SourceId, FrameTransform>,
    pub(crate) filters: BTreeMap<super::SourceId, Vec<FrameFilter>>,
}
