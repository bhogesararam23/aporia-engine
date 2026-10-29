//! What execution promises: a real child, a real exit status, a bounded lifetime, a bounded
//! retained output whose digest is still over the whole stream, and an environment that does not
//! hand over credentials by default.

use aporia_sandbox::{ExecSpec, ExecStatus, Sandbox, is_sensitive_env};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-sandbox-exec-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sandbox() -> (Temp, Sandbox) {
    let temp = Temp::new("work");
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("model.ap"), b"model x { }\n").expect("source file");
    let workspace = temp.path().join("workspace");
    let (sandbox, _) = Sandbox::create(&source, &workspace).expect("copy");
    (temp, sandbox)
}

fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_sandbox-fixture")
}

#[test]
fn a_child_is_captured_with_its_exit_status() {
    let (_temp, sandbox) = sandbox();
    let spec = ExecSpec::new(vec![
        fixture().to_string(),
        "echo".to_string(),
        "hello".to_string(),
    ]);

    let outcome = sandbox.exec(&spec);

    assert_eq!(outcome.status, ExecStatus::Completed, "{outcome:?}");
    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(outcome.stdout.text.trim_end(), "hello");
    assert_eq!(outcome.stderr.text, "");
    assert_eq!(
        outcome.stdout.sha256,
        aporia_store::digest::sha256_hex(outcome.stdout.text.as_bytes()),
        "the digest does not cover the captured stream"
    );
    assert!(outcome.output_complete);
}

#[test]
fn a_nonzero_exit_is_an_observation_not_a_launch_failure() {
    let (_temp, sandbox) = sandbox();
    let spec = ExecSpec::new(vec![
        fixture().to_string(),
        "fail".to_string(),
        "7".to_string(),
    ]);

    let outcome = sandbox.exec(&spec);

    assert_eq!(outcome.status, ExecStatus::Completed, "{outcome:?}");
    assert_eq!(outcome.exit_code, Some(7));
}

#[test]
fn a_hung_child_is_killed_and_reported_as_a_timeout() {
    let (_temp, sandbox) = sandbox();
    // Thirty seconds, not five: on this machine the first launch of a freshly linked fixture can
    // take seconds while Smart App Control evaluates it, and a five-second child would then be
    // indistinguishable from a child that ended on its own. Ten seconds is still far below the
    // child's lifetime and far above any plausible launch delay.
    let spec = ExecSpec::new(vec![
        fixture().to_string(),
        "sleep".to_string(),
        "30000".to_string(),
    ])
    .with_timeout(100);

    let outcome = sandbox.exec(&spec);

    assert_eq!(outcome.status, ExecStatus::Timeout, "{outcome:?}");
    assert_eq!(outcome.exit_code, None, "a killed child has no exit code");
    assert!(
        outcome.duration_ms < 10_000,
        "the timeout took {} ms, which is not a timeout",
        outcome.duration_ms
    );
}

#[test]
fn output_past_the_cap_is_truncated_but_the_digest_still_covers_it() {
    let (_temp, sandbox) = sandbox();
    let total = 2 * 1024 * 1024u64;
    let spec = ExecSpec::new(vec![
        fixture().to_string(),
        "emit".to_string(),
        total.to_string(),
    ])
    .with_output_limit(1024);

    let outcome = sandbox.exec(&spec);

    assert_eq!(outcome.status, ExecStatus::Completed, "{outcome:?}");
    assert!(outcome.stdout.truncated, "the cap was not reported");
    assert_eq!(outcome.stdout.text.len(), 1024);
    assert_eq!(
        outcome.stdout.bytes, total,
        "the count must be of the stream, not the retained prefix"
    );
    assert_eq!(
        outcome.stdout.sha256,
        aporia_store::digest::sha256_hex(&vec![b'x'; total as usize]),
        "the digest changed with the retention cap"
    );
}

#[test]
fn the_child_runs_in_the_workspace_with_an_explicit_environment() {
    let (_temp, sandbox) = sandbox();
    let root = sandbox.root().canonicalize().expect("canonical root");

    let cwd = sandbox.exec(&ExecSpec::new(vec![
        fixture().to_string(),
        "cwd".to_string(),
    ]));
    assert_eq!(
        PathBuf::from(cwd.stdout.text.trim())
            .canonicalize()
            .expect("child cwd"),
        root,
        "the child did not run in the sandbox"
    );

    let visible = sandbox.exec(
        &ExecSpec::new(vec![
            fixture().to_string(),
            "dump".to_string(),
            "VISIBLE".to_string(),
        ])
        .with_env("VISIBLE", "yes"),
    );
    assert_eq!(visible.stdout.text.trim(), "yes");

    let marker = sandbox.exec(&ExecSpec::new(vec![
        fixture().to_string(),
        "dump".to_string(),
        "APORIA_SANDBOX".to_string(),
    ]));
    assert_eq!(marker.stdout.text.trim(), "1");
}

#[test]
fn a_missing_program_is_a_launch_failure_with_a_reason() {
    let (_temp, sandbox) = sandbox();
    let spec = ExecSpec::new(vec!["definitely-not-a-program-aporia".to_string()]);

    let outcome = sandbox.exec(&spec);

    assert!(
        matches!(outcome.status, ExecStatus::LaunchFailed(_)),
        "{outcome:?}"
    );
    assert_eq!(outcome.exit_code, None);
}

#[test]
fn credential_shaped_names_are_refused_the_environment() {
    for name in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "SECRET_TOKEN",
        "GITHUB_TOKEN",
        "DATABASE_PASSWORD",
        "AWS_CREDENTIAL",
        "MY_SECRET",
        "SSH_AUTH_SOCK",
    ] {
        assert!(
            is_sensitive_env(name),
            "{name} was not treated as sensitive"
        );
    }
    for name in [
        "PATH",
        "HOME",
        "RUSTUP_HOME",
        "CARGO_HOME",
        "TEMP",
        "VISIBLE",
    ] {
        assert!(!is_sensitive_env(name), "{name} was wrongly scrubbed");
    }
}
