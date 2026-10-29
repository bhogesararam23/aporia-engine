//! Running a program the sandbox was asked to run.
//!
//! There is no shell: `argv` is the command, so a semicolon in a filename is a semicolon in a
//! filename. The child gets the workspace as its working directory, a scrubbed environment, a
//! deadline and two bounded capture buffers. Retention is bounded; the digest is not -- it covers
//! every byte read, so changing the display cap cannot change the recorded identity of the stream.
//!
//! The timeout kills the process the sandbox launched. A grandchild that survives it can hold the
//! pipes open, which is why draining stops at a deadline of its own and the outcome says whether
//! the capture finished. That limitation is stated again in `documentation/limitations.md`.

use aporia_store::digest::Sha256;
use std::fmt::Write as _;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// How long to wait for the pipes after the direct child has ended. A grandchild that inherited
/// stdout can keep them open forever; the outcome reports whether both streams reached EOF.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Bytes retained by default from each stream. The digest still covers everything read.
pub const DEFAULT_OUTPUT_LIMIT: u64 = 1024 * 1024;

/// What a child was asked to run and how long it may run.
#[derive(Clone, Debug)]
pub struct ExecSpec {
    /// The program and its arguments. `argv[0]` is the program; there is no shell.
    pub argv: Vec<String>,
    pub timeout_ms: u64,
    pub output_limit: u64,
    /// Extra variables for this child, applied after the inherited environment is scrubbed.
    pub env: Vec<(String, String)>,
}

impl ExecSpec {
    #[must_use]
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            timeout_ms: 120_000,
            output_limit: DEFAULT_OUTPUT_LIMIT,
            env: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, millis: u64) -> Self {
        self.timeout_ms = millis;
        self
    }

    #[must_use]
    pub fn with_output_limit(mut self, bytes: u64) -> Self {
        self.output_limit = bytes;
        self
    }

    #[must_use]
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }
}

/// The outcome of a process that was launched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    /// The process ended on its own. A nonzero code is still `Completed`: it is an observation.
    Completed,
    /// The process never started.
    LaunchFailed(String),
    /// The deadline passed and the process was killed.
    Timeout,
}

/// One captured stream: a retained prefix, a count and a digest of everything read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captured {
    pub text: String,
    pub bytes: u64,
    pub sha256: String,
    pub truncated: bool,
}

impl Captured {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            text: String::new(),
            bytes: 0,
            sha256: aporia_store::digest::sha256_hex(b""),
            truncated: false,
        }
    }
}

/// What happened to one command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutcome {
    pub status: ExecStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: Captured,
    pub stderr: Captured,
    /// Both pipes reached EOF before the drain grace ended.
    pub output_complete: bool,
}

struct CaptureState {
    text: Vec<u8>,
    bytes: u64,
    hasher: Sha256,
    truncated: bool,
    done: bool,
}

impl CaptureState {
    fn new() -> Self {
        Self {
            text: Vec::new(),
            bytes: 0,
            hasher: Sha256::new(),
            truncated: false,
            done: false,
        }
    }
}

/// A name that is treated as a credential and is not inherited by a sandboxed command.
///
/// This is a name policy, not a security boundary: a secret under another name is still inherited.
#[must_use]
pub fn is_sensitive_env(name: &str) -> bool {
    const NEEDLES: [&str; 6] = [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "CREDENTIAL",
        "AUTH_SOCK",
    ];
    const PREFIXES: [&str; 8] = [
        "OPENAI_",
        "ANTHROPIC_",
        "AWS_",
        "AZURE_",
        "GOOGLE_",
        "GITHUB_",
        "GH_",
        "HF_",
    ];
    let upper = name.to_ascii_uppercase();
    NEEDLES.iter().any(|needle| upper.contains(needle))
        || PREFIXES.iter().any(|prefix| upper.starts_with(prefix))
}

fn retained_environment<I>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    vars.into_iter()
        .filter(|(name, _)| !is_sensitive_env(name))
        .collect()
}

fn drain(mut reader: impl Read + Send + 'static, limit: u64, state: Arc<Mutex<CaptureState>>) {
    // One thread per pipe: `read` has no timeout and a programme that waits is a normal thing.
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
                    state.bytes = state.bytes.saturating_add(read as u64);
                    state.hasher.update(&buffer[..read]);
                    let room = limit.saturating_sub(state.text.len() as u64) as usize;
                    if read <= room {
                        state.text.extend_from_slice(&buffer[..read]);
                    } else {
                        if room > 0 {
                            state.text.extend_from_slice(&buffer[..room]);
                        }
                        state.truncated = true;
                    }
                }
            }
        }
        state.lock().unwrap_or_else(PoisonError::into_inner).done = true;
    });
}

