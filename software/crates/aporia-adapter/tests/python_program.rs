//! The boundary, driven by a program written in another language.
//!
//! `subprocess.rs` drives `aporia-example-solver`, which is Rust and gets APORIA's number encoding for
//! free because it calls the same functions. That proves the pipe works. It does not prove the
//! *protocol* is the boundary rather than the Rust API wearing a pipe — a program that shares a
//! serialiser with its caller cannot show what a caller without one has to get right.
//!
//! So these tests launch `software/examples/beam.py`, which imports `json`, `math`, `os`, `sys` and
//! `time` and nothing else, and writes the wire format from scratch. Every case `subprocess.rs`
//! exercises is exercised here against it, and the answers are required to be the same numbers.
//!
//! ## When there is no Python
//!
//! The interpreter is looked for, not assumed: `python3`, `python`, then the Windows launcher `py -3`,
//! each *run* before being chosen, because a name in PATH is not a working program — on the machine
//! that wrote this, `python3` is a Store alias that prints an advertisement and exits nonzero. If none
//! of them runs, the tests print `SKIP` on stderr and pass with nothing checked, which is the one
//! outcome a reader has to be able to see. A green count from a language fixture that never executed
//! is the mistake this project refuses elsewhere: an archive executed by a program the caller does not
//! have is reported as *not replayed*, not as "0 mismatches".

use aporia_adapter::{AdapterError, Program, ProgramSpec, protocol};
use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_runtime::Executor;
use aporia_runtime::value::ExecConfig;
use aporia_search::{Config, Strategy, run_with};
use std::path::PathBuf;
use std::process::Command;

const QUICK_MS: u64 = 1_500;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/beam.py")
        .canonicalize()
        .expect("the Python example is part of the repository")
}

/// The first candidate that actually executes Python, as the argv prefix to use.
///
/// Each candidate is asked to run a one-line program; success and the expected stdout is the only test.
/// A stub that exists but does not work (Windows' Store alias) is rejected by it, which is the reason
/// this is not just `which python3`.
fn interpreter() -> Option<Vec<String>> {
    for candidate in [
        vec!["python3".to_string()],
        vec!["python".to_string()],
        vec!["py".to_string(), "-3".to_string()],
    ] {
        let Ok(out) = Command::new(&candidate[0])
            .args(&candidate[1..])
            .arg("-c")
            .arg("import sys; print(sys.version_info[0])")
            .output()
        else {
            continue;
        };
        if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "3" {
            return Some(candidate);
        }
    }
    None
}

fn program(mode: &str, timeout_ms: u64) -> Option<Program> {
    let mut argv = interpreter()?;
    argv.push(script().display().to_string());
    Some(Program::new(
        ProgramSpec::new(argv, timeout_ms).with_env("APORIA_PROGRAM_MODE", mode),
    ))
}

fn skip_if_absent(name: &str) -> bool {
    if interpreter().is_some() {
        return false;
    }
    eprintln!(
        "SKIP: {name} — no Python 3 interpreter runs on this machine, so the language boundary was \
         not exercised. `software/examples/beam.py` is the program; install Python 3 to check it."
    );
    true
}

/// A beam whose deflection comes from outside. Identical to the model in `subprocess.rs`, which is
/// the point: the same declaration, the same rule, a different language answering.
const BEAM: &str = "model beam \"deflection from an external solver\" {\n\
    input load : N in [0, 100]\n\
    output deflection : mm\n\
    require deflection >= 0\n\
}\n";

fn beam() -> Model {
    let c = compile("beam.ap", BEAM);
    assert!(!c.diagnostics.has_errors(), "{}\n{BEAM}", c.diagnostics);
    c.model
}

