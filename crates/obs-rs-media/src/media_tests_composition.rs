use super::*;

/// Builds a frame whose pixels all differ, so a wrong offset cannot pass.
fn gradient_frame(width: u32, height: u32) -> VideoFrame {
    let format =
        VideoFormat::new(width, height, FrameRate::new(30, 1).expect("rate")).expect("format");
    let pixels = (0..width * height)
        .flat_map(|index| {
            let value = u8::try_from(index % 251).expect("gradient value");
            [value, value.wrapping_add(7), value.wrapping_add(29), 255]
        })
        .collect::<Vec<_>>();
    VideoFrame::new(format, Timestamp::ZERO, pixels).expect("gradient frame")
}

/// The reference nearest-neighbour transform, kept as the correctness oracle
/// for the fast paths in `VideoFrame::transformed`.
fn reference_transform(frame: &VideoFrame, transform: FrameTransform) -> VideoFrame {
    let format = frame.format();
    let mut output = VideoFrame::solid(format, frame.timestamp(), [0, 0, 0, 0]);
    let width = i64::from(format.width());
    let height = i64::from(format.height());
    let mut pixels = output.pixels().to_vec();
    for y in 0..format.height() {
        let local_y = i64::from(y) - i64::from(transform.translate_y());
        if local_y < 0 {
            continue;
        }
        let mut source_y = local_y * 1_000 / i64::from(transform.scale_y_milli());
        if source_y >= height {
            continue;
        }
        if transform.flip_y() {
            source_y = height - 1 - source_y;
        }
        for x in 0..format.width() {
            let local_x = i64::from(x) - i64::from(transform.translate_x());
            if local_x < 0 {
                continue;
            }
            let mut source_x = local_x * 1_000 / i64::from(transform.scale_x_milli());
            if source_x >= width {
                continue;
            }
            if transform.flip_x() {
                source_x = width - 1 - source_x;
            }
            let source = frame
                .pixel(
                    u32::try_from(source_x).expect("source column"),
                    u32::try_from(source_y).expect("source row"),
                )
                .expect("source pixel");
            let offset = (y as usize * format.width() as usize + x as usize) * 4;
            pixels[offset..offset + 3].copy_from_slice(&source[..3]);
            pixels[offset + 3] = u8::try_from(
                (u32::from(source[3]) * u32::from(transform.opacity()) / 255).min(255),
            )
            .expect("alpha byte");
        }
    }
    output = VideoFrame::new(format, frame.timestamp(), pixels).expect("reference frame");
    output
}

#[test]
fn transform_fast_paths_match_the_reference_resampler() {
    let frame = gradient_frame(37, 21);
    let cases = [
        FrameTransform::IDENTITY,
        FrameTransform::new(1_000, 1_000, 0, 0, false, false, 128).expect("opacity only"),
        FrameTransform::new(1_000, 1_000, 5, 3, false, false, 255).expect("translate only"),
        FrameTransform::new(1_000, 1_000, -4, -2, false, false, 200).expect("negative translate"),
        FrameTransform::new(2_000, 1_500, 2, 1, false, false, 255).expect("upscale"),
        FrameTransform::new(500, 500, 0, 0, false, false, 255).expect("downscale"),
        FrameTransform::new(1_000, 1_000, 0, 0, true, false, 255).expect("flip x"),
        FrameTransform::new(1_000, 1_000, 0, 0, false, true, 255).expect("flip y"),
        FrameTransform::new(1_500, 800, 3, -2, true, true, 90).expect("everything"),
    ];

    for transform in cases {
        let expected = reference_transform(&frame, transform);
        let actual = frame.transformed(transform).expect("transform");
        assert_eq!(
            actual.pixels(),
            expected.pixels(),
            "transform {transform:?} diverged from the reference"
        );
        let owned = frame
            .clone()
            .into_transformed(transform)
            .expect("owned transform");
        assert_eq!(
            owned.pixels(),
            expected.pixels(),
            "owned transform {transform:?} diverged from the reference"
        );
    }
}

