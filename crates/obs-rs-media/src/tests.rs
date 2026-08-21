use super::*;
use std::sync::Arc;

fn format() -> VideoFormat {
    VideoFormat::new(2, 2, FrameRate::new(60, 2).expect("valid rate")).expect("valid format")
}

#[test]
fn reduces_rates_and_reports_period() {
    let rate = FrameRate::new(60, 2).expect("valid rate");

    assert_eq!(rate.numerator(), 30);
    assert_eq!(rate.denominator(), 1);
    assert_eq!(rate.period_nanos(), Some(33_333_333));
}

#[test]
fn rejects_invalid_formats_and_buffers() {
    let rate = FrameRate::new(30, 1).expect("valid rate");

    assert_eq!(FrameRate::new(0, 1), Err(MediaError::InvalidFrameRate));
    assert_eq!(VideoFormat::new(0, 2, rate), Err(MediaError::ZeroDimension));
    assert_eq!(
        VideoFrame::new(format(), Timestamp::ZERO, vec![0; 3]),
        Err(MediaError::BufferSize {
            expected: 16,
            actual: 3
        })
    );
}

#[test]
fn blends_and_reads_pixels_deterministically() {
    let background = VideoFrame::solid(format(), Timestamp::ZERO, [0, 0, 255, 255]);
    let foreground = VideoFrame::solid(format(), Timestamp::ZERO, [255, 0, 0, 128]);
    let mut result = background;
    result.blend_over(&foreground).expect("matching formats");

    assert_eq!(result.pixel(0, 0), Some([128, 0, 127, 255]));
    assert_eq!(result.pixel(2, 0), None);
    assert_ne!(result.checksum(), 0);
}

#[test]
fn transforms_flip_and_apply_opacity() {
    let format =
        VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format");
    let frame = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![255, 0, 0, 255, 0, 0, 255, 255],
    )
    .expect("valid pixels");
    let transform =
        FrameTransform::new(1_000, 1_000, 0, 0, true, false, 128).expect("valid transform");
    let transformed = frame.transformed(transform).expect("transform succeeds");

    assert_eq!(transformed.pixel(0, 0), Some([0, 0, 255, 128]));
    assert_eq!(transformed.pixel(1, 0), Some([255, 0, 0, 128]));
}

#[test]
fn transforms_crop_source_edges_before_scaling_and_flipping() {
    let format =
        VideoFormat::new(4, 1, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let frame = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255, 40, 0, 0, 255],
    )
    .expect("pixels");
    let cropped = FrameTransform::IDENTITY
        .with_crop(1, 0, 1, 0)
        .expect("crop");
    let result = frame.transformed(cropped).expect("render crop");

    assert_eq!(result.pixel(0, 0), Some([20, 0, 0, 255]));
    assert_eq!(result.pixel(1, 0), Some([30, 0, 0, 255]));
    assert_eq!(result.pixel(2, 0), Some([0, 0, 0, 0]));

    let flipped = FrameTransform::new(1_000, 1_000, 0, 0, true, false, 255)
        .expect("flip")
        .with_crop(1, 0, 1, 0)
        .expect("crop");
    let result = frame.transformed(flipped).expect("render flipped crop");
    assert_eq!(result.pixel(0, 0), Some([30, 0, 0, 255]));
    assert_eq!(result.pixel(1, 0), Some([20, 0, 0, 255]));
}

#[test]
fn transforms_rotate_clockwise_around_the_visible_source_centre() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let frame = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, // red, green
            0, 0, 255, 255, 255, 255, 255, 255, // blue, white
        ],
    )
    .expect("pixels");
    let transform = FrameTransform::IDENTITY
        .with_rotation_degrees(90)
        .expect("rotation");

    let result = frame.transformed(transform).expect("rotate");
    assert_eq!(result.pixel(0, 0), Some([0, 0, 255, 255]));
    assert_eq!(result.pixel(1, 0), Some([255, 0, 0, 255]));
    assert_eq!(result.pixel(0, 1), Some([255, 255, 255, 255]));
    assert_eq!(result.pixel(1, 1), Some([0, 255, 0, 255]));

    let owned = frame
        .clone()
        .into_transformed(transform)
        .expect("owned rotate");
    assert_eq!(owned, result);
}

#[test]
fn rotation_validation_is_bounded_and_subdegree_values_round_trip() {
    assert_eq!(
        FrameTransform::IDENTITY
            .with_rotation_milli_degrees(FrameTransform::MAX_ROTATION_MILLI_DEGREES + 1),
        Err(MediaError::InvalidTransform)
    );
    let transform = FrameTransform::IDENTITY
        .with_rotation_milli_degrees(-12_500)
        .expect("subdegree rotation");
    assert_eq!(transform.rotation_milli_degrees(), -12_500);
    assert_eq!(transform.rotation_degrees(), -12);
}

#[test]
fn a_crop_that_consumes_the_frame_is_rejected_at_render_time() {
    let frame = VideoFrame::solid(format(), Timestamp::ZERO, [1, 2, 3, 255]);
    let transform = FrameTransform::IDENTITY
        .with_crop(1, 0, 1, 0)
        .expect("bounded crop");

    assert_eq!(
        frame.transformed(transform),
        Err(MediaError::InvalidTransform)
    );
}

#[test]
fn filters_modify_owned_pixels_without_mutating_the_input() {
    let format =
        VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format");
    let frame = VideoFrame::solid(format, Timestamp::ZERO, [100, 150, 200, 255]);
    let filtered = frame
        .filtered(FrameFilter::Grayscale)
        .filtered(FrameFilter::Brightness { milli: 500 })
        .filtered(FrameFilter::Opacity(128));

    assert_eq!(frame.pixel(0, 0), Some([100, 150, 200, 255]));
    assert_eq!(filtered.pixel(0, 0), Some([210, 210, 210, 128]));
}

