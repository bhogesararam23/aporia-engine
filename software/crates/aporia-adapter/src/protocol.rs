//! The wire format between APORIA and a program it did not parse.
//!
//! One JSON object per line in each direction, request first, response next, in order. Nothing else:
//! no negotiation, no framing lengths, no session. A protocol this small is implementable in an
//! afternoon in any language that can read a line and print a line, which matters because the point of
//! the boundary is that the program is someone else's.
//!
//! ```text
//! --> {"x":[12.5]}
//! <-- {"y":[2.1]}
//! <-- {"y":["NaN"]}
//! <-- {"steps":1,"y":[2.1]}
//! <-- {"error":"matrix was singular"}
//! ```
//!
//! The number encoding is `aporia_store::Json`'s, the same one the archives use: the shortest decimal
//! string that reads back as the identical `f64`, with the three non-finite values written as the
//! strings `"NaN"`, `"Infinity"` and `"-Infinity"`. Reusing it is not laziness. It means a parameter
//! value that crosses this boundary and lands in an archive is the *same bits* in both places, so
//! replay can compare them exactly — and it means one definition of "what this number is" rather than
//! two that a reader has to reconcile.
//!
//! Two kinds of non-answer exist and they are deliberately different:
//!
//!   - `"NaN"` / `"Infinity"` is a **value**. The program computed something and the result left the
//!     real numbers. APORIA reads it, raises the corresponding flag, and the divergence channel treats
//!     it as evidence about that region — which is the whole reason this instrument exists.
//!   - `{"error": "..."}` is a **refusal**. The program declines to answer at all. That voids the run
//!     rather than becoming a NaN, because APORIA cannot tell "this point is unreachable" from "my
//!     build is broken", and letting a campaign silently fill a map with unanswered points is how a
//!     measurement becomes a fiction. A program that means the first thing returns `"NaN"`.

use std::fmt::Write as _;

/// A program's answer for one point.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    /// One value per declared output, in declaration order.
    pub y: Vec<f64>,
    /// Work the program did for this point, in whatever unit it counts. Absent means the program
    /// reported nothing, which is recorded as 0 rather than guessed at: an invented cost would
    /// corrupt the one metric this project compares across runs.
    pub steps: u64,
}

/// Why a line cannot be read as the protocol, or is a refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    /// Not one JSON object, or not an object at all.
    Malformed(String),
    /// A required field was absent.
    MissingField(&'static str),
    /// A field was present but not the kind the protocol says it is.
    WrongType { field: &'static str, found: String },
    /// The number of values did not match the declared outputs. Carrying both numbers is what makes
    /// the message actionable: this is almost always a model file and a program that disagree about
    /// how many quantities there are.
    Arity { found: usize, expected: usize },
    /// The program refused to answer this point. Distinct from every other variant because it is the
    /// program's own statement rather than a framing mistake, and a report should quote it.
    Refused { message: String },
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "not a valid protocol line: {why}"),
            Self::MissingField(name) => write!(f, "the line carries no `{name}` field"),
            Self::WrongType { field, found } => {
                write!(
                    f,
                    "`{field}` is {found}, and the protocol wants something else"
                )
            }
            Self::Arity { found, expected } => write!(
                f,
                "the program answered {found} value(s) for a model that declares {expected}"
            ),
            Self::Refused { message } => write!(f, "the program refused to answer: {message}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// One request line. `x` is in the units the parameters were declared in.
#[must_use]
pub fn encode_request(x: &[f64]) -> String {
    aporia_store::Json::object(vec![(
        "x",
        aporia_store::Json::Arr(x.iter().map(|v| aporia_store::Json::number(*v)).collect()),
    )])
    .to_compact()
}

/// One response line. `steps` is omitted when the program has no work count to report, because a
/// field that means "unknown" and a field that means "zero" should not be confused.
#[must_use]
pub fn encode_response(answer: &Answer) -> String {
    let y = answer
        .y
        .iter()
        .map(|v| aporia_store::Json::number(*v))
        .collect();
    let mut fields: Vec<(&str, aporia_store::Json)> = vec![("y", aporia_store::Json::Arr(y))];
    if answer.steps > 0 {
        fields.push(("steps", aporia_store::Json::count(answer.steps)));
    }
    aporia_store::Json::object(fields).to_compact()
}

/// The refusal a program writes when it will not answer.
#[must_use]
pub fn encode_refusal(message: &str) -> String {
    aporia_store::Json::object(vec![("error", aporia_store::Json::text(message))]).to_compact()
}

/// Read a request line, expecting `arity` parameters. Used by the program side, so the same decoder
/// that APORIA's counterpart uses is the one a foreign program gets to link against.
pub fn decode_request(line: &str, arity: usize) -> Result<Vec<f64>, ProtocolError> {
    let value = parse_object(line)?;
    let x = field(&value, "x")?;
    read_numbers(x, "x", arity)
}

/// Read a response line, expecting one value per declared output.
pub fn decode_response(line: &str, outputs: usize) -> Result<Answer, ProtocolError> {
    let value = parse_object(line)?;
    // A refusal is checked before anything else: it is a well-formed object that means "no answer",
    // and reporting it as a missing `y` would send the reader to the wrong place.
    if let Some(err) = value.get("error") {
        let message = match err {
            aporia_store::Json::Str(s) => s.clone(),
            other => other.to_compact(),
        };
        return Err(ProtocolError::Refused { message });
    }
    let y = field(&value, "y")?;
    let steps = match value.get("steps") {
        None => 0,
        Some(aporia_store::Json::Int(v)) => *v,
        // A float step count is not a refusal — a program that counts in seconds will write one — but
        // it is not a cost this project can add up either, so the line is rejected rather than rounded.
        Some(other) => {
            return Err(ProtocolError::WrongType {
                field: "steps",
                found: kind_of(other).to_string(),
            });
        }
    };
    Ok(Answer {
        y: read_numbers(y, "y", outputs)?,
        steps,
    })
}

fn parse_object(line: &str) -> Result<aporia_store::Json, ProtocolError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(ProtocolError::Malformed("empty line".to_string()));
    }
    let value = aporia_store::Json::parse_line(trimmed)
        .map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    if !matches!(value, aporia_store::Json::Obj(_)) {
        return Err(ProtocolError::Malformed(format!(
            "expected one object, found {}",
            kind_of(&value)
        )));
    }
    Ok(value)
}

