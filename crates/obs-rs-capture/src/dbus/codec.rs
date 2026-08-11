//! Little-endian D-Bus marshalling for the values in [`super::value`].
//!
//! D-Bus aligns every value to its own natural boundary, measured from the
//! start of the message body, so both halves of this module carry the buffer
//! offset rather than working on isolated slices.

use std::collections::BTreeMap;

use super::value::Value;
use crate::CaptureError;

/// Returns the alignment of the type a signature starts with.
fn alignment(signature: &str) -> usize {
    match signature.as_bytes().first() {
        Some(b'y' | b'g' | b'v') => 1,
        Some(b'n' | b'q') => 2,
        Some(b'x' | b't' | b'd' | b'(' | b'{') => 8,
        // b, i, u, s, o, a, h
        _ => 4,
    }
}

fn pad_to(bytes: &mut Vec<u8>, alignment: usize) {
    while !bytes.len().is_multiple_of(alignment) {
        bytes.push(0);
    }
}

/// Appends `value` to `bytes`, aligned from the start of `bytes`.
pub(crate) fn encode(bytes: &mut Vec<u8>, value: &Value) -> Result<(), CaptureError> {
    pad_to(bytes, alignment(&value.signature()));
    match value {
        Value::Byte(value) => bytes.push(*value),
        Value::Bool(value) => bytes.extend_from_slice(&u32::from(*value).to_le_bytes()),
        Value::Int32(value) => bytes.extend_from_slice(&value.to_le_bytes()),
        Value::Uint32(value) => bytes.extend_from_slice(&value.to_le_bytes()),
        Value::Int64(value) => bytes.extend_from_slice(&value.to_le_bytes()),
        Value::Uint64(value) => bytes.extend_from_slice(&value.to_le_bytes()),
        Value::Double(value) => bytes.extend_from_slice(&value.to_le_bytes()),
        Value::Str(text) | Value::ObjectPath(text) => encode_string(bytes, text)?,
        Value::Signature(text) => encode_signature(bytes, text)?,
        Value::Array { element, items } => encode_array(bytes, element, items)?,
        Value::Struct(fields) => {
            for field in fields {
                encode(bytes, field)?;
            }
        }
        Value::Dict(entries) => encode_dict(bytes, entries)?,
        Value::Variant(inner) => {
            encode_signature(bytes, &inner.signature())?;
            encode(bytes, inner)?;
        }
    }
    Ok(())
}

fn encode_string(bytes: &mut Vec<u8>, text: &str) -> Result<(), CaptureError> {
    let length = u32::try_from(text.len()).map_err(|_| protocol("D-Bus string is too long"))?;
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
    Ok(())
}

fn encode_signature(bytes: &mut Vec<u8>, text: &str) -> Result<(), CaptureError> {
    let length = u8::try_from(text.len()).map_err(|_| protocol("D-Bus signature is too long"))?;
    bytes.push(length);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
    Ok(())
}

/// Arrays carry the byte length of their contents, which is only known after
/// the contents have been written, so the length is patched afterwards.
fn encode_array(bytes: &mut Vec<u8>, element: &str, items: &[Value]) -> Result<(), CaptureError> {
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    let length_offset = bytes.len() - 4;
    pad_to(bytes, alignment(element));
    let content_start = bytes.len();
    for item in items {
        encode(bytes, item)?;
    }
    let length = u32::try_from(bytes.len() - content_start)
        .map_err(|_| protocol("D-Bus array is too large"))?;
    bytes[length_offset..length_offset + 4].copy_from_slice(&length.to_le_bytes());
    Ok(())
}

fn encode_dict(bytes: &mut Vec<u8>, entries: &BTreeMap<String, Value>) -> Result<(), CaptureError> {
    let items = entries
        .iter()
        .map(|(key, value)| Value::Struct(vec![Value::Str(key.clone()), value.clone()]))
        .collect::<Vec<_>>();
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    let length_offset = bytes.len() - 4;
    pad_to(bytes, 8);
    let content_start = bytes.len();
    for item in items {
        // A dict entry aligns like a struct but is written without extra
        // framing, so the struct encoder is reused directly.
        encode(bytes, &item)?;
    }
    let length = u32::try_from(bytes.len() - content_start)
        .map_err(|_| protocol("D-Bus dictionary is too large"))?;
    bytes[length_offset..length_offset + 4].copy_from_slice(&length.to_le_bytes());
    Ok(())
}

