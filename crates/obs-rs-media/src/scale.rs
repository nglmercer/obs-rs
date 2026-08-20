//! Resampling a rendered canvas down (or up) to the encoded output size.
//!
//! OBS-RS renders one canvas and can encode at a different resolution, so the
//! resampler is a media operation rather than a renderer detail: the same
//! filter has to produce the same pixels whichever backend drew the canvas.
//!
//! All three filters are separable — the horizontal pass runs once per source
//! row into a scratch plane, the vertical pass then combines those rows — so
//! the cost is proportional to `(kernel width) * (pixels)` rather than to its
//! square. [`FrameScaler`] owns the scratch plane and the precomputed tap
//! weights, which is what keeps a 60 fps output path free of per-frame kernel
//! arithmetic and of a multi-megabyte allocation per frame.

use rayon::prelude::*;

use super::{error::MediaError, format::VideoFormat, frame::VideoFrame};

/// How a frame is resampled when the output resolution differs from the canvas.
///
/// The variants are ordered by cost: `Bilinear` is the cheapest and softest,
/// `Lanczos` the sharpest and most expensive. They are persisted by
/// [`ScaleFilter::id`], never by discriminant, so reordering them cannot
/// rewrite a stored document.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ScaleFilter {
    /// Two-tap linear interpolation.
    Bilinear,
    /// Catmull-Rom cubic interpolation over four taps per axis.
    #[default]
    Bicubic,
    /// Windowed sinc over six taps per axis.
    Lanczos,
}

impl ScaleFilter {
    /// Every filter, in the order the settings window offers them.
    pub const ALL: [Self; 3] = [Self::Bilinear, Self::Bicubic, Self::Lanczos];

    /// Returns the stable identifier used in persisted documents.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Bilinear => "bilinear",
            Self::Bicubic => "bicubic",
            Self::Lanczos => "lanczos",
        }
    }

    /// Parses a stable identifier, returning `None` for an unknown filter.
    #[must_use]
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "bilinear" => Some(Self::Bilinear),
            "bicubic" => Some(Self::Bicubic),
            "lanczos" => Some(Self::Lanczos),
            _ => None,
        }
    }

    /// Returns how many source samples per axis the filter reads.
    #[must_use]
    pub const fn taps(self) -> u32 {
        match self {
            Self::Bilinear => 2,
            Self::Bicubic => 4,
            Self::Lanczos => 6,
        }
    }

    /// Evaluates the filter kernel at `distance` from the sample centre.
    fn weight(self, distance: f32) -> f32 {
        let distance = distance.abs();
        match self {
            Self::Bilinear => (1.0 - distance).max(0.0),
            Self::Bicubic => catmull_rom(distance),
            Self::Lanczos => lanczos(distance),
        }
    }
}

/// Catmull-Rom, the cubic OBS describes as "sharpened scaling".
fn catmull_rom(distance: f32) -> f32 {
    // a = -0.5 selects the interpolating Catmull-Rom member of the cubic family.
    const A: f32 = -0.5;
    if distance < 1.0 {
        (A + 2.0) * distance * distance * distance - (A + 3.0) * distance * distance + 1.0
    } else if distance < 2.0 {
        A * (distance * distance * distance - 5.0 * distance * distance + 8.0 * distance - 4.0)
    } else {
        0.0
    }
}

/// Sinc windowed by a wider sinc: the classic three-lobe Lanczos kernel.
fn lanczos(distance: f32) -> f32 {
    const LOBES: f32 = 3.0;
    if distance < f32::EPSILON {
        return 1.0;
    }
    if distance >= LOBES {
        return 0.0;
    }
    let scaled = std::f32::consts::PI * distance;
    LOBES * scaled.sin() * (scaled / LOBES).sin() / (scaled * scaled)
}

/// One output sample: where it starts in the source axis and its tap weights.
#[derive(Clone, Debug)]
struct Taps {
    start: i32,
    weights: Vec<f32>,
}

