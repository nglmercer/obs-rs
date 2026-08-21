//! JSON persistence for [`Project`].
//!
//! Documents are ordinary JSON so that a saved project can be inspected,
//! diffed, and merged with the same tools as any other configuration file.
//! Two properties are load-bearing:
//!
//! * **Determinism.** Profiles, sources, and scenes live in `BTreeMap`s, while
//!   scene items retain insertion order and settings serialize sorted, so saving
//!   unchanged state twice produces byte-identical files.
//! * **Explicit versioning.** Every document carries `format` and `version`
//!   members, so a future schema change is a checked migration rather than a
//!   silent misparse.

use super::{
    error::ProjectError,
    model::{
        Profile, Project, SceneItemSpec, SceneSpec, SourceFilterCategory, SourceFilterSpec,
        SourceSpec,
    },
    validation::identifier,
    MAX_PROJECT_BYTES,
};
use obs_rs_config::Config;
use obs_rs_media::{FrameRate, FrameTransform, VideoFormat};
use obs_rs_output::OutputProfileKind;
use obs_rs_util::{Identifier, Json};
use std::collections::HashSet;

use crate::RenderBackendPreference;

/// Value of the document's `format` member.
const FORMAT_TAG: &str = "obs-rs-project";

/// Schema version this build writes.
const FORMAT_VERSION: u32 = 4;
/// The format before nested scene-item targets were persisted.
const PREVIOUS_FORMAT_VERSION: u32 = 3;
/// The format before scene-item rotation was persisted.
const ROTATION_FORMAT_VERSION: u32 = 2;
/// The format before sources were moved out of scenes.
const LEGACY_FORMAT_VERSION: u32 = 1;

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
        let version = match root.get("version").and_then(Json::as_number::<u32>) {
            Some(LEGACY_FORMAT_VERSION) => LEGACY_FORMAT_VERSION,
            Some(ROTATION_FORMAT_VERSION) => ROTATION_FORMAT_VERSION,
            Some(PREVIOUS_FORMAT_VERSION) => PREVIOUS_FORMAT_VERSION,
            Some(FORMAT_VERSION) => FORMAT_VERSION,
            Some(version) => {
                return Err(invalid(format!(
                    "unsupported project schema version {version}; this build reads versions {LEGACY_FORMAT_VERSION}, {ROTATION_FORMAT_VERSION}, {PREVIOUS_FORMAT_VERSION}, and {FORMAT_VERSION}"
                )))
            }
            None => return Err(invalid("missing or invalid `version`")),
        };

        let mut project = Self::new(string_member(&root, "title")?)?;
        project.active_profile = identifier(string_member(&root, "active_profile")?, "profile id")?;

        for profile in array_member(&root, "profiles")? {
            if version == LEGACY_FORMAT_VERSION {
                decode_legacy_profile(&mut project, profile)?;
            } else {
                decode_profile(&mut project, profile)?;
            }
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
            "sources",
            Json::Array(profile.sources().map(encode_source).collect()),
        ),
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
            "items",
            Json::Array(scene.items.iter().map(encode_item).collect()),
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
        (
            "filters",
            Json::Array(source.filters.iter().map(encode_filter).collect()),
        ),
    ])
}

