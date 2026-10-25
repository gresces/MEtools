//! Minimal dependency-free JSON value representation, serializer, and parser.
//!
//! Relies only on the Rust standard library so domain validation and protocol
//! responses do not depend on a transport or storage implementation.

use std::collections::BTreeMap;
use std::fmt;

// ---- JsonValue --------------------------------------------------------------

/// Generic JSON value type. Objects use `BTreeMap` for deterministic key ordering.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(i64),
    Float(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    // ---- constructors -------------------------------------------------------

    pub fn object<I, K>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, JsonValue)>,
        K: Into<String>,
    {
        let mut map = BTreeMap::new();
        for (key, value) in entries {
            map.insert(key.into(), value);
        }
        Self::Object(map)
    }

    pub fn array<I>(items: I) -> Self
    where
        I: IntoIterator<Item = JsonValue>,
    {
        Self::Array(items.into_iter().collect())
    }

    pub fn string_array(items: &[&str]) -> Self {
        Self::Array(
            items
                .iter()
                .map(|item| Self::String((*item).to_string()))
                .collect(),
        )
    }

    // ---- accessors ----------------------------------------------------------

    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text.as_str()),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(n) if *n >= 0 => Some(*n as u64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<JsonValue>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<JsonValue>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object()?.get(key)
    }

    // ---- serialization ------------------------------------------------------

    pub fn to_json_string(&self) -> String {
        let mut buf = String::new();
        write_json_value(self, &mut buf);
        buf
    }

    pub fn to_pretty_json_string(&self) -> String {
        let mut buf = String::new();
        write_pretty_json_value(self, 0, &mut buf);
        buf.push('\n');
        buf
    }
}

impl From<f64> for JsonValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for JsonValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for JsonValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<bool> for JsonValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for JsonValue {
    fn from(value: i64) -> Self {
        Self::Number(value)
    }
}

impl From<u64> for JsonValue {
    fn from(value: u64) -> Self {
        Self::Number(value as i64)
    }
}

impl fmt::Display for JsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_json_string())
    }
}

// ---- write_json_value -------------------------------------------------------

/// Serialize `JsonValue` into a compact single-line RFC 8259 JSON string.
fn write_json_value(value: &JsonValue, out: &mut String) {
    match value {
        JsonValue::Null => out.push_str("null"),
        JsonValue::Bool(true) => out.push_str("true"),
        JsonValue::Bool(false) => out.push_str("false"),
        JsonValue::Number(num) => out.push_str(&num.to_string()),
        JsonValue::Float(num) => {
            if num.is_finite() {
                out.push_str(&num.to_string());
            } else {
                out.push_str("null");
            }
        }
        JsonValue::String(text) => write_escaped_string(text, out),
        JsonValue::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json_value(item, out);
            }
            out.push(']');
        }
        JsonValue::Object(entries) => write_json_object(entries, out),
    }
}

/// Serialize an object map directly, avoiding a temporary wrapper clone.
pub fn object_to_json_string(entries: &BTreeMap<String, JsonValue>) -> String {
    let mut out = String::new();
    write_json_object(entries, &mut out);
    out
}

fn write_json_object(entries: &BTreeMap<String, JsonValue>, out: &mut String) {
    out.push('{');
    for (index, (key, val)) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_escaped_string(key, out);
        out.push(':');
        write_json_value(val, out);
    }
    out.push('}');
}

// ---- write_pretty_json_value ------------------------------------------------

/// Serialize `JsonValue` into a 2-space indented JSON string for human-readable persistence.
fn write_pretty_json_value(value: &JsonValue, indent: usize, out: &mut String) {
    match value {
        JsonValue::Array(items) if items.is_empty() => out.push_str("[]"),
        JsonValue::Array(items) => {
            out.push_str("[\n");
            let child_indent = indent + 2;
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&" ".repeat(child_indent));
                write_pretty_json_value(item, child_indent, out);
            }
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            out.push(']');
        }
        JsonValue::Object(entries) if entries.is_empty() => out.push_str("{}"),
        JsonValue::Object(entries) => {
            out.push_str("{\n");
            let child_indent = indent + 2;
            for (index, (key, val)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&" ".repeat(child_indent));
                write_escaped_string(key, out);
                out.push_str(": ");
                write_pretty_json_value(val, child_indent, out);
            }
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            out.push('}');
        }
        other => write_json_value(other, out),
    }
}

