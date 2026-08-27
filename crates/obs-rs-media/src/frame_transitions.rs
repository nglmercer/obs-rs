//! Frame-to-frame transition rendering kept separate from frame transforms.

use super::{
    error::MediaError,
    frame::{to_byte, VideoFrame},
    transition::{FrameTransition, LumaWipePattern, SlideDirection},
};

impl VideoFrame {
    /// Produces a deterministic transition between two same-format frames.
    ///
    /// `destination` is taken by value and becomes the result buffer: a cut
    /// returns it untouched and a cross-fade blends `source` into it in place,
    /// so neither path copies a frame. The destination timestamp is used for
    /// the result. Cross-fades, fade-to-color, and slide transitions use
    /// integer arithmetic, which makes offline previews and live output use
    /// the same correctness oracle.
    ///
    /// # Errors
    ///
    /// Returns [`MediaError::FormatMismatch`] for different formats or
    /// [`MediaError::InvalidTransition`] for an invalid transition progress value.
    pub fn transitioned(
        source: &Self,
        destination: Self,
        transition: FrameTransition,
    ) -> Result<Self, MediaError> {
        if source.format() != destination.format() {
            return Err(MediaError::FormatMismatch {
                expected: source.format(),
                actual: destination.format(),
            });
        }

        match transition {
            FrameTransition::Cut => Ok(destination),
            FrameTransition::CrossFade { progress_milli } => {
                apply_cross_fade(source, destination, progress_milli)
            }
            FrameTransition::FadeToColor {
                progress_milli,
                color,
            } => apply_fade_to_color(source, destination, progress_milli, color),
            FrameTransition::Slide {
                progress_milli,
                direction,
            } => apply_slide_transition(source, destination, progress_milli, direction),
            FrameTransition::Swipe {
                progress_milli,
                direction,
                swipe_in,
            } => apply_swipe_transition(source, destination, progress_milli, direction, swipe_in),
            FrameTransition::LumaWipe {
                progress_milli,
                pattern,
                invert,
                softness_milli,
            } => apply_luma_wipe_transition(
                source,
                destination,
                progress_milli,
                pattern,
                invert,
                softness_milli,
            ),
        }
    }
}

fn validate_transition_progress(progress_milli: u16) -> Result<(), MediaError> {
    if progress_milli > 1_000 {
        return Err(MediaError::InvalidTransition { progress_milli });
    }
    Ok(())
}

fn apply_cross_fade(
    source: &VideoFrame,
    mut destination: VideoFrame,
    progress_milli: u16,
) -> Result<VideoFrame, MediaError> {
    validate_transition_progress(progress_milli)?;
    let destination_weight = u32::from(progress_milli);
    let source_weight = 1_000 - destination_weight;
    // Both buffers have the same format and therefore the same length, so this
    // is a straight paired walk with no branching and no intermediate
    // allocation.
    for (target, source_byte) in destination.pixels_mut().iter_mut().zip(source.pixels()) {
        let value =
            u32::from(*source_byte) * source_weight + u32::from(*target) * destination_weight;
        *target = to_byte((value + 500) / 1_000);
    }
    Ok(destination)
}

fn apply_fade_to_color(
    source: &VideoFrame,
    mut destination: VideoFrame,
    progress_milli: u16,
    color: [u8; 4],
) -> Result<VideoFrame, MediaError> {
    validate_transition_progress(progress_milli)?;
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
        let destination_weight = u32::from(progress_milli.saturating_sub(500)).saturating_mul(2);
        let color_weight = 1_000 - destination_weight;
        for (index, target) in destination.pixels_mut().iter_mut().enumerate() {
            let value = u32::from(color[index % 4]) * color_weight
                + u32::from(*target) * destination_weight;
            *target = to_byte((value + 500) / 1_000);
        }
    }
    Ok(destination)
}

