//! JSON helpers owned by the settings daemon.
//!
//! All socket frames, store values and backend replies use Foundation
//! (`foundation::serialization::JsonValue`) instead of serde. This module
//! re-exports the type and provides small builders so call sites stay
//! readable without a `json!` macro.

pub use foundation::serialization::JsonValue;

/// Null JSON value.
pub fn null() -> JsonValue {
    JsonValue::Null
}

/// Build an object from entries.
pub fn object(entries: Vec<(String, JsonValue)>) -> JsonValue {
    JsonValue::Object(entries)
}

/// Build an array.
pub fn array(items: Vec<JsonValue>) -> JsonValue {
    JsonValue::Array(items)
}

/// String value.
pub fn str_value(value: &str) -> JsonValue {
    JsonValue::Str(value.to_string())
}

/// Optional string value (null when `None`).
pub fn opt_str(value: Option<&str>) -> JsonValue {
    match value {
        Some(v) => str_value(v),
        None => JsonValue::Null,
    }
}

/// Parse JSON text, mapping Foundation errors to strings.
pub fn parse(text: &str) -> Result<JsonValue, String> {
    JsonValue::parse(text).map_err(|e| e.to_string())
}

/// Stringify a value (compact when `pretty` is false).
pub fn stringify(value: &JsonValue, pretty: bool) -> String {
    value.stringify(pretty)
}

/// Extract a string field from an object value.
pub fn get_str(value: &JsonValue, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

/// Extract an optional string field (None when absent or null).
pub fn get_opt_str(value: &JsonValue, key: &str) -> Option<String> {
    get_str(value, key)
}

/// Extract a bool field.
pub fn get_bool(value: &JsonValue, key: &str) -> Option<bool> {
    value.get(key)?.as_bool()
}

/// Extract an i64 field (integers only).
pub fn get_i64(value: &JsonValue, key: &str) -> Option<i64> {
    value.get(key)?.as_i64()
}

/// Extract a u64 field (non-negative integers only).
pub fn get_u64(value: &JsonValue, key: &str) -> Option<u64> {
    value.get(key)?.as_u64()
}

/// Extract an f64 field (integers coerce to floats).
pub fn get_f64(value: &JsonValue, key: &str) -> Option<f64> {
    value.get(key)?.as_f64()
}

/// Request id from a socket frame (`id` integer, 0 when missing).
pub fn frame_id(value: &JsonValue) -> u64 {
    value
        .get("id")
        .and_then(|v| v.as_i64())
        .and_then(|v| u64::try_from(v).ok())
        .unwrap_or(0)
}

/// `op` string from a socket frame (empty when missing).
pub fn frame_op(value: &JsonValue) -> String {
    get_str(value, "op").unwrap_or_default()
}

/// Clone the `params` member (null when missing).
pub fn frame_params(value: &JsonValue) -> JsonValue {
    value.get("params").cloned().unwrap_or(JsonValue::Null)
}

/// Success frame: `{"id": id, "ok": true, "result": result}`.
pub fn success_frame(id: u64, result: JsonValue) -> JsonValue {
    object(vec![
        ("id".to_string(), JsonValue::Integer(id as i64)),
        ("ok".to_string(), JsonValue::Bool(true)),
        ("result".to_string(), result),
    ])
}

/// Error frame: `{"id": id, "ok": false, "error": message}`.
pub fn error_frame(id: u64, error: String) -> JsonValue {
    object(vec![
        ("id".to_string(), JsonValue::Integer(id as i64)),
        ("ok".to_string(), JsonValue::Bool(false)),
        ("error".to_string(), str_value(&error)),
    ])
}
