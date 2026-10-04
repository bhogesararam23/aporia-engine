//! The JSON that APORIA writes and reads, implemented against `std`.
//!
//! Storage and the subprocess adapter both speak JSON, so a reader and a writer are needed. They are
//! here rather than in a crate for the reason the dependency policy gives: the writer controls
//! exactly how a float is printed, and reproducibility of an archive depends on that being the same
//! decision everywhere. The reader is deliberately strict and bounded, because the adapter feeds it
//! output from a foreign program.
//!
//! Three documented departures from RFC 8259, all forced by the data:
//!
//! - Objects keep their field order, because a report is read by people and a shuffled key order
//!   makes two runs' summaries impossible to diff.
//! - A non-finite number is written as the string `"NaN"`, `"Infinity"` or `"-Infinity"` and read
//!   back into the same value. JSON has no literal for them, a risk score can genuinely be NaN when
//!   a channel produced nothing, and `null` would silently erase the difference between "not a
//!   number" and "not measured". A strict third-party parser sees a string, which is the safe
//!   direction for a tool that never promises machine compatibility.
//! - Integers have their own variant, so a step budget is never routed through a float on its way
//!   to disk.

use std::fmt::Write as _;

/// A JSON value, with objects as ordered fields.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    /// Exact, non-negative integers: counts, ids, instruction budgets.
    Int(u64),
    Str(String),
    Arr(Vec<Json>),
    /// Insertion order is preserved; see the module docs.
    Obj(Vec<(String, Json)>),
}

/// A parse failure with the byte offset it happened at, because a malformed adapter response is
/// unreadable without one.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonError {
    pub position: usize,
    pub message: String,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for JsonError {}

/// How deep a document may nest before it is refused. The parser recurses, and the adapter reads
/// bytes from another process, so unbounded depth is a stack overflow waiting for someone else's
/// input.
const MAX_DEPTH: u32 = 64;

impl Json {
    #[must_use]
    pub fn object(fields: Vec<(&str, Json)>) -> Self {
        Self::Obj(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::Str(value.into())
    }

    #[must_use]
    pub fn number(value: f64) -> Self {
        Self::Num(value)
    }

    /// Counts, ids and step budgets: exact integers, never routed through a float.
    #[must_use]
    pub fn count(value: u64) -> Self {
        Self::Int(value)
    }

    /// Look up a field in an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Num(v) if *v >= 0.0 && v.fract() == 0.0 => Some(*v as u64),
            Self::Str(s) => s.parse().ok(),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Num(v) => Some(*v),
            Self::Int(v) => Some(*v as f64),
            Self::Str(s) if s == "NaN" => Some(f64::NAN),
            Self::Str(s) if s == "Infinity" => Some(f64::INFINITY),
            Self::Str(s) if s == "-Infinity" => Some(f64::NEG_INFINITY),
            Self::Str(s) => s.parse().ok(),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Self::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// Compact form: one line, used inside JSONL records.
    #[must_use]
    pub fn to_compact(&self) -> String {
        let mut out = String::new();
        write_value(self, None, 0, &mut out);
        out
    }

    /// Indented form for files a person opens.
    #[must_use]
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        write_value(self, Some(2), 0, &mut out);
        out.push('\n');
        out
    }

    /// Parse a whole document, refusing trailing content.
    pub fn parse(text: &str) -> Result<Self, JsonError> {
        let bytes = text.as_bytes();
        let mut at = skip_ws(bytes, 0);
        let value = parse_value(text, bytes, &mut at, 0)?;
        at = skip_ws(bytes, at);
        if at != bytes.len() {
            return Err(fail(text, at, "trailing content after the top-level value"));
        }
        Ok(value)
    }

    /// Parse one line of a JSONL stream.
    pub fn parse_line(text: &str) -> Result<Self, JsonError> {
        Self::parse(text)
    }
}

fn write_value(value: &Json, indent: Option<u64>, depth: u32, out: &mut String) {
    let pad = |out: &mut String, depth: u32| {
        if let Some(step) = indent {
            out.push('\n');
            for _ in 0..depth * u32::try_from(step).unwrap_or(2) {
                out.push(' ');
            }
        }
    };
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Num(v) => write_number(*v, out),
        Json::Int(v) => {
            let _ = write!(out, "{v}");
        }
        Json::Str(s) => write_string(s, out),
        Json::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                write_value(item, indent, depth + 1, out);
            }
            pad(out, depth);
            out.push(']');
        }
        Json::Obj(fields) => {
            if fields.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, item)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, depth + 1);
                write_string(key, out);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(item, indent, depth + 1, out);
            }
            pad(out, depth);
            out.push('}');
        }
    }
}

