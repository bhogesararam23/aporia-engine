//! Validation of tool arguments against the JSON-Schema subset the registry emits.
//!
//! Model arguments are untrusted input, so validation is fail-closed: a schema keyword this
//! validator does not understand is an error, never a silently skipped check. The subset is
//! deliberately small -- objects with required fields and no extras, strings, integers, numbers,
//! booleans, arrays with item schemas, enums and bounds -- because every built-in schema is
//! written in it and the contract tests verify that claim.

use crate::json;
use aporia_store::Json;

/// Validate `value` against `schema`. `path` names the position in the arguments for errors.
pub fn validate(schema: &Json, value: &Json, path: &str) -> Result<(), String> {
    let fields =
        json::fields(schema).ok_or_else(|| format!("{path}: the schema is not an object"))?;
    for (keyword, _) in fields {
        if !KNOWN_KEYWORDS.contains(&keyword.as_str()) {
            return Err(format!(
                "{path}: the schema uses unsupported keyword `{keyword}`; validation refuses to skip it"
            ));
        }
    }
    let kind = json::string(fields, "type").map_err(|reason| format!("{path}: {reason}"))?;
    match kind {
        "object" => object(fields, value, path),
        "array" => array(fields, value, path),
        "string" => string(fields, value, path),
        "integer" | "number" => number(fields, value, path, kind == "integer"),
        "boolean" => {
            if value.as_bool().is_none() {
                return Err(format!("{path} must be a boolean"));
            }
            enumeration(fields, value, path)
        }
        other => Err(format!("{path}: unsupported schema type `{other}`")),
    }
}

const KNOWN_KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "description",
];

fn object(fields: &json::Fields, value: &Json, path: &str) -> Result<(), String> {
    let actual = json::fields(value).ok_or_else(|| format!("{path} must be an object"))?;
    if let Some(required) = json::field(fields, "required") {
        let required = required
            .as_array()
            .ok_or_else(|| format!("{path}: schema `required` is not an array"))?;
        for name in required {
            let name = name
                .as_str()
                .ok_or_else(|| format!("{path}: schema `required` holds a non-string"))?;
            if json::field(actual, name).is_none() {
                return Err(format!("{path} is missing required field `{name}`"));
            }
        }
    }
    let properties = match json::field(fields, "properties") {
        Some(value) => json::fields(value)
            .ok_or_else(|| format!("{path}: schema `properties` is not an object"))?,
        None => &[],
    };
    let additional = match json::field(fields, "additionalProperties") {
        None => true,
        Some(Json::Bool(allow)) => *allow,
        Some(_) => {
            return Err(format!(
                "{path}: schema `additionalProperties` must be a boolean in this subset"
            ));
        }
    };
    for (name, field) in actual {
        let field_path = format!("{path}.{name}");
        match json::field(properties, name) {
            Some(property) => validate(property, field, &field_path)?,
            None if !additional => {
                return Err(format!("{path} has unknown field `{name}`"));
            }
            None => {}
        }
    }
    enumeration(fields, value, path)
}

