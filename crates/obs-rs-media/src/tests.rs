use super::*;

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
fn transitions_are_deterministic_and_validate_progress() {
    let source = VideoFrame::solid(format(), Timestamp::ZERO, [0, 0, 0, 0]);
    let destination = VideoFrame::solid(format(), Timestamp::from_millis(10), [100, 200, 255, 255]);
    let transition = FrameTransition::cross_fade(500).expect("valid progress");
    let halfway = VideoFrame::transitioned(&source, &destination, transition).expect("transition");
    assert_eq!(halfway.timestamp(), Timestamp::from_millis(10));
    assert_eq!(halfway.pixel(0, 0), Some([50, 100, 128, 128]));
    assert_eq!(
        FrameTransition::cross_fade(1_001),
        Err(MediaError::InvalidTransition {
            progress_milli: 1_001
        })
    );
}
