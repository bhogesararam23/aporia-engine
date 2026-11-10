//! Read-only Git context from the SOURCE project.
//!
//! The sandbox copy deliberately omits `.git`, so revision questions cannot be answered inside
//! the workspace. This tool answers them from the source with a fixed, host-built argv:
//! `git --no-optional-locks` (which stops Git writing refresh data), a query from a closed set,
//! and an optional path that is validated and placed after `--` so it can never become an option.
//! The model chooses a query, never a command line. The source is not modified.

use super::result::{ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::json;
use crate::scope::Risk;
use aporia_sandbox::{ExecSpec, ExecStatus, exec_in};
use aporia_store::Json;
use std::path::{Component, Path};

const OUTPUT_LIMIT: usize = 16 * 1024;
const GIT_TIMEOUT_MS: u64 = 10_000;

/// Register the source Git tool.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(SourceGit::new()))
}

struct SourceGit {
    meta: ToolMeta,
}

impl SourceGit {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "source_git",
                "Read-only Git context from the original project (the workspace copy has no .git): head, status, diff --stat, or recent log. Fixed host-built commands; the source is never written.",
                r#"{"type":"object","properties":{"query":{"type":"string","enum":["head","status","diff","log"]},"path":{"type":"string","description":"optional relative file path to narrow diff or log"},"max_commits":{"type":"integer","minimum":1,"maximum":100}},"required":["query"],"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for SourceGit {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, arguments: &Json) -> ToolResult {
        let fields = crate::json::fields(arguments).expect("validated object");
        let query = json::string(fields, "query")?;
        let path = json::optional_string(fields, "path")?;
        if let Some(path) = path {
            validate_relative(path)?;
        }
        let max_commits = json::optional_u64(fields, "max_commits")?.unwrap_or(20);
        let source = context.source().to_string_lossy().to_string();
        let mut argv: Vec<String> = vec![
            "git".to_string(),
            "--no-optional-locks".to_string(),
            "-C".to_string(),
            source,
        ];
        match query {
            "head" => argv.extend(["rev-parse".to_string(), "HEAD".to_string()]),
            "status" => argv.extend(["status".to_string(), "--porcelain".to_string()]),
            "diff" => {
                argv.extend(["diff".to_string(), "--stat".to_string()]);
                if let Some(path) = path {
                    argv.push("--".to_string());
                    argv.push(path.to_string());
                }
            }
            "log" => {
                argv.extend([
                    "log".to_string(),
                    "-n".to_string(),
                    max_commits.to_string(),
                    "--format=%H %ad %s".to_string(),
                    "--date=short".to_string(),
                ]);
                if let Some(path) = path {
                    argv.push("--".to_string());
                    argv.push(path.to_string());
                }
            }
            other => {
                return Err(ToolFailure::new(
                    ErrorClass::InvalidArguments,
                    format!("query `{other}` is not one of head, status, diff, log"),
                ));
            }
        }
        let spec = ExecSpec::new(argv)
            .with_timeout(GIT_TIMEOUT_MS)
            .with_output_limit(OUTPUT_LIMIT as u64);
        let outcome = exec_in(context.source(), &spec);
        match outcome.status {
            ExecStatus::LaunchFailed(reason) => Err(ToolFailure::new(
                ErrorClass::Unsupported,
                format!(
                    "git could not be started, so source revision context is unavailable: {reason}"
                ),
            )),
            ExecStatus::Timeout => Err(ToolFailure::new(
                ErrorClass::Timeout,
                "git did not answer within 10 seconds",
            )),
            ExecStatus::Completed => {
                let code = outcome.exit_code.unwrap_or(-1);
                if code != 0 {
                    return Err(ToolFailure::new(
                        ErrorClass::Execution,
                        format!("git exited {code}: {}", outcome.stderr.text.trim()),
                    ));
                }
                Ok(ToolOutput {
                    text: outcome.stdout.text.trim_end().to_string(),
                    data: Json::object(vec![
                        ("query", Json::text(query)),
                        ("exit_code", Json::count(code.max(0) as u64)),
                        ("stdout", Json::text(outcome.stdout.text)),
                    ]),
                    artifacts: Vec::new(),
                })
            }
        }
    }
}

