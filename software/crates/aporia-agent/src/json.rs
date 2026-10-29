//! Reading fields out of the workspace's JSON value, with errors that name the field.
//!
//! `aporia_store::Json` deliberately has no schema layer; this crate's inputs (a model's tool
//! arguments, a scripted transcript, a plan) are all untrusted, so every read is checked and every
//! refusal says which field was wrong.

use aporia_store::Json;

pub type Fields = [(String, Json)];

#[must_use]
pub fn fields(value: &Json) -> Option<&Fields> {
    if let Json::Obj(fields) = value {
        Some(fields)
    } else {
        None
    }
}

#[must_use]
pub fn field<'a>(object: &'a Fields, key: &str) -> Option<&'a Json> {
    object
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

pub fn required<'a>(object: &'a Fields, key: &str) -> Result<&'a Json, String> {
    field(object, key).ok_or_else(|| format!("missing field `{key}`"))
}

pub fn string<'a>(object: &'a Fields, key: &str) -> Result<&'a str, String> {
    required(object, key)?
        .as_str()
        .ok_or_else(|| format!("field `{key}` must be a string"))
}

pub fn optional_string<'a>(object: &'a Fields, key: &str) -> Result<Option<&'a str>, String> {
    match field(object, key) {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("field `{key}` must be a string or null")),
    }
}

pub fn array<'a>(object: &'a Fields, key: &str) -> Result<&'a [Json], String> {
    required(object, key)?
        .as_array()
        .ok_or_else(|| format!("field `{key}` must be an array"))
}

pub fn string_array(object: &Fields, key: &str) -> Result<Vec<String>, String> {
    array(object, key)?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("field `{key}` must contain only strings"))
        })
        .collect()
}

pub fn u64_value(object: &Fields, key: &str) -> Result<u64, String> {
    required(object, key)?
        .as_u64()
        .ok_or_else(|| format!("field `{key}` must be a non-negative integer"))
}

pub fn optional_u64(object: &Fields, key: &str) -> Result<Option<u64>, String> {
    match field(object, key) {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("field `{key}` must be a non-negative integer")),
    }
}

/// Refuse a field this code does not know, rather than silently ignoring a misspelling.
pub fn only(object: &Fields, allowed: &[&str]) -> Result<(), String> {
    for (name, _) in object {
        if !allowed.contains(&name.as_str()) {
            return Err(format!("unknown field `{name}`"));
        }
    }
    Ok(())
}
