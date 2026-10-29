//! The words the orchestrator sends. Kept apart so the contract can be read without the state
//! machine around it, and so a change in what the model is told is one diff.

use crate::execute::TaskResult;
use crate::inspect::Inspection;
use crate::plan::MAX_BUDGET;
use crate::provider::ToolSchema;
use aporia_store::Json;
use std::fmt::Write as _;
use std::path::Path;

const PROMPT_FACTS_LIMIT: usize = 64 * 1024;
const STREAM_LIMIT: usize = 16 * 1024;

/// The rules and the current permissions.
#[must_use]
pub fn system(allow_exec: &[String]) -> String {
    let exec = if allow_exec.is_empty() {
        "You may not run project programs: no --allow-exec was given, so any exec task will be \
         rejected."
            .to_string()
    } else {
        format!(
            "Project programs you may request with an exec task: {}. A program named in any other \
             way is rejected.",
            allow_exec.join(", ")
        )
    };
    format!(
        "You are APORIA's investigator, a bounded research agent. The instrument is a separate \
         deterministic tool; the host runs it for you and returns its captured output and archive.\n\
         \n\
         You are in one of two phases. In the first, read files and propose a plan. The host will \
         print the plan to a person, who approves or refuses it; nothing runs before approval. In \
         the second, the host has already executed the approved tasks. Read the recorded results, \
         record findings, suggest follow-up experiments, and finish.\n\
         \n\
         Rules:\n\
         - Every gap must cite inspection fact ids (F1, F2, ...). Every finding must cite fact or \
           task-result ids (F1 or R1) that the host recorded. Unknown ids are rejected.\n\
         - Priorities are exactly: critical (a verified defect or invalidating condition blocking \
           the intended conclusion), high (an unresolved question likely to affect the result), \
           recommended (a meaningful improvement supported by the current state), exploratory (a \
           plausible direction whose benefit is uncertain). State the condition that makes a step \
           necessary; standard practice alone is not a reason.\n\
         - A plan has 1 to 16 gaps and 1 to 16 tasks. Task tools are instrument_run (model path, \
           budget 1..={MAX_BUDGET}) and exec (argv, optional timeout_ms, allowed programs only). \
           Tasks may depend on earlier tasks.\n\
         - A finding's kind is observation (what the recorded output shows), interpretation (what \
           you infer from it), or hypothesis (a plausible but untested explanation). Severity is \
           critical, high, moderate or info. Say what remains uncertain.\n\
         - Never invent a result, a number, or an execution. Completing a run does not confirm a \
           hypothesis. If the evidence does not settle a question, record that as unresolved.\n\
         - finish requires a summary, next steps and an explicit unresolved list. Use it in the \
           first phase only when no investigation is warranted.\n\
         - Call exactly one tool at a time, with arguments that are one JSON object and no unknown \
           fields. Text without a tool call is an error the host will return to you.\n\
         - {exec}\n\
         - The investigation record is finite: turn, tool-call and wall-clock budgets are enforced. \
           If they run out, the record says the investigation is incomplete. Do not race the budget."
    )
}

/// The objective, the facts and the phase instruction.
#[must_use]
pub fn initial_user(objective: &str, root: &Path, inspection: &Inspection) -> String {
    let mut out = format!(
        "Objective:\n{objective}\n\nProject root: {}\n\nInspection facts (ids are citable):\n",
        root.display()
    );
    out.push_str(&inspection.rendered(PROMPT_FACTS_LIMIT));
    let _ = write!(
        out,
        "\nScan summary: {} entries, {} files, {} bytes, truncated: {}. Skipped: {}\n",
        inspection.summary.entries,
        inspection.summary.files,
        inspection.summary.bytes,
        inspection.summary.truncated,
        if inspection.summary.skipped.is_empty() {
            "none".to_string()
        } else {
            inspection.summary.skipped.join(", ")
        }
    );
    if inspection.summary.truncated {
        out.push_str(
            "The scan was truncated, so the facts are not a claim that anything absent is missing.\n",
        );
    }
    out.push_str(
        "\nPropose a plan grounded in these facts, or call finish if no investigation is warranted. \
         You cannot execute anything; approval comes first.",
    );
    out
}

