//! Conversions between the domain JSON type and `serde_json`.
//!
//! Domain command handlers use a dependency-free JSON type. The HTTP layer uses
//! `serde_json` for request extraction and response serialization; these
//! functions are the only place where the two representations meet.

use serde_json::{Map, Number, Value};
use shared::JsonValue;
use std::collections::BTreeMap;

/// Convert a `serde_json` value into the domain JSON representation.
pub fn to_shared(value: &Value) -> Result<JsonValue, String> {
    Ok(match value {
        Value::Null => JsonValue::Null,
        Value::Bool(flag) => JsonValue::Bool(*flag),
        Value::String(text) => JsonValue::String(text.clone()),
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                JsonValue::Number(int)
            } else if let Some(uint) = number.as_u64() {
                if uint <= i64::MAX as u64 {
                    JsonValue::Number(uint as i64)
                } else {
                    return Err("integer is too large".to_string());
                }
            } else if let Some(float) = number.as_f64() {
                JsonValue::Float(float)
            } else {
                return Err("unsupported number".to_string());
            }
        }
        Value::Array(items) => {
            JsonValue::Array(items.iter().map(to_shared).collect::<Result<Vec<_>, _>>()?)
        }
        Value::Object(map) => {
            let mut object = BTreeMap::new();
            for (key, item) in map {
                object.insert(key.clone(), to_shared(item)?);
            }
            JsonValue::Object(object)
        }
    })
}

/// Convert a domain JSON value into the `serde_json` representation.
pub fn to_serde(value: &JsonValue) -> Value {
    match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(flag) => Value::Bool(*flag),
        JsonValue::Number(number) => Value::Number(Number::from(*number)),
        JsonValue::Float(number) => Number::from_f64(*number)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        JsonValue::String(text) => Value::String(text.clone()),
        JsonValue::Array(items) => Value::Array(items.iter().map(to_serde).collect()),
        JsonValue::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), to_serde(item)))
                .collect::<Map<String, Value>>(),
        ),
    }
}

/// Convert a domain JSON value into `serde_json` without cloning its contents.
pub fn into_serde(value: JsonValue) -> Value {
    match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(flag) => Value::Bool(flag),
        JsonValue::Number(number) => Value::Number(Number::from(number)),
        JsonValue::Float(number) => Number::from_f64(number)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        JsonValue::String(text) => Value::String(text),
        JsonValue::Array(items) => {
            Value::Array(items.into_iter().map(into_serde).collect::<Vec<_>>())
        }
        JsonValue::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, item)| (key, into_serde(item)))
                .collect::<Map<String, Value>>(),
        ),
    }
}

/// Convert a `serde_json` value into the domain representation without cloning strings.
pub fn into_shared(value: Value) -> Result<JsonValue, String> {
    match value {
        Value::Null => Ok(JsonValue::Null),
        Value::Bool(flag) => Ok(JsonValue::Bool(flag)),
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Ok(JsonValue::Number(int))
            } else if let Some(uint) = number.as_u64() {
                if uint <= i64::MAX as u64 {
                    Ok(JsonValue::Number(uint as i64))
                } else {
                    Err("integer is too large".to_string())
                }
            } else if let Some(float) = number.as_f64() {
                Ok(JsonValue::Float(float))
            } else {
                Err("unsupported number".to_string())
            }
        }
        Value::String(text) => Ok(JsonValue::String(text)),
        Value::Array(items) => {
            let mut converted = Vec::with_capacity(items.len());
            for item in items {
                converted.push(into_shared(item)?);
            }
            Ok(JsonValue::Array(converted))
        }
        Value::Object(map) => {
            let mut object = BTreeMap::new();
            for (key, item) in map {
                object.insert(key, into_shared(item)?);
            }
            Ok(JsonValue::Object(object))
        }
    }
}
