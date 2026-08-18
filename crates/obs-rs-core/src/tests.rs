use super::*;
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_media::{
    FrameFilter, FrameRate, FrameTransform, FrameTransition, Timestamp, VideoFormat,
};
use obs_rs_plugin_api::{Plugin, PluginApiVersion, PluginManifest, SourceFactory, VideoRequest};
use obs_rs_util::Identifier;
use std::sync::Arc;

fn settings(width: u32, height: u32, color: &str) -> Config {
    let mut config = Config::new();
    config
        .set("width", &width.to_string())
        .expect("valid width");
    config
        .set("height", &height.to_string())
        .expect("valid height");
    config.set("color", color).expect("valid color");
    config
}

fn format() -> VideoFormat {
    VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("valid rate")).expect("valid format")
}

struct FutureApiPlugin {
    manifest: PluginManifest,
}

impl Plugin for FutureApiPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn source_factories(&self) -> &[Arc<dyn SourceFactory>] {
        &[]
    }
}

#[test]
fn registers_plugin_creates_scene_and_composites_sources() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("main").expect("scene is new");
    let background = runtime
        .create_source("color_source", "background", &settings(2, 2, "#0000FFFF"))
        .expect("background is valid");
    let foreground = runtime
        .create_source("color_source", "foreground", &settings(2, 2, "#FF000080"))
        .expect("foreground is valid");
    runtime
        .attach_source("main", background)
        .expect("attach background");
    runtime
        .attach_source("main", foreground)
        .expect("attach foreground");

    let request = VideoRequest::new(Timestamp::ZERO, format());
    let frame = runtime
        .render_scene("main", &request)
        .expect("render succeeds")
        .expect("scene has frames");

    assert_eq!(runtime.plugins().len(), 1);
    assert_eq!(runtime.source_count(), 2);
    assert_eq!(runtime.scene_count(), 1);
    assert_eq!(
        runtime.scene_sources("main"),
        Some(&[background, foreground][..])
    );
    assert_eq!(
        runtime.source_info(background),
        Some(("color_source", "background"))
    );
    assert_eq!(frame.pixel(0, 0), Some([128, 0, 127, 255]));
}

#[test]
fn runtime_limits_contain_plugin_scene_source_and_filter_resources() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::with_limits(RuntimeLimits::new(1, 8, 1, 1, 1, 1));
    runtime
        .register_plugin(&plugin)
        .expect("plugin fits the limits");
    runtime.create_scene("main").expect("scene fits the limits");
    assert_eq!(
        runtime.create_scene("second"),
        Err(RuntimeError::ResourceLimitExceeded {
            resource: "scenes",
            limit: 1
        })
    );
    let source = runtime
        .create_source("color_source", "background", &settings(2, 2, "#102030FF"))
        .expect("source fits the limit");
    runtime
        .attach_source("main", source)
        .expect("source item fits the limit");
    assert_eq!(
        runtime.create_source("color_source", "extra", &settings(2, 2, "#102030FF")),
        Err(RuntimeError::ResourceLimitExceeded {
            resource: "sources",
            limit: 1
        })
    );
    runtime
        .add_source_filter(source, FrameFilter::Grayscale)
        .expect("first filter fits");
    let usage = runtime.usage();
    assert_eq!(usage.plugins(), 1);
    assert!(usage.source_kinds() >= 5);
    assert_eq!(usage.scenes(), 1);
    assert_eq!(usage.sources(), 1);
    assert_eq!(usage.filters(), 1);
    assert_eq!(
        runtime.add_source_filter(source, FrameFilter::Grayscale),
        Err(RuntimeError::ResourceLimitExceeded {
            resource: "filters per source",
            limit: 1
        })
    );
}

#[test]
fn scene_item_transform_is_applied_before_composition() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("main").expect("scene is new");
    let source = runtime
        .create_source("color_source", "red", &settings(2, 2, "#FF0000FF"))
        .expect("source is valid");
    runtime
        .attach_source("main", source)
        .expect("attach source");
    let transform =
        FrameTransform::new(1_000, 1_000, 0, 0, false, false, 128).expect("transform is valid");
    runtime
        .set_source_transform("main", source, transform)
        .expect("set transform");
    runtime
        .add_source_filter(source, FrameFilter::Grayscale)
        .expect("add filter");

    let request = VideoRequest::new(Timestamp::ZERO, format());
    let frame = runtime
        .render_scene("main", &request)
        .expect("render succeeds")
        .expect("scene has a frame");

    assert_eq!(runtime.source_transform("main", source), Some(transform));
    assert_eq!(
        runtime.source_filters(source),
        Some(&[FrameFilter::Grayscale][..])
    );
    assert_eq!(frame.pixel(0, 0), Some([76, 76, 76, 128]));
}

#[test]
fn compositor_metrics_report_work_and_reset() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("main").expect("scene is new");
    let source = runtime
        .create_source("color_source", "red", &settings(2, 2, "#FF0000FF"))
        .expect("source is valid");
    runtime
        .attach_source("main", source)
        .expect("attach source");
    runtime
        .set_source_transform(
            "main",
            source,
            FrameTransform::new(1_000, 1_000, 0, 0, false, false, 128).expect("transform is valid"),
        )
        .expect("set transform");
    runtime
        .add_source_filter(source, FrameFilter::Grayscale)
        .expect("add filter");

    let request = VideoRequest::new(Timestamp::ZERO, format());
    runtime
        .render_scene("main", &request)
        .expect("render succeeds")
        .expect("scene has a frame");

    let metrics = runtime.compositor_metrics();
    assert_eq!(metrics.render_calls(), 1);
    assert_eq!(metrics.source_requests(), 1);
    assert_eq!(metrics.source_frames(), 1);
    assert_eq!(metrics.empty_sources(), 0);
    assert_eq!(metrics.transformed_frames(), 1);
    assert_eq!(metrics.filtered_frames(), 1);
    assert_eq!(metrics.blended_layers(), 0);
    assert_eq!(metrics.capture_latency().samples(), 1);

    runtime.reset_compositor_metrics();
    assert_eq!(runtime.compositor_metrics(), CompositorMetrics::default());
}