/// The recorded results, presented as observations rather than conclusions.
#[must_use]
pub fn results(results: &[TaskResult]) -> String {
    let mut out = String::from(
        "The approved tasks ran. These are recorded observations, not conclusions; completing a \
         task does not confirm anything. Cite result ids (R1, ...) and fact ids in findings.\n\n",
    );
    for result in results {
        let _ = writeln!(
            out,
            "{} task {} ({}) status={} exit={} duration_ms={}",
            result.id,
            result.task_id,
            result.tool,
            result.status,
            result
                .exit_code
                .map_or_else(|| "none".to_string(), |code| code.to_string()),
            result.duration_ms
        );
        if !result.note.is_empty() {
            let _ = writeln!(out, "note: {}", result.note);
        }
        let _ = writeln!(
            out,
            "stdout ({} bytes, sha256 {}, truncated {}):\n{}",
            result.stdout.bytes,
            result.stdout.sha256,
            result.stdout.truncated,
            cap(&result.stdout.text, STREAM_LIMIT)
        );
        if !result.stderr.text.is_empty() {
            let _ = writeln!(
                out,
                "stderr ({} bytes, sha256 {}, truncated {}):\n{}",
                result.stderr.bytes,
                result.stderr.sha256,
                result.stderr.truncated,
                cap(&result.stderr.text, STREAM_LIMIT)
            );
        }
        for artifact in &result.artifacts {
            let _ = writeln!(
                out,
                "artifact {} at {} (sha256 {}, {} files)",
                artifact.kind, artifact.path, artifact.sha256, artifact.files
            );
        }
        out.push('\n');
    }
    out.push_str(
        "Record findings with record_finding, optionally suggest_experiment for follow-up work \
         that was not run, then call finish with a summary, next steps and unresolved questions.",
    );
    out
}

fn cap(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[output truncated for this message]", &text[..end])
}

/// The tool schemas, which are also the record of what the model was offered.
#[must_use]
pub fn tools() -> Vec<ToolSchema> {
    vec![
        tool(
            "read_file",
            "Read at most 32 KiB of one relative path inside the sandbox copy.",
            r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
        ),
        tool(
            "propose_plan",
            "Propose gaps and an ordered plan. The host validates every evidence id and task argument before a person sees it.",
            r#"{"type":"object","properties":{"gaps":{"type":"array","items":{"type":"object"}},"tasks":{"type":"array","items":{"type":"object"}}},"required":["gaps","tasks"],"additionalProperties":false}"#,
        ),
        tool(
            "record_finding",
            "Record one grounded statement. Evidence must be fact or task-result ids the host recorded.",
            r#"{"type":"object","properties":{"kind":{"type":"string","enum":["observation","interpretation","hypothesis"]},"severity":{"type":"string","enum":["critical","high","moderate","info"]},"text":{"type":"string"},"evidence":{"type":"array","items":{"type":"string"}},"uncertainty":{"type":"string"}},"required":["kind","severity","text","evidence","uncertainty"],"additionalProperties":false}"#,
        ),
        tool(
            "suggest_experiment",
            "Suggest a follow-up experiment without running it.",
            r#"{"type":"object","properties":{"question":{"type":"string"},"why":{"type":"string"},"method":{"type":"string"},"estimated_cost":{"type":"string"}},"required":["question","why","method","estimated_cost"],"additionalProperties":false}"#,
        ),
        tool(
            "finish",
            "End the investigation with a summary, next steps and an explicit unresolved list. In the first phase this means no investigation is warranted.",
            r#"{"type":"object","properties":{"summary":{"type":"string"},"next_steps":{"type":"array","items":{"type":"string"}},"unresolved":{"type":"array","items":{"type":"string"}}},"required":["summary","next_steps","unresolved"],"additionalProperties":false}"#,
        ),
    ]
}

fn tool(name: &str, description: &str, parameters: &str) -> ToolSchema {
    ToolSchema {
        name: name.to_string(),
        description: description.to_string(),
        parameters: Json::parse(parameters).expect("the built-in tool schema is valid JSON"),
    }
}
