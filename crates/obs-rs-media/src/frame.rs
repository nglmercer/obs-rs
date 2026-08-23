use super::{
    error::MediaError, format::VideoFormat, time::Timestamp, transform::FrameTransform,
    transition::FrameTransition,
};
use crate::metrics::{record_copy_on_write, record_owned_buffer, record_shared_clone};
use rayon::prelude::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

mod filters;

/// Bytes of pixel data one parallel task processes at a time.
///
/// Splitting a frame into per-pixel rayon jobs costs far more in scheduling
/// than the few arithmetic operations each pixel needs — measured at roughly
/// 1.4 ms for a 640x360 transform that a plain copy does in 25 µs. Work is
/// therefore handed out in blocks large enough to amortize the scheduling and
/// to let the inner loops vectorize, and small enough to keep every core busy.
const PARALLEL_BLOCK_BYTES: usize = 4 * 1_024;

/// Frames below this size are processed on the calling thread.
///
/// Waking worker threads for a handful of rows costs more than the work.
const PARALLEL_MINIMUM_BYTES: usize = 2 * PARALLEL_BLOCK_BYTES;

/// Most transform plans retained, regardless of how small each one is.
const TRANSFORM_PLAN_CACHE_SIZE: usize = 64;

/// Total bytes of column indices the plan cache may retain.
///
/// The entry count alone is not a memory bound: one plan holds an index per
/// output column, so an entry costs 16 bytes at 1x1 and 60 KB at 4K. Sixty-four
/// 4K plans is a few megabytes of cache that a session may never reuse, so the
/// cache is bounded by bytes as well as by count and evicts oldest-first until
/// it fits.
const MAX_TRANSFORM_PLAN_BYTES: usize = 4 * 1_024 * 1_024;

type TransformColumns = Arc<Vec<Option<usize>>>;

/// Cached column plans, oldest first, with a running byte total.
#[derive(Default)]
struct TransformPlanCache {
    entries: VecDeque<(VideoFormat, FrameTransform, TransformColumns)>,
    bytes: usize,
}

impl TransformPlanCache {
    /// Returns the bytes of index storage one plan occupies.
    fn plan_bytes(columns: &TransformColumns) -> usize {
        columns.len() * std::mem::size_of::<Option<usize>>()
    }

    fn get(&self, format: VideoFormat, transform: FrameTransform) -> Option<TransformColumns> {
        self.entries
            .iter()
            .find(|(cached_format, cached_transform, _)| {
                *cached_format == format && *cached_transform == transform
            })
            .map(|(_, _, columns)| Arc::clone(columns))
    }

    fn insert(
        &mut self,
        format: VideoFormat,
        transform: FrameTransform,
        columns: &TransformColumns,
    ) {
        let bytes = Self::plan_bytes(columns);
        // A plan larger than the whole budget is still returned to the caller;
        // it is simply not worth retaining, so it is never inserted.
        if bytes > MAX_TRANSFORM_PLAN_BYTES {
            return;
        }

        while self.entries.len() >= TRANSFORM_PLAN_CACHE_SIZE
            || self.bytes + bytes > MAX_TRANSFORM_PLAN_BYTES
        {
            match self.entries.pop_front() {
                Some((_, _, evicted)) => {
                    self.bytes = self.bytes.saturating_sub(Self::plan_bytes(&evicted));
                }
                None => break,
            }
        }

        self.bytes += bytes;
        self.entries
            .push_back((format, transform, Arc::clone(columns)));
    }
}

static TRANSFORM_PLANS: OnceLock<Mutex<TransformPlanCache>> = OnceLock::new();

/// Runs `apply` over the frame's pixels, in parallel blocks when it pays off.
fn for_each_block(pixels: &mut [u8], apply: impl Fn(&mut [u8]) + Send + Sync) {
    if pixels.len() < PARALLEL_MINIMUM_BYTES {
        apply(pixels);
        return;
    }
    pixels.par_chunks_mut(PARALLEL_BLOCK_BYTES).for_each(apply);
}

