//! JSON persistence for [`Project`].
//!
//! Documents are ordinary JSON so that a saved project can be inspected,
//! diffed, and merged with the same tools as any other configuration file.
//! Two properties are load-bearing:
//!
//! * **Determinism.** Profiles and scenes live in `BTreeMap`s, sources in
//!   insertion-ordered `Vec`s, and settings serialize sorted, so saving
//!   unchanged state twice produces byte-identical files.
//! * **Explicit versioning.** Every document carries `format` and `version`
//!   members, so a future schema change is a checked migration rather than a
//!   silent misparse.

use super::{
    error::ProjectError,
    model::{Profile, Project, SceneSpec, SourceFilterCategory, SourceFilterSpec, SourceSpec},
    validation::identifier,
    MAX_PROJECT_BYTES,
};
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, FrameTransform, VideoFormat};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::Json;

use crate::RenderBackendPreference;

/// Value of the document's `format` member.
const FORMAT_TAG: &str = "obs-rs-project";

/// Schema version this build writes.
const FORMAT_VERSION: u32 = 1;

impl Project {
    /// Serializes the project as a deterministic JSON document.
    #[must_use]
    pub fn serialize(&self) -> String {
        Json::object([
            ("format", Json::string(FORMAT_TAG)),
            ("version", Json::number(FORMAT_VERSION)),
            ("title", Json::string(&self.title)),
            ("active_profile", Json::string(self.active_profile.as_str())),
            (
                "profiles",
                Json::Array(self.profiles.values().map(encode_profile).collect()),
            ),
        ])
        .to_pretty_string()
    }

    /// Parses a serialized project document.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectError`] for malformed JSON, an unknown schema version,
    /// missing or mistyped members, duplicate objects, invalid settings,
    /// invalid media values, or unknown references.
    pub fn parse(document: &str) -> Result<Self, ProjectError> {
        if document.len() > MAX_PROJECT_BYTES {
            return Err(ProjectError::DocumentTooLarge);
        }

        let root = Json::parse(document).map_err(|error| ProjectError::InvalidDocument {
            line: error.line,
            reason: error.message,
        })?;

        if root.get("format").and_then(Json::as_str) != Some(FORMAT_TAG) {
            return Err(invalid(format!(
                "not an {FORMAT_TAG} document; expected a `format` member naming it"
            )));
        }
        match root.get("version").and_then(Json::as_number::<u32>) {
            Some(FORMAT_VERSION) => {}
            Some(version) => {
                return Err(invalid(format!(
                    "unsupported project schema version {version}; this build reads version {FORMAT_VERSION}"
                )))
            }
            None => return Err(invalid("missing or invalid `version`")),
        }

        let mut project = Self::new(string_member(&root, "title")?)?;
        project.active_profile = identifier(string_member(&root, "active_profile")?, "profile id")?;

        for profile in array_member(&root, "profiles")? {
            decode_profile(&mut project, profile)?;
        }

        if !project.profiles.is_empty() && !project.profiles.contains_key(&project.active_profile) {
            return Err(ProjectError::UnknownProfile(project.active_profile));
        }
        Ok(project)
    }
}

fn encode_profile(profile: &Profile) -> Json {
    let format = profile.video_format;
    Json::object([
        ("id", Json::string(profile.id.as_str())),
        ("name", Json::string(&profile.name)),
        (
            "video",
            Json::object([
                ("width", Json::number(format.width())),
                ("height", Json::number(format.height())),
                (
                    "frame_rate",
                    Json::object([
                        ("numerator", Json::number(format.frame_rate().numerator())),
                        (
                            "denominator",
                            Json::number(format.frame_rate().denominator()),
                        ),
                    ]),
                ),
            ]),
        ),
        ("render_backend", Json::string(profile.render_backend.id())),
        ("output_kind", Json::string(profile.output_kind.id())),
        (
            "scenes",
            Json::Array(profile.scenes.values().map(encode_scene).collect()),
        ),
    ])
}

fn encode_scene(scene: &SceneSpec) -> Json {
    Json::object([
        ("id", Json::string(scene.id.as_str())),
        ("name", Json::string(&scene.name)),
        (
            "sources",
            Json::Array(scene.sources.iter().map(encode_source).collect()),
        ),
    ])
}

