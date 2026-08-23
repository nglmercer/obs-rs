//! CPU filter application methods and pixel kernels for `VideoFrame`.

use std::sync::Arc;

use super::{divide_by_255, for_each_block, to_byte, VideoFrame};
use crate::{
    filters::{ChromaKey, ColorCorrection, ColorKey, ColorMultiplyAdd, FrameFilter, LumaKey},
    time::Timestamp,
};
use rayon::prelude::*;

impl VideoFrame {
    /// Applies one CPU filter and returns a new owned frame.
    ///
    /// This allocates and copies a full frame buffer (roughly 33 MB at 4K), so
    /// it suits offline and test call sites. On a media callback or any other
    /// per-frame path, use [`VideoFrame::apply_filter`] to filter in place.
    #[must_use]
    pub fn filtered(&self, filter: FrameFilter) -> Self {
        let mut output = self.clone();
        output.apply_filter(filter);
        output
    }

    /// Applies one CPU filter in place without allocating another frame.
    pub fn apply_filter(&mut self, filter: FrameFilter) {
        self.apply_filters(std::slice::from_ref(&filter));
    }

    /// Applies an ordered filter chain in one pass over the frame.
    ///
    /// Fusing the chain avoids reading and writing the complete pixel buffer for
    /// every individual filter while preserving the caller's filter order.
    pub fn apply_filters(&mut self, filters: &[FrameFilter]) {
        if filters.is_empty() {
            return;
        }
        // Crop/Pad is coordinate-dependent, and color correction/key filters
        // have derived parameters that should be built once per frame rather
        // than once per pixel. Apply them as their own bounded passes while
        // retaining the ordered semantics of filters around them.
        if filters.iter().any(|filter| {
            matches!(
                filter,
                FrameFilter::CropPad { .. }
                    | FrameFilter::ColorCorrection(_)
                    | FrameFilter::ColorMultiplyAdd(_)
                    | FrameFilter::LumaKey(_)
                    | FrameFilter::ColorKey(_)
                    | FrameFilter::ChromaKey(_)
                    | FrameFilter::Sharpen { .. }
                    | FrameFilter::Scroll { .. }
                    | FrameFilter::RenderDelay(_)
            )
        }) {
            for filter in filters {
                match *filter {
                    FrameFilter::CropPad {
                        left,
                        top,
                        right,
                        bottom,
                    } => self.apply_crop_pad(left, top, right, bottom),
                    FrameFilter::Sharpen { milli } => self.apply_sharpen(milli),
                    FrameFilter::Scroll {
                        speed_x,
                        speed_y,
                        looped,
                    } => self.apply_scroll(speed_x, speed_y, looped),
                    FrameFilter::RenderDelay(_) => {
                        unreachable!("source-level render delay is resolved by the runtime")
                    }
                    _ => self.apply_single_pixel_filter(*filter),
                }
            }
            return;
        }
        self.apply_pixel_filters(filters);
    }

    fn apply_pixel_filters(&mut self, filters: &[FrameFilter]) {
        // A single filter is the common case, and matching on it once per block
        // rather than once per pixel is what lets the inner loop stay a tight
        // arithmetic pass the compiler can vectorize.
        if let [filter] = filters {
            self.apply_single_pixel_filter(*filter);
            return;
        }
        for_each_block(self.pixels_mut(), |block| {
            for pixel in block.chunks_exact_mut(4) {
                for filter in filters {
                    match *filter {
                        FrameFilter::Grayscale => apply_grayscale(pixel),
                        FrameFilter::Brightness { milli } => {
                            apply_brightness(pixel, i32::from(milli) + 1_000);
                        }
                        FrameFilter::Opacity(opacity) => {
                            apply_opacity(pixel, u32::from(opacity));
                        }
                        FrameFilter::CropPad { .. } => unreachable!(
                            "coordinate-dependent crop filters are handled before pixel filters"
                        ),
                        FrameFilter::ColorCorrection(_) => unreachable!(
                            "derived color correction filters are handled before pixel filters"
                        ),
                        FrameFilter::ColorMultiplyAdd(_) => unreachable!(
                            "color multiply/add filters are handled with their channel values"
                        ),
                        FrameFilter::LumaKey(_) => unreachable!(
                            "derived luma key filters are handled before pixel filters"
                        ),
                        FrameFilter::ColorKey(_) => unreachable!(
                            "derived color key filters are handled before pixel filters"
                        ),
                        FrameFilter::ChromaKey(_) => unreachable!(
                            "derived chroma key filters are handled before pixel filters"
                        ),
                        FrameFilter::Sharpen { .. } => unreachable!(
                            "neighbour-based sharpen filters are handled before pixel filters"
                        ),
                        FrameFilter::Scroll { .. } => unreachable!(
                            "coordinate-dependent scroll filters are handled before pixel filters"
                        ),
                        FrameFilter::RenderDelay(_) => {
                            unreachable!("source-level render delay is resolved by the runtime")
                        }
                    }
                }
            }
        });
    }

