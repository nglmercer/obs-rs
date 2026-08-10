use std::io::{Cursor, Read, Write};

use super::error::OutputError;

pub(crate) fn write_all(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), OutputError> {
    output
        .write_all(bytes)
        .map_err(|error| OutputError::Write(error.to_string()))
}

pub(crate) fn write_u32(output: &mut Vec<u8>, value: u32) -> Result<(), OutputError> {
    write_all(output, &value.to_le_bytes())
}

pub(crate) fn write_u16(output: &mut Vec<u8>, value: u16) -> Result<(), OutputError> {
    write_all(output, &value.to_le_bytes())
}

pub(crate) fn write_u64(output: &mut Vec<u8>, value: u64) -> Result<(), OutputError> {
    write_all(output, &value.to_le_bytes())
}

pub(crate) fn read_exact(input: &mut Cursor<&[u8]>, bytes: &mut [u8]) -> Result<(), OutputError> {
    input.read_exact(bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            OutputError::Truncated
        } else {
            OutputError::Write(error.to_string())
        }
    })
}

pub(crate) fn read_u32(input: &mut Cursor<&[u8]>) -> Result<u32, OutputError> {
    let mut bytes = [0_u8; 4];
    read_exact(input, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

pub(crate) fn read_u64(input: &mut Cursor<&[u8]>) -> Result<u64, OutputError> {
    let mut bytes = [0_u8; 8];
    read_exact(input, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

pub(crate) fn read_u8(input: &mut Cursor<&[u8]>) -> Result<u8, OutputError> {
    let mut byte = [0_u8; 1];
    read_exact(input, &mut byte)?;
    Ok(byte[0])
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn pcm16_bits(sample: f32) -> u16 {
    let sample = sample.clamp(-1.0, 1.0);
    let value = if sample <= -1.0 {
        i32::from(i16::MIN)
    } else {
        (sample * f32::from(i16::MAX)).round() as i32
    };
    let value = i16::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i16::MIN
        } else {
            i16::MAX
        }
    });
    u16::from_le_bytes(value.to_le_bytes())
}