fn write_number(v: f64, out: &mut String) {
    // Display for f64 gives the shortest representation that round-trips, which is exactly the
    // property an archive needs; the non-finite cases are the documented string encoding.
    if v.is_nan() {
        out.push_str("\"NaN\"");
    } else if v == f64::INFINITY {
        out.push_str("\"Infinity\"");
    } else if v == f64::NEG_INFINITY {
        out.push_str("\"-Infinity\"");
    } else if v.fract() == 0.0 && v.abs() < 1e15 {
        // A float that happens to be whole still has to read back as a float. Without the `.0`,
        // `1.0` is printed as `1`, parsed as an integer, and the archive no longer says which of
        // the two it held.
        let _ = write!(out, "{v}.0");
    } else {
        let _ = write!(out, "{v}");
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn skip_ws(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

/// An error that names where it happened. A byte offset alone is not readable when the adapter
/// emits a multi-line stream.
fn fail(text: &str, position: usize, message: &str) -> JsonError {
    let before = &text[..position.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = position - before.rfind('\n').map_or(0, |i| i + 1);
    JsonError {
        position,
        message: format!("{message} (line {line}, column {})", column + 1),
    }
}

fn parse_value(text: &str, bytes: &[u8], at: &mut usize, depth: u32) -> Result<Json, JsonError> {
    if depth > MAX_DEPTH {
        return Err(fail(text, *at, "nested too deeply"));
    }
    let Some(&c) = bytes.get(*at) else {
        return Err(fail(text, *at, "unexpected end of input"));
    };
    match c {
        b'{' => {
            *at += 1;
            let mut fields = Vec::new();
            loop {
                *at = skip_ws(bytes, *at);
                if bytes.get(*at) == Some(&b'}') {
                    *at += 1;
                    break;
                }
                if !fields.is_empty() {
                    if bytes.get(*at) != Some(&b',') {
                        return Err(fail(text, *at, "expected `,` or `}`"));
                    }
                    *at += 1;
                    *at = skip_ws(bytes, *at);
                }
                if bytes.get(*at) != Some(&b'"') {
                    return Err(fail(text, *at, "expected a quoted key"));
                }
                let key = parse_string(text, bytes, at)?;
                *at = skip_ws(bytes, *at);
                if bytes.get(*at) != Some(&b':') {
                    return Err(fail(text, *at, "expected `:` after the key"));
                }
                *at += 1;
                *at = skip_ws(bytes, *at);
                fields.push((key, parse_value(text, bytes, at, depth + 1)?));
            }
            Ok(Json::Obj(fields))
        }
        b'[' => {
            *at += 1;
            let mut items = Vec::new();
            loop {
                *at = skip_ws(bytes, *at);
                if bytes.get(*at) == Some(&b']') {
                    *at += 1;
                    break;
                }
                if !items.is_empty() {
                    if bytes.get(*at) != Some(&b',') {
                        return Err(fail(text, *at, "expected `,` or `]`"));
                    }
                    *at += 1;
                    *at = skip_ws(bytes, *at);
                }
                items.push(parse_value(text, bytes, at, depth + 1)?);
            }
            Ok(Json::Arr(items))
        }
        b'"' => Ok(Json::Str(parse_string(text, bytes, at)?)),
        b't' if bytes[*at..].starts_with(b"true") => {
            *at += 4;
            Ok(Json::Bool(true))
        }
        b'f' if bytes[*at..].starts_with(b"false") => {
            *at += 5;
            Ok(Json::Bool(false))
        }
        b'n' if bytes[*at..].starts_with(b"null") => {
            *at += 4;
            Ok(Json::Null)
        }
        b'-' | b'0'..=b'9' => {
            let start = *at;
            let mut end = start;
            while matches!(
                bytes.get(end),
                Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
            ) {
                end += 1;
            }
            let lexeme = &text[start..end];
            if lexeme.starts_with('0') && lexeme.as_bytes().get(1).is_some_and(u8::is_ascii_digit) {
                return Err(fail(text, start, "a leading zero is not a number"));
            }
            // An integral literal with no fractional or exponent part stays an exact integer.
            if !lexeme.contains(['.', 'e', 'E'])
                && let Ok(v) = lexeme.parse::<u64>()
            {
                *at = end;
                return Ok(Json::Int(v));
            }
            match lexeme.parse::<f64>() {
                Ok(v) => {
                    *at = end;
                    Ok(Json::Num(v))
                }
                Err(_) => Err(fail(text, start, &format!("not a number: {lexeme}"))),
            }
        }
        _ => Err(fail(text, *at, "unexpected character")),
    }
}

fn parse_string(text: &str, bytes: &[u8], at: &mut usize) -> Result<String, JsonError> {
    // The opening quote is at `at`.
    *at += 1;
    let mut out = String::new();
    loop {
        let Some(&c) = bytes.get(*at) else {
            return Err(fail(text, *at, "string never closed"));
        };
        match c {
            b'"' => {
                *at += 1;
                return Ok(out);
            }
            b'\\' => {
                *at += 1;
                let Some(&e) = bytes.get(*at) else {
                    return Err(fail(text, *at, "escape at end of input"));
                };
                match e {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'u' => {
                        let hex = match bytes.get(*at + 1..*at + 5) {
                            Some(slice) => String::from_utf8_lossy(slice).to_string(),
                            None => return Err(fail(text, *at, "truncated \\u escape")),
                        };
                        let Ok(code) = u32::from_str_radix(&hex, 16) else {
                            return Err(fail(text, *at, "\\u needs four hex digits"));
                        };
                        // A lone surrogate is refused rather than silently patched: half an escaped
                        // astral character is a bug in whoever wrote it, and guessing hides it.
                        let Some(ch) = char::from_u32(code) else {
                            return Err(fail(text, *at, "\\u escapes a lone surrogate"));
                        };
                        out.push(ch);
                        *at += 4;
                    }
                    other => {
                        return Err(fail(
                            text,
                            *at,
                            &format!("unknown escape \\{}", other as char),
                        ));
                    }
                }
                *at += 1;
            }
            _ => {
                // Copy the next whole UTF-8 character rather than one byte at a time.
                let rest = &text[*at..];
                let Some(ch) = rest.chars().next() else {
                    return Err(fail(text, *at, "end of input inside a string"));
                };
                if (ch as u32) < 0x20 {
                    return Err(fail(text, *at, "a control character must be escaped"));
                }
                out.push(ch);
                *at += ch.len_utf8();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_round_trips_through_the_two_forms() {
        let value = Json::object(vec![
            ("name", Json::text("projectile")),
            ("evaluations", Json::count(412)),
            ("budget", Json::count(9_007_199_254_740_993)),
            ("risk", Json::number(0.632_120_558_828_557_7)),
            ("bands", Json::Arr(vec![Json::number(1.0), Json::Null])),
            (
                "policy",
                Json::object(vec![
                    ("suspicious", Json::number(0.55)),
                    ("loud", Json::Bool(false)),
                ]),
            ),
        ]);
        let text = value.to_pretty();
        let back = Json::parse(&text).expect("the writer's own output must parse");
        assert_eq!(value, back);
        assert_eq!(Json::parse(&value.to_compact()).unwrap(), value);
    }

    #[test]
    fn field_order_survives_because_diffing_a_report_needs_it_to() {
        let value = Json::object(vec![
            ("z", Json::count(1)),
            ("a", Json::count(2)),
            ("m", Json::count(3)),
        ]);
        assert_eq!(value.to_compact(), r#"{"z":1,"a":2,"m":3}"#);
    }

    #[test]
    fn integers_are_printed_as_integers_and_stay_exact() {
        // Past 2^53 a float cannot hold the value, which is why counts get their own variant.
        let big = 9_007_199_254_740_993u64;
        let text = Json::count(big).to_compact();
        assert_eq!(text, "9007199254740993");
        assert_eq!(Json::parse(&text).unwrap().as_u64(), Some(big));
    }

    #[test]
    fn floats_print_in_their_shortest_round_tripping_form() {
        let value = Json::number(0.1 + 0.2);
        assert_eq!(value.to_compact(), "0.30000000000000004");
        assert_eq!(Json::parse(&value.to_compact()).unwrap(), value);
        assert_eq!(Json::number(400.0).to_compact(), "400.0");
        // The `.0` is what keeps the archive honest: `400` and `400.0` are different kinds of number
        // here, and a reader must be able to tell which one was stored.
        assert_eq!(Json::parse("400.0").unwrap(), Json::number(400.0));
        assert_eq!(Json::parse("400").unwrap(), Json::count(400));
    }

    #[test]
    fn non_finite_numbers_are_kept_as_themselves_and_not_as_null() {
        for (v, tag) in [
            (f64::NAN, "\"NaN\""),
            (f64::INFINITY, "\"Infinity\""),
            (f64::NEG_INFINITY, "\"-Infinity\""),
        ] {
            let text = Json::number(v).to_compact();
            assert_eq!(text, tag);
            let back = Json::parse(&text).unwrap().as_f64().unwrap();
            assert_eq!(back.is_nan(), v.is_nan());
            // NaN is not equal to itself, so the NaN case is covered by the line above and the
            // equality only applies to the infinities.
            if !v.is_nan() {
                assert_eq!(back, v);
            }
        }
    }

    #[test]
    fn strings_keep_their_escapes_and_their_unicode() {
        let tricky = "a \"quoted\" line\twith a tab, an é and an emoji: 🎯";
        let text = Json::text(tricky).to_compact();
        assert_eq!(Json::parse(&text).unwrap().as_str().unwrap(), tricky);
        // Control characters are escaped on the way out and refused raw on the way in.
        let escaped = Json::text("x\u{1}y").to_compact();
        assert_eq!(escaped, "\"x\\u0001y\"");
        assert_eq!(Json::parse(&escaped).unwrap().as_str().unwrap(), "x\u{1}y");
    }

    #[test]
    fn a_raw_control_character_in_a_string_is_refused() {
        let bad = "\"line\nbreak\"";
        let e = Json::parse(bad).unwrap_err();
        assert!(
            e.message.contains("control character"),
            "unexpected message: {}",
            e.message
        );
    }

    #[test]
    fn errors_point_at_a_line_and_column() {
        let text = "{\n  \"a\": 1,\n  \"b\": tru\n}";
        let e = Json::parse(text).unwrap_err();
        // `tru` sits on the third line at the eighth character: the offset alone would not tell a
        // reader which line of an adapter's stream went wrong.
        assert!(e.message.contains("line 3"), "{}", e.message);
        assert!(e.message.contains("column 8"), "{}", e.message);
        assert!(e.message.contains("unexpected character"), "{}", e.message);
    }

    #[test]
    fn malformed_documents_are_rejected_rather_than_guessed() {
        for bad in [
            "",
            "{",
            "{\"a\"}",
            "{\"a\": }",
            "[1,2,]",
            "01",
            "truth",
            "{\"a\":1}{\"b\":2}",
            "\"unterminated",
            "\"\\q\"",
            "\"\\uZZZZ\"",
            "\"\\uD800\"",
        ] {
            assert!(Json::parse(bad).is_err(), "accepted: {bad:?}");
        }
    }

    #[test]
    fn a_lone_surrogate_escape_is_refused_not_patched() {
        // \uD83D without its partner would need the parser to guess what was meant.
        assert!(Json::parse("\"\\uD83D\"").is_err());
        // A proper pair is also refused: this parser takes code points, not UTF-16 sequences.
        assert!(Json::parse("\"\\uD83D\\uDE00\"").is_err());
    }

    #[test]
    fn deeply_nested_input_is_refused_before_it_recurses_the_stack_away() {
        let deep = "[".repeat(200) + &"]".repeat(200);
        let e = Json::parse(&deep).unwrap_err();
        assert!(e.message.contains("nested too deeply"), "{}", e.message);
        // One level inside the limit still parses.
        let ok = "[".repeat(64) + &"]".repeat(64);
        assert!(Json::parse(&ok).is_ok());
    }

    #[test]
    fn accessors_read_the_shapes_the_store_needs() {
        let doc =
            Json::parse(r#"{"n":7,"f":2.5,"s":"x","b":true,"a":[1,2],"o":{"k":"v"}}"#).unwrap();
        assert_eq!(doc.get("n").and_then(Json::as_u64), Some(7));
        assert_eq!(doc.get("f").and_then(Json::as_f64), Some(2.5));
        assert_eq!(doc.get("s").and_then(Json::as_str), Some("x"));
        assert_eq!(doc.get("b").and_then(Json::as_bool), Some(true));
        assert_eq!(
            doc.get("a").and_then(Json::as_array).map(<[Json]>::len),
            Some(2)
        );
        assert_eq!(
            doc.get("o").and_then(|o| o.get("k")).and_then(Json::as_str),
            Some("v")
        );
        assert!(doc.get("missing").is_none());
    }

    #[test]
    fn a_jsonl_line_parses_on_its_own() {
        let line = Json::object(vec![
            ("evaluation", Json::count(3)),
            ("family", Json::text("boundary")),
        ])
        .to_compact();
        let back = Json::parse_line(&line).unwrap();
        assert_eq!(back.get("family").and_then(Json::as_str), Some("boundary"));
    }
}
