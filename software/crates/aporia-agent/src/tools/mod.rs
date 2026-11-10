//! The host-owned tool system.
//!
//! Every capability the model can use is a registered [`Tool`] with a stable name, a typed
//! argument schema, a risk class and execution limits. The schemas the model receives are
//! generated from the registry ([`Registry::schemas`]); every call passes argument validation and
//! the permission check before it runs. A command written in prose is not a command that runs:
//! dispatch is the only path from a model request to an effect.

pub mod command;
pub mod context;
pub mod control;
pub mod edit;
pub mod files;
pub mod instrument;
pub mod intel;
pub mod invocations;
pub mod meta;
pub mod registry;
pub mod result;
pub mod results;
pub mod schema;
pub mod source_git;

pub use context::{Session, ToolContext};
pub use invocations::{Begin, InvocationTable};
pub use meta::ToolMeta;
pub use registry::Registry;
pub use result::{ArtifactRef, ErrorClass, ToolFailure, ToolOutput, ToolResult};

use crate::provider::ToolCall;
use aporia_store::Json;

/// One capability the host offers the model.
///
/// Implementations are stateless with respect to the session: everything they may touch arrives
/// through the [`ToolContext`], so the registry owns the tools and the engine owns the state.
pub trait Tool {
    fn meta(&self) -> &ToolMeta;

    /// Run with arguments the host has already parsed and validated against the schema. A
    /// permission refusal the schema cannot express (an unapproved program, a write outside the
    /// approved scope) is a [`ErrorClass::Denied`] failure, not a panic and not a silent no-op.
    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult;
}

/// Validate and run one model call. The result is what the model sees, with its call id attached
/// by the engine; failures are classified so the journal and the model both know what kind of
/// refusal or error happened.
pub fn dispatch(registry: &Registry, context: &mut ToolContext, call: &ToolCall) -> ToolResult {
    let Some(tool) = registry.get(&call.name) else {
        return Err(ToolFailure::new(
            ErrorClass::UnknownTool,
            format!("`{}` is not a tool this session offers", call.name),
        ));
    };
    if context.cancelled() {
        return Err(ToolFailure::new(
            ErrorClass::Cancelled,
            "the investigation was cancelled before this call ran",
        ));
    }
    let arguments = Json::parse(&call.arguments).map_err(|error| {
        ToolFailure::new(
            ErrorClass::InvalidArguments,
            format!("the arguments are not JSON: {error}"),
        )
    })?;
    schema::validate(&tool.meta().parameters, &arguments, "arguments")
        .map_err(|reason| ToolFailure::new(ErrorClass::InvalidArguments, reason))?;
    context.check_risk(tool.meta().risk)?;
    let mut output = tool.run(context, &arguments)?;
    output.text = cap_output(&output.text, tool.meta().max_output_bytes);
    Ok(output)
}

