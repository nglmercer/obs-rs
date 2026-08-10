use super::super::CaptureError;
use super::{
    error::protocol_error,
    protocol::{ImageByteOrder, VisualMasks},
};

pub(crate) fn decode_pixels(
    width: usize,
    height: usize,
    row_stride: usize,
    bits_per_pixel: u8,
    byte_order: ImageByteOrder,
    masks: VisualMasks,
    data: &[u8],
) -> Result<Vec<u8>, CaptureError> {
    let bytes_per_pixel = usize::from(bits_per_pixel / 8);
    let pixel_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    let mut output = vec![0_u8; pixel_bytes];
    for y in 0..height {
        let row_start = y
            .checked_mul(row_stride)
            .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
        for x in 0..width {
            let offset = row_start
                .checked_add(
                    x.checked_mul(bytes_per_pixel)
                        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?,
                )
                .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
            let end = offset
                .checked_add(bytes_per_pixel)
                .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
            let source = data
                .get(offset..end)
                .ok_or_else(|| protocol_error("X11 pixel row is truncated"))?;
            let pixel = read_pixel(source, byte_order);
            let destination = (y * width + x) * 4;
            output[destination] = scale_channel(pixel, masks.red);
            output[destination + 1] = scale_channel(pixel, masks.green);
            output[destination + 2] = scale_channel(pixel, masks.blue);
            output[destination + 3] = 255;
        }
    }
    Ok(output)
}

fn read_pixel(bytes: &[u8], byte_order: ImageByteOrder) -> u32 {
    match byte_order {
        ImageByteOrder::LeastSignificantFirst => bytes
            .iter()
            .enumerate()
            .fold(0_u32, |value, (index, byte)| {
                value | (u32::from(*byte) << (index * 8))
            }),
        ImageByteOrder::MostSignificantFirst => bytes
            .iter()
            .fold(0_u32, |value, byte| (value << 8) | u32::from(*byte)),
    }
}

pub(crate) fn scale_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let maximum = u64::from(mask >> shift);
    let value = u64::from((pixel & mask) >> shift);
    let scaled = value.saturating_mul(u64::from(u8::MAX)) / maximum.max(1);
    u8::try_from(scaled.min(u64::from(u8::MAX))).unwrap_or(u8::MAX)
}

pub(crate) fn packed_row_bytes(width: usize, bits_per_pixel: u8) -> Result<usize, CaptureError> {
    width
        .checked_mul(usize::from(bits_per_pixel / 8))
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })
}

pub(crate) fn padded_row_bytes(row_bytes: usize, scanline_pad: u8) -> Result<usize, CaptureError> {
    let pad = usize::from(scanline_pad / 8);
    if pad == 0 {
        return Err(protocol_error("X11 scanline padding is zero"));
    }
    let rounded = row_bytes
        .checked_add(pad - 1)
        .ok_or(CaptureError::ReplyTooLarge { bytes: u64::MAX })?;
    Ok(rounded / pad * pad)
}
