use super::{
    error::ProjectError,
    model::{Profile, Project, SceneSpec, SourceSpec},
    validation::{fields, identifier, number, parse_flag},
    MAGIC, MAX_PROJECT_BYTES,
};
use obs_rs_config::Config;
use obs_rs_media::{FrameFilter, FrameRate, FrameTransform, VideoFormat};

impl Project {
    /// Serializes the project into a deterministic, escaped line format.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut document = String::new();
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
            document.push_str(&format.width().to_string());
            document.push('|');
            document.push_str(&format.height().to_string());
            document.push('|');
            document.push_str(&format.frame_rate().numerator().to_string());
            document.push('|');
            document.push_str(&format.frame_rate().denominator().to_string());
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
                    document.push_str(&serialize_filters(&source.filters));
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
    let values = line.split('|').collect::<Vec<_>>();
    if (values.len() != 9 && values.len() != 11) || values.first() != Some(&"source") {
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
    if values.len() == 11 {
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
    document.push_str(&transform.scale_x_milli().to_string());
    document.push(',');
    document.push_str(&transform.scale_y_milli().to_string());
    document.push(',');
    document.push_str(&transform.translate_x().to_string());
    document.push(',');
    document.push_str(&transform.translate_y().to_string());
    document.push(',');
    document.push_str(if transform.flip_x() { "1" } else { "0" });
    document.push(',');
    document.push_str(if transform.flip_y() { "1" } else { "0" });
    document.push(',');
    document.push_str(&transform.opacity().to_string());
}

fn parse_transform(value: &str, line: usize) -> Result<FrameTransform, ProjectError> {
    let values = value.split(',').collect::<Vec<_>>();
    if values.len() != 7 {
        return Err(ProjectError::InvalidDocument {
            line,
            reason: "invalid transform field count".to_owned(),
        });
    }
    FrameTransform::new(
        number(values[0], line, "horizontal scale")?,
        number(values[1], line, "vertical scale")?,
        number(values[2], line, "horizontal translation")?,
        number(values[3], line, "vertical translation")?,
        number::<u8>(values[4], line, "horizontal flip")? != 0,
        number::<u8>(values[5], line, "vertical flip")? != 0,
        number(values[6], line, "opacity")?,
    )
    .map_err(ProjectError::Media)
}

fn serialize_filters(filters: &[FrameFilter]) -> String {
    filters
        .iter()
        .map(|filter| match filter {
            FrameFilter::Grayscale => "gray".to_owned(),
            FrameFilter::Brightness { milli } => format!("brightness:{milli}"),
            FrameFilter::Opacity(opacity) => format!("opacity:{opacity}"),
        })
        .collect::<Vec<_>>()
        .join(",")
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
    let mut escaped = String::new();
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
    match value {
        0..=9 => char::from(b'0' + value),
        _ => char::from(b'A' + value - 10),
    }
}

fn from_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