    fn apply_single_pixel_filter(&mut self, filter: FrameFilter) {
        if let FrameFilter::ColorCorrection(correction) = filter {
            let parameters = ColorCorrectionParameters::new(correction);
            let gamma_lut = parameters.gamma_lut();
            for_each_block(self.pixels_mut(), move |block| {
                for pixel in block.chunks_exact_mut(4) {
                    apply_color_correction(pixel, parameters, &gamma_lut);
                }
            });
            return;
        }
        if let FrameFilter::ColorMultiplyAdd(color_wash) = filter {
            for_each_block(self.pixels_mut(), move |block| {
                for pixel in block.chunks_exact_mut(4) {
                    apply_color_multiply_add(pixel, color_wash);
                }
            });
            return;
        }
        if let FrameFilter::ColorKey(color_key) = filter {
            let parameters = ColorKeyParameters::new(color_key);
            for_each_block(self.pixels_mut(), move |block| {
                for pixel in block.chunks_exact_mut(4) {
                    apply_color_key(pixel, parameters);
                }
            });
            return;
        }
        if let FrameFilter::LumaKey(luma_key) = filter {
            let parameters = LumaKeyParameters::new(luma_key);
            for_each_block(self.pixels_mut(), move |block| {
                for pixel in block.chunks_exact_mut(4) {
                    apply_luma_key(pixel, parameters);
                }
            });
            return;
        }
        if let FrameFilter::ChromaKey(chroma_key) = filter {
            let parameters = ChromaKeyParameters::new(chroma_key);
            for_each_block(self.pixels_mut(), move |block| {
                for pixel in block.chunks_exact_mut(4) {
                    apply_chroma_key(pixel, parameters);
                }
            });
            return;
        }
        for_each_block(self.pixels_mut(), move |block| match filter {
            FrameFilter::Grayscale => {
                for pixel in block.chunks_exact_mut(4) {
                    apply_grayscale(pixel);
                }
            }
            FrameFilter::Brightness { milli } => {
                let multiplier = i32::from(milli) + 1_000;
                for pixel in block.chunks_exact_mut(4) {
                    apply_brightness(pixel, multiplier);
                }
            }
            FrameFilter::Opacity(opacity) => {
                for pixel in block.chunks_exact_mut(4) {
                    apply_opacity(pixel, u32::from(opacity));
                }
            }
            FrameFilter::CropPad { .. } => {
                unreachable!("coordinate-dependent crop filters are handled before pixel filters")
            }
            FrameFilter::ColorCorrection(_) => {
                unreachable!("color correction filters are handled with their gamma lookup table")
            }
            FrameFilter::ColorMultiplyAdd(_) => {
                unreachable!("color multiply/add filters are handled with their channel values")
            }
            FrameFilter::LumaKey(_) => {
                unreachable!("luma key filters are handled with derived parameters")
            }
            FrameFilter::ColorKey(_) => {
                unreachable!("color key filters are handled with derived parameters")
            }
            FrameFilter::ChromaKey(_) => {
                unreachable!("chroma key filters are handled with derived parameters")
            }
            FrameFilter::Scroll { .. } => {
                unreachable!("scroll filters are handled with the source snapshot")
            }
            FrameFilter::Sharpen { .. } => {
                unreachable!("sharpen filters are handled with the source snapshot")
            }
            FrameFilter::RenderDelay(_) => {
                unreachable!("source-level render delay is resolved by the runtime")
            }
        });
    }

