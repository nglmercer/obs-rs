use super::{
    error::ProjectError,
    model::{Profile, Project, SceneSpec, SourceSpec},
    validation::{fields, identifier, number, parse_flag},
    MAGIC, MAX_PROJECT_BYTES,
};
use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, VideoFormat};

/// Rough serialized bytes contributed by one source record.
///
/// Used only to size the output buffer; being approximate costs at most one
/// growth, while `String::new` guaranteed several.
const SOURCE_RECORD_ESTIMATE: usize = 160;

/// Rough serialized bytes contributed by one profile or scene record.
const RECORD_ESTIMATE: usize = 96;

impl Project {
    /// Estimates the serialized length so the document buffer is reserved once.
    fn serialized_size_estimate(&self) -> usize {
        self.profiles
            .values()
            .fold(MAGIC.len() + RECORD_ESTIMATE, |total, profile| {
                profile.scenes().fold(
                    total.saturating_add(RECORD_ESTIMATE),
                    |profile_total, scene| {
                        profile_total
                            .saturating_add(RECORD_ESTIMATE)
                            .saturating_add(
                                scene.sources().len().saturating_mul(SOURCE_RECORD_ESTIMATE),
                            )
                    },
                )
            })
    }

    /// Serializes the project into a deterministic, escaped line format.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut document = String::with_capacity(self.serialized_size_estimate());
        document.push_str(MAGIC);
        document.push('\n');
        document.push_str("project|");
        document.push_str(&escape(&self.title));
        document.push('|');
        document.push_str(self.active_profile.as_str());
        document.push('\n');
        for profile in self.profiles.values() {
            let format = profile.video_format;
            document.push_str("profile|");
            document.push_str(profile.id.as_str());
            document.push('|');
            document.push_str(&escape(&profile.name));
            document.push('|');
            push_display(&mut document, &format.width());
            document.push('|');
            push_display(&mut document, &format.height());
            document.push('|');
            push_display(&mut document, &format.frame_rate().numerator());
            document.push('|');
            push_display(&mut document, &format.frame_rate().denominator());
            document.push('\n');

            for scene in profile.scenes.values() {
                document.push_str("scene|");
                document.push_str(profile.id.as_str());
                document.push('|');
                document.push_str(scene.id.as_str());
                document.push('|');
                document.push_str(&escape(&scene.name));
                document.push('\n');
                for source in &scene.sources {
                    document.push_str("source|");
                    document.push_str(profile.id.as_str());
                    document.push('|');
                    document.push_str(scene.id.as_str());
                    document.push('|');
                    document.push_str(source.id.as_str());
                    document.push('|');
                    document.push_str(source.kind.as_str());
                    document.push('|');
                    document.push_str(&escape(&source.name));
                    document.push('|');
                    document.push_str(&escape(&source.settings.serialize()));
                    document.push('|');
                    append_transform(&mut document, source.transform);
                    document.push('|');
                    append_filters(&mut document, &source.filters);
                    document.push('|');
                    document.push_str(if source.visible { "1" } else { "0" });
                    document.push('|');
                    document.push_str(if source.locked { "1" } else { "0" });
                    document.push('\n');
                }
            }
        }
        document
    }

    /// Parses a serialized project document.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] for malformed lines, duplicate objects, invalid
    /// settings, invalid media values, or unknown references.
    pub fn parse(document: &str) -> Result<Self, ProjectError> {
        if document.len() > MAX_PROJECT_BYTES {
            return Err(ProjectError::DocumentTooLarge);
        }
        let mut lines = document.lines();
        if lines.next() != Some(MAGIC) {
            return Err(ProjectError::InvalidDocument {
                line: 1,
                reason: "invalid project header".to_owned(),
            });
        }

        let project_line = lines.next().ok_or_else(|| ProjectError::InvalidDocument {
            line: 2,
            reason: "missing project record".to_owned(),
        })?;
        let project_fields = fields(project_line, 2, "project", 3)?;
        let title = decode(project_fields[1], 2)?;
        let mut project = Self::new(&title)?;
        project.active_profile = identifier(project_fields[2], "active profile id")?;

        for (index, line) in lines.enumerate() {
            let line_number = index + 3;
            if line.trim().is_empty() {
                return Err(ProjectError::InvalidDocument {
                    line: line_number,
                    reason: "blank lines are not allowed".to_owned(),
                });
            }
            let kind = line.split('|').next().unwrap_or_default();
            match kind {
                "profile" => parse_profile(&mut project, line, line_number)?,
                "scene" => parse_scene(&mut project, line, line_number)?,
                "source" => parse_source(&mut project, line, line_number)?,
                _ => {
                    return Err(ProjectError::InvalidDocument {
                        line: line_number,
                        reason: format!("unknown record type: {kind}"),
                    });
                }
            }
        }
        if !project.profiles.is_empty() && !project.profiles.contains_key(&project.active_profile) {
            return Err(ProjectError::UnknownProfile(project.active_profile));
        }
        Ok(project)
    }
}

