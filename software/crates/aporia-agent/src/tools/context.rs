//! Everything a tool may touch, handed to it by the host.
//!
//! A tool never reaches for global state: the workspace, the source, the approved scope and the
//! cancellation flag arrive here, so the same registry can serve a batch run, an interactive
//! session and a test with different permissions. The risk check is the host's first gate; a
//! tool whose permission depends on its arguments (which program, which path) checks those itself
//! and returns `Denied` when the scope does not cover them.

use super::result::{ErrorClass, ToolFailure};
use crate::scope::{Risk, Scope};
use aporia_sandbox::Sandbox;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The host-controlled context of one tool invocation.
#[derive(Debug)]
pub struct ToolContext<'a> {
    sandbox: &'a Sandbox,
    source: &'a Path,
    instrument: &'a Path,
    scope: Option<&'a Scope>,
    cancel: &'a AtomicBool,
}

impl<'a> ToolContext<'a> {
    #[must_use]
    pub fn new(
        sandbox: &'a Sandbox,
        source: &'a Path,
        instrument: &'a Path,
        scope: Option<&'a Scope>,
        cancel: &'a AtomicBool,
    ) -> Self {
        Self {
            sandbox,
            source,
            instrument,
            scope,
            cancel,
        }
    }

    /// The isolated copy. Tools read and write here, never in the source.
    #[must_use]
    pub fn sandbox(&self) -> &Sandbox {
        self.sandbox
    }

    /// The original project, read-only: source revision and history live here, not in the copy.
    #[must_use]
    pub fn source(&self) -> &Path {
        self.source
    }

    /// The `aporia` binary the instrument tool runs. A process boundary, not a linked crate.
    #[must_use]
    pub fn instrument(&self) -> &Path {
        self.instrument
    }

    /// The approved scope, or `None` while nothing has been approved.
    #[must_use]
    pub fn scope(&self) -> Option<&Scope> {
        self.scope
    }

    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// The host's risk gate. Read-only tools always pass; writing needs approved write access;
    /// execution needs an approved scope at all (the specific program is checked by the tool,
    /// because only its arguments name it).
    pub fn check_risk(&self, risk: Risk) -> Result<(), ToolFailure> {
        match risk {
            Risk::ReadOnly => Ok(()),
            Risk::WorkspaceWrite => match self.scope {
                Some(scope) if scope.allow_write => Ok(()),
                _ => Err(ToolFailure::new(
                    ErrorClass::Denied,
                    "editing the workspace has not been approved in this session's scope",
                )),
            },
            Risk::Execute => {
                if self.scope.is_some() {
                    Ok(())
                } else {
                    Err(ToolFailure::new(
                        ErrorClass::Denied,
                        "no execution scope has been approved yet; propose a plan and get approval first",
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(std::path::PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "aporia-toolctx-{tag}-{}-{}",
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

    #[test]
    fn read_only_needs_no_approval_but_writing_and_execution_do() {
        let source = Temp::new("src");
        std::fs::write(source.0.join("a.txt"), b"a").expect("file");
        let workspace = Temp::new("ws");
        let target = workspace.0.join("copy");
        let (sandbox, _) = Sandbox::create(&source.0, &target).expect("sandbox");
        let instrument = std::env::current_exe().expect("exe");
        let cancel = AtomicBool::new(false);

        let unapproved = ToolContext::new(&sandbox, &source.0, &instrument, None, &cancel);
        assert!(unapproved.check_risk(Risk::ReadOnly).is_ok());
        assert!(unapproved.check_risk(Risk::WorkspaceWrite).is_err());
        assert!(unapproved.check_risk(Risk::Execute).is_err());

        let scope = Scope {
            allow_exec: vec!["python".to_string()],
            allow_write: true,
            max_turns: 8,
            max_tool_calls: 16,
            timeout_ms: 60_000,
        };
        let approved = ToolContext::new(&sandbox, &source.0, &instrument, Some(&scope), &cancel);
        assert!(approved.check_risk(Risk::WorkspaceWrite).is_ok());
        assert!(approved.check_risk(Risk::Execute).is_ok());

        let closed = Scope {
            allow_write: false,
            ..scope.clone()
        };
        let no_write = ToolContext::new(&sandbox, &source.0, &instrument, Some(&closed), &cancel);
        assert!(no_write.check_risk(Risk::WorkspaceWrite).is_err());
        assert!(no_write.check_risk(Risk::Execute).is_ok());
    }

    #[test]
    fn the_cancel_flag_is_visible_to_tools() {
        let source = Temp::new("src2");
        std::fs::write(source.0.join("a.txt"), b"a").expect("file");
        let workspace = Temp::new("ws2");
        let (sandbox, _) = Sandbox::create(&source.0, &workspace.0.join("copy")).expect("sandbox");
        let instrument = std::env::current_exe().expect("exe");
        let cancel = AtomicBool::new(false);
        let context = ToolContext::new(&sandbox, &source.0, &instrument, None, &cancel);
        assert!(!context.cancelled());
        cancel.store(true, Ordering::SeqCst);
        assert!(context.cancelled());
    }
}
