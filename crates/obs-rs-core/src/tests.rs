use super::*;
use obs_rs_builtins::BuiltinPlugin;
use obs_rs_config::Config;
use obs_rs_media::{
    FrameFilter, FrameRate, FrameTransform, FrameTransition, Timestamp, VideoFormat,
};
use obs_rs_plugin_api::{
    DockDescriptor, Plugin, PluginApiVersion, PluginManifest, SourceFactory, VideoRequest,
};
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

struct DockPlugin {
    manifest: PluginManifest,
    docks: Vec<DockDescriptor>,
}

impl Plugin for DockPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn source_factories(&self) -> &[Arc<dyn SourceFactory>] {
        &[]
    }

    fn dock_descriptors(&self) -> &[DockDescriptor] {
        &self.docks
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
    let source_kind_limit = plugin.source_factories().len();
    let mut runtime = Runtime::with_limits(RuntimeLimits::new(1, source_kind_limit, 1, 1, 1, 1));
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
    assert_eq!(usage.source_kinds(), source_kind_limit);
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
fn registers_plugin_docks_in_a_bounded_plugin_namespace() {
    let plugin = DockPlugin {
        manifest: PluginManifest::new("dock_plugin", "Dock plugin", "1.0.0").expect("manifest"),
        docks: vec![
            DockDescriptor::new("stats", "Plugin stats").expect("stats dock"),
            DockDescriptor::new("events", "Plugin events").expect("events dock"),
        ],
    };
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("plugin docks register atomically");

    let docks = runtime
        .plugin_docks()
        .map(|(plugin, dock)| (plugin.as_str().to_owned(), dock.id().as_str().to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(
        docks,
        vec![
            ("dock_plugin".to_owned(), "events".to_owned()),
            ("dock_plugin".to_owned(), "stats".to_owned()),
        ]
    );
    assert_eq!(runtime.usage().docks(), 2);
}

#[test]
fn plugin_dock_registration_rejects_duplicate_and_oversized_lists_atomically() {
    let duplicate = DockPlugin {
        manifest: PluginManifest::new("duplicate_docks", "Duplicate docks", "1.0.0")
            .expect("manifest"),
        docks: vec![
            DockDescriptor::new("stats", "Stats").expect("stats dock"),
            DockDescriptor::new("stats", "Stats again").expect("duplicate dock"),
        ],
    };
    let mut runtime = Runtime::new();
    assert_eq!(
        runtime.register_plugin(&duplicate),
        Err(RuntimeError::DuplicatePluginDock {
            plugin: Identifier::new("duplicate_docks").expect("plugin id"),
            dock: Identifier::new("stats").expect("dock id"),
        })
    );
    assert_eq!(runtime.plugins().len(), 0);
    assert_eq!(runtime.usage().docks(), 0);

    let docks = (0..=obs_rs_plugin_api::MAX_PLUGIN_DOCKS)
        .map(|index| DockDescriptor::new(&format!("dock_{index}"), "Plugin dock").expect("dock"))
        .collect();
    let oversized = DockPlugin {
        manifest: PluginManifest::new("oversized_docks", "Oversized docks", "1.0.0")
            .expect("manifest"),
        docks,
    };
    assert_eq!(
        runtime.register_plugin(&oversized),
        Err(RuntimeError::ResourceLimitExceeded {
            resource: "plugin docks",
            limit: obs_rs_plugin_api::MAX_PLUGIN_DOCKS,
        })
    );
    assert_eq!(runtime.plugins().len(), 0);
    assert_eq!(runtime.usage().docks(), 0);
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
fn render_delay_is_bounded_and_warms_up_before_emitting_old_frames() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("main").expect("scene is new");
    let source = runtime
        .create_source("test_pattern", "pattern", &settings(2, 2, "#000000FF"))
        .expect("test pattern is valid");
    runtime
        .attach_source("main", source)
        .expect("attach source");
    runtime
        .add_source_filter(
            source,
            FrameFilter::RenderDelay(obs_rs_media::RenderDelay { milliseconds: 100 }),
        )
        .expect("render delay fits the source filter limit");

    for timestamp in [0, 33, 66] {
        assert!(runtime
            .render_scene(
                "main",
                &VideoRequest::new(Timestamp::from_millis(timestamp), format()),
            )
            .expect("warm-up render succeeds")
            .is_none());
    }
    let first_delayed = runtime
        .render_scene(
            "main",
            &VideoRequest::new(Timestamp::from_millis(100), format()),
        )
        .expect("delayed render succeeds")
        .expect("first delayed frame is ready");
    assert_eq!(first_delayed.pixel(0, 0), Some([32, 0, 0, 255]));
    assert_eq!(first_delayed.timestamp(), Timestamp::from_millis(100));

    let second_delayed = runtime
        .render_scene(
            "main",
            &VideoRequest::new(Timestamp::from_millis(133), format()),
        )
        .expect("second delayed render succeeds")
        .expect("second delayed frame is ready");
    assert_eq!(second_delayed.pixel(0, 0), Some([224, 0, 0, 255]));
}

#[test]
fn duplicate_scene_items_share_capture_but_keep_transforms() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("main").expect("scene is new");
    let source = runtime
        .create_source("color_source", "shared", &settings(2, 2, "#204060FF"))
        .expect("source is valid");

    let first = runtime
        .attach_source_instance("main", source)
        .expect("first item attaches");
    let second = runtime
        .attach_source_instance("main", source)
        .expect("second item attaches");
    assert_eq!((first, second), (0, 1));
    assert_eq!(
        runtime.scene_item_ids("main"),
        Some(vec!["item-0".to_owned(), "item-1".to_owned()])
    );
    let second_transform =
        FrameTransform::new(500, 500, 100, 50, false, false, 128).expect("transform");
    runtime
        .set_scene_item_transform("main", first, FrameTransform::IDENTITY)
        .expect("first transform");
    runtime
        .set_scene_item_transform("main", second, second_transform)
        .expect("second transform");

    let layers = runtime
        .render_scene_layers("main", &VideoRequest::new(Timestamp::ZERO, format()))
        .expect("scene renders");
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].item_id(), "item-0");
    assert_eq!(layers[1].item_id(), "item-1");
    assert_eq!(layers[0].transform(), FrameTransform::IDENTITY);
    assert_eq!(layers[1].transform(), second_transform);
    assert_eq!(runtime.scene_sources("main"), Some(&[source, source][..]));
    assert_eq!(runtime.source_count(), 1);
    assert_eq!(runtime.compositor_metrics().capture_latency().samples(), 1);

    runtime
        .clear_scene_sources("main")
        .expect("scene references clear");
    runtime
        .destroy_source(source)
        .expect("shared source can be destroyed after clear");
}