    fn apply_crop_pad(&mut self, left: u32, top: u32, right: u32, bottom: u32) {
        let width = self.format.width_index();
        let height = self.format.height_index();
        let left = usize::try_from(left).unwrap_or(usize::MAX).min(width);
        let top = usize::try_from(top).unwrap_or(usize::MAX).min(height);
        let right = usize::try_from(right).unwrap_or(usize::MAX).min(width);
        let bottom = usize::try_from(bottom).unwrap_or(usize::MAX).min(height);
        let row_bytes = width * 4;
        self.pixels_mut()
            .par_chunks_exact_mut(row_bytes)
            .enumerate()
            .for_each(|(y, row)| {
                let outside_vertical = y < top || y >= height.saturating_sub(bottom);
                for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
                    if outside_vertical || x < left || x >= width.saturating_sub(right) {
                        pixel.copy_from_slice(&[0, 0, 0, 0]);
                    }
                }
            });
    }

    /// Applies a timestamp-driven source scroll using a copy-on-write source
    /// snapshot. Positive speed moves the image left/up, matching OBS's
    /// increasing texture offset; non-looping edges become transparent.
    fn apply_scroll(&mut self, speed_x: i16, speed_y: i16, looped: bool) {
        let width = self.format.width_index();
        let height = self.format.height_index();
        let offset_x = scroll_offset_pixels(self.timestamp, speed_x);
        let offset_y = scroll_offset_pixels(self.timestamp, speed_y);
        if offset_x == 0 && offset_y == 0 {
            return;
        }
        let source = Arc::clone(&self.pixels);
        let pixels = self.pixels_mut();
        pixels
            .par_chunks_exact_mut(width * 4)
            .enumerate()
            .for_each(|(y, row)| {
                for x in 0..width {
                    let source_x = i64::try_from(x).unwrap_or(i64::MAX) + offset_x;
                    let source_y = i64::try_from(y).unwrap_or(i64::MAX) + offset_y;
                    let (source_x, source_y) = if looped {
                        (
                            source_x.rem_euclid(i64::try_from(width).unwrap_or(i64::MAX)),
                            source_y.rem_euclid(i64::try_from(height).unwrap_or(i64::MAX)),
                        )
                    } else if source_x < 0
                        || source_y < 0
                        || source_x >= i64::try_from(width).unwrap_or(i64::MAX)
                        || source_y >= i64::try_from(height).unwrap_or(i64::MAX)
                    {
                        row[x * 4..x * 4 + 4].fill(0);
                        continue;
                    } else {
                        (source_x, source_y)
                    };
                    let pixel = source_pixel(
                        &source,
                        width,
                        height,
                        usize::try_from(source_x).unwrap_or(0),
                        usize::try_from(source_y).unwrap_or(0),
                    );
                    row[x * 4..x * 4 + 4].copy_from_slice(&pixel);
                }
            });
    }

    /// Applies OBS's bounded 3x3 sharpen kernel using a copy-on-write source
    /// snapshot. Clamped neighbours match the effect sampler at frame edges,
    /// and the snapshot keeps one ordered filter pass from reading pixels that
    /// an earlier output pixel has already modified.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the fixed-point strength is bounded to the u16 UI range"
    )]
    fn apply_sharpen(&mut self, milli: u16) {
        if milli == 0 {
            return;
        }
        let source = Arc::clone(&self.pixels);
        let width = self.format.width_index();
        let height = self.format.height_index();
        let strength = f32::from(milli.min(1_000)) / 1_000.0;
        let pixels = self.pixels_mut();
        pixels
            .par_chunks_exact_mut(width * 4)
            .enumerate()
            .for_each(|(y, row)| {
                let source = source.as_slice();
                for x in 0..width {
                    let center = source_pixel(source, width, height, x, y);
                    let left = source_pixel(source, width, height, x.saturating_sub(1), y);
                    let right = source_pixel(source, width, height, x.saturating_add(1), y);
                    let top = source_pixel(source, width, height, x, y.saturating_sub(1));
                    let bottom = source_pixel(source, width, height, x, y.saturating_add(1));
                    let active =
                        (left != center && right != center) || (top != center && bottom != center);
                    let output = &mut row[x * 4..x * 4 + 4];
                    if !active {
                        output.copy_from_slice(&center);
                        continue;
                    }

                    for channel in 0..4 {
                        let mut kernel = 8.0 * f32::from(center[channel]);
                        for neighbour in [
                            left,
                            right,
                            top,
                            bottom,
                            source_pixel(
                                source,
                                width,
                                height,
                                x.saturating_sub(1),
                                y.saturating_sub(1),
                            ),
                            source_pixel(
                                source,
                                width,
                                height,
                                x.saturating_add(1),
                                y.saturating_sub(1),
                            ),
                            source_pixel(
                                source,
                                width,
                                height,
                                x.saturating_sub(1),
                                y.saturating_add(1),
                            ),
                            source_pixel(
                                source,
                                width,
                                height,
                                x.saturating_add(1),
                                y.saturating_add(1),
                            ),
                        ] {
                            kernel -= f32::from(neighbour[channel]);
                        }
                        output[channel] =
                            float_to_byte((f32::from(center[channel]) + kernel * strength) / 255.0);
                    }
                }
            });
    }
}