/// Reads one value of `signature` from `bytes` at `offset`.
///
/// `offset` is relative to the start of `bytes`, which must therefore begin at
/// the start of the message body for alignment to be correct.
pub(crate) fn decode(
    bytes: &[u8],
    offset: &mut usize,
    signature: &str,
) -> Result<Value, CaptureError> {
    align(offset, alignment(signature));
    let mut signature = signature.chars();
    let code = signature
        .next()
        .ok_or_else(|| protocol("D-Bus signature is empty"))?;
    let rest = signature.as_str();
    match code {
        'y' => Ok(Value::Byte(read_u8(bytes, offset)?)),
        'b' => Ok(Value::Bool(read_u32(bytes, offset)? != 0)),
        'i' => Ok(Value::Int32(i32::from_le_bytes(read_array(bytes, offset)?))),
        'u' => Ok(Value::Uint32(read_u32(bytes, offset)?)),
        'x' => Ok(Value::Int64(i64::from_le_bytes(read_array(bytes, offset)?))),
        't' => Ok(Value::Uint64(u64::from_le_bytes(read_array(
            bytes, offset,
        )?))),
        'd' => Ok(Value::Double(f64::from_le_bytes(read_array(
            bytes, offset,
        )?))),
        's' => Ok(Value::Str(read_string(bytes, offset)?)),
        'o' => Ok(Value::ObjectPath(read_string(bytes, offset)?)),
        'g' => Ok(Value::Signature(read_signature(bytes, offset)?)),
        'v' => {
            let inner = read_signature(bytes, offset)?;
            Ok(Value::Variant(Box::new(decode(bytes, offset, &inner)?)))
        }
        'a' => decode_array(bytes, offset, rest),
        '(' => decode_struct(bytes, offset, rest),
        other => Err(protocol(format!("unsupported D-Bus type code {other}"))),
    }
}

fn decode_array(bytes: &[u8], offset: &mut usize, rest: &str) -> Result<Value, CaptureError> {
    let element = leading_type(rest)?;
    let length = read_u32(bytes, offset)? as usize;
    align(offset, alignment(element));
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| protocol("D-Bus array runs past the message"))?;
    // `a{sv}` is decoded into the map shape the portal results are read as.
    if element.starts_with('{') {
        let mut entries = BTreeMap::new();
        while *offset < end {
            align(offset, 8);
            let key = read_string(bytes, offset)?;
            let value = decode(bytes, offset, "v")?;
            entries.insert(key, value);
        }
        *offset = end;
        return Ok(Value::Dict(entries));
    }
    let mut items = Vec::new();
    while *offset < end {
        items.push(decode(bytes, offset, element)?);
    }
    *offset = end;
    Ok(Value::Array {
        element: element.to_owned(),
        items,
    })
}

fn decode_struct(bytes: &[u8], offset: &mut usize, rest: &str) -> Result<Value, CaptureError> {
    let mut fields = Vec::new();
    let mut remaining = rest;
    while !remaining.starts_with(')') {
        if remaining.is_empty() {
            return Err(protocol("D-Bus struct signature is unterminated"));
        }
        let field = leading_type(remaining)?;
        fields.push(decode(bytes, offset, field)?);
        remaining = &remaining[field.len()..];
    }
    Ok(Value::Struct(fields))
}

/// Returns the leading complete type in `signature`.
pub(crate) fn leading_type(signature: &str) -> Result<&str, CaptureError> {
    let bytes = signature.as_bytes();
    let first = *bytes
        .first()
        .ok_or_else(|| protocol("D-Bus signature is empty"))?;
    match first {
        b'a' => {
            let inner = leading_type(&signature[1..])?;
            Ok(&signature[..=inner.len()])
        }
        b'(' | b'{' => {
            let (open, close) = if first == b'(' {
                (b'(', b')')
            } else {
                (b'{', b'}')
            };
            let mut depth = 0_usize;
            for (index, byte) in bytes.iter().enumerate() {
                if *byte == open {
                    depth += 1;
                } else if *byte == close {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(&signature[..=index]);
                    }
                }
            }
            Err(protocol("D-Bus container signature is unterminated"))
        }
        _ => Ok(&signature[..1]),
    }
}

fn align(offset: &mut usize, alignment: usize) {
    while !offset.is_multiple_of(alignment) {
        *offset += 1;
    }
}