#[test]
fn a_python_program_answers_the_same_points_the_rust_one_does() {
    // The cross-language claim, and the one that would break if either side's number formatting moved:
    // the same request produces bit-identical answers from both implementations of the protocol, with
    // the program's own work count carried through unchanged.
    if skip_if_absent("the Python boundary") {
        return;
    }
    let m = beam();
    let mut py = program("answer", 10_000).expect("an interpreter");
    py.start(1).expect("launches");
    let mut rs = Program::new(ProgramSpec::new(
        vec![env!("CARGO_BIN_EXE_aporia-example-solver").to_string()],
        10_000,
    ));
    rs.start(1).expect("launches");
    for load in [0.0, 7.5, 59.9, 60.1, 100.0, 1e-7, 1.0e18] {
        let a = py.execute(&m, &[load], ExecConfig::default());
        let b = rs.execute(&m, &[load], ExecConfig::default());
        assert_eq!(a.outputs, b.outputs, "load {load} answered differently");
        assert_eq!(a.steps, 12, "the Python program reports its own work");
        assert!(py.failure().is_none(), "{:?}", py.failure());
    }
    // And the sign flip is where the Rust example puts it, so the two fixtures stay one example.
    let past = py.execute(&m, &[61.0], ExecConfig::default());
    assert_eq!(past.outputs, vec![-1.4]);
}

#[test]
fn a_python_non_finite_answer_arrives_as_the_value_it_meant() {
    // `json.dumps(float('nan'))` writes the bare token `NaN`, which is not valid JSON and not the
    // protocol. The example encodes non-finite values as strings for exactly that reason, so this is
    // the check that the divergence channel sees a *value* rather than a framing error.
    if skip_if_absent("the Python boundary") {
        return;
    }
    let m = beam();
    let mut p = program("nan", 10_000).expect("an interpreter");
    p.start(1).expect("launches");
    let out = p.execute(&m, &[10.0], ExecConfig::default());
    assert!(out.outputs[0].is_nan(), "{:?}", out.outputs);
    assert!(out.flags.nan, "the flag is how a NaN becomes evidence");
    assert!(p.failure().is_none(), "{:?}", p.failure());
}

#[test]
fn every_way_a_python_program_can_fail_is_reported_as_that_thing() {
    // The same table `subprocess.rs` runs against the Rust example, answered by a language that shares
    // no code with the caller. A refusal is not a NaN, a wrong arity is not a hang, and a process that
    // exits is not a program that answered nothing.
    if skip_if_absent("the Python boundary") {
        return;
    }
    let m = beam();
    for (mode, expect) in [
        ("refuse", "the program refused to answer"),
        ("arity", "not the protocol"),
        ("garbage", "not the protocol"),
        ("banner", "not the protocol"),
        ("exit", "the program stopped answering"),
    ] {
        let mut p = program(mode, 10_000).expect("an interpreter");
        p.start(1).expect("launches");
        // `exit` survives one answer, so the first read must succeed for the failure to mean what the
        // mode says it means.
        let _ = p.execute(&m, &[10.0], ExecConfig::default());
        let _ = p.execute(&m, &[20.0], ExecConfig::default());
        let failure = p
            .failure()
            .unwrap_or_else(|| panic!("mode {mode} did not fail at all"));
        let text = failure.to_string();
        assert!(
            text.contains(expect),
            "mode {mode} said {text:?}, which is not {expect:?}"
        );
    }
}

#[test]
fn a_python_program_that_never_answers_is_stopped_by_the_timeout() {
    // A script waiting on input it will never receive is an ordinary thing. The caller ends the run,
    // says so, and does not hang with it.
    if skip_if_absent("the Python boundary") {
        return;
    }
    let m = beam();
    let mut p = program("hang", QUICK_MS).expect("an interpreter");
    p.start(1).expect("launches");
    let out = p.execute(&m, &[10.0], ExecConfig::default());
    assert!(out.outputs[0].is_nan(), "a stopped run is a non-answer");
    assert_eq!(
        p.failure(),
        Some(&AdapterError::Timeout { millis: QUICK_MS }),
        "the run was ended by something other than the timeout"
    );
}