/// Precomputes the tap positions and weights for one axis.
///
/// Downscaling widens the kernel by the scale ratio, which is what stops a
/// large reduction from aliasing: every source pixel still contributes.
fn axis_taps(source: u32, target: u32, filter: ScaleFilter) -> Vec<Taps> {
    let ratio = f64::from(source) / f64::from(target);
    // Upscaling keeps the kernel at its natural width; only downscaling
    // stretches it, so the support is never narrower than one source pixel.
    let support_scale = if ratio > 1.0 { ratio } else { 1.0 };
    let half_width = f64::from(filter.taps()) / 2.0 * support_scale;
    (0..target)
        .map(|index| {
            let centre = (f64::from(index) + 0.5) * ratio - 0.5;
            let first = (centre - half_width + 0.5).floor();
            let last = (centre + half_width + 0.5).floor();
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the support width is bounded by the source dimension"
            )]
            let count = (last - first).max(1.0) as usize;
            let mut weights = Vec::with_capacity(count);
            let mut total = 0.0f32;
            for offset in 0..count {
                #[allow(clippy::cast_precision_loss, reason = "tap offsets are small integers")]
                let position = first + offset as f64;
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "kernel distances are bounded by the support width"
                )]
                let distance = ((position - centre) / support_scale) as f32;
                let weight = filter.weight(distance);
                weights.push(weight);
                total += weight;
            }
            normalise(&mut weights, total);
            #[allow(
                clippy::cast_possible_truncation,
                reason = "source dimensions are bounded by VideoFormat::MAX_PIXELS"
            )]
            Taps {
                start: first as i32,
                weights,
            }
        })
        .collect()
}

/// Scales `weights` so they sum to one.
///
/// A kernel whose weights cancel out would paint black pixels, so that case
/// degenerates to nearest-neighbour rather than to a hole in the image.
fn normalise(weights: &mut [f32], total: f32) {
    if total.abs() < f32::EPSILON {
        let nearest = weights.len() / 2;
        for weight in weights.iter_mut() {
            *weight = 0.0;
        }
        if let Some(weight) = weights.get_mut(nearest) {
            *weight = 1.0;
        }
        return;
    }
    for weight in weights.iter_mut() {
        *weight /= total;
    }
}

/// Clamps a tap onto the source, so edge pixels extend rather than wrap.
fn clamp_index(index: i32, limit: u32) -> usize {
    usize::try_from(index.clamp(0, i32::try_from(limit).unwrap_or(i32::MAX) - 1)).unwrap_or(0)
}

/// A reusable resampler between one pair of resolutions.
///
/// # Real-time behaviour
///
/// [`FrameScaler::scale`] allocates the output buffer plus one small float
/// accumulator per output row; the tap tables and the intermediate plane are
/// rebuilt only when the geometry or the filter changes. It takes no locks and
/// never blocks. Rows are resampled in parallel through rayon's global pool, so it
/// must not be called from an audio callback.
#[derive(Debug)]
pub struct FrameScaler {
    source: VideoFormat,
    target: VideoFormat,
    filter: ScaleFilter,
    horizontal: Vec<Taps>,
    vertical: Vec<Taps>,
    /// Horizontally resampled source rows, in `target.width() * 4` channels.
    plane: Vec<f32>,
}

impl FrameScaler {
    /// Creates a resampler from `source` geometry to `target` geometry.
    #[must_use]
    pub fn new(source: VideoFormat, target: VideoFormat, filter: ScaleFilter) -> Self {
        let horizontal = axis_taps(source.width(), target.width(), filter);
        let vertical = axis_taps(source.height(), target.height(), filter);
        let plane = vec![0.0; row_channels(target) * source.height() as usize];
        Self {
            source,
            target,
            filter,
            horizontal,
            vertical,
            plane,
        }
    }

    /// Rebuilds the tap tables when the geometry or the filter has changed.
    ///
    /// Callers that hold one scaler across a settings change use this rather
    /// than dropping and rebuilding, so an unchanged setting costs nothing.
    pub fn reconfigure(&mut self, source: VideoFormat, target: VideoFormat, filter: ScaleFilter) {
        if self.filter == filter
            && self.source.width() == source.width()
            && self.source.height() == source.height()
            && self.target.width() == target.width()
            && self.target.height() == target.height()
        {
            self.source = source;
            self.target = target;
            return;
        }
        *self = Self::new(source, target, filter);
    }

    /// Returns the geometry this scaler reads.
    #[must_use]
    pub const fn source(&self) -> VideoFormat {
        self.source
    }