fn apply_grayscale(pixel: &mut [u8]) {
    let luma =
        (u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29) / 256;
    let luma = to_byte(luma);
    pixel[0] = luma;
    pixel[1] = luma;
    pixel[2] = luma;
}

/// Scales the colour channels by a thousandths multiplier.
fn apply_brightness(pixel: &mut [u8], multiplier: i32) {
    for channel in &mut pixel[..3] {
        let value = i32::from(*channel) * multiplier / 1_000;
        *channel = to_byte(u32::try_from(value.max(0)).unwrap_or(u32::MAX));
    }
}

/// Scales the alpha channel by a 0-255 opacity.
fn apply_opacity(pixel: &mut [u8], opacity: u32) {
    pixel[3] = to_byte(divide_by_255(u32::from(pixel[3]) * opacity));
}

#[derive(Clone, Copy)]
struct ColorCorrectionParameters {
    gamma_exponent: f32,
    contrast: f32,
    brightness: f32,
    saturation: f32,
    hue_matrix: [f32; 9],
    opacity: f32,
}

impl ColorCorrectionParameters {
    /// Precomputes the matrix/scalars that are constant across a frame.
    #[allow(
        clippy::cast_precision_loss,
        reason = "all fixed-point color controls are bounded to small UI ranges"
    )]
    fn new(correction: ColorCorrection) -> Self {
        let gamma = correction.gamma_milli() as f32 / 1_000.0;
        let gamma_exponent = if gamma < 0.0 {
            -gamma + 1.0
        } else {
            1.0 / (gamma + 1.0)
        };
        let contrast_value = correction.contrast_milli() as f32 / 1_000.0;
        let contrast = if contrast_value < 0.0 {
            1.0 / (-contrast_value + 1.0)
        } else {
            contrast_value + 1.0
        };
        let saturation = correction.saturation_milli() as f32 / 1_000.0 + 1.0;
        let half_angle = correction.hue_shift_degrees() as f32 * std::f32::consts::PI / 360.0;
        let quaternion_axis = (1.0 / 3.0_f32.sqrt()) * half_angle.sin();
        let square = quaternion_axis * quaternion_axis;
        let cross = square;
        let wimag = quaternion_axis * half_angle.cos();
        let diagonal = 0.5 - 2.0 * square;
        let a_line = cross + wimag;
        let b_line = cross - wimag;
        Self {
            gamma_exponent,
            contrast,
            brightness: correction.brightness_milli() as f32 / 1_000.0,
            saturation,
            hue_matrix: [
                2.0 * diagonal,
                2.0 * b_line,
                2.0 * a_line,
                2.0 * a_line,
                2.0 * diagonal,
                2.0 * b_line,
                2.0 * b_line,
                2.0 * a_line,
                2.0 * diagonal,
            ],
            opacity: correction.opacity_milli() as f32 / 1_000.0,
        }
    }

    /// Builds a bounded 8-bit lookup table for the per-channel gamma step.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the table index is bounded to the 8-bit channel range"
    )]
    fn gamma_lut(self) -> [f32; 256] {
        let mut table = [0.0; 256];
        for (index, value) in table.iter_mut().enumerate() {
            *value = (index as f32 / 255.0).powf(self.gamma_exponent);
        }
        table
    }
}

