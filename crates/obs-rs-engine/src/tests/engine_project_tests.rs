use super::*;

#[test]
fn nested_scene_items_flatten_to_shared_runtime_sources() {
    let mut project = project();
    let child_transform =
        FrameTransform::new(1_500, 800, 10, -4, false, false, 200).expect("child transform");
    let mut child = SceneSpec::new("child", "Child").expect("child scene");
    let mut child_item = SceneItemSpec::for_source("pattern").expect("child item");
    child_item.set_transform(child_transform);
    child.add_item(child_item).expect("child item attach");
    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: child,
        })
        .expect("add child scene");

    project
        .apply(ProjectCommand::AddScene {
            profile: "live".to_owned(),
            scene: SceneSpec::new("parent", "Parent").expect("parent scene"),
        })
        .expect("add parent scene");
    let parent_transform =
        FrameTransform::new(2_000, 1_500, 20, 30, false, false, 128).expect("parent transform");
    let mut nested = SceneItemSpec::for_scene("child-item", "child").expect("nested item");
    nested.set_transform(parent_transform);
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "parent".to_owned(),
            item: nested,
        })
        .expect("add nested item");

    let mut engine = EngineSession::new(project, EngineConfig::default()).expect("engine");
    assert_eq!(engine.runtime.source_count(), 1);
    assert_eq!(
        engine.runtime.scene_sources("child").map(<[SourceId]>::len),
        Some(1)
    );
    assert_eq!(
        engine
            .runtime
            .scene_sources("parent")
            .map(<[SourceId]>::len),
        Some(1)
    );
    assert_eq!(
        engine.runtime.scene_item_ids("parent"),
        Some(vec!["child-item/pattern".to_owned()])
    );
    assert_eq!(
        engine.runtime.scene_item_transform("parent", 0),
        Some(
            child_transform
                .compose_simple(parent_transform)
                .expect("compose")
        )
    );

    let frame = engine
        .render_scene("parent")
        .expect("render nested scene")
        .expect("nested scene has a frame");
    assert_eq!(frame.format(), engine.format());
    assert_eq!(engine.runtime.compositor_metrics().source_requests(), 1);
}