/// Runs `apply` over paired blocks of a frame and a same-length source.
fn for_each_block_pair(
    pixels: &mut [u8],
    source: &[u8],
    apply: impl Fn(&mut [u8], &[u8]) + Send + Sync,
) {
    if pixels.len() < PARALLEL_MINIMUM_BYTES {
        apply(pixels, source);
        return;
    }
    pixels
        .par_chunks_mut(PARALLEL_BLOCK_BYTES)
        .zip(source.par_chunks(PARALLEL_BLOCK_BYTES))
        .for_each(|(block, source)| apply(block, source));
}

/// An owned, tightly packed RGBA8 video frame.
#[derive(Debug, Eq, PartialEq)]
pub struct VideoFrame {
    format: VideoFormat,
    timestamp: Timestamp,
    pixels: Arc<Vec<u8>>,
}

impl Clone for VideoFrame {
    fn clone(&self) -> Self {
        record_shared_clone();
        Self {
            format: self.format,
            timestamp: self.timestamp,
            pixels: Arc::clone(&self.pixels),
        }
    }
}

impl VideoFrame {
    /// Creates a frame after checking the buffer length against `format`.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::BufferSize`] when `pixels` is not exactly the number
    /// of RGBA8 bytes required by `format`.
    pub fn new(
        format: VideoFormat,
        timestamp: Timestamp,
        pixels: Vec<u8>,
    ) -> Result<Self, MediaError> {
        let expected = format.rgba_bytes();
        if pixels.len() != expected {
            return Err(MediaError::BufferSize {
                expected,
                actual: pixels.len(),
            });
        }

        record_owned_buffer(pixels.len());
        Ok(Self {
            format,
            timestamp,
            pixels: Arc::new(pixels),
        })
    }

    /// Creates a frame from validated shared RGBA storage.
    ///
    /// Capture adapters use this path to publish their newest immutable buffer
    /// without copying every pixel. Mutating operations retain value semantics
    /// through copy-on-write storage.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::BufferSize`] when `pixels` has the wrong length.
    pub fn from_shared(
        format: VideoFormat,
        timestamp: Timestamp,
        pixels: Arc<Vec<u8>>,
    ) -> Result<Self, MediaError> {
        let expected = format.rgba_bytes();
        if pixels.len() != expected {
            return Err(MediaError::BufferSize {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            format,
            timestamp,
            pixels,
        })
    }

    /// Creates a frame from a buffer whose length is already known to match.
    ///
    /// Internal fast path for call sites that derive `pixels` from
    /// [`VideoFormat::rgba_bytes`] and therefore cannot violate the length
    /// invariant. Callers outside this crate must use [`VideoFrame::new`].
    pub(crate) fn new_unchecked(
        format: VideoFormat,
        timestamp: Timestamp,
        pixels: Vec<u8>,
    ) -> Self {
        debug_assert_eq!(pixels.len(), format.rgba_bytes());
        record_owned_buffer(pixels.len());
        Self {
            format,
            timestamp,
            pixels: Arc::new(pixels),
        }
    }

    fn pixels_mut(&mut self) -> &mut [u8] {
        if Arc::strong_count(&self.pixels) > 1 {
            record_copy_on_write(self.pixels.len());
        }
        Arc::make_mut(&mut self.pixels).as_mut_slice()
    }

    /// Creates a solid-color frame.
    #[must_use]
    pub fn solid(format: VideoFormat, timestamp: Timestamp, color: [u8; 4]) -> Self {
        let len = format.rgba_bytes();
        // A transparent or black frame is the common case and is exactly what
        // the allocator already hands back zeroed.
        if color == [0, 0, 0, 0] {
            return Self::new_unchecked(format, timestamp, vec![0_u8; len]);
        }
        let mut pixels = vec![0_u8; len];
        // One pixel is written, then the filled prefix is doubled into the rest.
        // Each `copy_within` is a memmove, so the whole buffer is filled in
        // log2(len) bulk copies rather than one write per pixel.
        if let Some(first) = pixels.get_mut(..4) {
            first.copy_from_slice(&color);
        }
        let mut filled = 4;
        while filled < len {
            let take = filled.min(len - filled);
            pixels.copy_within(..take, filled);
            filled += take;
        }

        Self::new_unchecked(format, timestamp, pixels)
    }

    /// Returns the frame format.
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Returns the frame timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Returns a cheap frame view carrying a different presentation timestamp.
    ///
    /// Immutable pixel storage is shared. This is useful for static sources
    /// such as color mattes that would otherwise allocate identical pixels on
    /// every render.
    #[must_use]
    pub fn at_timestamp(&self, timestamp: Timestamp) -> Self {
        let mut frame = self.clone();
        frame.timestamp = timestamp;
        frame
    }

    /// Returns the immutable RGBA8 bytes.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Consumes the frame and returns its owned RGBA8 buffer.
    ///
    /// Lets an encoder at the end of the pipeline take ownership of the pixels
    /// instead of copying them out of a borrowed frame.
    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        if Arc::strong_count(&self.pixels) > 1 {
            record_copy_on_write(self.pixels.len());
        }
        Arc::try_unwrap(self.pixels).unwrap_or_else(|pixels| pixels.as_ref().clone())
    }