#[test]
fn the_campaign_finds_the_boundary_that_lives_in_another_language() {
    // The end-to-end claim for this file. `beam.py` computes the deflection, APORIA never sees the
    // arithmetic, and the atlas still has to place the region where the model's own rule stops
    // holding. Nothing downstream of the pipe knows the program was Python.
    if skip_if_absent("the Python boundary") {
        return;
    }
    let m = beam();
    let mut p = program("answer", 10_000).expect("an interpreter");
    p.start(1).expect("launches");
    let campaign = run_with(
        &m,
        Config {
            budget: 240,
            strategy: Strategy::Adaptive,
            seed: 5,
            probe_every: 0,
            numerical_every: 0,
            differential_every: 0,
            symmetric_every: 0,
            ..Config::default()
        },
        &mut p,
    );
    assert!(p.failure().is_none(), "{:?}", p.failure());
    let flagged: Vec<[f64; 2]> = campaign
        .findings
        .iter()
        .flat_map(|f| f.bounds.iter().copied())
        .collect();
    assert!(
        !campaign.findings.is_empty(),
        "the atlas found nothing in a program that goes negative past 60 N"
    );
    assert!(
        flagged.iter().any(|[lo, hi]| *hi > 60.0 && *lo < 100.0),
        "no reported region reaches past the flip: {flagged:?}"
    );
    // Every answer it took is the program's own count of its work, not APORIA's instruction steps.
    assert_eq!(
        campaign.instruction_steps,
        campaign.evaluations * 12,
        "the program's reported work is not what the run recorded"
    );
}

#[test]
fn a_program_that_finishes_its_own_work_at_end_of_input_gets_to_finish_it() {
    // The lifecycle claim, and the reason `stop` waits before it kills. A solver that flushes a log,
    // writes a checkpoint or releases a licence when stdin closes is doing exactly what the contract
    // tells it to do — and APORIA used to kill it on the way past that line. Measured with a probe that
    // reported its request tally at EOF: the tally never appeared, across a whole run.
    if skip_if_absent("the Python boundary") {
        return;
    }
    let mark = std::env::temp_dir().join(format!("aporia-eof-mark-{}", std::process::id()));
    let _ = std::fs::remove_file(&mark);
    let m = beam();
    {
        let mut argv = interpreter().expect("checked above");
        argv.push(script().display().to_string());
        let mut p = Program::new(
            // The mark path travels by environment, like the mode does: configuration is the
            // environment, the wire carries parameters and nothing else.
            ProgramSpec::new(argv, 10_000)
                .with_env("APORIA_PROGRAM_MODE", "finalize")
                .with_env("APORIA_PROGRAM_MARK", &mark.display().to_string()),
        );
        p.start(1).expect("launches");
        for load in [10.0, 30.0, 70.0] {
            p.execute(&m, &[load], ExecConfig::default());
        }
        assert!(p.failure().is_none(), "{:?}", p.failure());
        // Dropping the program is the end of the run: the pipe closes and the program finalizes.
    }
    let text = std::fs::read_to_string(&mark).unwrap_or_else(|e| {
        panic!(
            "the program never reached its end-of-input handler: {e} — it was killed rather than \
             allowed to exit"
        )
    });
    let _ = std::fs::remove_file(&mark);
    assert!(
        text.contains("\"answered\": 3"),
        "the program finalized with the wrong tally: {text}"
    );
}

#[test]
fn the_protocol_reads_what_the_python_program_writes() {
    // Byte-level, without a process: what the example prints must decode as the protocol says. This
    // pins the wire format against the reference encoder, so a change to either is caught here rather
    // than in a run whose answers silently stop matching.
    let line = protocol::encode_response(&protocol::Answer {
        y: vec![2.1, f64::NAN],
        steps: 12,
    });
    let decoded = protocol::decode_response(&line, 2).expect("the reference form decodes");
    assert_eq!(decoded.steps, 12);
    assert_eq!(decoded.y[0], 2.1);
    // Now the same two values as the Python example writes them.
    if skip_if_absent("the Python boundary") {
        return;
    }
    let mut argv = interpreter().expect("checked above");
    argv.push(script().display().to_string());
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .env("APORIA_PROGRAM_MODE", "nan")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("a pipe to the program")
                .write_all(b"{\"x\":[12.5]}\n")?;
            child.wait_with_output()
        })
        .expect("the example runs");
    let text = String::from_utf8_lossy(&out.stdout);
    let answer = text.lines().next().unwrap_or_default().to_string();
    assert!(
        answer.contains("\"NaN\""),
        "a non-finite value must be a string on the wire: {answer}"
    );
    assert!(
        protocol::decode_response(&answer, 1).is_ok(),
        "APORIA's parser cannot read what Python wrote: {answer}"
    );
}