#[test]
fn stable_scene_item_ids_address_transforms_without_order_indices() {
    let plugin = BuiltinPlugin::new().expect("builtins are valid");
    let mut runtime = Runtime::new();
    runtime
        .register_plugin(&plugin)
        .expect("registration succeeds");
    runtime.create_scene("main").expect("scene is new");
    let source = runtime
        .create_source("color_source", "shared", &settings(2, 2, "#204060FF"))
        .expect("source is valid");

    runtime
        .attach_source_instance_with_id("main", source, "group/foreground")
        .expect("first item attaches");
    runtime
        .attach_source_instance_with_id("main", source, "group/background")
        .expect("second item attaches");
    assert_eq!(
        runtime.scene_item_ids("main"),
        Some(vec![
            "group/foreground".to_owned(),
            "group/background".to_owned()
        ])
    );

    let transform = FrameTransform::new(750, 600, 32, -12, false, false, 192).expect("transform");
    runtime
        .set_scene_item_transform_by_id("main", "group/background", transform)
        .expect("stable identity addresses the second item");
    assert_eq!(
        runtime.scene_item_transform_by_id("main", "group/background"),
        Some(transform)
    );
    assert_eq!(
        runtime.attach_source_instance_with_id("main", source, "group/background"),
        Err(RuntimeError::DuplicateSceneItem(
            "group/background".to_owned()
        ))
    );
    assert_eq!(
        runtime.set_scene_item_transform_by_id("main", "missing", transform),
        Err(RuntimeError::SceneItemNotAttached("missing".to_owned()))
    );
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

    let color_fade = runtime
        .render_scene_transition(
            "from",
            "to",
            &request,
            FrameTransition::fade_to_color(500, [0, 255, 0, 255]).expect("valid color fade"),
        )
        .expect("color fade succeeds")
        .expect("both scenes have frames");
    assert_eq!(color_fade.pixel(0, 0), Some([0, 255, 0, 255]));
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