    /// Returns one pixel if `(x, y)` is inside the frame.
    ///
    /// The bounds check above makes the index conversions exact, so this reads
    /// the four bytes directly rather than running fallible conversions.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.format.width || y >= self.format.height {
            return None;
        }

        let offset = (y as usize * self.format.width_index() + x as usize) * 4;
        let pixel: &[u8; 4] = self.pixels.get(offset..offset + 4)?.try_into().ok()?;
        Some(*pixel)
    }

    /// Overlays `foreground` using straight-alpha RGBA blending.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] when the two frames do not share the
    /// same video format.
    pub fn blend_over(&mut self, foreground: &Self) -> Result<(), MediaError> {
        if self.format != foreground.format {
            return Err(MediaError::FormatMismatch {
                expected: self.format,
                actual: foreground.format,
            });
        }

        for_each_block_pair(
            self.pixels_mut(),
            foreground.pixels(),
            |background, source| {
                // A fully opaque run is the overwhelmingly common case — an ordinary
                // camera, screen, or colour layer has no transparency — and it
                // reduces to a copy, so it is detected per block before any
                // per-pixel arithmetic runs. This is not a redundant extra pass:
                // when the block is opaque the scan replaces the per-pixel loop
                // with a memcpy, and when it is not, `all` short-circuits at the
                // first translucent pixel. The only case that pays for the scan
                // without benefiting is a block whose sole translucent pixel is
                // at its very end.
                if source.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX) {
                    background.copy_from_slice(source);
                    return;
                }
                for (background, source) in
                    background.chunks_exact_mut(4).zip(source.chunks_exact(4))
                {
                    let source_alpha = u32::from(source[3]);
                    if source_alpha == 0 {
                        continue;
                    }
                    if source_alpha == u32::from(u8::MAX) {
                        background.copy_from_slice(source);
                        continue;
                    }

                    let inverse_alpha = 255 - source_alpha;
                    let background_alpha = u32::from(background[3]);

                    // Compositing onto an opaque background — a canvas, or any
                    // layer already covering the frame — keeps the result
                    // opaque, so the divisor is the constant 255*255 and both
                    // divisions become the exact shift form below.
                    if background_alpha == u32::from(u8::MAX) {
                        for channel in 0..3 {
                            let numerator = u32::from(source[channel]) * source_alpha
                                + u32::from(background[channel]) * inverse_alpha;
                            background[channel] = to_byte(divide_by_255(numerator));
                        }
                        background[3] = u8::MAX;
                        continue;
                    }

                    let output_alpha =
                        source_alpha + divide_by_255(background_alpha * inverse_alpha);
                    if output_alpha == 0 {
                        background.fill(0);
                        continue;
                    }

                    // The denominator and both alpha weights are identical for all
                    // three colour channels, so they are computed once per pixel.
                    let denominator = output_alpha * 255;
                    let source_weight = source_alpha * 255;
                    let background_weight = background_alpha * inverse_alpha;
                    for channel in 0..3 {
                        let numerator = u32::from(source[channel]) * source_weight
                            + u32::from(background[channel]) * background_weight;
                        background[channel] = to_byte(numerator / denominator);
                    }
                    background[3] = to_byte(output_alpha);
                }
            },
        );

        Ok(())
    }

    /// Composites this foreground over `background` while retaining this
    /// frame's storage for the result.
    ///
    /// This is algebraically identical to `background.blend_over(self)`, but it
    /// lets a compositor reuse the newest layer buffer rather than copying a
    /// cached/static first layer before every frame.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] when formats differ.
    pub fn blend_under(&mut self, background: &Self) -> Result<(), MediaError> {
        if self.format != background.format {
            return Err(MediaError::FormatMismatch {
                expected: self.format,
                actual: background.format,
            });
        }

        for_each_block_pair(
            self.pixels_mut(),
            background.pixels(),
            |source, background| {
                for (source, background) in
                    source.chunks_exact_mut(4).zip(background.chunks_exact(4))
                {
                    let source_alpha = u32::from(source[3]);
                    if source_alpha == u32::from(u8::MAX) {
                        continue;
                    }
                    if source_alpha == 0 {
                        source.copy_from_slice(background);
                        continue;
                    }
                    let inverse_alpha = 255 - source_alpha;
                    let background_alpha = u32::from(background[3]);
                    if background_alpha == u32::from(u8::MAX) {
                        for channel in 0..3 {
                            let numerator = u32::from(source[channel]) * source_alpha
                                + u32::from(background[channel]) * inverse_alpha;
                            source[channel] = to_byte(divide_by_255(numerator));
                        }
                        source[3] = u8::MAX;
                        continue;
                    }
                    let output_alpha =
                        source_alpha + divide_by_255(background_alpha * inverse_alpha);
                    let denominator = output_alpha * 255;
                    let source_weight = source_alpha * 255;
                    let background_weight = background_alpha * inverse_alpha;
                    for channel in 0..3 {
                        let numerator = u32::from(source[channel]) * source_weight
                            + u32::from(background[channel]) * background_weight;
                        source[channel] = to_byte(numerator / denominator);
                    }
                    source[3] = to_byte(output_alpha);
                }
            },
        );
        Ok(())
    }

    /// Produces a deterministic transition between two same-format frames.
    ///
    /// `destination` is taken by value and becomes the result buffer: a cut
    /// returns it untouched and a cross-fade blends `source` into it in place,
    /// so neither path copies a frame. The destination timestamp is used for the
    /// result. Cross-fades and fade-to-color transitions interpolate every RGBA
    /// byte with integer arithmetic, which makes offline previews and live
    /// output use the same correctness oracle.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] for different formats or
    /// [`MediaError::InvalidTransition`] for an invalid transition progress value.
    pub fn transitioned(
        source: &Self,
        mut destination: Self,
        transition: FrameTransition,
    ) -> Result<Self, MediaError> {
        if source.format != destination.format {
            return Err(MediaError::FormatMismatch {
                expected: source.format,
                actual: destination.format,
            });
        }

        match transition {
            FrameTransition::Cut => Ok(destination),
            FrameTransition::CrossFade { progress_milli } => {
                if progress_milli > 1_000 {
                    return Err(MediaError::InvalidTransition { progress_milli });
                }
                let destination_weight = u32::from(progress_milli);
                let source_weight = 1_000 - destination_weight;
                // Both buffers have the same format and therefore the same
                // length, so this is a straight paired walk with no branching
                // and no intermediate allocation.
                for (target, source_byte) in
                    destination.pixels_mut().iter_mut().zip(source.pixels())
                {
                    let value = u32::from(*source_byte) * source_weight
                        + u32::from(*target) * destination_weight;
                    *target = to_byte((value + 500) / 1_000);
                }
                Ok(destination)
            }
            FrameTransition::FadeToColor {
                progress_milli,
                color,
            } => {
                if progress_milli > 1_000 {
                    return Err(MediaError::InvalidTransition { progress_milli });
                }
                if progress_milli <= 500 {
                    let color_weight = u32::from(progress_milli).saturating_mul(2);
                    let source_weight = 1_000 - color_weight;
                    for (index, (target, source_byte)) in destination
                        .pixels_mut()
                        .iter_mut()
                        .zip(source.pixels())
                        .enumerate()
                    {
                        let value = u32::from(*source_byte) * source_weight
                            + u32::from(color[index % 4]) * color_weight;
                        *target = to_byte((value + 500) / 1_000);
                    }
                } else {
                    let destination_weight =
                        u32::from(progress_milli.saturating_sub(500)).saturating_mul(2);
                    let color_weight = 1_000 - destination_weight;
                    for (index, target) in destination.pixels_mut().iter_mut().enumerate() {
                        let value = u32::from(color[index % 4]) * color_weight
                            + u32::from(*target) * destination_weight;
                        *target = to_byte((value + 500) / 1_000);
                    }
                }
                Ok(destination)
            }
        }
    }

    /// Applies a nearest-neighbor transform into a new transparent frame.
    ///
    /// Pixels outside the transformed source remain transparent. Alpha is
    /// multiplied by the transform opacity; RGB values are otherwise preserved.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] if the transform was not constructed
    /// through a valid constructor.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "source_x/source_y are bounds-checked against the validated frame \
                  dimensions above, so both are non-negative and below u32::MAX"
    )]
    #[allow(
        clippy::too_many_lines,
        reason = "the resampler keeps its column mapping, contiguous-row copy, \
                  and per-pixel fallback in one place so they cannot drift"
    )]
    pub fn transformed(&self, transform: FrameTransform) -> Result<Self, MediaError> {
        if transform.scale_x_milli == 0
            || transform.scale_y_milli == 0
            || transform.scale_x_milli > FrameTransform::MAX_SCALE_MILLI
            || transform.scale_y_milli > FrameTransform::MAX_SCALE_MILLI
        {
            return Err(MediaError::InvalidTransform);
        }

        // The identity transform is what every untransformed scene item carries,
        // so it must not cost a full nearest-neighbour resample.
        if transform == FrameTransform::IDENTITY {
            return Ok(self.clone());
        }
        // Keep the established integer resampler byte-identical for the
        // overwhelmingly common unrotated path. Rotation has a different
        // inverse-mapping geometry and is isolated so it cannot perturb the
        // existing crop/scale/flip fast paths.
        if transform.rotation_milli_degrees() != 0 {
            return self.transformed_rotated(transform);
        }

        let mut output = Self::solid(self.format, self.timestamp, [0, 0, 0, 0]);
        let width = i64::from(self.format.width);
        let height = i64::from(self.format.height);
        let crop_left = i64::from(transform.crop_left);
        let crop_top = i64::from(transform.crop_top);
        let crop_right = i64::from(transform.crop_right);
        let crop_bottom = i64::from(transform.crop_bottom);
        let visible_right = width - crop_right;
        let visible_bottom = height - crop_bottom;
        if crop_left >= visible_right || crop_top >= visible_bottom {
            return Err(MediaError::InvalidTransform);
        }
        let scale_x = i64::from(transform.scale_x_milli);
        let scale_y = i64::from(transform.scale_y_milli);
        let output_width = self.format.width_index();
        let translate_x = i64::from(transform.translate_x);
        let translate_y = i64::from(transform.translate_y);
        let opacity = u32::from(transform.opacity);
        let opaque = transform.opacity == u8::MAX;

        // The source column for an output column does not depend on the row, so
        // the whole mapping is built once instead of dividing per pixel. Each
        // entry is the source byte offset within its row, or `None` outside it.
        let columns = transform_columns(
            self.format,
            transform,
            crop_left,
            visible_right,
            translate_x,
            scale_x,
        );
        // Consecutive source columns copy as one memmove per row, which is the
        // case for every unscaled, unflipped layer. The visible columns must
        // also form a single run, so the mapping is checked in one pass:
        // covered columns step by exactly one pixel and never resume after a gap.
        let contiguous = !transform.flip_x && {
            let mut previous: Option<usize> = None;
            let mut ended = false;
            columns.iter().all(|column| match column {
                None => {
                    ended = previous.is_some();
                    true
                }
                Some(offset) => {
                    if ended {
                        return false;
                    }
                    let steps = previous.is_none_or(|previous| *offset == previous + 4);
                    previous = Some(*offset);
                    steps
                }
            })
        };

        let output_pixels = output.pixels_mut();
        for y in 0..self.format.height {
            let local_y = i64::from(y) - translate_y;
            if local_y < 0 {
                continue;
            }
            let mut source_y = crop_top + local_y * 1_000 / scale_y;
            if source_y >= visible_bottom {
                continue;
            }
            if transform.flip_y {
                source_y = crop_top + visible_bottom - 1 - source_y;
            }
            // Both row bases are constant across the scanline. `source_y` and
            // `y` are within the validated frame height here, so the index
            // conversions are exact.
            let source_row = source_y as usize * output_width * 4;
            let output_row = y as usize * output_width * 4;

            if contiguous {
                // Copy the visible span of the row in one move, then apply the
                // opacity to the span rather than pixel by pixel.
                let Some(first) = columns.iter().position(Option::is_some) else {
                    continue;
                };
                let last = columns.iter().rposition(Option::is_some).unwrap_or(first);
                let span = last - first + 1;
                let Some(source_offset) = columns[first] else {
                    continue;
                };
                let source_start = source_row + source_offset;
                let output_start = output_row + first * 4;
                output_pixels[output_start..output_start + span * 4]
                    .copy_from_slice(&self.pixels[source_start..source_start + span * 4]);
                if !opaque {
                    for pixel in
                        output_pixels[output_start..output_start + span * 4].chunks_exact_mut(4)
                    {
                        pixel[3] = to_byte(u32::from(pixel[3]) * opacity / u32::from(u8::MAX));
                    }
                }
                continue;
            }

            for (x, column) in columns.iter().enumerate() {
                let Some(source_offset) = column else {
                    continue;
                };
                let source_offset = source_row + source_offset;
                let output_offset = output_row + x * 4;
                let source_pixel = &self.pixels[source_offset..source_offset + 4];
                let target_pixel = &mut output_pixels[output_offset..output_offset + 4];
                target_pixel.copy_from_slice(source_pixel);
                if !opaque {
                    let alpha = u32::from(target_pixel[3]) * opacity / u32::from(u8::MAX);
                    target_pixel[3] = to_byte(alpha);
                }
            }
        }

        Ok(output)
    }

    /// Applies a nearest-neighbor rotation around the visible source centre.
    ///
    /// The integer resampler above intentionally remains the reference for
    /// zero-degree transforms. This path computes one sine/cosine pair per
    /// frame, then maps each output pixel back into the cropped source. It is
    /// deterministic for a given transform and keeps the same transparent
    /// outside-the-layer semantics as the unrotated path.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "validated coordinates are converted after bounds checks; fixed-point media values are intentionally converted to f64 for the rotation matrix"
    )]
    fn transformed_rotated(&self, transform: FrameTransform) -> Result<Self, MediaError> {
        let mut output = Self::solid(self.format, self.timestamp, [0, 0, 0, 0]);
        let width = i64::from(self.format.width);
        let height = i64::from(self.format.height);
        let crop_left = i64::from(transform.crop_left());
        let crop_top = i64::from(transform.crop_top());
        let crop_right = i64::from(transform.crop_right());
        let crop_bottom = i64::from(transform.crop_bottom());
        let visible_right = width - crop_right;
        let visible_bottom = height - crop_bottom;
        if crop_left >= visible_right || crop_top >= visible_bottom {
            return Err(MediaError::InvalidTransform);
        }

        let visible_width = visible_right - crop_left;
        let visible_height = visible_bottom - crop_top;
        let scale_x = f64::from(transform.scale_x_milli());
        let scale_y = f64::from(transform.scale_y_milli());
        let scaled_width = visible_width as f64 * scale_x / 1_000.0;
        let scaled_height = visible_height as f64 * scale_y / 1_000.0;
        let center_x = f64::from(transform.translate_x()) + scaled_width / 2.0;
        let center_y = f64::from(transform.translate_y()) + scaled_height / 2.0;
        let angle =
            f64::from(transform.rotation_milli_degrees()) / 180_000.0 * std::f64::consts::PI;
        let (sin, cos) = angle.sin_cos();
        let output_width = self.format.width_index();
        let opacity = u32::from(transform.opacity());

        let output_pixels = output.pixels_mut();
        for y in 0..self.format.height {
            for x in 0..self.format.width {
                // Pixel centres make quarter-turns preserve a symmetric
                // 2x2 source rather than introducing a half-pixel drift.
                let dx = f64::from(x) + 0.5 - center_x;
                let dy = f64::from(y) + 0.5 - center_y;
                let local_x = cos * dx + sin * dy + scaled_width / 2.0;
                let local_y = -sin * dx + cos * dy + scaled_height / 2.0;
                if local_x < 0.0
                    || local_y < 0.0
                    || local_x >= scaled_width
                    || local_y >= scaled_height
                {
                    continue;
                }

                let mut source_x = crop_left + (local_x * 1_000.0 / scale_x).floor() as i64;
                let mut source_y = crop_top + (local_y * 1_000.0 / scale_y).floor() as i64;
                if transform.flip_x() {
                    source_x = crop_left + visible_width - 1 - (source_x - crop_left);
                }
                if transform.flip_y() {
                    source_y = crop_top + visible_height - 1 - (source_y - crop_top);
                }
                if source_x < crop_left
                    || source_x >= visible_right
                    || source_y < crop_top
                    || source_y >= visible_bottom
                {
                    continue;
                }

                let source_offset = (source_y as usize * output_width + source_x as usize) * 4;
                let output_offset = (y as usize * output_width + x as usize) * 4;
                let source_pixel = &self.pixels[source_offset..source_offset + 4];
                let target_pixel = &mut output_pixels[output_offset..output_offset + 4];
                target_pixel.copy_from_slice(source_pixel);
                if opacity != u32::from(u8::MAX) {
                    target_pixel[3] =
                        to_byte(u32::from(target_pixel[3]) * opacity / u32::from(u8::MAX));
                }
            }
        }
        Ok(output)
    }

    /// Applies a transform while reusing uniquely owned storage for the common
    /// unscaled full-frame flip/opacity path.
    ///
    /// Other transforms retain [`Self::transformed`] as their pixel-identical
    /// correctness implementation.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::InvalidTransform`] for an invalid transform.
    pub fn into_transformed(mut self, transform: FrameTransform) -> Result<Self, MediaError> {
        if transform == FrameTransform::IDENTITY {
            return Ok(self);
        }
        let full_frame = transform.scale_x_milli == 1_000
            && transform.scale_y_milli == 1_000
            && transform.translate_x == 0
            && transform.translate_y == 0
            && transform.rotation_milli_degrees == 0
            && !transform.is_cropped();
        if !full_frame {
            return self.transformed(transform);
        }

        let width = self.format.width_index();
        let width_bytes = width * 4;
        let height = self.format.height_index();
        let pixels = self.pixels_mut();
        if transform.flip_x {
            pixels.par_chunks_exact_mut(width_bytes).for_each(|row| {
                for x in 0..width / 2 {
                    let opposite = width - 1 - x;
                    for channel in 0..4 {
                        row.swap(x * 4 + channel, opposite * 4 + channel);
                    }
                }
            });
        }
        if transform.flip_y {
            for y in 0..height / 2 {
                let opposite = height - 1 - y;
                for byte in 0..width_bytes {
                    pixels.swap(y * width_bytes + byte, opposite * width_bytes + byte);
                }
            }
        }
        if transform.opacity != u8::MAX {
            let opacity = u32::from(transform.opacity);
            pixels.par_chunks_exact_mut(4).for_each(|pixel| {
                pixel[3] = to_byte(u32::from(pixel[3]) * opacity / u32::from(u8::MAX));
            });
        }
        Ok(self)
    }

    /// Clears RGB values on fully transparent pixels for canonical composition.
    pub fn clear_transparent_rgb(&mut self) {
        if !self.pixels.par_chunks_exact(4).any(|pixel| pixel[3] == 0) {
            return;
        }
        self.pixels_mut().par_chunks_exact_mut(4).for_each(|pixel| {
            let mask = 0_u8.wrapping_sub(u8::from(pixel[3] != 0));
            pixel[0] &= mask;
            pixel[1] &= mask;
            pixel[2] &= mask;
        });
    }

    /// Calculates a stable FNV-1a checksum of the frame bytes.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for chunk in self.pixels.chunks_exact(8) {
            hash = fnv_step(hash, chunk[0]);
            hash = fnv_step(hash, chunk[1]);
            hash = fnv_step(hash, chunk[2]);
            hash = fnv_step(hash, chunk[3]);
            hash = fnv_step(hash, chunk[4]);
            hash = fnv_step(hash, chunk[5]);
            hash = fnv_step(hash, chunk[6]);
            hash = fnv_step(hash, chunk[7]);
        }
        for byte in self.pixels.chunks_exact(8).remainder() {
            hash = fnv_step(hash, *byte);
        }
        hash
    }
}