fn encode_item(item: &SceneItemSpec) -> Json {
    let target = item.scene_id().map_or_else(
        || ("source", Json::string(item.source_id().as_str())),
        |scene| ("scene", Json::string(scene.as_str())),
    );
    Json::object([
        ("id", Json::string(item.id.as_str())),
        target,
        ("transform", encode_transform(item.transform)),
        ("visible", Json::Bool(item.visible)),
        ("locked", Json::Bool(item.locked)),
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
        (
            "rotation_milli_degrees",
            Json::number(transform.rotation_milli_degrees()),
        ),
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

fn decode_profile_header(value: &Json) -> Result<Profile, ProjectError> {
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

    Ok(profile)
}

fn decode_profile(project: &mut Project, value: &Json) -> Result<(), ProjectError> {
    let mut profile = decode_profile_header(value)?;
    for source in array_member(value, "sources")? {
        profile.add_source(decode_source(source)?)?;
    }
    for scene in array_member(value, "scenes")? {
        profile.add_scene(decode_scene(scene, &profile)?)?;
    }
    validate_scene_references(&profile)?;
    project.add_profile(profile)
}

fn decode_scene(value: &Json, profile: &Profile) -> Result<SceneSpec, ProjectError> {
    let mut scene = SceneSpec::new(string_member(value, "id")?, string_member(value, "name")?)?;
    for item in array_member(value, "items")? {
        let item = decode_item(item)?;
        if item.is_source() && !profile.has_source(item.source_id()) {
            return Err(ProjectError::UnknownSource(item.source_id().clone()));
        }
        scene.add_item(item)?;
    }
    Ok(scene)
}

fn validate_scene_references(profile: &Profile) -> Result<(), ProjectError> {
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for scene in profile.scenes() {
        validate_scene_graph(profile, scene.id(), &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn validate_scene_graph(
    profile: &Profile,
    scene_id: &Identifier,
    visiting: &mut HashSet<Identifier>,
    visited: &mut HashSet<Identifier>,
) -> Result<(), ProjectError> {
    if visited.contains(scene_id) {
        return Ok(());
    }
    if !visiting.insert(scene_id.clone()) {
        return Err(ProjectError::CircularSceneReference(scene_id.clone()));
    }
    let scene = profile
        .scene(scene_id)
        .ok_or_else(|| ProjectError::UnknownScene(scene_id.clone()))?;
    for item in scene.items() {
        if let Some(target) = item.scene_id() {
            if profile.scene(target).is_none() {
                return Err(ProjectError::UnknownScene(target.clone()));
            }
            validate_scene_graph(profile, target, visiting, visited)?;
        }
    }
    visiting.remove(scene_id);
    visited.insert(scene_id.clone());
    Ok(())
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
    for (index, filter) in array_member(value, "filters")?.iter().enumerate() {
        source.add_filter(decode_filter(filter, index)?)?;
    }
    Ok(source)
}

fn decode_item(value: &Json) -> Result<SceneItemSpec, ProjectError> {
    let id = string_member(value, "id")?;
    let mut item = if let Some(scene) = value.get("scene") {
        let scene = scene
            .as_str()
            .ok_or_else(|| invalid("scene item `scene` is not a string"))?;
        if value.get("source").is_some() {
            return Err(invalid(
                "scene item cannot contain both `source` and `scene`",
            ));
        }
        SceneItemSpec::for_scene(id, scene)?
    } else {
        SceneItemSpec::new(id, string_member(value, "source")?)?
    };
    item.set_transform(decode_transform(
        value
            .get("transform")
            .ok_or_else(|| invalid("scene item is missing `transform`"))?,
    )?);
    item.set_visible(bool_member(value, "visible")?);
    item.set_locked(bool_member(value, "locked")?);
    Ok(item)
}

/// Reads the version-one scene-local source representation and normalizes it
/// into the profile registry plus scene-item references. Identical old source
/// definitions with the same ID become shared sources; conflicting old
/// definitions receive deterministic IDs so no data is lost.
fn decode_legacy_profile(project: &mut Project, value: &Json) -> Result<(), ProjectError> {
    let mut profile = decode_profile_header(value)?;
    for scene_value in array_member(value, "scenes")? {
        let scene = decode_legacy_scene(scene_value, &mut profile)?;
        profile.add_scene(scene)?;
    }
    project.add_profile(profile)
}

fn decode_legacy_scene(value: &Json, profile: &mut Profile) -> Result<SceneSpec, ProjectError> {
    let mut scene = SceneSpec::new(string_member(value, "id")?, string_member(value, "name")?)?;
    for (index, source_value) in array_member(value, "sources")?.iter().enumerate() {
        let (mut source, mut item) = decode_legacy_source(source_value, index)?;
        let original_id = source.id().clone();
        let source_id = if let Some(existing) = profile.source(&original_id) {
            if existing == &source {
                original_id
            } else {
                let new_id = legacy_source_id(profile, original_id.as_str(), scene.id().as_str())?;
                source.id = new_id.clone();
                profile.add_source(source)?;
                new_id
            }
        } else {
            profile.add_source(source)?;
            original_id
        };
        item.set_source_id(source_id);
        scene.add_item(item)?;
    }
    Ok(scene)
}

fn decode_legacy_source(
    value: &Json,
    index: usize,
) -> Result<(SourceSpec, SceneItemSpec), ProjectError> {
    let source = decode_source(value)?;
    let mut item = SceneItemSpec::for_source(source.id().as_str())?;
    item.set_transform(decode_transform(
        value
            .get("transform")
            .ok_or_else(|| invalid("source is missing `transform`"))?,
    )?);
    item.set_visible(bool_member(value, "visible")?);
    item.set_locked(bool_member(value, "locked")?);
    // Keep the index in the signature so adding another compatibility field is
    // straightforward and so legacy call sites remain explicit about order.
    let _ = index;
    Ok((source, item))
}

fn legacy_source_id(
    profile: &Profile,
    base_id: &str,
    scene_id: &str,
) -> Result<obs_rs_util::Identifier, ProjectError> {
    for ordinal in 1..=10_000_u32 {
        let suffix = if ordinal == 1 {
            format!("_{scene_id}")
        } else {
            format!("_{scene_id}_{ordinal}")
        };
        let prefix_length = obs_rs_util::MAX_IDENTIFIER_BYTES.saturating_sub(suffix.len());
        let prefix = base_id
            .get(..base_id.len().min(prefix_length))
            .unwrap_or(base_id);
        let candidate = format!("{prefix}{suffix}");
        if !profile.has_source(candidate.as_str()) {
            return identifier(&candidate, "migrated source id");
        }
    }
    Err(invalid("could not allocate a migrated source identifier"))
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
    .with_rotation_milli_degrees(optional_number_member(value, "rotation_milli_degrees", 0)?)
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

fn optional_number_member<T: std::str::FromStr>(
    value: &Json,
    key: &str,
    default: T,
) -> Result<T, ProjectError> {
    match value.get(key) {
        Some(_) => number_member(value, key),
        None => Ok(default),
    }
}

fn array_member<'a>(value: &'a Json, key: &str) -> Result<&'a [Json], ProjectError> {
    value
        .get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| invalid(format!("missing or non-array `{key}`")))
}