#[test]
fn group_items_flatten_to_shared_runtime_sources() {
    let mut project = project();
    let mut group = SceneItemSpec::for_group("group", "Group").expect("group");
    group
        .group_mut()
        .expect("group target")
        .add_item(SceneItemSpec::for_source("pattern").expect("group child"))
        .expect("group child attach");
    project
        .apply(ProjectCommand::AddSceneItem {
            profile: "live".to_owned(),
            scene: "program".to_owned(),
            item: group,
        })
        .expect("add group");

    let mut engine = EngineSession::new(project, EngineConfig::default()).expect("engine");
    assert_eq!(engine.runtime.source_count(), 1);
    assert_eq!(
        engine
            .runtime
            .scene_sources("program")
            .expect("program scene")
            .len(),
        2
    );
    let layers = engine
        .runtime
        .render_scene_layers(
            "program",
            &VideoRequest::new(Timestamp::ZERO, engine.format()),
        )
        .expect("group renders");
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].item_id(), "pattern");
    assert_eq!(layers[1].item_id(), "group/pattern");
    assert_eq!(engine.runtime.compositor_metrics().source_requests(), 2);
    assert_eq!(
        engine
            .runtime
            .compositor_metrics()
            .capture_latency()
            .samples(),
        1
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the compiler fixture covers each supported filter mapping at one project boundary"
)]
fn filter_compiler_keeps_renderer_details_out_of_project_values() {
    let brightness = SourceFilterSpec::new(
        "brightness",
        "Brightness",
        "brightness",
        Config::parse("milli = 350\n").expect("settings"),
    )
    .expect("filter");
    assert_eq!(
        compile_filter(&brightness),
        Some(FrameFilter::Brightness { milli: 350 })
    );

    let crop = SourceFilterSpec::new(
        "crop",
        "Crop/Pad",
        "crop_pad",
        Config::parse("bottom = 1\nleft = 2\nright = 3\ntop = 4\n").expect("crop settings"),
    )
    .expect("crop filter");
    assert_eq!(
        compile_filter(&crop),
        Some(FrameFilter::CropPad {
            left: 2,
            top: 4,
            right: 3,
            bottom: 1,
        })
    );

    let color_correction = SourceFilterSpec::new(
            "color",
            "Color Correction",
            "color_correction",
            Config::parse(
                "brightness = 125\ncontrast = -500\ngamma = 250\nhue_shift = 30\nopacity = 900\nsaturation = 750\n",
            )
            .expect("color correction settings"),
        )
        .expect("color correction filter");
    assert_eq!(
        compile_filter(&color_correction),
        Some(FrameFilter::ColorCorrection(
            ColorCorrection::new(250, -500, 125, 750, 30, 900).expect("valid color correction"),
        ))
    );

    let color_multiply_add = SourceFilterSpec::new(
            "color_wash",
            "Color Multiply/Add",
            "color_multiply_add",
            Config::parse(
                "add_blue = 12\nadd_green = 8\nadd_red = 4\nmultiply_blue = 255\nmultiply_green = 240\nmultiply_red = 220\n",
            )
            .expect("color wash settings"),
        )
        .expect("color multiply/add filter");
    assert_eq!(
        compile_filter(&color_multiply_add),
        Some(FrameFilter::ColorMultiplyAdd(ColorMultiplyAdd::new(
            [220, 240, 255],
            [4, 8, 12],
        )))
    );

    let luma_key = SourceFilterSpec::new(
        "luma",
        "Luma Key",
        "luma_key",
        Config::parse(
            "luma_max = 900\nluma_max_smooth = 40\nluma_min = 100\nluma_min_smooth = 60\n",
        )
        .expect("luma key settings"),
    )
    .expect("luma key filter");
    assert_eq!(
        compile_filter(&luma_key),
        Some(FrameFilter::LumaKey(
            LumaKey::new(900, 100, 40, 60).expect("valid luma key"),
        ))
    );

    let color_key = SourceFilterSpec::new(
        "key",
        "Color Key",
        "color_key",
        Config::parse(
            "key_blue = 0\nkey_green = 255\nkey_red = 0\nsimilarity = 120\nsmoothness = 80\n",
        )
        .expect("color key settings"),
    )
    .expect("color key filter");
    assert_eq!(
        compile_filter(&color_key),
        Some(FrameFilter::ColorKey(
            ColorKey::new(0, 255, 0, 120, 80).expect("valid color key"),
        ))
    );

    let chroma_key = SourceFilterSpec::new(
            "chroma",
            "Chroma Key",
            "chroma_key",
            Config::parse(
                "key_blue = 0\nkey_green = 255\nkey_red = 0\nsimilarity = 400\nsmoothness = 80\nspill = 100\n",
            )
            .expect("chroma key settings"),
        )
        .expect("chroma key filter");
    assert_eq!(
        compile_filter(&chroma_key),
        Some(FrameFilter::ChromaKey(
            ChromaKey::new(0, 255, 0, 400, 80, 100).expect("valid chroma key"),
        ))
    );

    let sharpen = SourceFilterSpec::new(
        "sharpen",
        "Sharpen",
        "sharpen",
        Config::parse("sharpness = 80\n").expect("sharpen settings"),
    )
    .expect("sharpen filter");
    assert_eq!(
        compile_filter(&sharpen),
        Some(FrameFilter::Sharpen { milli: 80 })
    );

    let scroll = SourceFilterSpec::new(
        "scroll",
        "Scroll",
        "scroll",
        Config::parse("loop = false\nspeed_x = 120\nspeed_y = -80\n").expect("scroll settings"),
    )
    .expect("scroll filter");
    assert_eq!(
        compile_filter(&scroll),
        Some(FrameFilter::Scroll {
            speed_x: 120,
            speed_y: -80,
            looped: false,
        })
    );

    let render_delay = SourceFilterSpec::new(
        "render-delay",
        "Render Delay",
        "render_delay",
        Config::parse("milliseconds = 100\n").expect("render delay settings"),
    )
    .expect("render delay filter");
    assert_eq!(
        compile_filter(&render_delay),
        Some(FrameFilter::RenderDelay(RenderDelay { milliseconds: 100 }))
    );
    let invalid_render_delay = SourceFilterSpec::new(
        "invalid-render-delay",
        "Invalid Render Delay",
        "render_delay",
        Config::parse("milliseconds = 501\n").expect("invalid delay settings"),
    )
    .expect("invalid render delay filter");
    assert_eq!(compile_filter(&invalid_render_delay), None);
    let mut invalid_scroll_settings = Config::new();
    invalid_scroll_settings
        .set("loop", "maybe")
        .expect("invalid boolean can be stored as an explicit string");
    invalid_scroll_settings
        .set("speed_x", "501")
        .expect("out-of-range speed can be stored");
    invalid_scroll_settings
        .set("speed_y", "0")
        .expect("valid vertical speed can be stored");
    let invalid_scroll = SourceFilterSpec::new(
        "invalid-scroll",
        "Invalid Scroll",
        "scroll",
        invalid_scroll_settings,
    )
    .expect("invalid scroll filter");
    assert_eq!(compile_filter(&invalid_scroll), None);

    let mut disabled = brightness.clone();
    disabled.set_enabled(false);
    assert_eq!(compile_filter(&disabled), None);

    let audio = SourceFilterSpec::with_category(
        "compressor",
        "Compressor",
        "compressor",
        SourceFilterCategory::AudioVideo,
        Config::new(),
    )
    .expect("audio filter");
    assert_eq!(compile_filter(&audio), None);

    let gain = SourceFilterSpec::with_category(
        "gain",
        "Gain",
        "gain",
        SourceFilterCategory::AudioVideo,
        Config::parse("db_milli = -6000\n").expect("gain settings"),
    )
    .expect("gain filter");
    assert_eq!(
        compile_audio_filter(&gain),
        Some(AudioFilter::gain_db_milli(-6_000).expect("valid gain"))
    );
    let invert = SourceFilterSpec::with_category(
        "invert",
        "Invert Polarity",
        "invert_polarity",
        SourceFilterCategory::AudioVideo,
        Config::new(),
    )
    .expect("invert polarity filter");
    assert_eq!(
        compile_audio_filter(&invert),
        Some(AudioFilter::InvertPolarity)
    );
    let limiter = SourceFilterSpec::with_category(
        "limiter",
        "Limiter",
        "limiter",
        SourceFilterCategory::AudioVideo,
        Config::parse("threshold_db_milli = -6000\nrelease_ms = 60\n").expect("limiter settings"),
    )
    .expect("limiter filter");
    assert_eq!(
        compile_audio_filter(&limiter),
        Some(AudioFilter::limiter_db_milli(-6_000, 60).expect("valid limiter"))
    );
    let compressor = SourceFilterSpec::with_category(
            "compressor_runtime",
            "Compressor",
            "compressor",
            SourceFilterCategory::AudioVideo,
            Config::parse(
                "ratio_milli = 10000\nthreshold_db_milli = -18000\nattack_ms = 6\nrelease_ms = 60\noutput_gain_db_milli = 0\n",
            )
            .expect("compressor settings"),
        )
        .expect("compressor filter");
    assert_eq!(
        compile_audio_filter(&compressor),
        Some(AudioFilter::compressor(10_000, -18_000, 6, 60, 0).expect("valid compressor"))
    );
    let expander = SourceFilterSpec::with_category(
            "expander_runtime",
            "Expander",
            "expander",
            SourceFilterCategory::AudioVideo,
            Config::parse(
                "ratio_milli = 10000\nthreshold_db_milli = -40000\nattack_ms = 10\nrelease_ms = 50\noutput_gain_db_milli = 0\n",
            )
            .expect("expander settings"),
        )
        .expect("expander filter");
    assert_eq!(
        compile_audio_filter(&expander),
        Some(AudioFilter::expander(10_000, -40_000, 10, 50, 0).expect("valid expander"))
    );
    let gate = SourceFilterSpec::with_category(
            "gate_runtime",
            "Gate",
            "gate",
            SourceFilterCategory::AudioVideo,
            Config::parse(
                "open_threshold_db_milli = -26000\nclose_threshold_db_milli = -32000\nattack_ms = 25\nhold_ms = 200\nrelease_ms = 150\n",
            )
            .expect("gate settings"),
        )
        .expect("gate filter");
    assert_eq!(
        compile_audio_filter(&gate),
        Some(AudioFilter::noise_gate(-26_000, -32_000, 25, 200, 150).expect("valid gate"))
    );
    assert_eq!(compile_audio_filter(&brightness), None);
}

