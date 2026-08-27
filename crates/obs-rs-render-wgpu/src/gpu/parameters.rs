//! Fixed-size WGPU filter parameter encoding.

use obs_rs_media::{FrameFilter, FrameTransform, Timestamp, VideoFormat};

#[allow(
    clippy::too_many_lines,
    reason = "the fixed-size shader ABI keeps every supported filter record explicit"
)]
pub(super) fn layer_parameters(
    source_format: VideoFormat,
    target_format: VideoFormat,
    timestamp: Timestamp,
    transform: FrameTransform,
    filters: &[FrameFilter],
) -> Vec<u8> {
    let mut values = Vec::with_capacity(17 + filters.len() * 7);
    values.extend([
        i32::try_from(target_format.width()).unwrap_or(i32::MAX),
        i32::try_from(target_format.height()).unwrap_or(i32::MAX),
        i32::try_from(source_format.width()).unwrap_or(i32::MAX),
        i32::try_from(source_format.height()).unwrap_or(i32::MAX),
        i32::try_from(transform.scale_x_milli()).unwrap_or(i32::MAX),
        i32::try_from(transform.scale_y_milli()).unwrap_or(i32::MAX),
        transform.translate_x(),
        transform.translate_y(),
        i32::from(transform.flip_x()),
        i32::from(transform.flip_y()),
        i32::from(transform.opacity()),
        i32::try_from(transform.crop_left()).unwrap_or(i32::MAX),
        i32::try_from(transform.crop_top()).unwrap_or(i32::MAX),
        i32::try_from(transform.crop_right()).unwrap_or(i32::MAX),
        i32::try_from(transform.crop_bottom()).unwrap_or(i32::MAX),
        transform.rotation_milli_degrees(),
        i32::try_from(filters.len()).unwrap_or(i32::MAX),
    ]);
    for filter in filters {
        match *filter {
            FrameFilter::Grayscale => values.extend([0, 0, 0, 0, 0, 0, 0]),
            FrameFilter::Brightness { milli } => {
                values.extend([1, i32::from(milli), 0, 0, 0, 0, 0]);
            }
            FrameFilter::Opacity(opacity) => {
                values.extend([2, i32::from(opacity), 0, 0, 0, 0, 0]);
            }
            FrameFilter::CropPad {
                left,
                top,
                right,
                bottom,
            } => values.extend([
                3,
                i32::try_from(left).unwrap_or(i32::MAX),
                i32::try_from(top).unwrap_or(i32::MAX),
                i32::try_from(right).unwrap_or(i32::MAX),
                i32::try_from(bottom).unwrap_or(i32::MAX),
                0,
                0,
            ]),
            FrameFilter::ColorCorrection(correction) => values.extend([
                4,
                correction.gamma_milli(),
                correction.contrast_milli(),
                correction.brightness_milli(),
                correction.saturation_milli(),
                correction.hue_shift_degrees(),
                correction.opacity_milli(),
            ]),
            FrameFilter::ColorKey(color_key) => values.extend([
                5,
                i32::from(color_key.key_red()),
                i32::from(color_key.key_green()),
                i32::from(color_key.key_blue()),
                color_key.similarity_milli(),
                color_key.smoothness_milli(),
                0,
            ]),
            FrameFilter::LumaKey(luma_key) => values.extend([
                6,
                luma_key.luma_max_milli(),
                luma_key.luma_min_milli(),
                luma_key.luma_max_smooth_milli(),
                luma_key.luma_min_smooth_milli(),
                0,
                0,
            ]),
            FrameFilter::ChromaKey(chroma_key) => values.extend([
                7,
                i32::from(chroma_key.key_red()),
                i32::from(chroma_key.key_green()),
                i32::from(chroma_key.key_blue()),
                chroma_key.similarity_milli(),
                chroma_key.smoothness_milli(),
                chroma_key.spill_milli(),
            ]),
            FrameFilter::Sharpen { milli } => values.extend([8, i32::from(milli), 0, 0, 0, 0, 0]),
            FrameFilter::ColorMultiplyAdd(color_wash) => {
                let multiply = color_wash.multiply();
                let add = color_wash.add();
                values.extend([
                    9,
                    i32::from(multiply[0]),
                    i32::from(multiply[1]),
                    i32::from(multiply[2]),
                    i32::from(add[0]),
                    i32::from(add[1]),
                    i32::from(add[2]),
                ]);
            }
            FrameFilter::Scroll {
                speed_x,
                speed_y,
                looped,
            } => values.extend([
                10,
                scroll_offset_pixels(timestamp, speed_x),
                scroll_offset_pixels(timestamp, speed_y),
                i32::from(looped),
                0,
                0,
                0,
            ]),
            FrameFilter::RenderDelay(_) => unreachable!(
                "source-level render delay must be resolved by the runtime before WGPU"
            ),
        }
    }
    values.into_iter().flat_map(i32::to_le_bytes).collect()
}

/// Converts the media filter's pixel-per-second value to the same bounded
/// integer frame offset as the CPU reference path.
fn scroll_offset_pixels(timestamp: Timestamp, speed: i16) -> i32 {
    let numerator = i128::from(speed) * i128::from(timestamp.as_nanos());
    let pixels = numerator.div_euclid(1_000_000_000);
    i32::try_from(pixels).unwrap_or_else(|_| {
        if pixels.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}