// ---- write_escaped_string ---------------------------------------------------

fn write_escaped_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---- parse_json -------------------------------------------------------------

/// Parse UTF-8 JSON text, rejecting trailing characters and non-finite numbers.
pub fn parse_json(input: &str) -> Result<JsonValue, String> {
    let mut parser = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    parser.skip_whitespace();
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.pos != parser.bytes.len() {
        return Err("trailing characters after JSON value".to_string());
    }
    Ok(value)
}

// ---- Parser -----------------------------------------------------------------

/// Append one unescaped byte run after validating it as UTF-8.
fn push_utf8_run(bytes: &[u8], out: &mut String) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| "invalid UTF-8 in JSON string".to_string())?;
    out.push_str(text);
    Ok(())
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    // ---- helpers ------------------------------------------------------------

    fn skip_whitespace(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next_byte(&mut self) -> Result<u8, String> {
        let byte = self
            .bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| "unexpected end of JSON input".to_string())?;
        self.pos += 1;
        Ok(byte)
    }

    // ---- value parsing ------------------------------------------------------

    /// Dispatch parsing based on the leading byte of the current JSON value.
    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b't') => self.parse_literal(b"true", JsonValue::Bool(true)),
            Some(b'f') => self.parse_literal(b"false", JsonValue::Bool(false)),
            Some(b'n') => self.parse_literal(b"null", JsonValue::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(_) => Err("unexpected token in JSON".to_string()),
            None => Err("empty JSON input".to_string()),
        }
    }

    fn parse_literal(&mut self, expected: &[u8], value: JsonValue) -> Result<JsonValue, String> {
        if self.bytes.get(self.pos..self.pos + expected.len()) == Some(expected) {
            self.pos += expected.len();
            Ok(value)
        } else {
            Err("invalid literal in JSON".to_string())
        }
    }

    /// Parse a JSON object `{ "key": value, ... }`.
    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.pos += 1; // '{'
        self.skip_whitespace();
        let mut map = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err("expected string key in JSON object".to_string());
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.next_byte()? != b':' {
                return Err("expected ':' after key in JSON object".to_string());
            }
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_whitespace();
            match self.next_byte()? {
                b',' => continue,
                b'}' => break,
                _ => return Err("expected ',' or '}' in JSON object".to_string()),
            }
        }
        Ok(JsonValue::Object(map))
    }

    /// Parse a JSON array `[ value, ... ]`.
    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.pos += 1; // '['
        self.skip_whitespace();
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            let item = self.parse_value()?;
            items.push(item);
            self.skip_whitespace();
            match self.next_byte()? {
                b',' => continue,
                b']' => break,
                _ => return Err("expected ',' or ']' in JSON array".to_string()),
            }
        }
        Ok(JsonValue::Array(items))
    }

    /// Parse a double-quoted JSON string and standard escape sequences (including UTF-16 surrogate pairs).
    ///
    /// Unescaped runs are validated and copied in one batch. Validating a single
    /// character at a time would re-scan the remaining buffer for every
    /// character and make parsing quadratic on large documents.
    fn parse_string(&mut self) -> Result<String, String> {
        if self.next_byte()? != b'"' {
            return Err("expected '\"' at start of string".to_string());
        }
        let mut out = String::new();
        let mut run_start = self.pos;
        loop {
            let byte = match self.bytes.get(self.pos) {
                Some(byte) => *byte,
                None => return Err("unterminated JSON string".to_string()),
            };
            match byte {
                b'"' => {
                    push_utf8_run(&self.bytes[run_start..self.pos], &mut out)?;
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    push_utf8_run(&self.bytes[run_start..self.pos], &mut out)?;
                    self.pos += 1;
                    let esc = self.next_byte()?;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let code = self.parse_hex4()?;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if self.bytes.get(self.pos..self.pos + 2) != Some(b"\\u") {
                                    return Err(
                                        "unpaired high surrogate in unicode escape".to_string()
                                    );
                                }
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(
                                        "invalid low surrogate in unicode escape".to_string()
                                    );
                                }
                                let combined = 0x10000
                                    + (((code as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
                                let ch = char::from_u32(combined)
                                    .ok_or_else(|| "invalid unicode codepoint".to_string())?;
                                out.push(ch);
                            } else {
                                let ch = char::from_u32(code as u32)
                                    .ok_or_else(|| "invalid unicode escape".to_string())?;
                                out.push(ch);
                            }
                        }
                        _ => return Err("invalid escape sequence in JSON string".to_string()),
                    }
                    run_start = self.pos;
                }
                0x00..=0x1f => {
                    return Err("unescaped control character in JSON string".to_string());
                }
                _ => self.pos += 1,
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        let slice = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| "incomplete \\u escape".to_string())?;
        let text = std::str::from_utf8(slice).map_err(|_| "invalid hex escape".to_string())?;
        let value = u16::from_str_radix(text, 16).map_err(|_| "invalid hex escape".to_string())?;
        self.pos += 4;
        Ok(value)
    }

    /// Parse an integer or finite floating-point number according to the JSON grammar.
    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err("leading zeros are not allowed in JSON numbers".to_string());
                }
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err("invalid number in JSON".to_string()),
        }

        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("missing digits after decimal point".to_string());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err("missing digits in exponent".to_string());
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "invalid number encoding".to_string())?;
        if is_float {
            let value: f64 = text
                .parse()
                .map_err(|_| "invalid floating-point number".to_string())?;
            if !value.is_finite() {
                return Err("non-finite JSON number".to_string());
            }
            Ok(JsonValue::Float(value))
        } else {
            let value: i64 = text
                .parse()
                .map_err(|_| "integer out of range".to_string())?;
            Ok(JsonValue::Number(value))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonValue, parse_json};

    #[test]
    fn parses_unescaped_ascii_and_multibyte_runs() {
        let value =
            parse_json(r#"{"title":"组会 · 13:30","tags":["工作","健康"]}"#).expect("parse");
        assert_eq!(
            value.get("title").and_then(JsonValue::as_str),
            Some("组会 · 13:30")
        );
        assert_eq!(
            value
                .get("tags")
                .and_then(JsonValue::as_array)
                .map(|items| items.len()),
            Some(2)
        );
    }

    #[test]
    fn parses_escapes_and_surrogate_pairs() {
        let value =
            parse_json(r#"{"text":"line\nquote\"slash\\tab\t\u4e2d\ud83d\ude00"}"#).expect("parse");
        assert_eq!(
            value.get("text").and_then(JsonValue::as_str),
            Some("line\nquote\"slash\\tab\t中😀")
        );
    }

    #[test]
    fn rejects_invalid_strings() {
        for input in [
            r#"{"text":"unterminated"#,
            r#"{"text":"bad\q"}"#,
            "{\"text\":\"bad\nline\"}",
            r#"{"text":"\ud800\u0041"}"#,
        ] {
            assert!(parse_json(input).is_err(), "{input:?} must be rejected");
        }
    }

    #[test]
    fn parses_long_unescaped_values_without_quadratic_scans() {
        let text = "a".repeat(200_000);
        let input = format!(r#"{{"text":"{text}"}}"#);
        let value = parse_json(&input).expect("parse");
        assert_eq!(
            value.get("text").and_then(JsonValue::as_str).map(str::len),
            Some(200_000)
        );
    }
}