#[test]
fn filter_compiler_reports_disabled_unsupported_and_invalid_instances() {
    let brightness = SourceFilterSpec::new(
        "brightness-report",
        "Brightness",
        "brightness",
        Config::parse("milli = 350\n").expect("settings"),
    )
    .expect("filter");
    assert!(matches!(
        compile_filter_report(&brightness),
        FilterCompilation::Applied(FrameFilter::Brightness { milli: 350 })
    ));

    let mut disabled = brightness.clone();
    disabled.set_enabled(false);
    assert_eq!(compile_filter_report(&disabled), FilterCompilation::Ignored);

    let invalid = SourceFilterSpec::new(
        "invalid-report",
        "Invalid Render Delay",
        "render_delay",
        Config::parse("milliseconds = 501\n").expect("settings"),
    )
    .expect("filter");
    assert!(matches!(
        compile_filter_report(&invalid),
        FilterCompilation::Unavailable(FilterDiagnostic {
            failure: FilterCompileFailure::InvalidSettings,
            ..
        })
    ));

    let unknown =
        SourceFilterSpec::new("unknown-report", "Unknown", "future_effect", Config::new())
            .expect("filter");
    assert!(matches!(
        compile_filter_report(&unknown),
        FilterCompilation::Unavailable(FilterDiagnostic {
            failure: FilterCompileFailure::UnsupportedKind,
            ..
        })
    ));

    let audio = SourceFilterSpec::with_category(
        "audio-report",
        "Compressor",
        "compressor",
        SourceFilterCategory::AudioVideo,
        Config::new(),
    )
    .expect("audio filter");
    assert!(matches!(
        compile_filter_report(&audio),
        FilterCompilation::Unavailable(FilterDiagnostic {
            failure: FilterCompileFailure::UnsupportedCategory,
            ..
        })
    ));
    assert!(matches!(
        compile_audio_filter_report(&audio),
        FilterCompilation::Unavailable(FilterDiagnostic {
            failure: FilterCompileFailure::InvalidSettings,
            ..
        })
    ));
}

#[test]
fn engine_snapshot_names_persisted_filters_not_available_in_renderer() {
    let mut project = project();
    let filter = SourceFilterSpec::new(
        "future-filter",
        "Future filter",
        "future_effect",
        Config::new(),
    )
    .expect("filter");
    project
        .apply(ProjectCommand::AddSourceFilter {
            profile: "live".to_owned(),
            source: "pattern".to_owned(),
            filter,
        })
        .expect("add filter");

    let engine = EngineSession::new(project, EngineConfig::default()).expect("engine");
    assert_eq!(
            engine.snapshot().filter_diagnostics,
            vec![
                "source 'Pattern' filter 'Future filter': filter 'future_effect' (effect) unavailable: unsupported kind"
                    .to_owned()
            ]
        );
}
