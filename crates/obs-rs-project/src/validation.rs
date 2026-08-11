use obs_rs_util::Identifier;

use super::error::ProjectError;
pub(crate) fn source_id(input: &str) -> Result<Identifier, ProjectError> {
    identifier(input, "source id")
}

pub(crate) fn identifier(input: &str, kind: &'static str) -> Result<Identifier, ProjectError> {
    Identifier::new(input).map_err(|error| ProjectError::InvalidIdentifier { kind, error })
}