fn apply_slide_transition(
    source: &VideoFrame,
    mut destination: VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
) -> Result<VideoFrame, MediaError> {
    validate_transition_progress(progress_milli)?;
    apply_slide(source, &mut destination, progress_milli, direction);
    Ok(destination)
}

fn apply_swipe_transition(
    source: &VideoFrame,
    mut destination: VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
    swipe_in: bool,
) -> Result<VideoFrame, MediaError> {
    validate_transition_progress(progress_milli)?;
    apply_swipe(
        source,
        &mut destination,
        progress_milli,
        direction,
        swipe_in,
    );
    Ok(destination)
}

fn apply_luma_wipe_transition(
    source: &VideoFrame,
    mut destination: VideoFrame,
    progress_milli: u16,
    pattern: LumaWipePattern,
    invert: bool,
    softness_milli: u16,
) -> Result<VideoFrame, MediaError> {
    validate_transition_progress(progress_milli)?;
    if softness_milli > 1_000 {
        return Err(MediaError::InvalidLumaWipeSoftness { softness_milli });
    }
    apply_luma_wipe(
        source,
        &mut destination,
        progress_milli,
        pattern,
        invert,
        softness_milli,
    );
    Ok(destination)
}

/// Applies the portable linear subset of OBS's luminance-mask transition.
/// The destination remains the output buffer, so the operation does not need
/// a second full-frame allocation.
fn apply_luma_wipe(
    source: &VideoFrame,
    destination: &mut VideoFrame,
    progress_milli: u16,
    pattern: LumaWipePattern,
    invert: bool,
    softness_milli: u16,
) {
    let width = destination.format().width_index();
    let height = destination.format().height_index();
    let time_milli = u32::from(progress_milli)
        .saturating_mul(1_000_u32.saturating_add(u32::from(softness_milli)))
        / 1_000;
    let source_pixels = source.pixels();
    let target_pixels = destination.pixels_mut();

    for y in 0..height {
        for x in 0..width {
            let mut luma = luma_value_milli(pattern, width, height, x, y);
            if invert {
                luma = 1_000_u32.saturating_sub(luma);
            }
            let (source_weight, destination_weight) =
                luma_weights(luma, time_milli, softness_milli, progress_milli);
            let target_start = pixel_start(width, x, y);
            if destination_weight == 0 {
                let source_pixel: [u8; 4] = source_pixels[target_start..target_start + 4]
                    .try_into()
                    .expect("RGBA pixel has four bytes");
                target_pixels[target_start..target_start + 4].copy_from_slice(&source_pixel);
            } else if source_weight != 0 {
                let source_start = target_start;
                for channel in 0..4 {
                    let value = u32::from(source_pixels[source_start + channel])
                        .saturating_mul(source_weight)
                        .saturating_add(
                            u32::from(target_pixels[target_start + channel])
                                .saturating_mul(destination_weight),
                        );
                    target_pixels[target_start + channel] = to_byte((value + 500) / 1_000);
                }
            }
        }
    }
}

