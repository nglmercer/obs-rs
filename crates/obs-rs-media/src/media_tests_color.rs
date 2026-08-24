use super::*;

#[test]
fn color_correction_uses_fixed_point_obs_ranges_and_preserves_noop_frames() {
    let format =
        VideoFormat::new(1, 1, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let frame = VideoFrame::solid(format, Timestamp::ZERO, [64, 128, 255, 255]);
    let identity = ColorCorrection::new(0, 0, 0, 0, 0, 1_000).expect("identity");
    assert_eq!(
        frame.filtered(FrameFilter::ColorCorrection(identity)),
        frame
    );

    let corrected = frame.filtered(FrameFilter::ColorCorrection(
        ColorCorrection::new(1_000, 0, 0, 0, 0, 500).expect("gamma and opacity"),
    ));
    assert_eq!(corrected.pixel(0, 0), Some([128, 181, 255, 128]));
    assert!(
        ColorCorrection::new(ColorCorrection::MIN_GAMMA_MILLI - 1, 0, 0, 0, 0, 1_000,).is_none()
    );
}

#[test]
fn color_multiply_add_matches_obs_color_wash_and_preserves_alpha() {
    let format =
        VideoFormat::new(2, 1, FrameRate::new(30, 1).expect("valid rate")).expect("valid format");
    let frame = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![100, 150, 200, 77, 255, 255, 255, 12],
    )
    .expect("frame");
    let color_wash = ColorMultiplyAdd::new([128, 255, 0], [10, 20, 30]);
    let washed = frame.filtered(FrameFilter::ColorMultiplyAdd(color_wash));

    assert_eq!(washed.pixel(0, 0), Some([60, 170, 30, 77]));
    assert_eq!(washed.pixel(1, 0), Some([138, 255, 30, 12]));
    assert_eq!(
        frame.filtered(FrameFilter::ColorMultiplyAdd(ColorMultiplyAdd::new(
            [255, 255, 255],
            [0, 0, 0],
        ))),
        frame,
        "OBS's default multiply/add colors are a bit-exact no-op"
    );
}

#[test]
fn color_key_clears_matching_pixels_and_feathers_the_edge() {
    let format =
        VideoFormat::new(3, 1, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let frame = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![0, 255, 0, 255, 32, 52, 200, 255, 255, 0, 0, 255],
    )
    .expect("frame");
    let key = ColorKey::new(0, 255, 0, 0, 0).expect("exact key");
    let keyed = frame.filtered(FrameFilter::ColorKey(key));

    assert_eq!(keyed.pixel(0, 0), Some([0, 0, 0, 0]));
    assert_eq!(keyed.pixel(1, 0), Some([32, 52, 200, 255]));
    assert_eq!(keyed.pixel(2, 0), Some([255, 0, 0, 255]));

    let feathered = frame.filtered(FrameFilter::ColorKey(
        ColorKey::new(0, 255, 0, 0, 1_000).expect("feathered key"),
    ));
    assert!(feathered.pixel(1, 0).expect("middle pixel")[3] < 255);
    assert!(ColorKey::new(0, 0, 0, 1_001, 0).is_none());
    assert!(ColorKey::new(0, 0, 0, 0, -1).is_none());
}

#[test]
fn chroma_key_uses_ycbcr_distance_and_reduces_spill() {
    let format =
        VideoFormat::new(3, 1, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let frame = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![
            0, 255, 0, 255, // exact green key
            0, 128, 0, 255, // green spill, outside the key threshold
            255, 0, 0, 255, // a distant chroma value
        ],
    )
    .expect("frame");
    let key = ChromaKey::new(0, 255, 0, 1, 80, 1_000).expect("chroma key");
    let keyed = frame.filtered(FrameFilter::ChromaKey(key));

    assert_eq!(keyed.pixel(0, 0), Some([0, 0, 0, 0]));
    let spill = keyed.pixel(1, 0).expect("spill pixel");
    assert!(spill[3] > 0);
    assert!(
        spill[0] > 0 && spill[2] > 0,
        "spill should desaturate green"
    );
    assert!(
        spill[1] < 128,
        "spill reduction should lower green dominance"
    );
    let distant = keyed.pixel(2, 0).expect("distant chroma pixel");
    assert_eq!(distant[3], 255);
    assert!(distant[0] > 200 && distant[1] < 20 && distant[2] < 20);
    assert!(ChromaKey::new(0, 255, 0, 0, 80, 100).is_none());
    assert!(ChromaKey::new(0, 255, 0, 400, 80, 1_001).is_none());
}

#[test]
fn sharpen_uses_a_bounded_three_by_three_kernel() {
    let format =
        VideoFormat::new(3, 3, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let mut pixels: Vec<u8> = (0..9).flat_map(|_| [100, 100, 100, 255]).collect();
    pixels[4 * 4..4 * 4 + 4].copy_from_slice(&[120, 120, 120, 255]);
    let frame = VideoFrame::new(format, Timestamp::ZERO, pixels).expect("frame");
    let sharpened = frame.filtered(FrameFilter::Sharpen { milli: 500 });

    assert_eq!(sharpened.pixel(1, 1), Some([200, 200, 200, 255]));
    assert_eq!(sharpened.pixel(0, 0), Some([100, 100, 100, 255]));
    assert_eq!(
        VideoFrame::solid(format, Timestamp::ZERO, [64, 128, 192, 255])
            .filtered(FrameFilter::Sharpen { milli: 1_000 }),
        VideoFrame::solid(format, Timestamp::ZERO, [64, 128, 192, 255])
    );
}

#[test]
fn luma_key_keeps_the_interval_and_smooths_both_edges() {
    let format =
        VideoFormat::new(4, 1, FrameRate::new(30, 1).expect("valid rate")).expect("format");
    let frame = VideoFrame::new(
        format,
        Timestamp::ZERO,
        vec![
            0, 0, 0, 255, // below the lower threshold
            64, 64, 64, 255, // inside the interval
            217, 217, 217, 255, // inside the upper transition
            255, 255, 255, 255, // above the upper threshold
        ],
    )
    .expect("frame");
    let key = LumaKey::new(900, 100, 100, 100).expect("luma key");
    let keyed = frame.filtered(FrameFilter::LumaKey(key));

    assert_eq!(keyed.pixel(0, 0), Some([0, 0, 0, 0]));
    assert_eq!(keyed.pixel(1, 0), Some([64, 64, 64, 255]));
    let transition_alpha = keyed.pixel(2, 0).expect("transition pixel")[3];
    assert!(transition_alpha > 0 && transition_alpha < 255);
    assert_eq!(keyed.pixel(3, 0), Some([0, 0, 0, 0]));
    assert!(LumaKey::new(1_001, 0, 0, 0).is_none());
    assert!(LumaKey::new(0, 0, -1, 0).is_none());
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