fn array(fields: &json::Fields, value: &Json, path: &str) -> Result<(), String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{path} must be an array"))?;
    if let Some(minimum) = bound(fields, "minItems", path)?
        && (items.len() as u64) < minimum
    {
        return Err(format!("{path} needs at least {minimum} items"));
    }
    if let Some(maximum) = bound(fields, "maxItems", path)?
        && (items.len() as u64) > maximum
    {
        return Err(format!("{path} allows at most {maximum} items"));
    }
    if let Some(item_schema) = json::field(fields, "items") {
        for (index, item) in items.iter().enumerate() {
            validate(item_schema, item, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

fn string(fields: &json::Fields, value: &Json, path: &str) -> Result<(), String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{path} must be a string"))?;
    let length = text.chars().count() as u64;
    if let Some(minimum) = bound(fields, "minLength", path)?
        && length < minimum
    {
        return Err(format!("{path} needs at least {minimum} characters"));
    }
    if let Some(maximum) = bound(fields, "maxLength", path)?
        && length > maximum
    {
        return Err(format!("{path} allows at most {maximum} characters"));
    }
    enumeration(fields, value, path)
}

fn number(
    fields: &json::Fields,
    value: &Json,
    path: &str,
    integer_only: bool,
) -> Result<(), String> {
    let number = match value {
        Json::Num(number) => *number,
        other => other
            .as_u64()
            .map(|count| count as f64)
            .ok_or_else(|| format!("{path} must be a number"))?,
    };
    if integer_only && number.fract() != 0.0 {
        return Err(format!("{path} must be an integer"));
    }
    if let Some(minimum) = numeric_bound(fields, "minimum", path)?
        && number < minimum
    {
        return Err(format!("{path} must be at least {minimum}"));
    }
    if let Some(maximum) = numeric_bound(fields, "maximum", path)?
        && number > maximum
    {
        return Err(format!("{path} must be at most {maximum}"));
    }
    enumeration(fields, value, path)
}

fn enumeration(fields: &json::Fields, value: &Json, path: &str) -> Result<(), String> {
    let Some(choices) = json::field(fields, "enum") else {
        return Ok(());
    };
    let choices = choices
        .as_array()
        .ok_or_else(|| format!("{path}: schema `enum` is not an array"))?;
    if choices.contains(value) {
        Ok(())
    } else {
        Err(format!("{path} is not one of the permitted values"))
    }
}

fn bound(fields: &json::Fields, keyword: &str, path: &str) -> Result<Option<u64>, String> {
    match json::field(fields, keyword) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{path}: schema `{keyword}` is not a non-negative integer")),
    }
}

fn numeric_bound(fields: &json::Fields, keyword: &str, path: &str) -> Result<Option<f64>, String> {
    match json::field(fields, keyword) {
        None => Ok(None),
        Some(Json::Num(number)) => Ok(Some(*number)),
        Some(value) => value
            .as_u64()
            .map(|count| Some(count as f64))
            .ok_or_else(|| format!("{path}: schema `{keyword}` is not a number")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(text: &str) -> Json {
        Json::parse(text).expect("schema json")
    }

    fn value(text: &str) -> Json {
        Json::parse(text).expect("value json")
    }

    #[test]
    fn a_matching_object_passes() {
        let schema = schema(
            r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false}"#,
        );
        assert_eq!(
            validate(&schema, &value(r#"{"path":"src/main.py"}"#), "arguments"),
            Ok(())
        );
    }

    #[test]
    fn missing_required_and_unknown_fields_are_named() {
        let schema = schema(
            r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
        );
        let error = validate(&schema, &value("{}"), "arguments").expect_err("missing");
        assert!(error.contains("path"), "{error}");
        let error = validate(&schema, &value(r#"{"path":"a","extra":1}"#), "arguments")
            .expect_err("unknown");
        assert!(error.contains("extra"), "{error}");
    }

    #[test]
    fn a_nested_error_names_the_position() {
        let schema = schema(
            r#"{"type":"object","properties":{"opts":{"type":"object","properties":{"n":{"type":"integer","minimum":1}},"required":["n"],"additionalProperties":false}},"required":["opts"],"additionalProperties":false}"#,
        );
        let error =
            validate(&schema, &value(r#"{"opts":{"n":0}}"#), "arguments").expect_err("bound");
        assert!(error.contains("arguments.opts.n"), "{error}");
    }

    #[test]
    fn enums_strings_arrays_and_booleans_are_checked() {
        assert!(
            validate(
                &schema(r#"{"type":"string","enum":["a","b"]}"#),
                &value(r#""c""#),
                "x"
            )
            .is_err()
        );
        assert!(
            validate(
                &schema(r#"{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":2}"#),
                &value("[]"),
                "x"
            )
            .is_err()
        );
        assert!(
            validate(
                &schema(r#"{"type":"array","items":{"type":"string"}}"#),
                &value(r#"["a",3]"#),
                "x"
            )
            .is_err()
        );
        assert!(validate(&schema(r#"{"type":"boolean"}"#), &value("true"), "x").is_ok());
        assert!(
            validate(
                &schema(r#"{"type":"string","maxLength":3}"#),
                &value(r#""four""#),
                "x"
            )
            .is_err()
        );
    }

    #[test]
    fn an_unknown_schema_keyword_fails_closed() {
        let error = validate(
            &schema(r#"{"type":"string","pattern":"^a"}"#),
            &value(r#""abc""#),
            "x",
        )
        .expect_err("unsupported keyword");
        assert!(error.contains("pattern"), "{error}");
    }

    #[test]
    fn integers_reject_fractions_and_numbers_accept_them() {
        assert!(validate(&schema(r#"{"type":"integer"}"#), &Json::Num(1.5), "x").is_err());
        assert!(
            validate(
                &schema(r#"{"type":"number","maximum":2}"#),
                &Json::Num(1.5),
                "x"
            )
            .is_ok()
        );
    }
}