    /// Returns the geometry this scaler writes.
    #[must_use]
    pub const fn target(&self) -> VideoFormat {
        self.target
    }

    /// Returns the filter this scaler applies.
    #[must_use]
    pub const fn filter(&self) -> ScaleFilter {
        self.filter
    }

    /// Returns whether the two geometries are identical, so scaling is a copy.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        self.source.width() == self.target.width() && self.source.height() == self.target.height()
    }

    /// Resamples `frame` into this scaler's target format.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] when `frame` is not at the
    /// source geometry this scaler was built for, and [`MediaError::BufferSize`]
    /// when the resampled buffer does not match the target format.
    pub fn scale(&mut self, frame: &VideoFrame) -> Result<VideoFrame, MediaError> {
        if frame.format().width() != self.source.width()
            || frame.format().height() != self.source.height()
        {
            return Err(MediaError::FormatMismatch {
                expected: self.source,
                actual: frame.format(),
            });
        }
        if self.is_identity() {
            return VideoFrame::new(self.target, frame.timestamp(), frame.pixels().to_vec());
        }

        let stride = row_channels(self.target);
        let source_width = self.source.width();
        let source_stride = self.source.width() as usize * 4;
        let horizontal = &self.horizontal;
        let source_pixels = frame.pixels();
        self.plane
            .par_chunks_mut(stride)
            .enumerate()
            .for_each(|(row, target)| {
                let base = row * source_stride;
                for (column, taps) in horizontal.iter().enumerate() {
                    let mut channels = [0.0f32; 4];
                    for (offset, weight) in taps.weights.iter().enumerate() {
                        let sample = clamp_index(taps.start + offset_index(offset), source_width);
                        let pixel = base + sample * 4;
                        for (channel, value) in channels.iter_mut().enumerate() {
                            *value += f32::from(source_pixels[pixel + channel]) * weight;
                        }
                    }
                    target[column * 4..column * 4 + 4].copy_from_slice(&channels);
                }
            });

        let plane = &self.plane;
        let vertical = &self.vertical;
        let source_height = self.source.height();
        let mut pixels = vec![0u8; self.target.rgba_bytes()];
        pixels
            .par_chunks_mut(stride)
            .zip(vertical.par_iter())
            .for_each(|(target, taps)| {
                // Cubic and Lanczos kernels have negative lobes, so the taps
                // are accumulated in float and clamped once: clamping each tap
                // in place would clip the ringing that sharpens the result.
                let mut accumulator = vec![0.0f32; stride];
                for (offset, weight) in taps.weights.iter().enumerate() {
                    let row = clamp_index(taps.start + offset_index(offset), source_height);
                    let source_row = &plane[row * stride..row * stride + stride];
                    for (value, sample) in accumulator.iter_mut().zip(source_row) {
                        *value += sample * weight;
                    }
                }
                for (byte, value) in target.iter_mut().zip(&accumulator) {
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "the value is clamped into the u8 range first"
                    )]
                    {
                        *byte = value.clamp(0.0, 255.0) as u8;
                    }
                }
            });

        VideoFrame::new(self.target, frame.timestamp(), pixels)
    }
}

/// Returns the channel count of one output row.
const fn row_channels(format: VideoFormat) -> usize {
    format.width() as usize * 4
}

/// Widens a tap offset for the signed arithmetic the tap start uses.
fn offset_index(offset: usize) -> i32 {
    i32::try_from(offset).unwrap_or(i32::MAX)
}