fn parse_profile(
    project: &mut Project,
    line: &str,
    line_number: usize,
) -> Result<(), ProjectError> {
    let values = fields(line, line_number, "profile", 7)?;
    let id = values[1];
    let name = decode(values[2], line_number)?;
    let width = number(values[3], line_number, "profile width")?;
    let height = number(values[4], line_number, "profile height")?;
    let numerator = number(values[5], line_number, "profile frame-rate numerator")?;
    let denominator = number(values[6], line_number, "profile frame-rate denominator")?;
    let rate = FrameRate::new(numerator, denominator).map_err(ProjectError::Media)?;
    let format = VideoFormat::new(width, height, rate).map_err(ProjectError::Media)?;
    project.add_profile(Profile::new(id, &name, format)?)
}

fn parse_scene(project: &mut Project, line: &str, line_number: usize) -> Result<(), ProjectError> {
    let values = fields(line, line_number, "scene", 4)?;
    let profile_id = identifier(values[1], "profile id")?;
    let scene = SceneSpec::new(values[2], &decode(values[3], line_number)?)?;
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    profile.add_scene(scene)
}

fn parse_source(project: &mut Project, line: &str, line_number: usize) -> Result<(), ProjectError> {
    // A source record is 9 or 11 fields; both fit a fixed array, so the split
    // is drained into it instead of into a per-record Vec.
    let mut values = [""; 11];
    let mut count = 0_usize;
    for field in line.split('|') {
        if count == values.len() {
            count += 1;
            break;
        }
        values[count] = field;
        count += 1;
    }
    if (count != 9 && count != 11) || values[0] != "source" {
        return Err(ProjectError::InvalidDocument {
            line: line_number,
            reason: "expected source record with 9 or 11 fields".to_owned(),
        });
    }
    let profile_id = identifier(values[1], "profile id")?;
    let scene_id = identifier(values[2], "scene id")?;
    let name = decode(values[5], line_number)?;
    let settings_text = decode(values[6], line_number)?;
    let settings = Config::parse(&settings_text).map_err(ProjectError::Config)?;
    let transform = parse_transform(values[7], line_number)?;
    let filters = parse_filters(values[8], line_number)?;
    let mut source = SourceSpec::new(values[3], values[4], &name, settings)?;
    source.set_transform(transform);
    for filter in filters {
        source.add_filter(filter);
    }
    // Legacy 9-field records carry no visibility or lock flags.
    if count == 11 {
        source.set_visible(parse_flag(values[9], line_number, "source visibility")?);
        source.set_locked(parse_flag(values[10], line_number, "source lock")?);
    }
    let profile = project
        .profile_mut(&profile_id)
        .ok_or_else(|| ProjectError::UnknownProfile(profile_id.clone()))?;
    let scene = profile
        .scene_mut(&scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    scene.add_source(source)
}
fn append_transform(document: &mut String, transform: FrameTransform) {
    push_display(document, &transform.scale_x_milli());
    document.push(',');
    push_display(document, &transform.scale_y_milli());
    document.push(',');
    push_display(document, &transform.translate_x());
    document.push(',');
    push_display(document, &transform.translate_y());
    document.push(',');
    document.push_str(if transform.flip_x() { "1" } else { "0" });
    document.push(',');
    document.push_str(if transform.flip_y() { "1" } else { "0" });
    document.push(',');
    push_display(document, &transform.opacity());
    document.push(',');
    push_display(document, &transform.crop_left());
    document.push(',');
    push_display(document, &transform.crop_top());
    document.push(',');
    push_display(document, &transform.crop_right());
    document.push(',');
    push_display(document, &transform.crop_bottom());
}

fn parse_transform(value: &str, line: usize) -> Result<FrameTransform, ProjectError> {
    // Seven fields is the V1 legacy shape; four crop fields were appended so
    // old documents remain readable without a migration step.
    let values = value.split(',').collect::<Vec<_>>();
    if values.len() != 7 && values.len() != 11 {
        return Err(ProjectError::InvalidDocument {
            line,
            reason: "invalid transform field count".to_owned(),
        });
    }
    let transform = FrameTransform::new(
        number(values[0], line, "horizontal scale")?,
        number(values[1], line, "vertical scale")?,
        number(values[2], line, "horizontal translation")?,
        number(values[3], line, "vertical translation")?,
        number::<u8>(values[4], line, "horizontal flip")? != 0,
        number::<u8>(values[5], line, "vertical flip")? != 0,
        number(values[6], line, "opacity")?,
    )
    .map_err(ProjectError::Media)?;
    if values.len() == 7 {
        return Ok(transform);
    }
    transform
        .with_crop(
            number(values[7], line, "left crop")?,
            number(values[8], line, "top crop")?,
            number(values[9], line, "right crop")?,
            number(values[10], line, "bottom crop")?,
        )
        .map_err(ProjectError::Media)
}

/// Appends a comma-separated filter chain directly to `document`.
///
/// Writing in place avoids the String-per-filter, Vec, and join that the
/// previous formulation allocated for every source record.
fn append_filters(document: &mut String, filters: &[FrameFilter]) {
    for (index, filter) in filters.iter().enumerate() {
        if index > 0 {
            document.push(',');
        }
        match filter {
            FrameFilter::Grayscale => document.push_str("gray"),
            FrameFilter::Brightness { milli } => {
                document.push_str("brightness:");
                push_display(document, milli);
            }
            FrameFilter::Opacity(opacity) => {
                document.push_str("opacity:");
                push_display(document, opacity);
            }
        }
    }
}

/// Appends a value's `Display` form without building an intermediate String.
fn push_display(document: &mut String, value: &impl std::fmt::Display) {
    use std::fmt::Write as _;
    let _ = write!(document, "{value}");
}

fn parse_filters(value: &str, line: usize) -> Result<Vec<FrameFilter>, ProjectError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|filter| {
            if filter == "gray" {
                return Ok(FrameFilter::Grayscale);
            }
            if let Some(value) = filter.strip_prefix("brightness:") {
                return Ok(FrameFilter::Brightness {
                    milli: number(value, line, "brightness")?,
                });
            }
            if let Some(value) = filter.strip_prefix("opacity:") {
                return Ok(FrameFilter::Opacity(number(value, line, "opacity")?));
            }
            Err(ProjectError::InvalidDocument {
                line,
                reason: format!("unknown filter: {filter}"),
            })
        })
        .collect()
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            escaped.push(char::from(byte));
        } else {
            escaped.push('%');
            escaped.push(hex(byte >> 4));
            escaped.push(hex(byte & 0x0F));
        }
    }
    escaped
}

const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

fn decode(value: &str, line: usize) -> Result<String, ProjectError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(ProjectError::InvalidDocument {
                line,
                reason: "truncated escaped value".to_owned(),
            });
        }
        let high = from_hex(bytes[index + 1]).ok_or_else(|| ProjectError::InvalidDocument {
            line,
            reason: "invalid escaped value".to_owned(),
        })?;
        let low = from_hex(bytes[index + 2]).ok_or_else(|| ProjectError::InvalidDocument {
            line,
            reason: "invalid escaped value".to_owned(),
        })?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| ProjectError::InvalidDocument {
        line,
        reason: "escaped value is not UTF-8".to_owned(),
    })
}

fn hex(value: u8) -> char {
    char::from(HEX_DIGITS[usize::from(value & 0x0F)])
}

fn from_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
