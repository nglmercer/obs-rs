//! Source selection, clipboard, and nested scene-item operations.

use std::collections::HashSet;

use super::super::helpers::identifier;
use super::{DesktopState, UiError};
use obs_rs_project::{ProjectCommand, SceneItemDuplicateMode, SceneItemSpec, SceneSpec};

impl DesktopState {
    pub(super) fn paste_source(
        &mut self,
        mode: SceneItemDuplicateMode,
        target: &str,
    ) -> Result<&'static str, UiError> {
        let item = self
            .clipboard
            .clone()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "clipboard",
                id: "none".to_owned(),
            })?;
        let profile = self.project.project().active_profile().to_string();
        let scene = self
            .preview_scene
            .as_ref()
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: "none".to_owned(),
            })?
            .to_string();
        let group_path = self.paste_group_path(&scene, target)?;
        let paste_at_root = group_path.is_empty();
        let before = self
            .project
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene(scene.as_str()))
            .map(|scene| {
                scene
                    .items()
                    .iter()
                    .map(|item| item.id().clone())
                    .collect::<HashSet<_>>()
            })
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: scene.clone(),
            })?;
        if paste_at_root {
            self.project.dispatch(ProjectCommand::PasteSceneItem {
                profile,
                scene: scene.clone(),
                item,
                mode,
            })?;
        } else {
            self.project.dispatch(ProjectCommand::PasteGroupItem {
                profile,
                scene: scene.clone(),
                group_path,
                item,
                mode,
            })?;
        }
        if paste_at_root {
            // The root paste path has a stable top-level selection affordance;
            // nested rows are deliberately not part of the canvas selection
            // model yet, so leave that selection untouched.
            let pasted_id = self
                .project
                .project()
                .active_profile_spec()
                .and_then(|profile| profile.scene(scene.as_str()))
                .and_then(|scene| {
                    scene
                        .items()
                        .iter()
                        .find(|item| !before.contains(item.id()))
                })
                .map(|item| item.id().clone());
            let pasted_id = pasted_id.ok_or_else(|| UiError::UnknownSelection {
                kind: "pasted source",
                id: scene.clone(),
            })?;
            self.selected_sources.clear();
            self.selected_sources.push(pasted_id);
        }
        self.sync_selections_after_project_update();
        Ok(match mode {
            SceneItemDuplicateMode::Reference => "source reference pasted",
            SceneItemDuplicateMode::DuplicateSource => "source duplicate pasted",
        })
    }

    fn paste_group_path(&self, scene_id: &str, target: &str) -> Result<Vec<String>, UiError> {
        if target.is_empty() {
            return Ok(Vec::new());
        }
        let scene = self
            .project
            .project()
            .active_profile_spec()
            .and_then(|profile| profile.scene(scene_id))
            .ok_or_else(|| UiError::UnknownSelection {
                kind: "scene",
                id: scene_id.to_owned(),
            })?;
        let parts = target_parts(target).ok_or_else(|| UiError::UnknownSelection {
            kind: "scene item",
            id: target.to_owned(),
        })?;
        let item = scene_item_at_parts(scene, &parts).ok_or_else(|| UiError::UnknownSelection {
            kind: "scene item",
            id: target.to_owned(),
        })?;
        let end = if item.is_group() {
            parts.len()
        } else {
            parts.len().saturating_sub(1)
        };
        Ok(parts[..end].iter().map(|part| (*part).to_owned()).collect())
    }

    pub(super) fn select_one_source(&mut self, id: &str) -> Result<(), UiError> {
        self.ensure_source(id)?;
        self.selected_sources.clear();
        self.selected_sources.push(identifier(id, "source")?);
        Ok(())
    }

    pub(super) fn toggle_source_selection(&mut self, id: &str) -> Result<(), UiError> {
        self.ensure_source(id)?;
        if let Some(index) = self
            .selected_sources
            .iter()
            .position(|selected| selected.as_str() == id)
        {
            self.selected_sources.remove(index);
        } else if self.selected_sources.len() < crate::MAX_CANVAS_SELECTIONS {
            self.selected_sources.push(identifier(id, "source")?);
        }
        Ok(())
    }

    pub(super) fn select_sources(
        &mut self,
        ids: Vec<String>,
        additive: bool,
    ) -> Result<(), UiError> {
        let mut validated = Vec::with_capacity(ids.len().min(crate::MAX_CANVAS_SELECTIONS));
        for id in ids.into_iter().take(crate::MAX_CANVAS_SELECTIONS) {
            self.ensure_source(&id)?;
            let id = identifier(&id, "source")?;
            if !validated.contains(&id) {
                validated.push(id);
            }
        }
        let mut next = if additive {
            self.selected_sources.clone()
        } else {
            Vec::with_capacity(validated.len())
        };
        for id in validated {
            if !next.contains(&id) && next.len() < crate::MAX_CANVAS_SELECTIONS {
                next.push(id);
            }
        }
        self.selected_sources = next;
        Ok(())
    }
}

const MAX_SCENE_ITEM_PATH_DEPTH: usize = 64;

fn target_parts(target: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::with_capacity(4);
    for part in target.split('/') {
        if part.is_empty() || parts.len() >= MAX_SCENE_ITEM_PATH_DEPTH {
            return None;
        }
        parts.push(part);
    }
    (!parts.is_empty()).then_some(parts)
}

pub(super) fn scene_item_at_target<'a>(
    scene: &'a SceneSpec,
    target: &str,
) -> Option<&'a SceneItemSpec> {
    let parts = target_parts(target)?;
    scene_item_at_parts(scene, &parts)
}

fn scene_item_at_parts<'a>(scene: &'a SceneSpec, parts: &[&str]) -> Option<&'a SceneItemSpec> {
    let mut items = scene.items();
    for (index, part) in parts.iter().enumerate() {
        let item = items.iter().find(|item| item.id().as_str() == *part)?;
        if index + 1 == parts.len() {
            return Some(item);
        }
        items = item.group()?.items();
    }
    None
}