/// Applies the six numeric controls of OBS's v2 Color Correction filter.
///
/// The media contract stores straight-alpha RGBA8 frames, so the operation is
/// evaluated on straight RGB and opacity is applied to alpha only. This keeps
/// the no-op correction bit-for-bit stable while matching the visible color
/// operation order: gamma, contrast/brightness, saturation, hue, opacity.
fn apply_color_correction(
    pixel: &mut [u8],
    parameters: ColorCorrectionParameters,
    gamma_lut: &[f32; 256],
) {
    let mut red = gamma_lut[usize::from(pixel[0])];
    let mut green = gamma_lut[usize::from(pixel[1])];
    let mut blue = gamma_lut[usize::from(pixel[2])];

    red = red * parameters.contrast + parameters.brightness;
    green = green * parameters.contrast + parameters.brightness;
    blue = blue * parameters.contrast + parameters.brightness;

    let luma = red * 0.299 + green * 0.587 + blue * 0.114;
    red = luma + parameters.saturation * (red - luma);
    green = luma + parameters.saturation * (green - luma);
    blue = luma + parameters.saturation * (blue - luma);

    let matrix = parameters.hue_matrix;
    let red_hue = matrix[0] * red + matrix[1] * green + matrix[2] * blue;
    let green_hue = matrix[3] * red + matrix[4] * green + matrix[5] * blue;
    let blue_hue = matrix[6] * red + matrix[7] * green + matrix[8] * blue;
    pixel[0] = float_to_byte(red_hue);
    pixel[1] = float_to_byte(green_hue);
    pixel[2] = float_to_byte(blue_hue);
    pixel[3] = float_to_byte(f32::from(pixel[3]) / 255.0 * parameters.opacity);
}

/// Applies OBS's RGB color wash in normalized straight-alpha space.
///
/// The multiply color is normalized by 255 and the add color is normalized by
/// 255 before the result is clamped and rounded back to RGBA8. This is the
/// same operation represented by OBS's color matrix and leaves alpha alone.
fn apply_color_multiply_add(pixel: &mut [u8], color_wash: ColorMultiplyAdd) {
    let multiply = color_wash.multiply();
    let add = color_wash.add();
    for ((channel, multiplier), additive) in pixel[..3].iter_mut().zip(multiply).zip(add) {
        let value = u32::from(*channel) * u32::from(multiplier)
            + u32::from(additive) * u32::from(u8::MAX)
            + 127;
        *channel = to_byte(value / u32::from(u8::MAX));
    }
}

#[derive(Clone, Copy)]
struct ColorKeyParameters {
    key_red: f32,
    key_green: f32,
    key_blue: f32,
    similarity: f32,
    smoothness: f32,
}

impl ColorKeyParameters {
    /// Precomputes normalized key thresholds that are constant across a frame.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the key channels and thresholds are bounded to byte/UI ranges"
    )]
    fn new(color_key: ColorKey) -> Self {
        Self {
            key_red: f32::from(color_key.key_red()) / 255.0,
            key_green: f32::from(color_key.key_green()) / 255.0,
            key_blue: f32::from(color_key.key_blue()) / 255.0,
            similarity: color_key.similarity_milli() as f32 / 1_000.0,
            smoothness: color_key.smoothness_milli() as f32 / 1_000.0,
        }
    }
}