#[test]
fn blending_onto_an_opaque_background_matches_the_general_formula() {
    let format = VideoFormat::new(4, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    // The opaque-background fast path must agree with straight-alpha blending
    // for every source alpha, including the boundaries.
    for source_alpha in [0_u8, 1, 64, 128, 200, 254, 255] {
        let background = VideoFrame::solid(format, Timestamp::ZERO, [10, 200, 30, 255]);
        let foreground = VideoFrame::solid(format, Timestamp::ZERO, [250, 20, 90, source_alpha]);
        let mut actual = background.clone();
        actual.blend_over(&foreground).expect("blend");

        let alpha = u32::from(source_alpha);
        let inverse = 255 - alpha;
        let channel = |source: u32, background: u32| {
            u8::try_from(((source * alpha + background * inverse) / 255).min(255))
                .expect("channel byte")
        };
        let expected = [channel(250, 10), channel(20, 200), channel(90, 30), 255];
        assert_eq!(
            actual.pixel(0, 0),
            Some(expected),
            "source alpha {source_alpha}"
        );
    }
}

#[test]
fn retaining_the_foreground_buffer_is_pixel_identical_to_blend_over() {
    let format = VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    for (source_alpha, background_alpha) in [
        (0, 0),
        (0, 255),
        (1, 1),
        (64, 128),
        (200, 33),
        (254, 255),
        (255, 64),
    ] {
        let background =
            VideoFrame::solid(format, Timestamp::ZERO, [10, 200, 30, background_alpha]);
        let foreground = VideoFrame::solid(format, Timestamp::ZERO, [250, 20, 90, source_alpha]);
        let mut expected = background.clone();
        expected.blend_over(&foreground).expect("blend over");
        let mut actual = foreground;
        actual.blend_under(&background).expect("blend under");
        assert_eq!(actual.pixels(), expected.pixels());
    }
}

#[test]
fn solid_frames_fill_every_pixel_with_the_requested_colour() {
    // The doubling fill must cover buffers whose length is not a power of two.
    for (width, height) in [(1, 1), (3, 5), (17, 9), (64, 64)] {
        let format =
            VideoFormat::new(width, height, FrameRate::new(30, 1).expect("rate")).expect("format");
        let frame = VideoFrame::solid(format, Timestamp::ZERO, [11, 22, 33, 44]);

        assert_eq!(frame.pixels().len(), format.rgba_bytes());
        assert!(
            frame
                .pixels()
                .chunks_exact(4)
                .all(|pixel| pixel == [11, 22, 33, 44]),
            "{width}x{height} was not filled uniformly"
        );
    }
}

/// Reports the cost of the composition primitives at a typical canvas size.
///
/// Kept as an ignored test so a change to the pixel paths can be measured with
/// `cargo test --release -p obs-rs-media -- --ignored --nocapture` rather than
/// guessed at. It asserts nothing: the machine decides the numbers.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the timing report keeps all composition measurements in one comparable fixture"
)]
#[ignore = "timing report, not a pass/fail assertion"]
fn composition_primitives_timing_report() {
    use std::time::Instant;

    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("rate")).expect("format");
    let background = VideoFrame::solid(format, Timestamp::ZERO, [10, 20, 30, 255]);
    let overlay = VideoFrame::solid(format, Timestamp::ZERO, [40, 50, 60, 200]);
    let scaled = FrameTransform::new(1_500, 1_500, 10, 10, false, false, 200).expect("transform");
    let rotated = FrameTransform::IDENTITY
        .with_rotation_degrees(90)
        .expect("rotation");
    let crop = FrameFilter::CropPad {
        left: 8,
        top: 8,
        right: 8,
        bottom: 8,
    };
    let color_key = ColorKey::new(32, 52, 200, 100, 100).expect("color key");
    let luma_key = LumaKey::new(900, 100, 40, 60).expect("luma key");
    let chroma_key = ChromaKey::new(0, 255, 0, 400, 80, 100).expect("chroma key");
    let sharpen = FrameFilter::Sharpen { milli: 80 };
    let color_wash =
        FrameFilter::ColorMultiplyAdd(ColorMultiplyAdd::new([220, 240, 255], [4, 8, 12]));
    let runs = 200;

    let measure = |label: &str, mut work: Box<dyn FnMut()>| {
        let start = Instant::now();
        for _ in 0..runs {
            work();
        }
        println!("{label}: {:?}", start.elapsed() / runs);
    };

    let frame = background.clone();
    measure(
        "transformed(identity)",
        Box::new(move || {
            std::hint::black_box(
                frame
                    .transformed(FrameTransform::IDENTITY)
                    .expect("identity"),
            );
        }),
    );
    let frame = background.clone();
    measure(
        "transformed(scaled)",
        Box::new(move || {
            std::hint::black_box(frame.transformed(scaled).expect("scaled"));
        }),
    );
    let frame = background.clone();
    measure(
        "transformed(rotated-90deg)",
        Box::new(move || {
            std::hint::black_box(frame.transformed(rotated).expect("rotated"));
        }),
    );
    let (frame, source) = (background.clone(), overlay.clone());
    measure(
        "clone + blend_over",
        Box::new(move || {
            let mut target = frame.clone();
            target.blend_over(&source).expect("blend");
            std::hint::black_box(target);
        }),
    );
    let frame = background.clone();
    measure(
        "clone + grayscale",
        Box::new(move || {
            let mut target = frame.clone();
            target.apply_filter(FrameFilter::Grayscale);
            std::hint::black_box(target);
        }),
    );
    let frame = background.clone();
    measure(
        "clone + crop-pad",
        Box::new(move || {
            let mut target = frame.clone();
            target.apply_filter(crop);
            std::hint::black_box(target);
        }),
    );
    let frame = background.clone();
    let correction = ColorCorrection::new(250, -500, 125, 750, 30, 900).expect("correction");
    measure(
        "clone + color-correction",
        Box::new(move || {
            let mut target = frame.clone();
            target.apply_filter(FrameFilter::ColorCorrection(correction));
            std::hint::black_box(target);
        }),
    );
    let frame = background.clone();
    measure(
        "clone + color-key",
        Box::new(move || {
            let mut target = frame.clone();
            target.apply_filter(FrameFilter::ColorKey(color_key));
            std::hint::black_box(target);
        }),
    );
    let frame = background.clone();
    measure(
        "clone + luma-key",
        Box::new(move || {
            let mut target = frame.clone();
            target.apply_filter(FrameFilter::LumaKey(luma_key));
            std::hint::black_box(target);
        }),
    );
    let frame = background.clone();
    measure(
        "clone + chroma-key",
        Box::new(move || {
            let mut target = frame.clone();
            target.apply_filter(FrameFilter::ChromaKey(chroma_key));
            std::hint::black_box(target);
        }),
    );
    let frame = background.clone();
    measure(
        "clone + sharpen",
        Box::new(move || {
            let mut target = frame.clone();
            target.apply_filter(sharpen);
            std::hint::black_box(target);
        }),
    );
    let frame = background.clone();
    measure(
        "clone + color-multiply-add",
        Box::new(move || {
            let mut target = frame.clone();
            target.apply_filter(color_wash);
            std::hint::black_box(target);
        }),
    );
    measure(
        "solid",
        Box::new(move || {
            std::hint::black_box(VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 4]));
        }),
    );
}

