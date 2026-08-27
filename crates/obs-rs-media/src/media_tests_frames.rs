use super::*;

#[test]
fn parses_bounded_rgba_hex_colors() {
    assert_eq!(parse_rgba8_hex("#00FF00"), Some([0, 255, 0, 255]));
    assert_eq!(parse_rgba8_hex("0000FF80"), Some([0, 0, 255, 128]));
    assert_eq!(parse_rgba8_hex("#12345"), None);
    assert_eq!(parse_rgba8_hex("#GG0000"), None);
    assert_eq!(parse_rgba8_hex("#000000000"), None);
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
fn simple_nested_transforms_compose_without_approximating_unsupported_features() {
    let child =
        FrameTransform::new(1_500, 800, 10, -4, false, false, 200).expect("child transform");
    let parent =
        FrameTransform::new(2_000, 1_500, 20, 30, false, false, 128).expect("parent transform");

    let composed = child.compose_simple(parent).expect("simple composition");
    assert_eq!(composed.scale_x_milli(), 3_000);
    assert_eq!(composed.scale_y_milli(), 1_200);
    assert_eq!(composed.translate_x(), 40);
    assert_eq!(composed.translate_y(), 24);
    assert_eq!(composed.opacity(), 100);

    let cropped = child.with_crop(1, 0, 0, 0).expect("bounded crop");
    assert_eq!(
        cropped.compose_simple(parent),
        Err(MediaError::InvalidTransform)
    );
    let rotated = child.with_rotation_degrees(15).expect("bounded rotation");
    assert_eq!(
        rotated.compose_simple(parent),
        Err(MediaError::InvalidTransform)
    );
    let flipped = FrameTransform::new(1_000, 1_000, 0, 0, true, false, 255).expect("bounded flip");
    assert_eq!(
        flipped.compose_simple(parent),
        Err(MediaError::InvalidTransform)
    );
}

#[test]
fn axis_aligned_nested_transforms_compose_crop_and_mirroring_around_canvas_bounds() {
    let child = FrameTransform::new(500, 500, 1, 2, true, false, 200).expect("child transform");
    let parent =
        FrameTransform::new(2_000, 2_000, 3, 4, true, true, 128).expect("parent transform");

    let composed = child
        .compose_axis_aligned(parent, 8, 6)
        .expect("axis-aligned composition");
    assert_eq!(composed.scale_x_milli(), 1_000);
    assert_eq!(composed.scale_y_milli(), 1_000);
    assert_eq!(composed.translate_x(), 9);
    assert_eq!(composed.translate_y(), 6);
    assert!(!composed.flip_x());
    assert!(composed.flip_y());
    assert_eq!(composed.opacity(), 100);

    let cropped = child.with_crop(1, 0, 0, 0).expect("bounded crop");
    let cropped_composed = cropped
        .compose_axis_aligned(parent, 8, 6)
        .expect("cropped leaf composition");
    assert_eq!(cropped_composed.scale_x_milli(), 1_000);
    assert_eq!(cropped_composed.scale_y_milli(), 1_000);
    assert_eq!(cropped_composed.translate_x(), 11);
    assert_eq!(cropped_composed.translate_y(), 6);
    assert!(!cropped_composed.flip_x());
    assert!(cropped_composed.flip_y());
    assert_eq!(cropped_composed.crop_left(), 1);
    assert_eq!(cropped_composed.crop_top(), 0);
    assert_eq!(cropped_composed.crop_right(), 0);
    assert_eq!(cropped_composed.crop_bottom(), 0);

    let rotated = child.with_rotation_degrees(15).expect("bounded rotation");
    assert_eq!(
        rotated.compose_axis_aligned(parent, 8, 6),
        Err(MediaError::InvalidTransform)
    );

    let uniform_parent =
        FrameTransform::new(2_000, 2_000, 3, 4, false, false, 128).expect("uniform parent");
    let rotated_composed = rotated
        .compose_axis_aligned(uniform_parent, 8, 6)
        .expect("uniform parent preserves leaf rotation");
    assert_eq!(rotated_composed.scale_x_milli(), 1_000);
    assert_eq!(rotated_composed.scale_y_milli(), 1_000);
    assert_eq!(rotated_composed.translate_x(), 5);
    assert_eq!(rotated_composed.translate_y(), 8);
    assert!(rotated_composed.flip_x());
    assert!(!rotated_composed.flip_y());
    assert_eq!(rotated_composed.rotation_milli_degrees(), 15_000);

    let non_uniform_parent =
        FrameTransform::new(2_000, 1_500, 3, 4, false, false, 128).expect("non-uniform parent");
    assert_eq!(
        rotated.compose_axis_aligned(non_uniform_parent, 8, 6),
        Err(MediaError::InvalidTransform)
    );
    assert_eq!(
        child.compose_axis_aligned(parent, 0, 6),
        Err(MediaError::InvalidTransform)
    );
}

#[test]
fn axis_aligned_nested_mirroring_matches_the_reference_renderer() {
    let format =
        VideoFormat::new(4, 2, FrameRate::new(30, 1).expect("rate")).expect("valid format");
    let frame = VideoFrame::new(
        format,
        Timestamp::ZERO,
        (0_u8..8).flat_map(|value| [value, 0, 0, u8::MAX]).collect(),
    )
    .expect("pixels");
    let child =
        FrameTransform::new(1_000, 1_000, 0, 0, true, false, u8::MAX).expect("child transform");
    let parent =
        FrameTransform::new(1_000, 1_000, 0, 0, false, true, u8::MAX).expect("parent transform");
    let composed = child
        .compose_axis_aligned(parent, format.width(), format.height())
        .expect("axis-aligned composition");

    let sequential = frame
        .clone()
        .transformed(child)
        .and_then(|intermediate| intermediate.transformed(parent))
        .expect("sequential transforms");
    let direct = frame.transformed(composed).expect("composed transform");
    assert_eq!(direct, sequential);
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
fn crop_pad_filter_clears_edges_without_changing_frame_geometry() {
    let format =
        VideoFormat::new(4, 3, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let pixels = (0_u8..12).flat_map(|value| [value, 10, 20, 255]).collect();
    let frame = VideoFrame::new(format, Timestamp::ZERO, pixels).expect("frame");
    let filtered = frame.filtered(FrameFilter::CropPad {
        left: 1,
        top: 1,
        right: 1,
        bottom: 1,
    });

    assert_eq!(filtered.format(), format);
    assert_eq!(filtered.pixel(0, 0), Some([0, 0, 0, 0]));
    assert_eq!(filtered.pixel(1, 1), Some([5, 10, 20, 255]));
    assert_eq!(filtered.pixel(3, 1), Some([0, 0, 0, 0]));
    assert_eq!(filtered.pixel(1, 2), Some([0, 0, 0, 0]));
    assert_eq!(frame.pixel(1, 1), Some([5, 10, 20, 255]));
}

#[test]
fn scroll_filter_uses_timestamped_pixel_offsets_and_bounded_edges() {
    let format = VideoFormat::new(4, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    let frame = VideoFrame::new(
        format,
        Timestamp::from_nanos(1_000_000_000),
        vec![10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255, 40, 0, 0, 255],
    )
    .expect("frame");

    let looped = frame.filtered(FrameFilter::Scroll {
        speed_x: 1,
        speed_y: 0,
        looped: true,
    });
    assert_eq!(looped.pixel(0, 0), Some([20, 0, 0, 255]));
    assert_eq!(looped.pixel(3, 0), Some([10, 0, 0, 255]));

    let bounded = frame.filtered(FrameFilter::Scroll {
        speed_x: 1,
        speed_y: 0,
        looped: false,
    });
    assert_eq!(bounded.pixel(2, 0), Some([40, 0, 0, 255]));
    assert_eq!(bounded.pixel(3, 0), Some([0, 0, 0, 0]));
    assert_eq!(
        VideoFrame::solid(format, Timestamp::ZERO, [9, 8, 7, 255]).filtered(FrameFilter::Scroll {
            speed_x: 500,
            speed_y: -500,
            looped: true,
        },),
        VideoFrame::solid(format, Timestamp::ZERO, [9, 8, 7, 255])
    );
}

#[test]
fn render_delay_buffer_warms_up_and_preserves_timestamped_pixels() {
    let format = VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut history = RenderDelayBuffer::new();
    history
        .set_milliseconds(100)
        .expect("100 ms is in the OBS range");

    assert!(history
        .push(VideoFrame::solid(format, Timestamp::ZERO, [10, 0, 0, 255]))
        .expect("first frame")
        .is_none());
    assert!(history
        .push(VideoFrame::solid(
            format,
            Timestamp::from_millis(33),
            [20, 0, 0, 255],
        ))
        .expect("second frame")
        .is_none());
    assert!(history
        .push(VideoFrame::solid(
            format,
            Timestamp::from_millis(66),
            [30, 0, 0, 255],
        ))
        .expect("third frame")
        .is_none());

    let delayed = history
        .push(VideoFrame::solid(
            format,
            Timestamp::from_millis(100),
            [40, 0, 0, 255],
        ))
        .expect("warm-up completes")
        .expect("oldest frame is ready");
    assert_eq!(delayed.pixel(0, 0), Some([10, 0, 0, 255]));
    assert_eq!(delayed.timestamp(), Timestamp::from_millis(100));

    let next = history
        .push(VideoFrame::solid(
            format,
            Timestamp::from_millis(133),
            [50, 0, 0, 255],
        ))
        .expect("next frame")
        .expect("second delayed frame");
    assert_eq!(next.pixel(0, 0), Some([20, 0, 0, 255]));
}

#[test]
fn render_delay_buffer_resets_on_timeline_jumps_and_rejects_unbounded_history() {
    let format = VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("rate")).expect("format");
    let mut history = RenderDelayBuffer::new();
    history
        .set_milliseconds(100)
        .expect("100 ms is in the OBS range");
    history
        .push(VideoFrame::solid(format, Timestamp::ZERO, [1, 0, 0, 255]))
        .expect("first frame");
    history
        .push(VideoFrame::solid(
            format,
            Timestamp::from_millis(1_500),
            [2, 0, 0, 255],
        ))
        .expect("timeline reset frame");
    assert_eq!(history.buffered_frames(), 1);

    let high_rate =
        VideoFormat::new(1, 1, FrameRate::new(1_000, 1).expect("rate")).expect("format");
    let mut bounded = RenderDelayBuffer::new();
    bounded
        .set_milliseconds(MAX_RENDER_DELAY_MILLISECONDS)
        .expect("maximum delay is valid");
    assert_eq!(
        bounded.push(VideoFrame::solid(
            high_rate,
            Timestamp::ZERO,
            [0, 0, 0, 255]
        )),
        Err(RenderDelayError::FrameCapacity {
            required: 501,
            maximum: MAX_RENDER_DELAY_HISTORY_FRAMES,
        })
    );

    let mut stagnant = RenderDelayBuffer::new();
    stagnant
        .set_milliseconds(100)
        .expect("100 ms is in the OBS range");
    for _ in 0..MAX_RENDER_DELAY_HISTORY_FRAMES {
        stagnant
            .push(VideoFrame::solid(format, Timestamp::ZERO, [3, 0, 0, 255]))
            .expect("duplicate timestamps remain bounded until the cap");
    }
    assert_eq!(
        stagnant.push(VideoFrame::solid(format, Timestamp::ZERO, [4, 0, 0, 255])),
        Err(RenderDelayError::FrameCapacity {
            required: MAX_RENDER_DELAY_HISTORY_FRAMES + 1,
            maximum: MAX_RENDER_DELAY_HISTORY_FRAMES,
        })
    );
}

/// Reports the bounded timestamp queue overhead for the CPU Render Delay
/// oracle. Pixel storage is intentionally shared here so the measurement
/// isolates queue/state work from capture allocation.
#[test]
#[ignore = "timing report, not a pass/fail assertion"]
fn render_delay_buffer_timing_report() {
    use std::time::Instant;

    let format = VideoFormat::new(640, 360, FrameRate::new(60, 1).expect("rate")).expect("format");
    let source = VideoFrame::solid(format, Timestamp::ZERO, [32, 96, 160, 255]);
    let mut history = RenderDelayBuffer::new();
    history
        .set_milliseconds(100)
        .expect("100 ms is in the OBS range");
    let runs = 120_u64;
    let start = Instant::now();
    let mut checksum = 0_u64;
    for index in 0..runs {
        let frame = source.at_timestamp(Timestamp::from_nanos(
            (index + 1).saturating_mul(16_666_667),
        ));
        if let Some(delayed) = history.push(frame).expect("bounded history") {
            checksum = checksum.wrapping_add(u64::from(delayed.pixels()[0]));
            std::hint::black_box(&delayed);
        }
    }
    let elapsed = start.elapsed();
    println!(
        "render delay buffer: {runs} timestamped 640x360 pushes = {elapsed:?} total (about {:?}/push), buffered={}, checksum={checksum}",
        elapsed / u32::try_from(runs).expect("runs fit"),
        history.buffered_frames(),
    );
}
