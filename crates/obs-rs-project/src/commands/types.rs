//! Public project mutation command values.

use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameTransform, TransitionSpec, VideoFormat};
use obs_rs_output::OutputProfileKind;

use super::super::model::{
    Profile, RenderBackendPreference, SceneItemSpec, SceneSpec, SourceFilterSpec, SourceSpec,
};

/// How a scene item is copied when a reference is pasted or duplicated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneItemDuplicateMode {
    /// Add another item pointing at the existing source definition.
    Reference,
    /// Clone the source definition and add an item pointing at the clone.
    DuplicateSource,
}
/// Commands that mutate project state through one validated path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectCommand {
    /// Adds a profile.
    AddProfile(Profile),
    /// Selects an existing profile.
    SetActiveProfile { id: String },
    /// Replaces one profile's canvas resolution and frame rate.
    SetProfileVideoFormat {
        profile: String,
        format: VideoFormat,
    },
    /// Selects the preferred renderer while retaining runtime fallback.
    SetProfileRenderBackend {
        profile: String,
        backend: RenderBackendPreference,
    },
    /// Selects an exact output profile for runtime capability negotiation.
    SetProfileOutput {
        profile: String,
        output: OutputProfileKind,
    },
    /// Adds a scene to a profile.
    AddScene { profile: String, scene: SceneSpec },
    /// Duplicates one scene with a fresh project-local ID.
    DuplicateScene { profile: String, scene: String },
    /// Duplicates one scene and chooses whether its items keep source
    /// references or receive cloned source definitions.
    DuplicateSceneWithMode {
        profile: String,
        scene: String,
        mode: SceneItemDuplicateMode,
    },
    /// Adds a source to the profile registry and creates its first scene item.
    AddSource {
        profile: String,
        scene: String,
        source: SourceSpec,
    },
    /// Adds an item referencing an already registered source.
    AddSceneItem {
        profile: String,
        scene: String,
        item: SceneItemSpec,
    },
    /// Atomically wraps two or more scene items from the same parent in a new
    /// group. Item targets use root IDs or outer-to-inner paths such as
    /// `overlay-group/source`.
    GroupSceneItems {
        profile: String,
        scene: String,
        /// Scene-item targets in any selection order; the command restores
        /// their existing parent order inside the new group.
        items: Vec<String>,
        group: SceneItemSpec,
    },
    /// Atomically expands one unlocked group back into its parent. The target
    /// is a root group ID or an outer-to-inner group path.
    UngroupSceneItem {
        profile: String,
        scene: String,
        group: String,
    },
    /// Removes one scene item while retaining the source definition.
    RemoveSceneItem {
        profile: String,
        scene: String,
        item: String,
    },
    /// Removes several root or nested scene items as one atomic edit.
    ///
    /// Targets use root IDs or outer-to-inner paths such as
    /// `overlay-group/source`. Selecting both a group and one of its
    /// descendants removes the group once; all selected targets are validated
    /// before any item is changed.
    RemoveSceneItems {
        profile: String,
        scene: String,
        items: Vec<String>,
    },
    /// Duplicates one scene item as a reference or with a cloned source.
    DuplicateSceneItem {
        profile: String,
        scene: String,
        item: String,
        mode: SceneItemDuplicateMode,
    },
    /// Pastes a previously copied scene item into a scene as a reference or
    /// with a cloned source definition.
    PasteSceneItem {
        profile: String,
        scene: String,
        item: SceneItemSpec,
        mode: SceneItemDuplicateMode,
    },
    /// Pastes a previously copied scene item into a group as a reference or
    /// with a cloned source definition.
    PasteGroupItem {
        profile: String,
        scene: String,
        /// Outermost-to-innermost group scene-item IDs.
        group_path: Vec<String>,
        item: SceneItemSpec,
        mode: SceneItemDuplicateMode,
    },
    /// Removes a scene from a profile.
    RemoveScene { profile: String, scene: String },
    /// Renames a scene in a profile.
    SetSceneName {
        profile: String,
        scene: String,
        name: String,
    },
    /// Replaces a scene name and its optional transition policy as one edit.
    ///
    /// Keeping these fields together makes the Scene properties dialog one
    /// undoable operation instead of creating a history entry for each field.
    SetSceneProperties {
        profile: String,
        scene: String,
        name: String,
        transition: Option<TransitionSpec>,
    },
    /// Replaces the optional transition policy used when a scene is taken to
    /// program. `None` restores inheritance from the desktop transition.
    SetSceneTransitionOverride {
        profile: String,
        scene: String,
        transition: Option<TransitionSpec>,
    },
    /// Moves one scene to an existing position in the profile scene order.
    MoveScene {
        profile: String,
        scene: String,
        target_index: usize,
    },
    /// Duplicates one source definition without attaching it to a scene.
    DuplicateSource { profile: String, source: String },
    /// Replaces one source's display name.
    SetSourceName {
        profile: String,
        source: String,
        name: String,
    },
    /// Replaces a group's display name using its outermost-to-innermost
    /// scene-item path.
    SetGroupName {
        profile: String,
        scene: String,
        group_path: Vec<String>,
        name: String,
    },
    /// Replaces one source's validated settings document.
    SetSourceSettings {
        profile: String,
        source: String,
        settings: Config,
    },
    /// Replaces one scene item's transform.
    SetSceneItemTransform {
        profile: String,
        scene: String,
        item: String,
        transform: FrameTransform,
    },
    /// Replaces several scene-item transforms as one atomic undoable edit.
    ///
    /// Canvas multi-selection uses this command so moving a group never
    /// creates one history entry per item.
    SetSceneItemTransforms {
        profile: String,
        scene: String,
        items: Vec<(String, FrameTransform)>,
    },
    /// Adds one persistent source filter instance.
    AddSourceFilter {
        profile: String,
        source: String,
        filter: SourceFilterSpec,
    },
    /// Removes one persistent source filter instance.
    RemoveSourceFilter {
        profile: String,
        source: String,
        filter: String,
    },
    /// Replaces one source filter's display name.
    SetSourceFilterName {
        profile: String,
        source: String,
        filter: String,
        name: String,
    },
    /// Enables or disables one source filter instance.
    SetSourceFilterEnabled {
        profile: String,
        source: String,
        filter: String,
        enabled: bool,
    },
    /// Replaces one source filter's independent settings document.
    SetSourceFilterSettings {
        profile: String,
        source: String,
        filter: String,
        settings: Config,
    },
    /// Moves one source filter to an existing order position.
    MoveSourceFilter {
        profile: String,
        source: String,
        filter: String,
        target_index: usize,
    },
    /// Reorders one scene item within a scene.
    MoveSceneItem {
        profile: String,
        scene: String,
        item: String,
        target_index: usize,
    },
    /// Moves one scene item between the scene root and an existing group.
    ///
    /// `item` is a root ID or an outer-to-inner path such as
    /// `overlay-group/source`. `destination` is the enclosing group path;
    /// an empty path means the scene root. The item keeps its stable ID,
    /// transform, visibility, and lock state.
    MoveSceneItemToParent {
        profile: String,
        scene: String,
        item: String,
        destination: Vec<String>,
        target_index: usize,
    },
    /// Removes one source item from a scene.
    RemoveSource { profile: String, source: String },
    /// Replaces the ordered filter chain for one source.
    SetSourceFilters {
        profile: String,
        source: String,
        filters: Vec<FrameFilter>,
    },
    /// Changes whether one scene item participates in scene composition.
    /// `item` may be a root ID or a stable flattened group/Scene-reference
    /// path; nested paths are written to the scene that owns the leaf.
    SetSceneItemVisibility {
        profile: String,
        scene: String,
        item: String,
        visible: bool,
    },
    /// Changes whether one scene item is protected from desktop editing.
    /// `item` accepts the same stable flattened paths as visibility.
    SetSceneItemLocked {
        profile: String,
        scene: String,
        item: String,
        locked: bool,
    },
    /// Changes visibility for a child item addressed by its enclosing group path.
    SetGroupItemVisibility {
        profile: String,
        scene: String,
        /// Outermost-to-innermost group scene-item IDs.
        group_path: Vec<String>,
        item: String,
        visible: bool,
    },
    /// Changes lock state for a child item addressed by its enclosing group path.
    SetGroupItemLocked {
        profile: String,
        scene: String,
        /// Outermost-to-innermost group scene-item IDs.
        group_path: Vec<String>,
        item: String,
        locked: bool,
    },
    /// Replaces a child transform addressed by its enclosing group path.
    ///
    /// The command validates composition against the current nested-group
    /// boundary before mutating the project, so a transform that the runtime
    /// cannot flatten is rejected atomically.
    SetGroupItemTransform {
        profile: String,
        scene: String,
        /// Outermost-to-innermost group scene-item IDs.
        group_path: Vec<String>,
        item: String,
        transform: FrameTransform,
    },
    /// Reorders a child item within its enclosing group.
    MoveGroupItem {
        profile: String,
        scene: String,
        /// Outermost-to-innermost group scene-item IDs.
        group_path: Vec<String>,
        item: String,
        target_index: usize,
    },
    /// Removes a child item from its enclosing group while retaining any
    /// profile-wide source definition.
    RemoveGroupItem {
        profile: String,
        scene: String,
        /// Outermost-to-innermost group scene-item IDs.
        group_path: Vec<String>,
        item: String,
    },
    /// Duplicates a child item inside its enclosing group as a reference or
    /// with a cloned profile source definition.
    DuplicateGroupItem {
        profile: String,
        scene: String,
        /// Outermost-to-innermost group scene-item IDs.
        group_path: Vec<String>,
        item: String,
        mode: SceneItemDuplicateMode,
    },
}
