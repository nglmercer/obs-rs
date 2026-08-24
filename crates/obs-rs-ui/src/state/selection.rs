//! Source selection, clipboard, and nested scene-item operations.

use std::collections::HashSet;

use super::super::helpers::{scene_item_at_parts, scene_item_target_parts};
use super::{DesktopState, UiError};
use obs_rs_project::{ProjectCommand, SceneItemDuplicateMode};

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
            // A root paste always yields a top-level target, which is also the
            // selection target used by the canvas for this operation.
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
            self.selected_sources.push(pasted_id.to_string());
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
        let parts = scene_item_target_parts(target).ok_or_else(|| UiError::UnknownSelection {
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
        self.selected_sources.push(id.to_owned());
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
            self.selected_sources.push(id.to_owned());
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