#[test]
fn shared_capture_storage_clones_without_copy_and_detaches_on_mutation() {
    // Thread-scoped counters: the process-wide ones are perturbed by any other
    // test rendering concurrently in this binary.
    reset_thread_frame_memory_metrics();
    let pixels = Arc::new(vec![
        100, 150, 200, 255, 100, 150, 200, 255, 100, 150, 200, 255, 100, 150, 200, 255,
    ]);
    let frame = VideoFrame::from_shared(format(), Timestamp::ZERO, Arc::clone(&pixels))
        .expect("valid shared frame");
    let mut filtered = frame.clone();

    assert_eq!(thread_frame_memory_metrics().shared_clones(), 1);
    assert_eq!(thread_frame_memory_metrics().copy_on_write_buffers(), 0);
    filtered.apply_filter(FrameFilter::Grayscale);

    assert_eq!(frame.pixel(0, 0), Some([100, 150, 200, 255]));
    assert_eq!(filtered.pixel(0, 0), Some([140, 140, 140, 255]));
    let metrics = thread_frame_memory_metrics();
    assert_eq!(metrics.copy_on_write_buffers(), 1);
    assert_eq!(metrics.copy_on_write_bytes(), format().rgba_bytes());
    assert_eq!(Arc::strong_count(&pixels), 2);
}

#[test]
fn transparent_rgb_can_be_canonicalized_for_composition() {
    let frame = VideoFrame::solid(format(), Timestamp::ZERO, [100, 150, 200, 0]);
    let mut canonical = frame.clone();
    canonical.clear_transparent_rgb();

    assert_eq!(frame.pixel(0, 0), Some([100, 150, 200, 0]));
    assert_eq!(canonical.pixel(0, 0), Some([0, 0, 0, 0]));
}

#[test]
fn converts_packed_layouts_to_rgba() {
    let format = VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    let bgra = RawVideoFrame::new(
        format,
        PixelFormat::Bgra8,
        Timestamp::from_millis(4),
        vec![3, 2, 1, 4, 7, 6, 5, 8],
    )
    .expect("bgra");
    assert_eq!(
        bgra.into_rgba8().expect("converted").pixels(),
        &[1, 2, 3, 4, 5, 6, 7, 8]
    );

    let gray =
        RawVideoFrame::new(format, PixelFormat::Gray8, Timestamp::ZERO, vec![9, 10]).expect("gray");
    assert_eq!(
        gray.into_rgba8().expect("converted").pixels(),
        &[9, 9, 9, 255, 10, 10, 10, 255]
    );
}

#[test]
fn converts_i420_and_rejects_odd_planar_dimensions() {
    let format = format();
    let i420 = RawVideoFrame::new(
        format,
        PixelFormat::I420,
        Timestamp::ZERO,
        vec![235, 235, 235, 235, 128, 128],
    )
    .expect("i420");
    assert_eq!(
        i420.into_rgba8().expect("converted").pixel(1, 1),
        Some([255, 255, 255, 255])
    );

    let odd = VideoFormat::new(3, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    assert_eq!(
        PixelFormat::I420.bytes_for(odd),
        Err(MediaError::UnsupportedPixelDimensions {
            pixel_format: PixelFormat::I420
        })
    );
}

#[test]
fn nv12_and_p010_have_validated_layouts_and_convert_to_rgba() {
    let format = VideoFormat::new(2, 2, FrameRate::new(30, 1).expect("rate")).expect("format");
    let nv12 = RawVideoFrame::new(
        format,
        PixelFormat::Nv12,
        Timestamp::ZERO,
        vec![16, 235, 81, 145, 128, 128],
    )
    .expect("NV12");
    assert_eq!(nv12.bytes().len(), 6);
    assert_eq!(nv12.into_rgba8().expect("RGBA").pixels()[3], 255);

    let word = |value: u16| (value << 6).to_le_bytes();
    let p010 = [
        word(64),
        word(940),
        word(256),
        word(580),
        word(512),
        word(512),
    ]
    .into_iter()
    .flatten()
    .collect();
    let p010 = RawVideoFrame::new(format, PixelFormat::P010, Timestamp::ZERO, p010).expect("P010");
    assert_eq!(p010.bytes().len(), 12);
    assert_eq!(p010.into_rgba8().expect("RGBA").pixels()[3], 255);
}

#[test]
fn transitions_are_deterministic_and_validate_progress() {
    let source = VideoFrame::solid(format(), Timestamp::ZERO, [0, 0, 0, 0]);
    let destination = VideoFrame::solid(format(), Timestamp::from_millis(10), [100, 200, 255, 255]);
    let transition = FrameTransition::cross_fade(500).expect("valid progress");
    let halfway = VideoFrame::transitioned(&source, destination, transition).expect("transition");
    assert_eq!(halfway.timestamp(), Timestamp::from_millis(10));
    assert_eq!(halfway.pixel(0, 0), Some([50, 100, 128, 128]));
    assert_eq!(
        FrameTransition::cross_fade(1_001),
        Err(MediaError::InvalidTransition {
            progress_milli: 1_001
        })
    );
}

#[test]
fn fast_division_by_255_matches_integer_division() {
    // Blending feeds this at most `255 * 255 * 2`, so the identity is checked
    // across the whole range a composite can produce.
    for value in 0..=(255_u32 * 255 * 2) {
        assert_eq!(
            crate::frame::divide_by_255(value),
            value / 255,
            "divide_by_255({value})"
        );
    }
}

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
    measure(
        "solid",
        Box::new(move || {
            std::hint::black_box(VideoFrame::solid(format, Timestamp::ZERO, [1, 2, 3, 4]));
        }),
    );
}