#[derive(Clone, Copy)]
struct LumaKeyParameters {
    max: f32,
    min: f32,
    max_smooth: f32,
    min_smooth: f32,
}

impl LumaKeyParameters {
    /// Precomputes normalized luma thresholds that are constant across a frame.
    #[allow(
        clippy::cast_precision_loss,
        reason = "all fixed-point luma controls are bounded to the UI range"
    )]
    fn new(luma_key: LumaKey) -> Self {
        Self {
            max: luma_key.luma_max_milli() as f32 / 1_000.0,
            min: luma_key.luma_min_milli() as f32 / 1_000.0,
            max_smooth: luma_key.luma_max_smooth_milli() as f32 / 1_000.0,
            min_smooth: luma_key.luma_min_smooth_milli() as f32 / 1_000.0,
        }
    }
}

#[derive(Clone, Copy)]
struct ChromaKeyParameters {
    key_cb: f32,
    key_cr: f32,
    similarity: f32,
    smoothness: f32,
    spill: f32,
}

impl ChromaKeyParameters {
    /// Precomputes the key chroma and normalized thresholds that are constant
    /// across a frame.
    #[allow(
        clippy::cast_precision_loss,
        reason = "key channels and fixed-point thresholds are bounded to UI ranges"
    )]
    fn new(chroma_key: ChromaKey) -> Self {
        let key_red = nonlinear_channel(f32::from(chroma_key.key_red()) / 255.0);
        let key_green = nonlinear_channel(f32::from(chroma_key.key_green()) / 255.0);
        let key_blue = nonlinear_channel(f32::from(chroma_key.key_blue()) / 255.0);
        let key_chroma = chroma_components(key_red, key_green, key_blue);
        Self {
            key_cb: key_chroma.0,
            key_cr: key_chroma.1,
            similarity: chroma_key.similarity_milli() as f32 / 1_000.0,
            smoothness: chroma_key.smoothness_milli() as f32 / 1_000.0,
            spill: chroma_key.spill_milli() as f32 / 1_000.0,
        }
    }
}

/// Applies a bounded RGB-distance key and canonicalizes fully transparent
/// pixels so CPU and GPU compositor paths share the same straight-alpha form.
fn apply_color_key(pixel: &mut [u8], parameters: ColorKeyParameters) {
    let red = f32::from(pixel[0]) / 255.0;
    let green = f32::from(pixel[1]) / 255.0;
    let blue = f32::from(pixel[2]) / 255.0;
    let red_delta = red - parameters.key_red;
    let green_delta = green - parameters.key_green;
    let blue_delta = blue - parameters.key_blue;
    let distance = (red_delta * red_delta + green_delta * green_delta + blue_delta * blue_delta)
        .sqrt()
        / 3.0_f32.sqrt();
    let alpha_factor = if distance <= parameters.similarity {
        0.0
    } else if parameters.smoothness <= 0.0
        || distance >= parameters.similarity + parameters.smoothness
    {
        1.0
    } else {
        (distance - parameters.similarity) / parameters.smoothness
    };
    pixel[3] = float_to_byte(f32::from(pixel[3]) / 255.0 * alpha_factor);
    if pixel[3] == 0 {
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
    }
}

/// Evaluates the rising bounded smoothstep mask used by the Luma Key effect.
fn luma_key_smoothstep(value: f32, edge: f32, width: f32) -> f32 {
    if width <= 0.0 {
        if value >= edge {
            return 1.0;
        }
        return 0.0;
    }
    let position = ((value - edge) / width).clamp(0.0, 1.0);
    position * position * (3.0 - 2.0 * position)
}

