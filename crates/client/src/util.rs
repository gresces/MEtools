//! Helpers shared by client subcommands.

use serde_json::{Map, Value};
use uuid::Uuid;

/// Build a compact request ID.
pub fn request_id() -> String {
    format!("req-{}", Uuid::new_v4().simple())
}

/// Parse a `--set key=value` value as a JSON literal when it clearly is one.
pub fn parse_set_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    let looks_like_json = trimmed == "true"
        || trimmed == "false"
        || trimmed == "null"
        || trimmed.starts_with('"')
        || trimmed.starts_with('[')
        || trimmed.starts_with('{')
        || trimmed.starts_with('-')
        || trimmed.as_bytes().first().is_some_and(u8::is_ascii_digit);
    if looks_like_json && let Ok(value) = serde_json::from_str(trimmed) {
        return value;
    }
    Value::String(raw.to_string())
}

/// Insert a dotted path such as `patch.status` into a JSON object.
pub fn set_nested(root: &mut Map<String, Value>, path: &str, value: Value) -> Result<(), String> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() || segments.iter().any(|segment| segment.trim().is_empty()) {
        return Err(format!("invalid field path '{path}'"));
    }
    insert_segments(root, &segments, value)
}

fn insert_segments(
    current: &mut Map<String, Value>,
    segments: &[&str],
    value: Value,
) -> Result<(), String> {
    let first = segments[0];
    if segments.len() == 1 {
        current.insert(first.to_string(), value);
        return Ok(());
    }

    let entry = current
        .entry(first.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let nested = entry
        .as_object_mut()
        .ok_or_else(|| format!("cannot set nested field on non-object field '{first}'"))?;
    insert_segments(nested, &segments[1..], value)
}

/// Build a JSON object from repeated `--set key=value` flags.
pub fn parse_set_pairs(pairs: &[String]) -> LocalResult<Map<String, Value>> {
    let mut root = Map::new();
    for pair in pairs {
        let (key, raw) = pair.split_once('=').ok_or_else(|| {
            LocalError::new("INVALID_INPUT", format!("expected key=value, got '{pair}'"))
        })?;
        set_nested(&mut root, key.trim(), parse_set_value(raw))
            .map_err(|message| LocalError::new("INVALID_INPUT", message))?;
    }
    Ok(root)
}

/// Extract the record version from a `/api/v1/versions/{resource}` response.
pub fn record_version(response: &Value, id: &str) -> Option<u64> {
    if response.get("record_id").and_then(Value::as_str) != Some(id) {
        return None;
    }
    response.get("record_version").and_then(Value::as_u64)
}

/// Extract the top-level version from a resource response.
pub fn top_version(response: &Value) -> Option<u64> {
    response.get("version").and_then(Value::as_u64)
}

/// Return a string field from an input object.
pub fn input_str<'a>(input: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

/// Error type for local client validation failures.
#[derive(Debug)]
pub struct LocalError {
    pub code: &'static str,
    pub message: String,
}

impl LocalError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub type LocalResult<T> = Result<T, LocalError>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_values_parse_json_literals_and_keep_plain_strings() {
        assert_eq!(parse_set_value("42"), json!(42));
        assert_eq!(parse_set_value("true"), json!(true));
        assert_eq!(parse_set_value("[1, 2]"), json!([1, 2]));
        assert_eq!(parse_set_value("hello world"), json!("hello world"));
        assert_eq!(parse_set_value("2026-10-20"), json!("2026-10-20"));
    }

    #[test]
    fn nested_paths_create_objects() {
        let mut root = Map::new();
        set_nested(&mut root, "patch.status", json!("done")).expect("set nested");
        set_nested(&mut root, "title", json!("Action")).expect("set title");
        assert_eq!(
            Value::Object(root),
            json!({"patch": {"status": "done"}, "title": "Action"})
        );
    }

    #[test]
    fn version_helpers_read_probe_responses() {
        let response = json!({
            "status": "succeeded",
            "resource": "actions",
            "version": 9,
            "record_id": "action-2",
            "record_version": 4
        });
        assert_eq!(record_version(&response, "action-2"), Some(4));
        assert_eq!(record_version(&response, "missing"), None);
        assert_eq!(top_version(&response), Some(9));
    }
}