fn snapshot(state: &Arc<Mutex<CaptureState>>) -> (Captured, bool) {
    let state = state.lock().unwrap_or_else(PoisonError::into_inner);
    let digest = state.hasher.clone().finish();
    let mut sha256 = String::with_capacity(64);
    for byte in digest {
        let _ = write!(sha256, "{byte:02x}");
    }
    (
        Captured {
            text: String::from_utf8_lossy(&state.text).into_owned(),
            bytes: state.bytes,
            sha256,
            truncated: state.truncated,
        },
        state.done,
    )
}

impl crate::copy::Sandbox {
    /// Run one command. The source of the workspace is never involved; `cwd` is the copy.
    pub fn exec(&self, spec: &ExecSpec) -> ExecOutcome {
        let started = Instant::now();
        if spec.argv.is_empty() {
            return failure("the command line is empty".to_string(), started, true);
        }
        let mut child = match self.spawn(spec) {
            Ok(child) => child,
            Err(reason) => return failure(reason, started, true),
        };
        let spawned = Instant::now();

        let stdout_state = Arc::new(Mutex::new(CaptureState::new()));
        let stderr_state = Arc::new(Mutex::new(CaptureState::new()));
        if let Some(pipe) = child.stdout.take() {
            drain(pipe, spec.output_limit, Arc::clone(&stdout_state));
        }
        if let Some(pipe) = child.stderr.take() {
            drain(pipe, spec.output_limit, Arc::clone(&stderr_state));
        }

        // The deadline starts when the process exists. Starting it before `spawn` meant a machine
        // whose first launch is delayed by a policy verdict could report a 5-second child as having
        // no timeout at all, because the launch itself had already spent the budget.
        let deadline = spawned + Duration::from_millis(spec.timeout_ms.max(1));
        let (exit_status, timed_out) = match wait_for(&mut child, deadline) {
            Ok(result) => result,
            Err(reason) => return failure(reason, started, false),
        };
        let (stdout, stderr, complete) = finish_capture(&stdout_state, &stderr_state);

        ExecOutcome {
            status: if timed_out {
                ExecStatus::Timeout
            } else {
                ExecStatus::Completed
            },
            exit_code: exit_status.and_then(|status| status.code()),
            duration_ms: elapsed(started),
            stdout,
            stderr,
            output_complete: complete,
        }
    }

    fn spawn(&self, spec: &ExecSpec) -> Result<Child, String> {
        let mut command = Command::new(&spec.argv[0]);
        command
            .args(&spec.argv[1..])
            .current_dir(self.root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in retained_environment(std::env::vars()) {
            command.env(name, value);
        }
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        command.env("APORIA_SANDBOX", "1");
        command
            .spawn()
            .map_err(|error| format!("cannot start `{}`: {error}", spec.argv[0]))
    }
}

fn wait_for(child: &mut Child, deadline: Instant) -> Result<(Option<ExitStatus>, bool), String> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok((Some(status), false)),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok((None, true));
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for the child: {error}"));
            }
        }
    }
}

fn finish_capture(
    stdout_state: &Arc<Mutex<CaptureState>>,
    stderr_state: &Arc<Mutex<CaptureState>>,
) -> (Captured, Captured, bool) {
    let deadline = Instant::now() + DRAIN_GRACE;
    loop {
        let out_done = stdout_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .done;
        let err_done = stderr_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .done;
        if out_done && err_done || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let (stdout, out_done) = snapshot(stdout_state);
    let (stderr, err_done) = snapshot(stderr_state);
    (stdout, stderr, out_done && err_done)
}

fn failure(reason: String, started: Instant, complete: bool) -> ExecOutcome {
    ExecOutcome {
        status: ExecStatus::LaunchFailed(reason),
        exit_code: None,
        duration_ms: elapsed(started),
        stdout: Captured::empty(),
        stderr: Captured::empty(),
        output_complete: complete,
    }
}

fn elapsed(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inherited_environment_loses_credential_shaped_names() {
        let vars = vec![
            ("PATH".to_string(), "/bin".to_string()),
            ("CARGO_HOME".to_string(), "/cargo".to_string()),
            ("OPENAI_API_KEY".to_string(), "sk-secret".to_string()),
            ("AWS_REGION".to_string(), "eu-west-1".to_string()),
            ("MY_SECRET".to_string(), "hunter2".to_string()),
            ("GH_TOKEN".to_string(), "ghp_x".to_string()),
        ];
        let kept = retained_environment(vars);
        let names: Vec<&str> = kept.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec!["PATH", "CARGO_HOME"],
            "the filter kept or dropped the wrong names"
        );
    }
}