#[test]
fn first_transparent_layer_has_canonical_rgb_values() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("main").expect("scene is new");
    let source = runtime
        .create_source("color_source", "transparent", &settings(2, 2, "#FF000000"))
        .expect("source is valid");
    runtime
        .attach_source("main", source)
        .expect("attach source");

    let frame = runtime
        .render_scene("main", &VideoRequest::new(Timestamp::ZERO, format()))
        .expect("render succeeds")
        .expect("scene has a frame");

    assert_eq!(frame.pixel(0, 0), Some([0, 0, 0, 0]));
}

#[test]
fn scene_transition_renders_cut_and_cross_fade() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("from").expect("source scene");
    runtime.create_scene("to").expect("destination scene");
    let from = runtime
        .create_source("color_source", "from-color", &settings(2, 2, "#FF0000FF"))
        .expect("source is valid");
    let to = runtime
        .create_source("color_source", "to-color", &settings(2, 2, "#0000FFFF"))
        .expect("destination is valid");
    runtime.attach_source("from", from).expect("attach source");
    runtime.attach_source("to", to).expect("attach destination");
    let request = VideoRequest::new(Timestamp::ZERO, format());

    let cut = runtime
        .render_scene_transition("from", "to", &request, FrameTransition::Cut)
        .expect("cut succeeds")
        .expect("destination has a frame");
    let fade = runtime
        .render_scene_transition(
            "from",
            "to",
            &request,
            FrameTransition::cross_fade(500).expect("valid progress"),
        )
        .expect("fade succeeds")
        .expect("both scenes have frames");

    assert_eq!(cut.pixel(0, 0), Some([0, 0, 255, 255]));
    assert_eq!(fade.pixel(0, 0), Some([128, 0, 128, 255]));
}

#[test]
fn rejects_duplicate_registration_and_unknown_values() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("first registration");
    assert_eq!(
        runtime.register_plugin(&plugin),
        Err(RuntimeError::DuplicatePlugin(
            Identifier::new("obs_rs_builtins").expect("valid id")
        ))
    );
    assert!(matches!(
        runtime.create_source("missing", "source", &Config::new()),
        Err(RuntimeError::UnknownSourceKind(_))
    ));
}

#[test]
fn rejects_plugins_from_a_newer_api_version() {
    let plugin = FutureApiPlugin {
        manifest: PluginManifest::with_api_version(
            "future_plugin",
            "Future plugin",
            "1.0.0",
            PluginApiVersion::new(2, 0),
        )
        .expect("manifest"),
    };
    let mut runtime = Runtime::new();
    assert_eq!(
        runtime.register_plugin(&plugin),
        Err(RuntimeError::UnsupportedPluginApi {
            expected: PluginApiVersion::current(),
            actual: PluginApiVersion::new(2, 0),
        })
    );
}

#[test]
fn empty_scene_renders_no_frame() {
    let mut runtime = Runtime::new();
    runtime.create_scene("empty").expect("scene is new");
    let request = VideoRequest::new(Timestamp::ZERO, format());

    assert_eq!(
        runtime
            .render_scene("empty", &request)
            .expect("render succeeds"),
        None
    );
}

#[test]
fn lifecycle_requires_detach_before_source_destruction() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("main").expect("scene is new");
    let source = runtime
        .create_source("color_source", "background", &settings(2, 2, "#000000FF"))
        .expect("source is valid");
    runtime
        .attach_source("main", source)
        .expect("attach source");
    assert_eq!(
        runtime.destroy_source(source),
        Err(RuntimeError::SourceInUse(source))
    );
    runtime
        .detach_source("main", source)
        .expect("detach source");
    runtime.destroy_source(source).expect("destroy source");
    runtime.destroy_scene("main").expect("destroy scene");
    assert_eq!(runtime.source_count(), 0);
    assert_eq!(runtime.scene_count(), 0);
}

#[test]
fn source_kinds_lists_registered_factories_in_identifier_order() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    assert_eq!(runtime.source_kinds().len(), 0);

    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    // Owned because `create_source` below needs `&mut runtime`, but the
    // membership checks compare borrowed text rather than allocating a String
    // per comparison.
    let kinds = runtime
        .source_kinds()
        .map(|kind| kind.as_str().to_owned())
        .collect::<Vec<_>>();

    assert!(kinds.iter().any(|kind| kind == "color_source"));
    assert!(kinds.iter().any(|kind| kind == "test_pattern"));
    let mut sorted = kinds.clone();
    sorted.sort_unstable();
    assert_eq!(kinds, sorted, "kinds are returned in identifier order");
    // Every advertised kind must actually construct.
    for kind in &kinds {
        assert!(
            runtime
                .create_source(kind, "probe", &settings(2, 2, "#0000FFFF"))
                .is_ok(),
            "advertised kind {kind} should create a source"
        );
    }
}
