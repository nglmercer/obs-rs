//! Host-specific project compatibility migrations.
//!
//! Project documents intentionally keep source IDs stable across hosts, but a
//! native source kind can be unavailable after moving a document to another
//! operating system. Migrations live at the GUI load boundary so the portable
//! project model does not need to know about Windows or Linux source aliases.

#[cfg(target_os = "windows")]
use obs_rs_config::Config;
use obs_rs_project::{Project, ProjectFileStore};
use obs_rs_ui::{DesktopState, UiError};
#[cfg(target_os = "windows")]
use obs_rs_util::Identifier;

/// Converts platform-specific capture sources to the equivalent Windows
/// Graphics Capture source kinds.
///
/// The returned boolean is true when the serialized project was changed. A
/// migrated project is deliberately left dirty by the state-loading wrapper,
/// allowing the user to review target choices and save the portable Windows
/// representation explicitly.
pub(crate) fn migrate_project_for_host(mut project: Project) -> (Project, bool) {
    #[cfg(target_os = "windows")]
    {
        let mut migrated_sources = 0_usize;
        let profile_ids: Vec<Identifier> = project
            .profiles()
            .map(|profile| profile.id().clone())
            .collect();

        for profile_id in profile_ids {
            let source_ids: Vec<Identifier> = project
                .profile(&profile_id)
                .into_iter()
                .flat_map(obs_rs_project::Profile::sources)
                .filter(|source| windows_capture_kind(source.kind().as_str()).is_some())
                .map(|source| source.id().clone())
                .collect();

            let Some(profile) = project.profile_mut(&profile_id) else {
                continue;
            };
            for source_id in source_ids {
                let Some(source) = profile.source_mut(&source_id) else {
                    continue;
                };
                let current_kind = source.kind().to_string();
                let Some(target_kind) = windows_capture_kind(&current_kind) else {
                    continue;
                };
                let mut settings = source.settings().clone();
                let mut changed = current_kind != target_kind;

                for key in ["restore_token", "display", "monitor", "window"] {
                    changed |= settings.remove(key).is_some();
                }

                let expected_prefix = if target_kind == "screen_capture" {
                    "wgc-screen-"
                } else {
                    "wgc-window-"
                };
                let has_compatible_target = settings
                    .get("device_id")
                    .is_some_and(|device_id| device_id.starts_with(expected_prefix));
                if !has_compatible_target {
                    let picker = if target_kind == "screen_capture" {
                        "wgc-screen-picker"
                    } else {
                        "wgc-window-picker"
                    };
                    set_setting(&mut settings, "device_id", picker);
                    changed = true;
                }
                if settings.get("capture_cursor").is_none() {
                    set_setting(&mut settings, "capture_cursor", "true");
                    changed = true;
                }
                if settings.get("capture_border").is_none() {
                    set_setting(&mut settings, "capture_border", "false");
                    changed = true;
                }

                if changed {
                    source
                        .set_kind(target_kind)
                        .expect("Windows capture migration uses a valid source kind");
                    source.set_settings(settings);
                    migrated_sources += 1;
                }
            }
        }

        (project, migrated_sources != 0)
    }

    #[cfg(not(target_os = "windows"))]
    {
        (project, false)
    }
}

#[cfg(target_os = "windows")]
fn windows_capture_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "wayland_screen_capture" | "x11_screen_capture" => Some("screen_capture"),
        "wayland_window_capture" | "x11_window_capture" => Some("window_capture"),
        _ => None,
    }
}

/// Returns the source kind that the current host can actually instantiate.
///
/// Projects created by older Linux builds can still be present in memory when
/// a user upgrades the application without reopening the project. Keeping
/// this small, non-mutating normalization alongside the load migration lets
/// property dialogs, discovery, and the preview use the Windows backend even
/// during that transition. The load migration remains responsible for writing
/// the canonical kind back to the project file.
pub(crate) fn host_source_kind(kind: &str) -> &str {
    let kind = kind.trim();
    #[cfg(target_os = "windows")]
    {
        windows_capture_kind(kind).unwrap_or(kind)
    }
    #[cfg(not(target_os = "windows"))]
    {
        kind
    }
}