/// Applies the bounded luma interval mask while retaining source alpha.
fn apply_luma_key(pixel: &mut [u8], parameters: LumaKeyParameters) {
    let red = f32::from(pixel[0]) / 255.0;
    let green = f32::from(pixel[1]) / 255.0;
    let blue = f32::from(pixel[2]) / 255.0;
    let luma = red * 0.2989 + green * 0.5870 + blue * 0.1140;
    let lower = luma_key_smoothstep(luma, parameters.min, parameters.min_smooth);
    let upper = if parameters.max_smooth <= 0.0 {
        if luma <= parameters.max {
            1.0
        } else {
            0.0
        }
    } else {
        1.0 - luma_key_smoothstep(
            luma,
            parameters.max - parameters.max_smooth,
            parameters.max_smooth,
        )
    };
    pixel[3] = float_to_byte(f32::from(pixel[3]) / 255.0 * lower * upper);
    if pixel[3] == 0 {
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
    }
}

/// Converts one linear-light channel to the nonlinear sRGB value used by the
/// OBS 32.2.2 Chroma Key shader's YCbCr distance calculation.
fn nonlinear_channel(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Computes the fixed Rec. 709-style chroma coordinates used by OBS's
/// production Chroma Key effect.
fn chroma_components(red: f32, green: f32, blue: f32) -> (f32, f32) {
    (
        -0.100_644 * red - 0.338_572 * green + 0.439_216 * blue + 0.501_961,
        0.439_216 * red - 0.398_942 * green - 0.040_274 * blue + 0.501_961,
    )
}

/// Evaluates OBS's bounded `pow(saturate(base / width), 1.5)` mask.
fn chroma_key_mask(base: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return f32::from(base > 0.0);
    }
    (base / width).clamp(0.0, 1.0).powf(1.5)
}

/// Applies the current-pixel Chroma Key core: YCbCr distance, feathered alpha,
/// and spill desaturation. Spatial box filtering, color-space negotiation,
/// and the optional color controls remain separate capabilities.
fn apply_chroma_key(pixel: &mut [u8], parameters: ChromaKeyParameters) {
    let red = f32::from(pixel[0]) / 255.0;
    let green = f32::from(pixel[1]) / 255.0;
    let blue = f32::from(pixel[2]) / 255.0;
    let nonlinear_red = nonlinear_channel(red);
    let nonlinear_green = nonlinear_channel(green);
    let nonlinear_blue = nonlinear_channel(blue);
    let (cb, cr) = chroma_components(nonlinear_red, nonlinear_green, nonlinear_blue);
    let chroma_distance =
        ((cb - parameters.key_cb).powi(2) + (cr - parameters.key_cr).powi(2)).sqrt();
    let base_mask = (chroma_distance - parameters.similarity).max(0.0);
    let full_mask = chroma_key_mask(base_mask, parameters.smoothness);
    let spill_mask = chroma_key_mask(base_mask, parameters.spill);
    let desaturated = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    pixel[0] = float_to_byte(desaturated + (red - desaturated) * spill_mask);
    pixel[1] = float_to_byte(desaturated + (green - desaturated) * spill_mask);
    pixel[2] = float_to_byte(desaturated + (blue - desaturated) * spill_mask);
    pixel[3] = float_to_byte(f32::from(pixel[3]) / 255.0 * full_mask);
    if pixel[3] == 0 {
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
    }
}

/// Reads one clamped RGBA pixel from a validated frame snapshot.
fn source_pixel(source: &[u8], width: usize, height: usize, x: usize, y: usize) -> [u8; 4] {
    let x = x.min(width.saturating_sub(1));
    let y = y.min(height.saturating_sub(1));
    let offset = (y * width + x) * 4;
    [
        source[offset],
        source[offset + 1],
        source[offset + 2],
        source[offset + 3],
    ]
}

/// Converts an OBS-style pixel-per-second scroll speed to a deterministic
/// integer offset at a frame timestamp.
fn scroll_offset_pixels(timestamp: Timestamp, speed: i16) -> i64 {
    let numerator = i128::from(speed) * i128::from(timestamp.as_nanos());
    let pixels = numerator.div_euclid(1_000_000_000);
    i64::try_from(pixels).unwrap_or_else(|_| {
        if pixels.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamping and rounding constrain the value to the byte range"
)]
fn float_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}
