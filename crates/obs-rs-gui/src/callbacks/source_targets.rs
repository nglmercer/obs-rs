//! Stable scene-item and profile-source target resolution.

use std::{cell::RefCell, rc::Rc};

use obs_rs_ui::DesktopState;

use crate::callbacks::canvas::{canvas_item_for_target, canvas_target_is_locked_in_profile};

/// A stable reference to one scene item and the source definition it shows.
///
/// Anything that outlives the click that started it — a dialog the user leaves
/// open, a portal handshake, a pointer gesture — has to carry one of these. The
/// alternative is asking "what is selected?" when the work finishes, which is a
/// different answer by then often enough to matter: it is how a screen
/// capture's portal token ends up on a camera.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceTarget {
    pub(crate) profile: String,
    pub(crate) scene: String,
    /// The scene item, which is what a transform and the dock selection name.
    pub(crate) item: String,
    /// The profile-wide source definition, which is what settings belong to.
    pub(crate) source: String,
}

/// A stable reference to any scene item, including scene and group targets.
///
/// Transform dialogs need only the scene-item address; source properties and
/// filters use [`SourceTarget`] because they additionally require a
/// profile-wide source definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SceneItemTarget {
    pub(crate) profile: String,
    pub(crate) scene: String,
    pub(crate) item: String,
}

/// Returns whether a scene-item target is locked, including group and
/// Scene-source ancestors addressed by its outer-to-inner path.
pub(crate) fn scene_item_target_is_locked(state: &DesktopState, target: &SceneItemTarget) -> bool {
    let project = state.project_session().project();
    let Some(profile) = project.profile(target.profile.as_str()) else {
        return true;
    };
    canvas_target_is_locked_in_profile(profile, target.scene.as_str(), target.item.as_str())
}

/// Resolves one preview-scene item, regardless of whether it points at a
/// source, nested scene, or embedded group.
pub(crate) fn scene_item_target(state: &DesktopState, item: &str) -> Option<SceneItemTarget> {
    let project = state.project_session().project();
    let scene = state.preview_scene()?.to_owned();
    let profile = project.active_profile_spec()?;
    canvas_item_for_target(profile, scene.as_str(), item)?;
    Some(SceneItemTarget {
        profile: project.active_profile().to_string(),
        scene,
        item: item.to_owned(),
    })
}

/// Resolves one scene item in the preview scene to a stable source target.
pub(crate) fn source_target(state: &DesktopState, item: &str) -> Option<SourceTarget> {
    let project = state.project_session().project();
    let scene = state.preview_scene()?.to_owned();
    let profile = project.active_profile_spec()?;
    let source = canvas_item_for_target(profile, scene.as_str(), item)?;
    let source = source.is_source().then(|| source.source_id().to_string())?;
    Some(SourceTarget {
        profile: project.active_profile().to_string(),
        scene,
        item: item.to_owned(),
        source,
    })
}

/// Resolves the selected scene item to a stable target.
pub(crate) fn selected_target(state: &DesktopState) -> Option<SourceTarget> {
    source_target(state, state.selected_source()?)
}

/// Returns the lock state for a target that may be a nested group path.
pub(crate) fn source_target_is_locked(state: &DesktopState, target: &SourceTarget) -> bool {
    let project = state.project_session().project();
    let Some(profile) = project.profile(target.profile.as_str()) else {
        return true;
    };
    canvas_target_is_locked_in_profile(profile, target.scene.as_str(), target.item.as_str())
}

/// Returns a target's settings document from the live project.
pub(crate) fn target_settings_document(
    state: &Rc<RefCell<DesktopState>>,
    target: &SourceTarget,
) -> Option<String> {
    let state = state.borrow();
    let session = state.project_session();
    let profile = session.project().profile(target.profile.as_str())?;
    Some(
        profile
            .source(target.source.as_str())?
            .settings()
            .serialize(),
    )
}