fn encode_source(source: &SourceSpec) -> Json {
    Json::object([
        ("id", Json::string(source.id.as_str())),
        ("kind", Json::string(source.kind.as_str())),
        ("name", Json::string(&source.name)),
        (
            "settings",
            Json::object(
                source
                    .settings
                    .iter()
                    .map(|(key, value)| (key, Json::string(value))),
            ),
        ),
        ("transform", encode_transform(source.transform)),
        (
            "filters",
            Json::Array(source.filters.iter().map(encode_filter).collect()),
        ),
        ("visible", Json::Bool(source.visible)),
        ("locked", Json::Bool(source.locked)),
    ])
}

fn encode_transform(transform: FrameTransform) -> Json {
    Json::object([
        ("scale_x_milli", Json::number(transform.scale_x_milli())),
        ("scale_y_milli", Json::number(transform.scale_y_milli())),
        ("translate_x", Json::number(transform.translate_x())),
        ("translate_y", Json::number(transform.translate_y())),
        ("flip_x", Json::Bool(transform.flip_x())),
        ("flip_y", Json::Bool(transform.flip_y())),
        ("opacity", Json::number(transform.opacity())),
        ("crop_left", Json::number(transform.crop_left())),
        ("crop_top", Json::number(transform.crop_top())),
        ("crop_right", Json::number(transform.crop_right())),
        ("crop_bottom", Json::number(transform.crop_bottom())),
    ])
}

fn encode_filter(filter: &SourceFilterSpec) -> Json {
    Json::object([
        ("id", Json::string(filter.id().as_str())),
        ("name", Json::string(filter.name())),
        ("kind", Json::string(filter.kind().as_str())),
        ("category", Json::string(filter.category().id())),
        ("enabled", Json::Bool(filter.enabled())),
        (
            "settings",
            Json::object(
                filter
                    .settings()
                    .iter()
                    .map(|(key, value)| (key, Json::string(value))),
            ),
        ),
    ])
}

fn decode_profile(project: &mut Project, value: &Json) -> Result<(), ProjectError> {
    let video = value
        .get("video")
        .ok_or_else(|| invalid("profile is missing `video`"))?;
    let rate = FrameRate::new(
        number_member(video.get("frame_rate").unwrap_or(&Json::Null), "numerator")?,
        number_member(
            video.get("frame_rate").unwrap_or(&Json::Null),
            "denominator",
        )?,
    )
    .map_err(ProjectError::Media)?;
    let format = VideoFormat::new(
        number_member(video, "width")?,
        number_member(video, "height")?,
        rate,
    )
    .map_err(ProjectError::Media)?;

    let mut profile = Profile::new(
        string_member(value, "id")?,
        string_member(value, "name")?,
        format,
    )?;

    // Both preferences default to the plain reference pipeline, so a document
    // that omits them still describes a complete profile.
    if let Some(backend) = value.get("render_backend") {
        let backend = backend
            .as_str()
            .and_then(RenderBackendPreference::from_id)
            .ok_or_else(|| invalid("unknown render backend"))?;
        profile.set_render_backend(backend);
    }
    if let Some(kind) = value.get("output_kind") {
        let kind = kind
            .as_str()
            .and_then(OutputProfileKind::from_id)
            .ok_or_else(|| invalid("unknown output profile"))?;
        profile.set_output_profile(kind);
    }

    for scene in array_member(value, "scenes")? {
        profile.add_scene(decode_scene(scene)?)?;
    }

    project.add_profile(profile)
}

fn decode_scene(value: &Json) -> Result<SceneSpec, ProjectError> {
    let mut scene = SceneSpec::new(string_member(value, "id")?, string_member(value, "name")?)?;
    for source in array_member(value, "sources")? {
        scene.add_source(decode_source(source)?)?;
    }
    Ok(scene)
}

fn decode_source(value: &Json) -> Result<SourceSpec, ProjectError> {
    let settings = value
        .get("settings")
        .and_then(Json::as_object)
        .ok_or_else(|| invalid("source is missing `settings`"))?;
    let mut config = Config::new();
    for (key, setting) in settings {
        let setting = setting
            .as_str()
            .ok_or_else(|| invalid(format!("setting `{key}` is not a string")))?;
        config.set(key, setting).map_err(ProjectError::Config)?;
    }

    let mut source = SourceSpec::new(
        string_member(value, "id")?,
        string_member(value, "kind")?,
        string_member(value, "name")?,
        config,
    )?;
    source.set_transform(decode_transform(
        value
            .get("transform")
            .ok_or_else(|| invalid("source is missing `transform`"))?,
    )?);
    for (index, filter) in array_member(value, "filters")?.iter().enumerate() {
        source.add_filter(decode_filter(filter, index)?)?;
    }
    source.set_visible(bool_member(value, "visible")?);
    source.set_locked(bool_member(value, "locked")?);
    Ok(source)
}