/// A path for `--`: relative, no parent components, no root. It goes after `--` anyway, but a
/// refusal here keeps the record clean of attempts.
fn validate_relative(path: &str) -> Result<(), ToolFailure> {
    if path.trim().is_empty() {
        return Err(ToolFailure::new(
            ErrorClass::InvalidArguments,
            "the path is empty",
        ));
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(ToolFailure::new(
                    ErrorClass::InvalidArguments,
                    format!("`{path}` is not a plain relative path"),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolCall;
    use crate::tools::{Registry, dispatch};
    use aporia_sandbox::Sandbox;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "aporia-sourcegit-{tag}-{}-{}",
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

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git command");
        assert!(
            status.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    fn run_query(source: &Path, sandbox: &Sandbox, arguments: &str) -> ToolResult {
        let mut registry = Registry::new();
        register(&mut registry).expect("register");
        let instrument = std::env::current_exe().expect("exe");
        let cancel = AtomicBool::new(false);
        let mut context = ToolContext::new(sandbox, source, &instrument, None, &cancel);
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "source_git".to_string(),
            arguments: arguments.to_string(),
        };
        dispatch(&registry, &mut context, &call)
    }

    fn sandbox_of(source: &Path, workspace: &Path) -> Sandbox {
        let (sandbox, _) = Sandbox::create(source, workspace).expect("sandbox");
        sandbox
    }

    #[test]
    fn a_non_repository_is_an_honest_execution_error() {
        if !git_available() {
            eprintln!("skipped: git is not installed");
            return;
        }
        let source = Temp::new("plain");
        std::fs::write(source.0.join("a.txt"), b"a").expect("file");
        let workspace = Temp::new("ws");
        let sandbox = sandbox_of(&source.0, &workspace.0.join("copy"));
        let failure = run_query(&source.0, &sandbox, r#"{"query":"head"}"#).expect_err("no repo");
        assert_eq!(failure.class, ErrorClass::Execution);
        assert!(
            failure.message.contains("not a git repository"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn head_status_and_log_describe_the_real_source() {
        if !git_available() {
            eprintln!("skipped: git is not installed");
            return;
        }
        let source = Temp::new("repo");
        git(&source.0, &["init", "-q"]);
        std::fs::write(source.0.join("a.txt"), b"first\n").expect("file");
        git(&source.0, &["add", "a.txt"]);
        git(
            &source.0,
            &[
                "-c",
                "user.name=aporia-test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "the first commit",
            ],
        );
        let workspace = Temp::new("ws2");
        let sandbox = sandbox_of(&source.0, &workspace.0.join("copy"));

        let head = run_query(&source.0, &sandbox, r#"{"query":"head"}"#).expect("head");
        assert_eq!(head.text.trim().len(), 40);
        assert!(head.text.trim().chars().all(|c| c.is_ascii_hexdigit()));

        let clean = run_query(&source.0, &sandbox, r#"{"query":"status"}"#).expect("status");
        assert!(clean.text.trim().is_empty());

        std::fs::write(source.0.join("a.txt"), b"changed\n").expect("modify");
        let dirty = run_query(&source.0, &sandbox, r#"{"query":"status"}"#).expect("status");
        assert!(dirty.text.contains("a.txt"), "{}", dirty.text);

        let log = run_query(&source.0, &sandbox, r#"{"query":"log"}"#).expect("log");
        assert!(log.text.contains("the first commit"));

        // The source was not written by any query: the change above is still the only one,
        // and no optional locks were taken.
        assert!(!source.0.join(".git").join("index.lock").exists());
    }

    #[test]
    fn a_parent_path_is_refused_before_git_sees_it() {
        let source = Temp::new("plain2");
        std::fs::write(source.0.join("a.txt"), b"a").expect("file");
        let workspace = Temp::new("ws3");
        let sandbox = sandbox_of(&source.0, &workspace.0.join("copy"));
        let failure = run_query(
            &source.0,
            &sandbox,
            r#"{"query":"log","path":"../outside"}"#,
        )
        .expect_err("escape");
        assert_eq!(failure.class, ErrorClass::InvalidArguments);
    }
}