/// Enforce the tool's declared output limit. The cut is on a character boundary and says it
/// happened, so a bounded view is never mistaken for the whole output.
fn cap_output(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[output truncated at {limit} bytes]", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolCall;
    use crate::scope::{Risk, Scope};
    use aporia_sandbox::Sandbox;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(std::path::PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "aporia-dispatch-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("temp");
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Echo {
        meta: ToolMeta,
    }

    impl Echo {
        fn new(risk: Risk) -> Self {
            Self {
                meta: ToolMeta::new(
                    "echo",
                    "return the message",
                    r#"{"type":"object","properties":{"message":{"type":"string","minLength":1}},"required":["message"],"additionalProperties":false}"#,
                    risk,
                    4096,
                ),
            }
        }
    }

    impl Tool for Echo {
        fn meta(&self) -> &ToolMeta {
            &self.meta
        }

        fn run(&self, _context: &mut ToolContext, arguments: &Json) -> ToolResult {
            let fields = crate::json::fields(arguments).expect("validated object");
            let message = crate::json::string(fields, "message").expect("validated string");
            Ok(ToolOutput::text(message))
        }
    }

    fn call(arguments: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            name: "echo".to_string(),
            arguments: arguments.to_string(),
        }
    }

    fn fixture() -> (Temp, Temp, Sandbox, std::path::PathBuf) {
        let source = Temp::new("src");
        std::fs::write(source.0.join("a.txt"), b"a").expect("file");
        let workspace = Temp::new("ws");
        let (sandbox, _) = Sandbox::create(&source.0, &workspace.0.join("copy")).expect("sandbox");
        let instrument = std::env::current_exe().expect("exe");
        (source, workspace, sandbox, instrument)
    }

    #[test]
    fn a_valid_call_reaches_the_tool() {
        let (source, _workspace, sandbox, instrument) = fixture();
        let cancel = AtomicBool::new(false);
        let mut context = ToolContext::new(&sandbox, &source.0, &instrument, None, &cancel);
        let mut registry = Registry::new();
        registry
            .register(Box::new(Echo::new(Risk::ReadOnly)))
            .expect("register");
        let result = dispatch(&registry, &mut context, &call(r#"{"message":"hello"}"#));
        assert_eq!(result.expect("ok").text, "hello");
    }

    #[test]
    fn unknown_tools_bad_json_and_schema_violations_are_classified() {
        let (source, _workspace, sandbox, instrument) = fixture();
        let cancel = AtomicBool::new(false);
        let mut context = ToolContext::new(&sandbox, &source.0, &instrument, None, &cancel);
        let mut registry = Registry::new();
        registry
            .register(Box::new(Echo::new(Risk::ReadOnly)))
            .expect("register");

        let mut unknown = call("{}");
        unknown.name = "no_such_tool".to_string();
        let failure = dispatch(&registry, &mut context, &unknown).expect_err("unknown");
        assert_eq!(failure.class, ErrorClass::UnknownTool);

        let failure = dispatch(&registry, &mut context, &call("not json")).expect_err("json");
        assert_eq!(failure.class, ErrorClass::InvalidArguments);

        let failure =
            dispatch(&registry, &mut context, &call(r#"{"wrong":"field"}"#)).expect_err("schema");
        assert_eq!(failure.class, ErrorClass::InvalidArguments);

        let failure =
            dispatch(&registry, &mut context, &call(r#"{"message":""}"#)).expect_err("empty");
        assert_eq!(failure.class, ErrorClass::InvalidArguments);
    }

    #[test]
    fn an_unapproved_risk_is_denied_before_the_tool_runs() {
        let (source, _workspace, sandbox, instrument) = fixture();
        let cancel = AtomicBool::new(false);
        let mut context = ToolContext::new(&sandbox, &source.0, &instrument, None, &cancel);
        let mut registry = Registry::new();
        registry
            .register(Box::new(Echo::new(Risk::WorkspaceWrite)))
            .expect("register");
        let failure =
            dispatch(&registry, &mut context, &call(r#"{"message":"hello"}"#)).expect_err("denied");
        assert_eq!(failure.class, ErrorClass::Denied);

        let scope = Scope {
            allow_exec: Vec::new(),
            allow_write: true,
            max_turns: 8,
            max_tool_calls: 16,
            timeout_ms: 60_000,
        };
        let mut approved =
            ToolContext::new(&sandbox, &source.0, &instrument, Some(&scope), &cancel);
        assert!(dispatch(&registry, &mut approved, &call(r#"{"message":"hello"}"#)).is_ok());
    }

    #[test]
    fn a_cancelled_session_runs_nothing() {
        let (source, _workspace, sandbox, instrument) = fixture();
        let cancel = AtomicBool::new(true);
        let mut context = ToolContext::new(&sandbox, &source.0, &instrument, None, &cancel);
        let mut registry = Registry::new();
        registry
            .register(Box::new(Echo::new(Risk::ReadOnly)))
            .expect("register");
        let failure =
            dispatch(&registry, &mut context, &call(r#"{"message":"hello"}"#)).expect_err("cancel");
        assert_eq!(failure.class, ErrorClass::Cancelled);
    }

    #[test]
    fn duplicate_registration_is_refused_and_schemas_come_from_the_registry() {
        let mut registry = Registry::new();
        registry
            .register(Box::new(Echo::new(Risk::ReadOnly)))
            .expect("first");
        assert!(
            registry
                .register(Box::new(Echo::new(Risk::ReadOnly)))
                .is_err()
        );
        let schemas = registry.schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "echo");
        assert_eq!(registry.names(), vec!["echo"]);
    }

    #[test]
    fn the_declared_output_limit_is_enforced_and_says_so() {
        let (source, _workspace, sandbox, instrument) = fixture();
        let cancel = AtomicBool::new(false);
        let mut context = ToolContext::new(&sandbox, &source.0, &instrument, None, &cancel);
        let mut registry = Registry::new();
        registry
            .register(Box::new(Echo::new(Risk::ReadOnly)))
            .expect("register");
        let long = "x".repeat(5000);
        let arguments = format!(r#"{{"message":"{long}"}}"#);
        let output = dispatch(&registry, &mut context, &call(&arguments)).expect("ok");
        assert!(output.text.len() < 5000);
        assert!(output.text.contains("[output truncated at 4096 bytes]"));
    }
}