fn decode_transform(value: &Json) -> Result<FrameTransform, ProjectError> {
    FrameTransform::new(
        number_member(value, "scale_x_milli")?,
        number_member(value, "scale_y_milli")?,
        number_member(value, "translate_x")?,
        number_member(value, "translate_y")?,
        bool_member(value, "flip_x")?,
        bool_member(value, "flip_y")?,
        number_member(value, "opacity")?,
    )
    .map_err(ProjectError::Media)?
    .with_crop(
        number_member(value, "crop_left")?,
        number_member(value, "crop_top")?,
        number_member(value, "crop_right")?,
        number_member(value, "crop_bottom")?,
    )
    .map_err(ProjectError::Media)
}

fn decode_filter(value: &Json, index: usize) -> Result<SourceFilterSpec, ProjectError> {
    let kind = string_member(value, "kind")?;
    // Version-one documents used renderer-shaped filter records. Read them as
    // a compatibility migration, but never store that shape again.
    let Some(id) = value.get("id").and_then(Json::as_str) else {
        let (name, settings) = match kind {
            "grayscale" => ("Grayscale", Config::new()),
            "brightness" => {
                let mut settings = Config::new();
                settings
                    .set("milli", &number_member::<i16>(value, "milli")?.to_string())
                    .map_err(ProjectError::Config)?;
                ("Brightness", settings)
            }
            "opacity" => {
                let mut settings = Config::new();
                settings
                    .set("value", &number_member::<u8>(value, "value")?.to_string())
                    .map_err(ProjectError::Config)?;
                ("Opacity", settings)
            }
            other => return Err(invalid(format!("unknown legacy filter: {other}"))),
        };
        return SourceFilterSpec::with_category(
            &format!("legacy_filter_{}", index.saturating_add(1)),
            name,
            kind,
            SourceFilterCategory::Effect,
            settings,
        );
    };

    let settings = value
        .get("settings")
        .and_then(Json::as_object)
        .ok_or_else(|| invalid("filter is missing `settings`"))?;
    let mut config = Config::new();
    for (key, setting) in settings {
        let setting = setting
            .as_str()
            .ok_or_else(|| invalid(format!("filter setting `{key}` is not a string")))?;
        config.set(key, setting).map_err(ProjectError::Config)?;
    }
    let category = value
        .get("category")
        .and_then(Json::as_str)
        .and_then(SourceFilterCategory::from_id)
        .ok_or_else(|| invalid("unknown or missing filter category"))?;
    let mut filter =
        SourceFilterSpec::with_category(id, string_member(value, "name")?, kind, category, config)?;
    filter.set_enabled(bool_member(value, "enabled")?);
    Ok(filter)
}

/// Builds a document error for a structural problem JSON parsing accepted.
///
/// The JSON layer already reported real syntax errors with their line; these
/// are schema failures found afterwards, where no single line is to blame.
fn invalid(reason: impl Into<String>) -> ProjectError {
    ProjectError::InvalidDocument {
        line: 0,
        reason: reason.into(),
    }
}

fn string_member<'a>(value: &'a Json, key: &str) -> Result<&'a str, ProjectError> {
    value
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| invalid(format!("missing or non-string `{key}`")))
}

fn bool_member(value: &Json, key: &str) -> Result<bool, ProjectError> {
    value
        .get(key)
        .and_then(Json::as_bool)
        .ok_or_else(|| invalid(format!("missing or non-boolean `{key}`")))
}

fn number_member<T: std::str::FromStr>(value: &Json, key: &str) -> Result<T, ProjectError> {
    value
        .get(key)
        .and_then(Json::as_number::<T>)
        .ok_or_else(|| invalid(format!("missing or out-of-range `{key}`")))
}

fn array_member<'a>(value: &'a Json, key: &str) -> Result<&'a [Json], ProjectError> {
    value
        .get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| invalid(format!("missing or non-array `{key}`")))
}
