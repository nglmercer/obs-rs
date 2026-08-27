//! Renderer support shared by runtime construction and GUI source catalogs.

use std::{collections::HashSet, rc::Rc};

use obs_rs_builtins::BuiltinPlugin;
use obs_rs_plugin_api::Plugin;
use obs_rs_project::{Profile, Project, SceneItemSpec};

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

pub(super) fn builtin_plugin() -> Rc<BuiltinPlugin> {
    BUILTIN_PLUGIN.with(Rc::clone)
}

/// Returns a project holding only `project`'s active profile identity.
///
/// [`PreviewRenderer::new`] starts from this so the first build runs through
/// the same diff as every later update.
pub(super) fn empty_project(project: &Project) -> Project {
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
pub(super) fn static_scenes(profile: &Profile) -> HashSet<String> {
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
