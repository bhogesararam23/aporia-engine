//! The boundary, driven by a real child process.
//!
//! Every test here launches `aporia-example-solver` as a separate program over a pipe. Nothing is
//! mocked: the answers cross a process boundary, are encoded and decoded as bytes, and come back as
//! observations the campaign consumes. The failure modes are produced by the program's own behaviour
//! (exit, garbage, silence), switched by the environment, because a simulated broken pipe tests the
//! simulation rather than the pipe.

use aporia_adapter::{AdapterError, Answer, Program, ProgramSpec, protocol};
use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_runtime::Executor;
use aporia_runtime::value::ExecConfig;
use aporia_search::{Config, Strategy, run_with};

const PROGRAM: &str = env!("CARGO_BIN_EXE_aporia-example-solver");
const QUICK_MS: u64 = 300;

/// A beam whose deflection is computed by another program. Declared in the DSL, because everything
/// downstream of the declaration -- domains, units, the rule, the budget -- should apply to a foreign
/// model exactly as it does to an interpreted one.
const BEAM: &str = "model beam \"deflection from an external solver\" {\n\
    input load : N in [0, 100]\n\
    output deflection : mm\n\
    require deflection >= 0\n\
}\n";

fn beam() -> Model {
    let c = compile("beam.ap", BEAM);
    assert!(
        !c.diagnostics.has_errors(),
        "{}\n{BEAM}",
        c.diagnostics
            .render_all(&aporia_dsl::span::Source::new("beam.ap", BEAM))
    );
    let report = aporia_ir::verify(&c.model);
    assert!(report.is_ok(), "A-IR does not verify: {:?}", report.errors);
    c.model
}

fn solver(mode: &str, timeout_ms: u64) -> Program {
    Program::new(
        ProgramSpec::new(vec![PROGRAM.to_string()], timeout_ms)
            .with_env("APORIA_PROGRAM_MODE", mode),
    )
}

#[test]
fn a_child_process_answers_the_same_points_the_same_way_twice() {
    let m = beam();
    let points: Vec<f64> = vec![0.0, 7.5, 59.9, 60.1, 100.0];
    let mut first = solver("answer", 5_000);
    first.start(1).expect("launches");
    let mut second = solver("answer", 5_000);
    second.start(1).expect("launches");
    for load in &points {
        let a = first.execute(&m, &[*load], ExecConfig::default());
        let b = second.execute(&m, &[*load], ExecConfig::default());
        assert_eq!(a.outputs, b.outputs, "load {load} answered differently");
        assert!(first.failure().is_none() && second.failure().is_none());
    }
    // The work count is the program's report, carried through unchanged rather than converted into
    // APORIA's instruction steps, which it is not.
    let out = first.execute(&m, &[10.0], ExecConfig::default());
    assert_eq!(out.steps, 12);
}

