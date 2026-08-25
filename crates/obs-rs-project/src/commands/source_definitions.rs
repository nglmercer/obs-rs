//! Commands that mutate profile-wide source definitions.

use super::super::{
    error::ProjectError,
    model::Project,
    validation::{identifier, source_id},
};
use obs_rs_config::Config;
use obs_rs_util::{Identifier, MAX_IDENTIFIER_BYTES};

pub(super) fn set_source_settings(
    project: &mut Project,
    profile: &str,
    source: &str,
    settings: Config,
) -> Result<(), ProjectError> {
    super::source_mut(project, profile, source)?.set_settings(settings);
    Ok(())
}

pub(super) fn duplicate_source(
    project: &mut Project,
    profile: &str,
    source: &str,
) -> Result<(), ProjectError> {
    let source_id = source_id(source)?;
    let profile_id = identifier(profile, "profile id")?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let original = profile
        .source(&source_id)
        .cloned()
        .ok_or_else(|| ProjectError::UnknownSource(source_id.clone()))?;
    let (id, name) = copy_identity(original.id().as_str(), original.name(), |candidate| {
        profile.has_source(candidate)
    })?;
    let mut duplicate = original;
    duplicate.id = id;
    duplicate.name = name;
    profile.add_source(duplicate)
}

pub(super) fn set_source_name(
    project: &mut Project,
    profile: &str,
    source: &str,
    name: &str,
) -> Result<(), ProjectError> {
    super::source_mut(project, profile, source)?.set_name(name)
}

/// Builds a deterministic copy ID and display name without letting the GUI
/// invent identifiers or bypass the project's validation rules.
pub(crate) fn copy_identity(
    base_id: &str,
    base_name: &str,
    is_taken: impl Fn(&str) -> bool,
) -> Result<(Identifier, String), ProjectError> {
    for ordinal in 1..=10_000_u32 {
        let suffix = if ordinal == 1 {
            "_copy".to_owned()
        } else {
            format!("_copy_{ordinal}")
        };
        let prefix_length = MAX_IDENTIFIER_BYTES.saturating_sub(suffix.len());
        let prefix = base_id
            .get(..base_id.len().min(prefix_length))
            .unwrap_or(base_id);
        let candidate = format!("{prefix}{suffix}");
        if !is_taken(&candidate) {
            let id =
                Identifier::new(&candidate).map_err(|error| ProjectError::InvalidIdentifier {
                    kind: "duplicate id",
                    error,
                })?;
            let name = if ordinal == 1 {
                format!("{base_name} Copy")
            } else {
                format!("{base_name} Copy {ordinal}")
            };
            return Ok((id, name));
        }
    }
    Err(ProjectError::InvalidIdentifier {
        kind: "duplicate id",
        error: obs_rs_util::IdentifierError::TooLong,
    })
}

pub(super) fn remove_source(
    project: &mut Project,
    profile: &str,
    source: &str,
) -> Result<(), ProjectError> {
    let source = source_id(source)?;
    let profile_id = identifier(profile, "profile id")?;
    project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?
        .remove_source(&source)
        .map(|_| ())
}