/// Reports the bounded CPU Scroll path, whose source snapshot makes the
/// reference implementation correct but allocates/copies once per active
/// frame. The GPU compositor is the production path for desktop preview.
#[test]
#[ignore = "timing report, not a pass/fail assertion"]
fn scroll_filter_timing_report() {
    use std::time::Instant;

    let format = VideoFormat::new(640, 360, FrameRate::new(30, 1).expect("rate")).expect("format");
    let source = VideoFrame::solid(format, Timestamp::ZERO, [32, 96, 160, 255]);
    let scroll = FrameFilter::Scroll {
        speed_x: 30,
        speed_y: -15,
        looped: true,
    };
    let runs = 120_u64;
    let start = Instant::now();
    let mut checksum = 0_u64;
    for index in 0..runs {
        let mut frame = source.at_timestamp(Timestamp::from_nanos(
            (index + 1).saturating_mul(33_333_333),
        ));
        frame.apply_filter(scroll);
        checksum = checksum.wrapping_add(u64::from(frame.pixels()[0]));
        std::hint::black_box(&frame);
    }
    let elapsed = start.elapsed();
    println!(
        "scroll filter: {runs} frames x 640x360 = {elapsed:?} total (about {:?}/frame), checksum={checksum}",
        elapsed / u32::try_from(runs).expect("runs fit")
    );
}
