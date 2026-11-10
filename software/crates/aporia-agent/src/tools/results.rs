//! Reading and comparing result files.
//!
//! An experiment's output is evidence, and evidence is only useful if it can be read precisely
//! and compared with what it was before. These tools parse a JSON result file and report it
//! structurally, and diff two of them path by path -- which is how the agent compares a run
//! before a change with the run after it, instead of asserting that the change helped.

use super::result::{ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::json;
use crate::scope::Risk;
use aporia_store::Json;
use std::fmt::Write as _;
use std::fs;

const OUTPUT_LIMIT: usize = 32 * 1024;
const MAX_RESULT_BYTES: u64 = 1024 * 1024;
const MAX_DIFFERENCES: usize = 200;

/// Register the result tools.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(ReadResults::new()))?;
    registry.register(Box::new(CompareResults::new()))?;
    Ok(())
}

fn read_result(context: &ToolContext, name: &str) -> Result<Json, ToolFailure> {
    let path = context
        .sandbox()
        .resolve(name)
        .map_err(|error| ToolFailure::new(ErrorClass::Denied, format!("{name}: {error}")))?;
    if !path.is_file() {
        return Err(ToolFailure::new(
            ErrorClass::NotFound,
            format!("`{name}` is not a file in the workspace"),
        ));
    }
    let metadata = fs::metadata(&path).map_err(|error| {
        ToolFailure::new(ErrorClass::Io, format!("cannot stat `{name}`: {error}"))
    })?;
    if metadata.len() > MAX_RESULT_BYTES {
        return Err(ToolFailure::new(
            ErrorClass::Unsupported,
            format!("`{name}` is over 1 MiB; result reading supports files up to 1 MiB"),
        ));
    }
    let text = fs::read_to_string(&path).map_err(|error| {
        ToolFailure::new(ErrorClass::Io, format!("cannot read `{name}`: {error}"))
    })?;
    Json::parse(&text).map_err(|error| {
        ToolFailure::new(
            ErrorClass::Unsupported,
            format!("`{name}` is not a JSON document: {error}"),
        )
    })
}

struct ReadResults {
    meta: ToolMeta,
}

impl ReadResults {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "read_results",
                "Read a JSON result file from the workspace and return it parsed and structured.",
                r#"{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for ReadResults {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let name = json::string(fields, "path")?;
        let value = read_result(context, name)?;
        let mut text = value.to_pretty();
        if text.len() > OUTPUT_LIMIT {
            text.truncate(OUTPUT_LIMIT);
            let _ = write!(text, "\n[result truncated]");
        }
        Ok(ToolOutput {
            text,
            data: value,
            artifacts: Vec::new(),
        })
    }
}

struct CompareResults {
    meta: ToolMeta,
}

impl CompareResults {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "compare_results",
                "Compare two JSON result files in the workspace and report every differing path, with its two values.",
                r#"{"type":"object","properties":{"left":{"type":"string","minLength":1},"right":{"type":"string","minLength":1}},"required":["left","right"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for CompareResults {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let left_name = json::string(fields, "left")?;
        let right_name = json::string(fields, "right")?;
        let left = read_result(context, left_name)?;
        let right = read_result(context, right_name)?;
        let mut differences = Vec::new();
        compare(&left, &right, "$", &mut differences);
        let truncated = differences.len() > MAX_DIFFERENCES;
        differences.truncate(MAX_DIFFERENCES);
        let mut text = String::new();
        for difference in &differences {
            let _ = writeln!(
                text,
                "{}: {} -> {}",
                difference.path,
                difference.left.to_compact(),
                difference.right.to_compact()
            );
        }
        if differences.is_empty() {
            text.push_str("the two results are identical\n");
        }
        if truncated {
            let _ = writeln!(text, "[the comparison was truncated]");
        }
        Ok(ToolOutput {
            text,
            data: Json::object(vec![
                (
                    "differences",
                    Json::Arr(
                        differences
                            .iter()
                            .map(|difference| {
                                Json::object(vec![
                                    ("path", Json::text(&difference.path)),
                                    ("left", difference.left.clone()),
                                    ("right", difference.right.clone()),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("identical", Json::Bool(differences.is_empty())),
            ]),
            artifacts: Vec::new(),
        })
    }
}

/// One differing path.
struct Difference {
    path: String,
    left: Json,
    right: Json,
}

/// Walk two JSON documents together. A difference is a path whose value differs, or a path
/// present on one side only.
fn compare(left: &Json, right: &Json, path: &str, out: &mut Vec<Difference>) {
    if out.len() >= MAX_DIFFERENCES {
        return;
    }
    match (left, right) {
        (Json::Obj(left_fields), Json::Obj(right_fields)) => {
            let mut names: Vec<&String> = left_fields
                .iter()
                .map(|(name, _)| name)
                .chain(right_fields.iter().map(|(name, _)| name))
                .collect();
            names.sort_unstable();
            names.dedup();
            for name in names {
                let child = format!("{path}.{name}");
                match (
                    left_fields
                        .iter()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value),
                    right_fields
                        .iter()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value),
                ) {
                    (Some(left_value), Some(right_value)) => {
                        compare(left_value, right_value, &child, out);
                    }
                    (Some(left_value), None) => out.push(Difference {
                        path: child,
                        left: left_value.clone(),
                        right: Json::Null,
                    }),
                    (None, Some(right_value)) => out.push(Difference {
                        path: child,
                        left: Json::Null,
                        right: right_value.clone(),
                    }),
                    (None, None) => {}
                }
            }
        }
        _ => {
            if left != right {
                out.push(Difference {
                    path: path.to_string(),
                    left: left.clone(),
                    right: right.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differences_are_reported_by_path() {
        let left = Json::parse(r#"{"a":1,"b":{"c":2},"d":3}"#).expect("json");
        let right = Json::parse(r#"{"a":1,"b":{"c":5},"e":4}"#).expect("json");
        let mut differences = Vec::new();
        compare(&left, &right, "$", &mut differences);
        let paths: Vec<&str> = differences
            .iter()
            .map(|difference| difference.path.as_str())
            .collect();
        assert_eq!(paths, vec!["$.b.c", "$.d", "$.e"]);
    }

    #[test]
    fn identical_documents_have_no_differences() {
        let left = Json::parse(r#"{"a":[1,2,3],"b":"x"}"#).expect("json");
        let right = Json::parse(r#"{"a":[1,2,3],"b":"x"}"#).expect("json");
        let mut differences = Vec::new();
        compare(&left, &right, "$", &mut differences);
        assert!(differences.is_empty());
    }
}