fn read_u8(bytes: &[u8], offset: &mut usize) -> Result<u8, CaptureError> {
    let value = *bytes
        .get(*offset)
        .ok_or_else(|| protocol("D-Bus message is truncated"))?;
    *offset += 1;
    Ok(value)
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, CaptureError> {
    Ok(u32::from_le_bytes(read_array(bytes, offset)?))
}

fn read_array<const N: usize>(bytes: &[u8], offset: &mut usize) -> Result<[u8; N], CaptureError> {
    let end = offset
        .checked_add(N)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| protocol("D-Bus message is truncated"))?;
    let value = bytes[*offset..end]
        .try_into()
        .map_err(|_| protocol("D-Bus message is truncated"))?;
    *offset = end;
    Ok(value)
}

fn read_string(bytes: &[u8], offset: &mut usize) -> Result<String, CaptureError> {
    align(offset, 4);
    let length = read_u32(bytes, offset)? as usize;
    read_text(bytes, offset, length)
}

fn read_signature(bytes: &[u8], offset: &mut usize) -> Result<String, CaptureError> {
    let length = usize::from(read_u8(bytes, offset)?);
    read_text(bytes, offset, length)
}

/// Reads `length` bytes of text plus the trailing NUL D-Bus always writes.
fn read_text(bytes: &[u8], offset: &mut usize, length: usize) -> Result<String, CaptureError> {
    let end = offset
        .checked_add(length)
        .filter(|end| *end < bytes.len())
        .ok_or_else(|| protocol("D-Bus string is truncated"))?;
    let text = std::str::from_utf8(&bytes[*offset..end])
        .map_err(|_| protocol("D-Bus string is not UTF-8"))?
        .to_owned();
    // Skip the NUL terminator, which is not counted in the length.
    *offset = end + 1;
    Ok(text)
}

pub(crate) fn protocol(message: impl Into<String>) -> CaptureError {
    CaptureError::Protocol {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: &Value) -> Value {
        let mut bytes = Vec::new();
        encode(&mut bytes, value).expect("encode");
        let mut offset = 0;
        decode(&bytes, &mut offset, &value.signature()).expect("decode")
    }

    #[test]
    fn scalars_and_strings_round_trip() {
        for value in [
            Value::Uint32(42),
            Value::Bool(true),
            Value::Int32(-7),
            Value::Str("handle_token".to_owned()),
            Value::ObjectPath("/org/freedesktop/portal/desktop".to_owned()),
        ] {
            assert_eq!(round_trip(&value), value);
        }
    }

    #[test]
    fn option_dictionaries_round_trip_through_variants() {
        let value = super::super::value::options([
            ("types", Value::Uint32(1)),
            ("multiple", Value::Bool(false)),
            ("handle_token", Value::Str("obsrs0".to_owned())),
        ]);

        let decoded = round_trip(&value);

        let entries = decoded.as_dict().expect("dictionary");
        assert_eq!(entries["types"].as_u32(), Some(1));
        assert_eq!(entries["handle_token"].as_str(), Some("obsrs0"));
    }

    #[test]
    fn stream_lists_decode_as_nested_structures() {
        // The shape `Start` answers with: a(ua{sv}).
        let streams = Value::Array {
            element: "(ua{sv})".to_owned(),
            items: vec![Value::Struct(vec![
                Value::Uint32(63),
                super::super::value::options([(
                    "size",
                    Value::Struct(vec![Value::Int32(1920), Value::Int32(1080)]),
                )]),
            ])],
        };

        let decoded = round_trip(&streams);

        let items = decoded.as_items().expect("streams");
        let fields = items[0].as_items().expect("stream fields");
        assert_eq!(fields[0].as_u32(), Some(63));
        let size = fields[1].as_dict().expect("properties")["size"]
            .as_items()
            .expect("size")
            .to_vec();
        assert_eq!(size, vec![Value::Int32(1920), Value::Int32(1080)]);
    }

    #[test]
    fn signature_scanner_splits_complete_types() {
        assert_eq!(leading_type("ua{sv}").expect("scan"), "u");
        assert_eq!(leading_type("a{sv}s").expect("scan"), "a{sv}");
        assert_eq!(leading_type("(ua{sv})x").expect("scan"), "(ua{sv})");
        assert!(leading_type("(us").is_err());
    }
}