fn field<'a>(
    value: &'a aporia_store::Json,
    name: &'static str,
) -> Result<&'a aporia_store::Json, ProtocolError> {
    value.get(name).ok_or(ProtocolError::MissingField(name))
}

/// An array of numbers, with the three non-finite strings accepted, because they are how the protocol
/// writes a value that is not a real number.
fn read_numbers(
    value: &aporia_store::Json,
    field_name: &'static str,
    expected: usize,
) -> Result<Vec<f64>, ProtocolError> {
    let Some(items) = value.as_array() else {
        return Err(ProtocolError::WrongType {
            field: field_name,
            found: kind_of(value).to_string(),
        });
    };
    if items.len() != expected {
        return Err(ProtocolError::Arity {
            found: items.len(),
            expected,
        });
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(read_number(item, field_name)?);
    }
    Ok(out)
}

fn read_number(value: &aporia_store::Json, field_name: &'static str) -> Result<f64, ProtocolError> {
    match value {
        aporia_store::Json::Num(v) => Ok(*v),
        // An integer literal is a number too: `{"y":[0]}` says zero, and refusing it would make
        // every program that has no fractional part write `0.0` to satisfy a parser.
        aporia_store::Json::Int(v) => Ok(*v as f64),
        aporia_store::Json::Str(s) => match s.as_str() {
            "NaN" => Ok(f64::NAN),
            "Infinity" => Ok(f64::INFINITY),
            "-Infinity" => Ok(f64::NEG_INFINITY),
            other => Err(ProtocolError::WrongType {
                field: field_name,
                found: format!("the string {other:?}"),
            }),
        },
        other => Err(ProtocolError::WrongType {
            field: field_name,
            found: kind_of(other).to_string(),
        }),
    }
}

fn kind_of(value: &aporia_store::Json) -> &'static str {
    match value {
        aporia_store::Json::Null => "null",
        aporia_store::Json::Bool(_) => "a boolean",
        aporia_store::Json::Num(_) => "a number",
        aporia_store::Json::Int(_) => "an integer",
        aporia_store::Json::Str(_) => "a string",
        aporia_store::Json::Arr(_) => "an array",
        aporia_store::Json::Obj(_) => "an object",
    }
}

