//! The child process that stands in for the arithmetic.
//!
//! One program, started once, asked one question at a time in the same order the campaign asked it.
//! That shape is chosen for two reasons. A fresh process per evaluation would cost more than the
//! answer, and would make the campaign's budget a fiction about operating-system speed rather than
//! about the model. And a single long-lived pipe keeps the request/response order visible, which is
//! what makes a run reproducible: point *n* was answered by the program's *n*th reply, so replaying
//! the same points at the same program is a claim anyone can check.
//!
//! Failure is not a value. When the program cannot be launched, dies mid-run, refuses a point, answers
//! with something that is not the protocol, or stops answering at all, the adapter records the reason,
//! stops asking, and returns non-answers so the driver can finish without panicking. `failure()` then
//! says whether the map that came out means anything. The alternative — substituting a plausible
//! number and carrying on — would produce an atlas whose suspicious regions could not be told apart
//! from its broken pipes.

use crate::protocol::{self, Answer, ProtocolError};
use aporia_ir::Model;
use aporia_runtime::Executor;
use aporia_runtime::interp::Outcome;
use aporia_runtime::value::{ExecConfig, Flags};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

/// How long a stopped program gets to finish what its end of input started.
///
/// Bounded and short: a program that means to exit at EOF does so in milliseconds, and a program that
/// ignores EOF is the case `AdapterError::Timeout` already exists for. See [`Program::stop`].
const FINALIZE_GRACE_MS: u64 = 250;

/// How to launch a program, and how long to wait for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramSpec {
    /// The command line. `argv[0]` is the program; there is no shell involved, so nothing is
    /// re-parsed, re-globbed or re-interpreted between APORIA and the process.
    pub argv: Vec<String>,
    /// Per-answer timeout in milliseconds. A program that hangs is a failed run, not a hang.
    pub timeout_ms: u64,
    /// Extra environment variables for the child. Programs read their configuration from the
    /// environment — a thread count, a data directory, a solver tolerance — and a boundary that
    /// cannot pass any is a boundary that only works for programs with no settings.
    pub env: Vec<(String, String)>,
}

impl ProgramSpec {
    #[must_use]
    pub fn new(argv: Vec<String>, timeout_ms: u64) -> Self {
        Self {
            argv,
            timeout_ms,
            env: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// Split a command line on whitespace for a CLI flag. Deliberately simple: a program whose path
    /// contains a space is passed as a separate argument rather than quoted, because implementing
    /// shell quoting here would mean trusting a shell.
    #[must_use]
    pub fn parse(text: &str, timeout_ms: u64) -> Option<Self> {
        let argv: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        if argv.is_empty() {
            return None;
        }
        Some(Self::new(argv, timeout_ms))
    }

    #[must_use]
    pub fn display(&self) -> String {
        self.argv.join(" ")
    }
}

/// Why the program is not answering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterError {
    /// The process could not be started at all.
    Launch { program: String, reason: String },
    /// The pipe or the process closed mid-run. `reason` is the OS's words where there are any.
    Closed { reason: String },
    /// The program answered with `{"error": ...}`. This is its own statement, so the message is
    /// quoted rather than paraphrased.
    Refused { message: String },
    /// A line arrived that is not the protocol, or not the protocol for this model.
    Protocol { reason: String },
    /// The program did not answer within the timeout and was stopped.
    Timeout { millis: u64 },
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Launch { program, reason } => write!(f, "cannot start `{program}`: {reason}"),
            Self::Closed { reason } => write!(f, "the program stopped answering: {reason}"),
            Self::Refused { message } => write!(f, "the program refused to answer: {message}"),
            Self::Protocol { reason } => {
                write!(f, "the program's answer is not the protocol: {reason}")
            }
            Self::Timeout { millis } => {
                write!(
                    f,
                    "the program did not answer within {millis} ms and was stopped"
                )
            }
        }
    }
}

impl std::error::Error for AdapterError {}

impl From<ProtocolError> for AdapterError {
    fn from(value: ProtocolError) -> Self {
        match value {
            ProtocolError::Refused { message } => Self::Refused { message },
            other => Self::Protocol {
                reason: other.to_string(),
            },
        }
    }
}

/// A running program, as an [`Executor`].
#[derive(Debug)]
pub struct Program {
    spec: ProgramSpec,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    answers: Option<Receiver<Result<String, std::io::Error>>>,
    outputs: usize,
    broken: Option<AdapterError>,
    /// Answers actually received. Kept because "how many evaluations did the program really do" is a
    /// question a cost claim has to be able to answer.
    served: u64,
}

impl Program {
    /// Prepare a program without starting it: `start` is where a launch failure becomes an error
    /// message instead of a NaN.
    #[must_use]
    pub fn new(spec: ProgramSpec) -> Self {
        Self {
            spec,
            child: None,
            stdin: None,
            answers: None,
            outputs: 0,
            broken: None,
            served: 0,
        }
    }