impl VideoFrame {
    /// Resamples this frame to `format` using `filter`.
    ///
    /// This builds a single-use [`FrameScaler`], which is the right shape for
    /// a one-off resample. A repeated output path should hold a scaler instead
    /// so the tap tables and the scratch plane are built once.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::BufferSize`] when the resampled buffer does not
    /// match `format`.
    pub fn scaled(&self, format: VideoFormat, filter: ScaleFilter) -> Result<Self, MediaError> {
        FrameScaler::new(self.format(), format, filter).scale(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameRate, Timestamp};

    fn format(width: u32, height: u32) -> VideoFormat {
        VideoFormat::new(width, height, FrameRate::new(60, 1).expect("frame rate"))
            .expect("video format")
    }

    fn solid(width: u32, height: u32, colour: [u8; 4]) -> VideoFrame {
        VideoFrame::solid(format(width, height), Timestamp::ZERO, colour)
    }

    #[test]
    fn filters_round_trip_through_their_identifiers() {
        for filter in ScaleFilter::ALL {
            assert_eq!(ScaleFilter::from_id(filter.id()), Some(filter));
        }
        assert_eq!(ScaleFilter::from_id("nearest"), None);
        assert_eq!(ScaleFilter::default(), ScaleFilter::Bicubic);
    }

    #[test]
    fn an_unchanged_resolution_copies_the_source_pixels() {
        let frame = solid(64, 32, [10, 20, 30, 255]);

        let scaled = frame
            .scaled(format(64, 32), ScaleFilter::Lanczos)
            .expect("identity resample");

        assert_eq!(scaled.pixels(), frame.pixels());
    }

    #[test]
    fn every_filter_preserves_a_flat_colour_when_downscaling() {
        let frame = solid(1920, 1080, [64, 128, 192, 255]);

        for filter in ScaleFilter::ALL {
            let scaled = frame
                .scaled(format(1280, 720), filter)
                .expect("downscale should succeed");

            assert_eq!(scaled.format().width(), 1_280);
            assert_eq!(scaled.format().height(), 720);
            // A constant field must survive any normalised kernel; a drift of
            // more than one level would mean the weights do not sum to one.
            for pixel in scaled.pixels().chunks_exact(4) {
                assert!(
                    pixel
                        .iter()
                        .zip([64u8, 128, 192, 255])
                        .all(|(actual, expected)| actual.abs_diff(expected) <= 1),
                    "{filter:?} shifted a flat colour: {pixel:?}"
                );
            }
        }
    }

    #[test]
    fn upscaling_keeps_the_edges_of_a_two_pixel_gradient() {
        let mut pixels = Vec::new();
        for _ in 0..2 {
            pixels.extend_from_slice(&[0, 0, 0, 255]);
            pixels.extend_from_slice(&[255, 255, 255, 255]);
        }
        let frame = VideoFrame::new(format(2, 2), Timestamp::ZERO, pixels).expect("gradient frame");

        let scaled = frame
            .scaled(format(16, 16), ScaleFilter::Bilinear)
            .expect("upscale should succeed");

        let left = scaled.pixel(0, 0).expect("left pixel");
        let right = scaled.pixel(15, 0).expect("right pixel");
        assert!(left[0] < 32, "left edge should stay dark: {left:?}");
        assert!(right[0] > 223, "right edge should stay light: {right:?}");
    }

    #[test]
    fn a_downscale_reduces_the_buffer_to_the_target_size() {
        let frame = solid(1920, 1080, [1, 2, 3, 255]);

        let scaled = frame
            .scaled(format(640, 360), ScaleFilter::Bicubic)
            .expect("downscale should succeed");

        assert_eq!(scaled.pixels().len(), format(640, 360).rgba_bytes());
        assert_eq!(scaled.timestamp(), frame.timestamp());
    }

    #[test]
    fn a_frame_at_the_wrong_geometry_is_rejected_rather_than_resampled() {
        let mut scaler =
            FrameScaler::new(format(1920, 1080), format(1280, 720), ScaleFilter::Bicubic);

        let error = scaler
            .scale(&solid(1280, 720, [0, 0, 0, 255]))
            .expect_err("a mismatched frame should be rejected");

        assert!(matches!(error, MediaError::FormatMismatch { .. }));
    }

    #[test]
    fn reconfiguring_to_the_same_geometry_keeps_the_tap_tables() {
        let mut scaler =
            FrameScaler::new(format(1920, 1080), format(1280, 720), ScaleFilter::Bicubic);
        let taps = scaler.horizontal.len();

        scaler.reconfigure(format(1920, 1080), format(1280, 720), ScaleFilter::Bicubic);
        assert_eq!(scaler.horizontal.len(), taps);

        scaler.reconfigure(format(1920, 1080), format(854, 480), ScaleFilter::Lanczos);
        assert_eq!(scaler.horizontal.len(), 854);
        assert_eq!(scaler.filter(), ScaleFilter::Lanczos);
        assert!(!scaler.is_identity());
    }
}
