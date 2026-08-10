use obs_rs_util::Identifier;

use super::error::ProjectError;
pub(crate) fn source_id(input: &str) -> Result<Identifier, ProjectError> {
    identifier(input, "source id")
}

pub(crate) fn identifier(input: &str, kind: &'static str) -> Result<Identifier, ProjectError> {
    Identifier::new(input).map_err(|error| ProjectError::InvalidIdentifier { kind, error })
}

pub(crate) fn fields<'a>(
    line: &'a str,
    line_number: usize,
    expected_kind: &str,
    expected_count: usize,
) -> Result<Vec<&'a str>, ProjectError> {
    let values = line.split('|').collect::<Vec<_>>();
    if values.len() != expected_count || values[0] != expected_kind {
        return Err(ProjectError::InvalidDocument {
            line: line_number,
            reason: format!("expected {expected_kind} record with {expected_count} fields"),
        });
    }
    Ok(values)
}
pub(crate) fn parse_flag(
    value: &str,
    line: usize,
    field: &'static str,
) -> Result<bool, ProjectError> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(ProjectError::InvalidDocument {
            line,
            reason: format!("invalid {field}; expected 0 or 1"),
        }),
    }
}

pub(crate) fn number<T>(value: &str, line: usize, field: &'static str) -> Result<T, ProjectError>
where
    T: std::str::FromStr,
{
    value.parse().map_err(|_| ProjectError::InvalidDocument {
        line,
        reason: format!("invalid {field}"),
    })
}
