//! A small, bounded JSON reader and writer for OBS-RS document formats.
//!
//! This is deliberately not a general-purpose serialization framework. It
//! exists so that on-disk documents are ordinary JSON — inspectable, diffable,
//! and readable by any other tool — without pulling a derive-macro stack into a
//! workspace that otherwise has almost no dependencies.
//!
//! Three properties matter more here than breadth of feature:
//!
//! * **Determinism.** Object members are stored in a [`BTreeMap`], so writing a
//!   value always produces the same bytes and two saves of unchanged state
//!   compare equal.
//! * **Exactness.** Numbers are retained as their source text and converted on
//!   demand, so a 64-bit integer never round-trips through `f64`.
//! * **Boundedness.** Nesting is capped at [`MAX_DEPTH`], so a hostile document
//!   cannot exhaust the stack.

use std::{collections::BTreeMap, fmt, str::FromStr};

/// Maximum array/object nesting accepted by [`Json::parse`].
///
/// Recursive descent is the clearest way to write this parser, which makes
/// nesting depth a stack-consumption risk; documents this crate produces nest
/// about six levels, so the cap is far above legitimate use.
pub const MAX_DEPTH: usize = 64;

/// A parsed JSON value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Json {
    /// The `null` literal.
    Null,
    /// A `true` or `false` literal.
    Bool(bool),
    /// A number, retained as its source text so no precision is lost.
    Number(String),
    /// A string with all escapes already decoded.
    String(String),
    /// An ordered list of values.
    Array(Vec<Json>),
    /// A set of members, kept sorted by key for deterministic output.
    Object(BTreeMap<String, Json>),
}

impl Json {
    /// Builds a string value.
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    /// Builds a number value from anything with a `Display` form.
    pub fn number(value: impl fmt::Display) -> Self {
        Self::Number(value.to_string())
    }

    /// Builds an object from an iterator of members.
    pub fn object<K: Into<String>>(members: impl IntoIterator<Item = (K, Self)>) -> Self {
        Self::Object(
            members
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    /// Returns the member named `key`, if this is an object that has one.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(members) => members.get(key),
            _ => None,
        }
    }

    /// Returns the string contents, if this is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the boolean contents, if this is a boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the elements, if this is an array.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// Returns the members, if this is an object.
    #[must_use]
    pub fn as_object(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Object(members) => Some(members),
            _ => None,
        }
    }

    /// Parses the number text into `T`, if this is a number.
    ///
    /// The source text is kept verbatim, so an integer target sees the exact
    /// digits that were written rather than a float approximation of them.
    #[must_use]
    pub fn as_number<T: FromStr>(&self) -> Option<T> {
        match self {
            Self::Number(text) => text.parse().ok(),
            _ => None,
        }
    }

    /// Parses a complete JSON document.
    ///
    /// # Errors
    ///
    /// Returns [`JsonError`] when the input is malformed, nests deeper than
    /// [`MAX_DEPTH`], repeats an object key, or has trailing content after the
    /// top-level value.
    pub fn parse(document: &str) -> Result<Self, JsonError> {
        let mut parser = Parser {
            bytes: document.as_bytes(),
            offset: 0,
            line: 1,
        };
        let value = parser.parse_value(0)?;
        parser.skip_whitespace();
        if parser.offset != parser.bytes.len() {
            return Err(parser.error("unexpected trailing content"));
        }
        Ok(value)
    }

    /// Writes the value as indented JSON with a trailing newline.
    ///
    /// Object members are emitted in sorted key order, so the output is a
    /// deterministic function of the value.
    #[must_use]
    pub fn to_pretty_string(&self) -> String {
        let mut document = String::new();
        self.write_pretty(&mut document, 0);
        document.push('\n');
        document
    }

    fn write_pretty(&self, document: &mut String, depth: usize) {
        match self {
            Self::Null => document.push_str("null"),
            Self::Bool(true) => document.push_str("true"),
            Self::Bool(false) => document.push_str("false"),
            Self::Number(text) => document.push_str(text),
            Self::String(value) => write_string(document, value),
            Self::Array(values) if values.is_empty() => document.push_str("[]"),
            Self::Array(values) => {
                document.push_str("[\n");
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        document.push_str(",\n");
                    }
                    push_indent(document, depth + 1);
                    value.write_pretty(document, depth + 1);
                }
                document.push('\n');
                push_indent(document, depth);
                document.push(']');
            }
            Self::Object(members) if members.is_empty() => document.push_str("{}"),
            Self::Object(members) => {
                document.push_str("{\n");
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        document.push_str(",\n");
                    }
                    push_indent(document, depth + 1);
                    write_string(document, key);
                    document.push_str(": ");
                    value.write_pretty(document, depth + 1);
                }
                document.push('\n');
                push_indent(document, depth);
                document.push('}');
            }
        }
    }
}

impl fmt::Display for Json {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_pretty_string())
    }
}

fn push_indent(document: &mut String, depth: usize) {
    for _ in 0..depth {
        document.push_str("  ");
    }
}

fn write_string(document: &mut String, value: &str) {
    document.push('"');
    for character in value.chars() {
        match character {
            '"' => document.push_str("\\\""),
            '\\' => document.push_str("\\\\"),
            '\u{8}' => document.push_str("\\b"),
            '\t' => document.push_str("\\t"),
            '\n' => document.push_str("\\n"),
            '\u{c}' => document.push_str("\\f"),
            '\r' => document.push_str("\\r"),
            // JSON forbids raw control characters in a string; everything else
            // is written verbatim so the document stays human-readable UTF-8.
            control if (control as u32) < 0x20 => {
                let _ = fmt::Write::write_fmt(document, format_args!("\\u{:04x}", control as u32));
            }
            other => document.push(other),
        }
    }
    document.push('"');
}