fn transform_columns(
    format: VideoFormat,
    transform: FrameTransform,
    crop_left: i64,
    visible_right: i64,
    translate_x: i64,
    scale_x: i64,
) -> TransformColumns {
    let cache = TRANSFORM_PLANS.get_or_init(|| Mutex::new(TransformPlanCache::default()));
    let mut plans = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(columns) = plans.get(format, transform) {
        return columns;
    }
    let columns = Arc::new(
        (0..format.width)
            .map(|x| {
                let local_x = i64::from(x) - translate_x;
                if local_x < 0 {
                    return None;
                }
                let mut source_x = crop_left + local_x * 1_000 / scale_x;
                if source_x >= visible_right {
                    return None;
                }
                if transform.flip_x {
                    source_x = crop_left + visible_right - 1 - source_x;
                }
                usize::try_from(source_x).ok().map(|source_x| source_x * 4)
            })
            .collect(),
    );
    plans.insert(format, transform, &columns);
    columns
}

#[inline]
fn fnv_step(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "min constrains the value to 0..=255, so the cast is exact"
)]
fn to_byte(value: u32) -> u8 {
    value.min(u32::from(u8::MAX)) as u8
}

/// Divides by 255 exactly, without a division instruction.
///
/// Blending divides by 255 several times per pixel, and integer division is an
/// order of magnitude slower than a multiply and a shift. This form is exact
/// for every value below 2^24 — far above the `255 * 255 * 2` a blend can
/// produce — and the tests pin it across the whole range a composite uses.
pub(crate) const fn divide_by_255(value: u32) -> u32 {
    ((value as u64 * 0x8080_8081) >> 39) as u32
}