#[test]
fn the_region_the_program_creates_is_the_region_the_atlas_finds() {
    // The end-to-end claim for this phase: a scientific program, a pipe, and the existing engine.
    // The solver deflects backwards past 60 N, the model says that is not allowed, and the campaign
    // has to find that boundary without knowing the arithmetic happened somewhere else.
    let m = beam();
    let mut program = solver("answer", 10_000);
    program.start(m.outputs.len()).expect("launches");
    let campaign = run_with(
        &m,
        Config {
            budget: 200,
            strategy: Strategy::Adaptive,
            probe_every: 0,
            symmetric_every: 0,
            ..Config::default()
        },
        &mut program,
    );
    assert!(
        program.failure().is_none(),
        "the run should have been clean: {:?}",
        program.failure()
    );
    assert!(!campaign.findings.is_empty(), "no region was found");
    let coverage = campaign.atlas.coverage();
    assert!(
        coverage.suspicious > 0.0 && coverage.suspicious < 0.9,
        "suspicious {coverage:?}"
    );
    assert!(coverage.trusted > 0.0, "nothing was trusted: {coverage:?}");
    // Every charged evaluation got an answer, and only the ones asked got one. This is the
    // accounting that makes the cost of an external run comparable with an interpreted one.
    assert_eq!(
        program.served(),
        campaign.evaluations,
        "answers received and evaluations charged disagree"
    );
    assert_eq!(campaign.records.len() as u64, campaign.evaluations);
    // The evidence is the model's own rule, violated in the region -- not a new channel invented for
    // foreign programs.
    assert!(
        campaign
            .evidence
            .iter()
            .any(|e| e.detail.contains("deflection") && e.detail.contains("violated")),
        "{:?}",
        campaign
            .evidence
            .iter()
            .map(|e| e.detail.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_program_that_dies_mid_run_is_reported_and_the_map_is_not_trusted() {
    let m = beam();
    let mut program = solver("exit", 5_000);
    program.start(1).expect("launches");
    // The example answers once and then exits, which is what a solver does when the second load makes
    // its matrix singular.
    let _ = program.execute(&m, &[10.0], ExecConfig::default());
    let after = program.execute(&m, &[20.0], ExecConfig::default());
    assert!(after.outputs[0].is_nan(), "a dead program produced a value");
    assert!(after.flags.nan, "{:?}", after.flags);
    let failure = program
        .failure()
        .cloned()
        .expect("the death must be remembered");
    assert!(
        matches!(failure, AdapterError::Closed { .. }),
        "unexpected failure: {failure:?}"
    );
    // Once broken it stays broken: the driver is not left to discover the pipe by inference.
    let more = program.execute(&m, &[30.0], ExecConfig::default());
    assert!(more.outputs[0].is_nan());
    assert_eq!(program.failure(), Some(&failure));
}

#[test]
fn a_banner_line_before_the_answers_is_a_protocol_error_not_a_value() {
    // The most likely way a real program gets this wrong: it prints a greeting. APORIA must refuse the
    // line rather than skip it, because skipping would silently re-align every later answer.
    let m = beam();
    let mut program = solver("banner", 5_000);
    program.start(1).expect("launches");
    let out = program.execute(&m, &[10.0], ExecConfig::default());
    assert!(out.outputs[0].is_nan());
    match program.failure().expect("recorded") {
        AdapterError::Protocol { reason } => {
            assert!(reason.contains("protocol"), "{reason}");
        }
        other => panic!("expected a protocol failure, got {other:?}"),
    }
}

#[test]
fn a_wrong_output_count_is_refused_with_both_numbers() {
    let m = beam();
    let mut program = solver("arity", 5_000);
    program.start(1).expect("launches");
    let _ = program.execute(&m, &[10.0], ExecConfig::default());
    match program.failure().expect("recorded") {
        AdapterError::Protocol { reason } => {
            assert!(reason.contains("2 value(s)"), "{reason}");
            assert!(reason.contains("declares 1"), "{reason}");
        }
        other => panic!("expected a protocol failure, got {other:?}"),
    }
}

#[test]
fn a_refusal_voids_the_run_instead_of_becoming_a_number() {
    // `{"error": ...}` means the program declined. It is reported with its own words and it is not
    // the same as a NaN, which would be a value the program did compute.
    let m = beam();
    let mut program = solver("refuse", 5_000);
    program.start(1).expect("launches");
    let out = program.execute(&m, &[10.0], ExecConfig::default());
    assert!(out.outputs[0].is_nan());
    assert_eq!(
        program.failure(),
        Some(&AdapterError::Refused {
            message: "matrix was singular".to_string()
        })
    );
}

#[test]
fn a_hanging_program_is_stopped_by_the_timeout() {
    let m = beam();
    let mut program = solver("hang", QUICK_MS);
    program.start(1).expect("launches");
    let started = std::time::Instant::now();
    let out = program.execute(&m, &[10.0], ExecConfig::default());
    let elapsed = started.elapsed();
    assert!(out.outputs[0].is_nan());
    assert_eq!(
        program.failure(),
        Some(&AdapterError::Timeout { millis: QUICK_MS })
    );
    // The timeout is the point, so it has to actually bound the wait.
    assert!(
        elapsed.as_secs() < 10,
        "the timeout did not stop the program: {elapsed:?}"
    );
    // And the process is gone rather than left running: asking again does not block for another
    // timeout, it returns the remembered failure.
    let again = program.execute(&m, &[20.0], ExecConfig::default());
    assert!(again.outputs[0].is_nan());
    assert_eq!(program.served(), 0, "nothing was ever answered");
}

#[test]
fn a_non_finite_answer_arrives_as_a_value_and_is_not_a_failure() {
    // The distinction the protocol exists to keep: NaN is something the program computed. It reaches
    // the divergence channel as evidence about that region, and the run is not void.
    let m = beam();
    let mut program = solver("nan", 5_000);
    program.start(1).expect("launches");
    let out = program.execute(&m, &[10.0], ExecConfig::default());
    assert!(out.outputs[0].is_nan());
    assert!(out.flags.nan);
    assert!(
        program.failure().is_none(),
        "a value was treated as a failure: {:?}",
        program.failure()
    );
    // And the campaign turns it into physical evidence rather than an error report.
    let campaign = run_with(
        &m,
        Config {
            budget: 60,
            probe_every: 0,
            symmetric_every: 0,
            ..Config::default()
        },
        &mut program,
    );
    assert!(
        campaign
            .evidence
            .iter()
            .any(|e| e.detail.contains("deflection is NaN")),
        "{:?}",
        campaign
            .evidence
            .iter()
            .map(|e| e.detail.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_program_that_cannot_be_launched_is_an_error_not_a_map_of_nans() {
    let mut program = Program::new(ProgramSpec::new(
        vec!["aporia-no-such-program-anywhere".to_string()],
        5_000,
    ));
    let err = program
        .start(1)
        .expect_err("launching a missing file must fail");
    assert!(
        matches!(err, AdapterError::Launch { .. }),
        "unexpected: {err:?}"
    );
    assert!(err.to_string().contains("aporia-no-such-program-anywhere"));
}

#[test]
fn both_sides_of_the_pipe_use_one_protocol() {
    // The example program decodes requests and encodes responses with these very functions, so this
    // is also a check that the format the adapter writes is the format a program reads -- the failure
    // it prevents is a protocol documented in one direction only.
    let request = protocol::encode_request(&[42.0]);
    assert_eq!(protocol::decode_request(&request, 1).unwrap(), vec![42.0]);
    let answer = Answer {
        y: vec![2.1],
        steps: 12,
    };
    assert_eq!(
        protocol::decode_response(&protocol::encode_response(&answer), 1).unwrap(),
        answer
    );
    // A CLI passes a command line through as text; the parse is where a stray flag would surface.
    let spec = ProgramSpec::parse(PROGRAM, 1_000).expect("parses");
    assert_eq!(spec.argv, vec![PROGRAM.to_string()]);
    assert!(ProgramSpec::parse("   ", 1_000).is_none());
}