    /// Launch the program. `outputs` is how many values the model declares: every answer must carry
    /// exactly that many, and finding out later is a worse conversation with a user.
    pub fn start(&mut self, outputs: usize) -> Result<(), AdapterError> {
        self.outputs = outputs;
        let mut command = Command::new(&self.spec.argv[0]);
        command
            .args(&self.spec.argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The program's own diagnostics go to the terminal it was launched from. Capturing them
            // would let a solver's warning scroll past unseen; they are not part of the answers and
            // are never parsed, so they cannot change a result.
            .stderr(Stdio::inherit());
        for (key, value) in &self.spec.env {
            command.env(key, value);
        }
        let mut child = command.spawn().map_err(|e| AdapterError::Launch {
            program: self.spec.display(),
            reason: e.to_string(),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| AdapterError::Launch {
            program: self.spec.display(),
            reason: "no pipe to its standard input".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| AdapterError::Launch {
            program: self.spec.display(),
            reason: "no pipe from its standard output".to_string(),
        })?;
        let (tx, rx) = channel::<Result<String, std::io::Error>>();
        // One thread per program, blocked on the pipe, because `read_line` has no timeout and a
        // scientific program that waits on input it never receives is a normal thing to happen. The
        // thread ends when the pipe closes: on clean drop, on kill, or when the program exits.
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break, // EOF: the program closed its stdout or died.
                    Ok(_) => {
                        let done = line.trim_end_matches(['\n', '\r']).to_string();
                        if tx.send(Ok(done)).is_err() {
                            break; // nobody is listening any more
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });
        self.child = Some(child);
        self.stdin = Some(stdin);
        self.answers = Some(rx);
        Ok(())
    }

    /// The reason the program stopped answering, if it did. A caller that is about to present a map
    /// has to ask this: after a failure the map is made of non-answers and means nothing.
    #[must_use]
    pub fn failure(&self) -> Option<&AdapterError> {
        self.broken.as_ref()
    }

    #[must_use]
    pub fn served(&self) -> u64 {
        self.served
    }

    fn ask(&mut self, x: &[f64]) -> Result<Answer, AdapterError> {
        let line = protocol::encode_request(x);
        {
            let stdin = self.stdin.as_mut().ok_or_else(|| AdapterError::Closed {
                reason: "its input pipe is already closed".to_string(),
            })?;
            // A write failure here usually means the program died before reading, which the
            // subsequent read will confirm; the error is kept rather than retried silently.
            stdin
                .write_all(line.as_bytes())
                .and_then(|()| stdin.write_all(b"\n"))
                .and_then(|()| stdin.flush())
                .map_err(|e| AdapterError::Closed {
                    reason: e.to_string(),
                })?;
        }
        let rx = self.answers.as_ref().ok_or_else(|| AdapterError::Closed {
            reason: "its answer stream is gone".to_string(),
        })?;
        match rx.recv_timeout(Duration::from_millis(self.spec.timeout_ms)) {
            Ok(Ok(line)) => {
                let answer = protocol::decode_response(&line, self.outputs)?;
                self.served += 1;
                Ok(answer)
            }
            Ok(Err(e)) => Err(AdapterError::Closed {
                reason: e.to_string(),
            }),
            Err(RecvTimeoutError::Timeout) => {
                self.stop();
                Err(AdapterError::Timeout {
                    millis: self.spec.timeout_ms,
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(AdapterError::Closed {
                reason: "it closed its answer stream".to_string(),
            }),
        }
    }

    /// Stop the process and release the pipes. Idempotent, and safe to call from `Drop`.
    fn stop(&mut self) {
        // Dropping the writer first is what actually unblocks a program waiting on input, and it is
        // the signal a well-behaved program treats as the end of the run.
        self.stdin = None;
        self.answers = None;
        if let Some(mut child) = self.child.take() {
            // Then give it that end-of-input to act on. Measured, not assumed: a program that
            // checkpoints, flushes a log or releases a lock when stdin closes had no chance to,
            // because the kill below landed first — a probe that reported its request tally at EOF
            // printed nothing at all across an entire run. Waiting is bounded, because a program that
            // ignores EOF is exactly the case `Timeout` exists for, and the caller must not be made to
            // hang for a child APORIA has already given up on.
            let deadline = Instant::now() + Duration::from_millis(FINALIZE_GRACE_MS);
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The non-answer the driver gets once the program is gone: every declared output is NaN, which
    /// the divergence channel reads as a fact about that point rather than as a number.
    fn unanswered(&self) -> Outcome {
        Outcome {
            outputs: vec![f64::NAN; self.outputs],
            traces: Vec::new(),
            flags: Flags {
                nan: self.outputs > 0,
                ..Flags::default()
            },
            steps: 0,
            rule_values: Vec::new(),
        }
    }
}

impl Executor for Program {
    fn execute(&mut self, model: &Model, x: &[f64], _cfg: ExecConfig) -> Outcome {
        if self.outputs == 0 {
            self.outputs = model.outputs.len();
        }
        if self.broken.is_some() {
            return self.unanswered();
        }
        match self.ask(x) {
            Ok(answer) => {
                let mut flags = Flags::default();
                for v in &answer.y {
                    if v.is_nan() {
                        flags.nan = true;
                    } else if v.is_infinite() {
                        flags.inf = true;
                    }
                }
                Outcome {
                    outputs: answer.y,
                    traces: Vec::new(),
                    flags,
                    steps: answer.steps,
                    rule_values: Vec::new(),
                }
            }
            Err(e) => {
                // Remembered, then reported: the campaign finishes without a panic, and the caller
                // learns from `failure()` that the map it holds is made of non-answers.
                self.broken = Some(e);
                self.unanswered()
            }
        }
    }

    /// One program, one path. Asking it the same question at `FpMode::F32` would measure whether the
    /// program ignores the field it was not sent, and record that as numerical evidence.
    fn varies_with_precision(&self) -> bool {
        false
    }

    /// `aporia_numerics::reference` re-does A-IR instructions. There are none here, so there is no
    /// second implementation to disagree with.
    fn has_reference_path(&self) -> bool {
        false
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        self.stop();
    }
}
