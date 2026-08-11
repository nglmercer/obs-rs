//! The subset of the D-Bus type system the desktop portal handshake uses.

use std::collections::BTreeMap;

/// One D-Bus value.
///
/// Only the types the `ScreenCast` portal exchanges are modelled. Anything
/// else in a reply is decoded structurally and ignored, so a portal that adds
/// fields does not break the handshake.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Byte(u8),
    Bool(bool),
    Int32(i32),
    Uint32(u32),
    Int64(i64),
    Uint64(u64),
    Double(f64),
    Str(String),
    ObjectPath(String),
    Signature(String),
    /// `a<element>` with the element signature retained for empty arrays.
    Array {
        element: String,
        items: Vec<Value>,
    },
    Struct(Vec<Value>),
    /// `a{sv}`, the shape every portal option and result set uses.
    Dict(BTreeMap<String, Value>),
    Variant(Box<Value>),
}

impl Value {
    /// Returns the D-Bus signature of this value.
    pub(crate) fn signature(&self) -> String {
        match self {
            Self::Byte(_) => "y".to_owned(),
            Self::Bool(_) => "b".to_owned(),
            Self::Int32(_) => "i".to_owned(),
            Self::Uint32(_) => "u".to_owned(),
            Self::Int64(_) => "x".to_owned(),
            Self::Uint64(_) => "t".to_owned(),
            Self::Double(_) => "d".to_owned(),
            Self::Str(_) => "s".to_owned(),
            Self::ObjectPath(_) => "o".to_owned(),
            Self::Signature(_) => "g".to_owned(),
            Self::Array { element, .. } => format!("a{element}"),
            Self::Struct(fields) => {
                let inner = fields
                    .iter()
                    .map(Self::signature)
                    .collect::<Vec<_>>()
                    .concat();
                format!("({inner})")
            }
            Self::Dict(_) => "a{sv}".to_owned(),
            Self::Variant(_) => "v".to_owned(),
        }
    }

    /// Returns the string a `s` or `o` value holds.
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) | Self::ObjectPath(value) => Some(value),
            Self::Variant(inner) => inner.as_str(),
            _ => None,
        }
    }

    /// Returns the number a `u` value holds.
    pub(crate) fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Uint32(value) => Some(*value),
            Self::Variant(inner) => inner.as_u32(),
            _ => None,
        }
    }

    /// Returns the entries of a dictionary, unwrapping a variant around it.
    pub(crate) fn as_dict(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Self::Dict(entries) => Some(entries),
            Self::Variant(inner) => inner.as_dict(),
            _ => None,
        }
    }

    /// Returns the items of an array or the fields of a struct.
    pub(crate) fn as_items(&self) -> Option<&[Value]> {
        match self {
            Self::Array { items, .. } => Some(items),
            Self::Struct(fields) => Some(fields),
            Self::Variant(inner) => inner.as_items(),
            _ => None,
        }
    }

    /// Wraps this value in a variant, which is how portal options are passed.
    pub(crate) fn into_variant(self) -> Self {
        Self::Variant(Box::new(self))
    }
}

/// Builds an `a{sv}` options dictionary from `(key, value)` pairs.
pub(crate) fn options<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Dict(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.into_variant()))
            .collect(),
    )
}