#[cfg(test)]
mod transform_plan_cache_tests {
    use super::*;

    fn columns(len: usize) -> TransformColumns {
        Arc::new(vec![Some(0_usize); len])
    }

    fn format(width: u32) -> VideoFormat {
        VideoFormat::new(width, 2, crate::FrameRate::new(30, 1).expect("valid rate"))
            .expect("valid format")
    }

    #[test]
    fn the_cache_stays_within_its_byte_budget() {
        let mut cache = TransformPlanCache::default();
        // Each plan is a quarter of the budget, so a fifth insert must evict.
        let per_plan = MAX_TRANSFORM_PLAN_BYTES / 4 / std::mem::size_of::<Option<usize>>();

        for index in 0..8_u32 {
            cache.insert(
                format(index + 1),
                FrameTransform::IDENTITY,
                &columns(per_plan),
            );
            assert!(
                cache.bytes <= MAX_TRANSFORM_PLAN_BYTES,
                "budget exceeded after {index} inserts: {} bytes",
                cache.bytes
            );
        }

        assert_eq!(cache.entries.len(), 4, "older plans must be evicted");
    }

    #[test]
    fn the_cache_stays_within_its_entry_count() {
        let mut cache = TransformPlanCache::default();

        for index in 0..u32::try_from(TRANSFORM_PLAN_CACHE_SIZE + 10).expect("small count") {
            cache.insert(format(index + 1), FrameTransform::IDENTITY, &columns(1));
        }

        assert_eq!(cache.entries.len(), TRANSFORM_PLAN_CACHE_SIZE);
    }

    #[test]
    fn a_plan_larger_than_the_budget_is_never_retained() {
        let mut cache = TransformPlanCache::default();
        let oversized = MAX_TRANSFORM_PLAN_BYTES / std::mem::size_of::<Option<usize>>() + 1;

        cache.insert(format(1), FrameTransform::IDENTITY, &columns(oversized));

        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn a_cached_plan_is_returned_for_a_repeated_lookup() {
        let mut cache = TransformPlanCache::default();
        let plan = columns(4);
        cache.insert(format(4), FrameTransform::IDENTITY, &plan);

        let found = cache
            .get(format(4), FrameTransform::IDENTITY)
            .expect("cached");

        assert!(Arc::ptr_eq(&found, &plan));
        assert!(cache.get(format(8), FrameTransform::IDENTITY).is_none());
    }
}