/// A JSON parse failure, located by line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonError {
    /// One-based line the failure was detected on.
    pub line: usize,
    /// Human-readable description of what was expected.
    pub message: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for JsonError {}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
    line: usize,
}

impl Parser<'_> {
    fn error(&self, message: impl Into<String>) -> JsonError {
        JsonError {
            line: self.line,
            message: message.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.offset += 1;
        if byte == b'\n' {
            self.line += 1;
        }
        Some(byte)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.bump();
        }
    }

    /// Consumes `literal`, or reports it as missing.
    fn expect_literal(&mut self, literal: &str) -> Result<(), JsonError> {
        let end = self.offset + literal.len();
        if self.bytes.get(self.offset..end) != Some(literal.as_bytes()) {
            return Err(self.error(format!("expected `{literal}`")));
        }
        for _ in 0..literal.len() {
            self.bump();
        }
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.error("value nests too deeply"));
        }
        self.skip_whitespace();
        match self.peek() {
            None => Err(self.error("unexpected end of document")),
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => self.parse_string().map(Json::String),
            Some(b't') => self.expect_literal("true").map(|()| Json::Bool(true)),
            Some(b'f') => self.expect_literal("false").map(|()| Json::Bool(false)),
            Some(b'n') => self.expect_literal("null").map(|()| Json::Null),
            Some(_) => self.parse_number(),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.bump();
        let mut members = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(Json::Object(members));
        }

        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a quoted member name"));
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.bump() != Some(b':') {
                return Err(self.error("expected `:` after a member name"));
            }
            let value = self.parse_value(depth + 1)?;
            // A repeated key makes the document's meaning depend on which one a
            // reader keeps, so it is rejected rather than silently resolved.
            if members.insert(key.clone(), value).is_some() {
                return Err(self.error(format!("duplicate member name `{key}`")));
            }

            self.skip_whitespace();
            match self.bump() {
                Some(b',') => {}
                Some(b'}') => return Ok(Json::Object(members)),
                _ => return Err(self.error("expected `,` or `}` in an object")),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.bump();
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(Json::Array(values));
        }

        loop {
            values.push(self.parse_value(depth + 1)?);
            self.skip_whitespace();
            match self.bump() {
                Some(b',') => {}
                Some(b']') => return Ok(Json::Array(values)),
                _ => return Err(self.error("expected `,` or `]` in an array")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.bump();
        let mut decoded = String::new();
        loop {
            let start = self.offset;
            // Ordinary runs are the common case, so they are located in bulk and
            // copied once instead of pushed a character at a time.
            while !matches!(self.peek(), None | Some(b'"' | b'\\' | 0x00..=0x1F)) {
                self.bump();
            }
            match self.bytes.get(start..self.offset).map(std::str::from_utf8) {
                Some(Ok(run)) => decoded.push_str(run),
                _ => return Err(self.error("string contains invalid UTF-8")),
            }

            match self.bump() {
                Some(b'"') => return Ok(decoded),
                Some(b'\\') => decoded.push(self.parse_escape()?),
                Some(_) => return Err(self.error("unescaped control character in a string")),
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    fn parse_escape(&mut self) -> Result<char, JsonError> {
        match self.bump() {
            Some(b'"') => Ok('"'),
            Some(b'\\') => Ok('\\'),
            Some(b'/') => Ok('/'),
            Some(b'b') => Ok('\u{8}'),
            Some(b'f') => Ok('\u{c}'),
            Some(b'n') => Ok('\n'),
            Some(b'r') => Ok('\r'),
            Some(b't') => Ok('\t'),
            Some(b'u') => self.parse_unicode_escape(),
            _ => Err(self.error("unknown string escape")),
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, JsonError> {
        let first = self.parse_hex4()?;
        // A scalar above the BMP is written as a surrogate pair, so a leading
        // surrogate is only meaningful together with the trailing one.
        if (0xD800..0xDC00).contains(&first) {
            self.expect_literal("\\u")?;
            let second = self.parse_hex4()?;
            if !(0xDC00..0xE000).contains(&second) {
                return Err(self.error("expected a trailing surrogate"));
            }
            let scalar = 0x1_0000 + ((first - 0xD800) << 10) + (second - 0xDC00);
            return char::from_u32(scalar).ok_or_else(|| self.error("invalid surrogate pair"));
        }

        char::from_u32(first).ok_or_else(|| self.error("invalid `\\u` escape"))
    }

    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let digit = self
                .bump()
                .and_then(|byte| char::from(byte).to_digit(16))
                .ok_or_else(|| self.error("expected four hex digits"))?;
            // Four hex digits fit a u16, so this cannot overflow.
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Json, JsonError> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.bump();
        }
        // JSON's integer part is `0` or a non-zero digit followed by digits;
        // accepting `01` here would write out as a different token than it read.
        match self.peek() {
            Some(b'0') => {
                self.bump();
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.error("number has a leading zero"));
                }
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.bump();
                }
            }
            _ => return Err(self.error("expected a value")),
        }
        if self.peek() == Some(b'.') {
            self.bump();
            let fraction_start = self.offset;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
            if self.offset == fraction_start {
                return Err(self.error("expected digits after the decimal point"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            let exponent_start = self.offset;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
            if self.offset == exponent_start {
                return Err(self.error("expected digits in the exponent"));
            }
        }

        // The scan above only ever accepts ASCII, so the slice is valid UTF-8.
        let text = std::str::from_utf8(&self.bytes[start..self.offset])
            .map_err(|_| self.error("invalid number"))?;
        Ok(Json::Number(text.to_owned()))
    }
}

#[cfg(test)]
mod tests;