/// A short, single-line description of the protocol for a `--help` output or a manifest note.
#[must_use]
pub fn describe() -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        "one JSON object per line, request then response: {}\n\
         response fields: y (one value per declared output; NaN, Infinity and -Infinity are written \
         as strings), steps (optional work count), error (a refusal, which voids the run)",
        encode_request(&[0.0]),
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_reads_back_as_the_values_it_carried() {
        let line = encode_request(&[12.5, -0.25, 0.0]);
        assert_eq!(decode_request(&line, 3).unwrap(), vec![12.5, -0.25, 0.0]);
        assert!(line.ends_with('}') && !line.contains('\n'), "{line}");
    }

    #[test]
    fn a_round_trip_survives_a_number_that_needs_every_digit() {
        // 0.1 is the classic, and one-third checks the other direction. If the encoding rounded, the
        // value that reaches the atlas would not be the value the archive stored.
        for v in [0.1f64, 1.0 / 3.0, 1e300, 5e-324, -1_000_000.000_001] {
            let line = encode_request(&[v]);
            let back = decode_request(&line, 1).unwrap();
            assert_eq!(back[0].to_bits(), v.to_bits(), "line {line} for {v}");
        }
    }

    #[test]
    fn a_non_finite_answer_is_a_value_and_reads_back_as_one() {
        let line = encode_response(&Answer {
            y: vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY],
            steps: 0,
        });
        assert!(line.contains("\"NaN\""), "{line}");
        let back = decode_response(&line, 3).unwrap();
        assert!(back.y[0].is_nan());
        assert_eq!(back.y[1], f64::INFINITY);
        assert_eq!(back.y[2], f64::NEG_INFINITY);
    }

    #[test]
    fn an_absent_step_count_is_zero_and_a_fractional_one_is_refused() {
        let line = r#"{"y":[2.1]}"#;
        assert_eq!(decode_response(line, 1).unwrap().steps, 0);
        let line = r#"{"y":[2.1],"steps":3}"#;
        assert_eq!(decode_response(line, 1).unwrap().steps, 3);
        assert_eq!(
            decode_response(r#"{"y":[2.1],"steps":1.5}"#, 1),
            Err(ProtocolError::WrongType {
                field: "steps",
                found: "a number".to_string()
            })
        );
    }

    #[test]
    fn a_refusal_is_reported_as_a_refusal() {
        let line = encode_refusal("matrix was singular");
        assert_eq!(
            decode_response(&line, 1),
            Err(ProtocolError::Refused {
                message: "matrix was singular".to_string()
            })
        );
        // And it wins over the missing-field reading, because the message a reader acts on is the
        // program's, not the protocol's.
        assert_eq!(
            decode_response(r#"{"error":"no answer","y":[1.0]}"#, 1),
            Err(ProtocolError::Refused {
                message: "no answer".to_string()
            })
        );
    }

    #[test]
    fn a_wrong_arity_names_both_numbers() {
        assert_eq!(
            decode_response(r#"{"y":[1.0,2.0]}"#, 1),
            Err(ProtocolError::Arity {
                found: 2,
                expected: 1
            })
        );
        assert_eq!(
            decode_request(r#"{"x":[1.0]}"#, 2),
            Err(ProtocolError::Arity {
                found: 1,
                expected: 2
            })
        );
    }

    #[test]
    fn malformed_lines_are_refused_rather_than_guessed_at() {
        for line in [
            "",
            "   ",
            "not json",
            "[1.0,2.0]",
            "null",
            r#"{"y":2.1}"#,
            r#"{"x":[1.0],"junk"}"#,
            r#"{"y":[1.0]} trailing"#,
        ] {
            let response = decode_response(line, 1);
            assert!(response.is_err(), "{line} read as {response:?}");
            // A malformed line is never reported as a refusal: those two mean different things to a
            // reader, and the second says the program is working and unhappy.
            assert!(
                !matches!(response, Err(ProtocolError::Refused { .. })),
                "{line} read as a refusal"
            );
        }
    }

    #[test]
    fn the_encoding_matches_what_the_archives_write() {
        // The claim in the module docs, checked: one number encoding on both sides of the boundary, so
        // a value that crosses it is the same value in the archive.
        let answer = Answer {
            y: vec![1.0 / 3.0],
            steps: 7,
        };
        let line = encode_response(&answer);
        let stored = aporia_store::Json::object(vec![
            (
                "y",
                aporia_store::Json::Arr(vec![aporia_store::Json::number(1.0 / 3.0)]),
            ),
            ("steps", aporia_store::Json::count(7)),
        ])
        .to_compact();
        assert_eq!(line, stored);
    }

    #[test]
    fn integers_are_accepted_where_a_number_is_expected() {
        // A Fortran or C program writes `0`, not `0.0`. Refusing that would be a protocol that only
        // works in languages with one float literal syntax.
        let back = decode_response(r#"{"y":[0]}"#, 1).unwrap();
        assert_eq!(back.y, vec![0.0]);
        assert_eq!(decode_request(r#"{"x":[3]}"#, 1).unwrap(), vec![3.0]);
    }
}