#[cfg(target_os = "windows")]
fn set_setting(settings: &mut Config, key: &str, value: &str) {
    settings
        .set(key, value)
        .expect("Windows capture migration uses valid settings");
}

/// Loads a keyed project through the host migration boundary.
pub(crate) fn load_project_for_host(
    state: &mut DesktopState,
    store: &ProjectFileStore,
    selection_key: &str,
) -> Result<bool, UiError> {
    state.load_project_for_key_with(store, selection_key, migrate_project_for_host)
}

/// Recovers a keyed project through the host migration boundary.
pub(crate) fn recover_project_for_host(
    state: &mut DesktopState,
    store: &ProjectFileStore,
    selection_key: &str,
) -> Result<bool, UiError> {
    state.recover_project_for_key_with(store, selection_key, migrate_project_for_host)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use obs_rs_media::{FrameRate, VideoFormat};
    use obs_rs_project::{Profile, SceneItemSpec, SceneSpec, SourceSpec};

    #[test]
    fn migrates_wayland_screen_and_preserves_scene_references() {
        let format = VideoFormat::new(1_280, 720, FrameRate::new(30, 1).expect("frame rate"))
            .expect("video format");
        let mut project = Project::new("migration").expect("project");
        let mut profile = Profile::new("live", "Live", format).expect("profile");
        let mut settings = Config::new();
        set_setting(&mut settings, "restore_token", "portal-token");
        set_setting(&mut settings, "width", "1280");
        let source = SourceSpec::new("screen", "wayland_screen_capture", "Screen", settings)
            .expect("source");
        profile.add_source(source).expect("source registry");
        let mut scene = SceneSpec::new("preview", "Preview").expect("scene");
        scene
            .add_item(SceneItemSpec::for_source("screen").expect("scene item"))
            .expect("scene item registry");
        profile.add_scene(scene).expect("scene registry");
        project.add_profile(profile).expect("profile registry");

        let (project, changed) = migrate_project_for_host(project);
        assert!(changed);
        let source = project
            .profile("live")
            .expect("profile")
            .source("screen")
            .expect("source");
        assert_eq!(source.kind().as_str(), "screen_capture");
        assert_eq!(
            source.settings().get("device_id"),
            Some("wgc-screen-picker")
        );
        assert_eq!(source.settings().get("capture_cursor"), Some("true"));
        assert_eq!(source.settings().get("capture_border"), Some("false"));
        assert_eq!(source.settings().get("restore_token"), None);
        assert!(project
            .profile("live")
            .expect("profile")
            .scene("preview")
            .expect("scene")
            .item("screen")
            .is_some());
    }

    #[test]
    fn leaves_native_windows_sources_unchanged() {
        let format = VideoFormat::new(1_280, 720, FrameRate::new(30, 1).expect("frame rate"))
            .expect("video format");
        let mut project = Project::new("migration").expect("project");
        let mut profile = Profile::new("live", "Live", format).expect("profile");
        let mut settings = Config::new();
        set_setting(&mut settings, "device_id", "wgc-screen-1");
        let source =
            SourceSpec::new("screen", "screen_capture", "Screen", settings).expect("source");
        profile.add_source(source).expect("source registry");
        project.add_profile(profile).expect("profile registry");

        let original = project.clone();
        let (project, changed) = migrate_project_for_host(project);
        assert!(!changed);
        assert_eq!(project, original);
    }

    #[test]
    fn normalizes_legacy_capture_kinds_without_mutating_the_input() {
        assert_eq!(
            host_source_kind(" wayland_screen_capture "),
            "screen_capture"
        );
        assert_eq!(host_source_kind("x11_window_capture"), "window_capture");
        assert_eq!(host_source_kind("color_source"), "color_source");
    }
}