fn luma_value_milli(
    pattern: LumaWipePattern,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> u32 {
    let (coordinate, extent) = match pattern {
        LumaWipePattern::LinearHorizontal => (x, width),
        LumaWipePattern::LinearVertical => (y, height),
    };
    let denominator = extent.saturating_sub(1);
    if denominator == 0 {
        return 0;
    }
    u32::try_from(coordinate.saturating_mul(1_000) / denominator).unwrap_or(1_000)
}

fn luma_weights(
    luma_milli: u32,
    time_milli: u32,
    softness_milli: u16,
    progress_milli: u16,
) -> (u32, u32) {
    if softness_milli == 0 {
        return if luma_milli <= u32::from(progress_milli) {
            (0, 1_000)
        } else {
            (1_000, 0)
        };
    }
    let softness = u32::from(softness_milli);
    let lower = time_milli.saturating_sub(softness);
    if luma_milli <= lower {
        return (0, 1_000);
    }
    if luma_milli >= time_milli {
        return (1_000, 0);
    }
    let destination_weight = time_milli.saturating_sub(luma_milli).saturating_mul(1_000) / softness;
    let destination_weight = destination_weight.min(1_000);
    (1_000 - destination_weight, destination_weight)
}

/// Returns the number of pixels in the axis a transition moves along.
fn transition_axis_length(frame: &VideoFrame, direction: SlideDirection) -> usize {
    match direction {
        SlideDirection::Left | SlideDirection::Right => frame.format().width_index(),
        SlideDirection::Up | SlideDirection::Down => frame.format().height_index(),
    }
}

/// Converts bounded progress into a pixel offset on the selected axis.
fn transition_progress_pixels(
    frame: &VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
) -> usize {
    let axis_length = transition_axis_length(frame, direction);
    let axis_length_u32 = match direction {
        SlideDirection::Left | SlideDirection::Right => frame.format().width(),
        SlideDirection::Up | SlideDirection::Down => frame.format().height(),
    };
    u64::from(progress_milli)
        .saturating_mul(u64::from(axis_length_u32))
        .checked_div(1_000)
        .and_then(|pixels| usize::try_from(pixels).ok())
        .unwrap_or(axis_length)
        .min(axis_length)
}

/// Converts a pixel coordinate into the start of its RGBA sample.
fn pixel_start(width: usize, x: usize, y: usize) -> usize {
    (y * width + x) * 4
}

/// Applies the bounded swipe transition in place without allocating a second
/// frame. Unlike slide, the destination stays fixed and only fills the area
/// uncovered by the source.
fn apply_swipe(
    source: &VideoFrame,
    destination: &mut VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
    swipe_in: bool,
) {
    if swipe_in {
        apply_swipe_in(source, destination, progress_milli, direction);
        return;
    }
    let width = destination.format().width_index();
    let height = destination.format().height_index();
    let progress_pixels = transition_progress_pixels(destination, progress_milli, direction);
    let axis_length = transition_axis_length(destination, direction);
    let source_pixels = source.pixels();
    let target_pixels = destination.pixels_mut();

    for y in 0..height {
        for x in 0..width {
            let (source_x, source_y, source_visible) = match direction {
                SlideDirection::Left => {
                    let boundary = axis_length.saturating_sub(progress_pixels);
                    if x < boundary {
                        (x + progress_pixels, y, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Right => {
                    if x >= progress_pixels {
                        (x - progress_pixels, y, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Up => {
                    let boundary = axis_length.saturating_sub(progress_pixels);
                    if y < boundary {
                        (x, y + progress_pixels, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Down => {
                    if y >= progress_pixels {
                        (x, y - progress_pixels, true)
                    } else {
                        (x, y, false)
                    }
                }
            };
            if !source_visible {
                continue;
            }
            let target_start = pixel_start(width, x, y);
            let source_start = pixel_start(width, source_x, source_y);
            let source_pixel: [u8; 4] = source_pixels[source_start..source_start + 4]
                .try_into()
                .expect("RGBA pixel has four bytes");
            target_pixels[target_start..target_start + 4].copy_from_slice(&source_pixel);
        }
    }
}

/// Applies the incoming Swipe variant in place. The source remains visible
/// outside the moving destination layer, so the destination buffer is read in
/// movement-safe order before each target pixel is overwritten.
fn apply_swipe_in(
    source: &VideoFrame,
    destination: &mut VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
) {
    let width = destination.format().width_index();
    let height = destination.format().height_index();
    let remaining_pixels = transition_axis_length(destination, direction).saturating_sub(
        transition_progress_pixels(destination, progress_milli, direction),
    );
    let source_pixels = source.pixels();
    let target_pixels = destination.pixels_mut();

    for row in 0..height {
        let y = if matches!(direction, SlideDirection::Up) {
            height - 1 - row
        } else {
            row
        };
        for column in 0..width {
            let x = if matches!(direction, SlideDirection::Left) {
                width - 1 - column
            } else {
                column
            };
            let (sample_x, sample_y, destination_visible) = match direction {
                SlideDirection::Left => {
                    if x >= remaining_pixels {
                        (x - remaining_pixels, y, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Right => {
                    if x + remaining_pixels < width {
                        (x + remaining_pixels, y, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Up => {
                    if y >= remaining_pixels {
                        (x, y - remaining_pixels, true)
                    } else {
                        (x, y, false)
                    }
                }
                SlideDirection::Down => {
                    if y + remaining_pixels < height {
                        (x, y + remaining_pixels, true)
                    } else {
                        (x, y, false)
                    }
                }
            };
            let target_start = pixel_start(width, x, y);
            let pixel: [u8; 4] = if destination_visible {
                let sample_start = pixel_start(width, sample_x, sample_y);
                target_pixels[sample_start..sample_start + 4]
                    .try_into()
                    .expect("RGBA pixel has four bytes")
            } else {
                let source_start = pixel_start(width, x, y);
                source_pixels[source_start..source_start + 4]
                    .try_into()
                    .expect("RGBA pixel has four bytes")
            };
            target_pixels[target_start..target_start + 4].copy_from_slice(&pixel);
        }
    }
}

/// Applies the bounded slide transition in place without allocating a second
/// frame. Traversal follows the movement direction so an original destination
/// pixel is read before the output can overwrite it.
fn apply_slide(
    source: &VideoFrame,
    destination: &mut VideoFrame,
    progress_milli: u16,
    direction: SlideDirection,
) {
    let width = destination.format().width_index();
    let height = destination.format().height_index();
    let progress_pixels = transition_progress_pixels(destination, progress_milli, direction);
    let axis_length = transition_axis_length(destination, direction);
    let source_pixels = source.pixels();
    let target_pixels = destination.pixels_mut();

    let row_order = 0..height;
    for row in row_order {
        let y = if matches!(direction, SlideDirection::Up) {
            height - 1 - row
        } else {
            row
        };
        let column_order = 0..width;
        for column in column_order {
            let x = if matches!(direction, SlideDirection::Left) {
                width - 1 - column
            } else {
                column
            };
            let (sample_x, sample_y, source_visible) = match direction {
                SlideDirection::Left => {
                    let boundary = axis_length.saturating_sub(progress_pixels);
                    if x < boundary {
                        (x + progress_pixels, y, true)
                    } else {
                        (x - boundary, y, false)
                    }
                }
                SlideDirection::Right => {
                    if x >= progress_pixels {
                        (x - progress_pixels, y, true)
                    } else {
                        (x + axis_length.saturating_sub(progress_pixels), y, false)
                    }
                }
                SlideDirection::Up => {
                    let boundary = axis_length.saturating_sub(progress_pixels);
                    if y < boundary {
                        (x, y + progress_pixels, true)
                    } else {
                        (x, y - boundary, false)
                    }
                }
                SlideDirection::Down => {
                    if y >= progress_pixels {
                        (x, y - progress_pixels, true)
                    } else {
                        (x, y + axis_length.saturating_sub(progress_pixels), false)
                    }
                }
            };
            let target_start = pixel_start(width, x, y);
            let sample_start = pixel_start(width, sample_x, sample_y);
            let sample_pixel: [u8; 4] = if source_visible {
                source_pixels[sample_start..sample_start + 4]
                    .try_into()
                    .expect("RGBA pixel has four bytes")
            } else {
                // Traversal leaves the original destination sample untouched
                // until it is read: lower coordinates for left/up and higher
                // coordinates for right/down.
                target_pixels[sample_start..sample_start + 4]
                    .try_into()
                    .expect("RGBA pixel has four bytes")
            };
            target_pixels[target_start..target_start + 4].copy_from_slice(&sample_pixel);
        }
    }
}
