//! A finding made by an external program, minimised by that same program.
//!
//! This is the architectural regression test for the execution boundary in minimisation. APORIA can
//! analyse a model whose arithmetic lives in another process, and minimisation is the second half of
//! that claim: after the atlas says *there* is a region worth trusting less, the instrument shrinks
//! the failure to the parameters and digits that cause it and says "this smaller case still fails".
//! Whether it still fails is a question about *the program's* numbers.
//!
//! Answered with the scalar interpreter instead, it is a question about APORIA's reading of a model
//! that declares a value it does not compute — which for this fixture means NaN everywhere, and a NaN
//! output violates `require deflection >= 0` at every load. The interpreter would therefore "verify"
//! that no parameter matters, and the report would carry that as a minimised counterexample of a beam
//! that is actually fine below 60 N. So the third test runs *both* paths on the same finding and
//! asserts they disagree; if routing is ever broken, that is the assertion that says so by name.
//!
//! Nothing is mocked: the answers cross a real pipe to `aporia-example-solver`, which deflects
//! backwards past 60 N and reports 12 units of its own work per answer.

use aporia_adapter::{Program, ProgramSpec};
use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_minimize::{AxisState, Config, FailureOracle, minimize};
use aporia_runtime::{Executor, Outcome};
use aporia_search::{Config as Search, Strategy, run_with};

const PROGRAM: &str = env!("CARGO_BIN_EXE_aporia-example-solver");

/// The beam of the other adapter tests: one load in newtons, one deflection in millimetres, and a
/// rule the program breaks past 60 N. The model deliberately contains no arithmetic for the output,
/// so the only path that can answer about it is the program.
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

/// A real child process that also remembers the points it was asked.
///
/// The capabilities are forwarded rather than restated. A wrapper that reported the interpreter's
/// flags would hand the campaign and the scorer a second precision and a reference path this program
/// does not have, which is the other half of the defect this file is about.
struct Counted {
    program: Program,
    asked: Vec<Vec<f64>>,
}

impl Counted {
    fn new(model: &Model) -> Self {
        let mut program = Program::new(ProgramSpec::new(vec![PROGRAM.to_string()], 10_000));
        program.start(model.outputs.len()).expect("launches");
        Self {
            program,
            asked: Vec::new(),
        }
    }
}

impl Executor for Counted {
    fn execute(&mut self, model: &Model, x: &[f64], cfg: aporia_runtime::ExecConfig) -> Outcome {
        self.asked.push(x.to_vec());
        self.program.execute(model, x, cfg)
    }

    fn varies_with_precision(&self) -> bool {
        self.program.varies_with_precision()
    }

    fn has_reference_path(&self) -> bool {
        self.program.has_reference_path()
    }
}

/// The campaign every test here starts from: the budget and rates the existing subprocess test proves
/// find this region, so the finding is a given rather than a hope. Returns the failing point.
fn failing_load(m: &Model) -> Vec<f64> {
    let mut engine = Counted::new(m);
    let campaign = run_with(
        m,
        Search {
            budget: 200,
            strategy: Strategy::Adaptive,
            probe_every: 0,
            symmetric_every: 0,
            ..Search::default()
        },
        &mut engine,
    );
    assert!(
        !campaign.findings.is_empty(),
        "the program's backwards deflection was not found"
    );
    let start = campaign.findings[0].representative.clone();
    assert!(
        start[0] > 60.0,
        "the finding's representative is at load {}, which the program answers upright",
        start[0]
    );
    start
}

#[test]
fn the_program_that_made_the_finding_is_the_one_that_shrinks_it() {
    let m = beam();
    let start = failing_load(&m);
    let mut engine = Counted::new(&m);
    let before = engine.asked.len();
    let minimal = minimize(
        &FailureOracle::new(&m),
        &mut engine,
        &m,
        &start,
        Config {
            budget: 1_500,
            ..Config::default()
        },
    );
    assert!(
        minimal.verified,
        "the program could not verify its own failure: {}",
        minimal.case.describe()
    );
    // The parameter survives: below 60 N the program answers +2.1 mm and the rule holds, so a case
    // that dropped `load` would claim a failure spanning loads this beam is upright at.
    assert_eq!(
        minimal.case.dimensions(),
        1,
        "the only parameter was dropped, which is what an interpreter-run minimisation does: {}",
        minimal.case.describe()
    );
    let AxisState::Band { lo, hi, .. } = minimal.case.axes[0].state else {
        panic!(
            "expected an interval, got {:?}",
            minimal.case.axes[0].state
        );
    };
    assert!(
        lo > 60.0 && lo < 60.001,
        "the lower edge {lo} is not the boundary the program creates at 60 N"
    );
    assert!(
        (hi - 100.0).abs() < 1e-9,
        "the upper edge {hi} should reach the end of the declared domain"
    );
    // Cost, in the unit this path is in: one answer per query, because a rule question is one
    // round-trip. The program's `steps` count is its own work and is deliberately not converted.
    assert_eq!(
        minimal.executions, minimal.queries,
        "a rule query through a program is exactly one execution of it"
    );
    assert!(minimal.queries > 0);
    let asked = engine.asked.len() - before;
    assert_eq!(
        asked,
        minimal.queries as usize,
        "the points sent down the pipe are not the points minimisation counted"
    );
    assert_eq!(
        engine.program.served(),
        asked as u64,
        "the program answered a different number of requests than were written to it"
    );
}

#[test]
fn every_witness_of_the_minimised_case_was_answered_by_the_program() {
    // A `case` claims "every point I sample here fails". This checks that the claim was produced by
    // the child process at exactly those points, not assumed from a re-verification that quietly used
    // a different computation.
    let m = beam();
    let start = failing_load(&m);
    let mut engine = Counted::new(&m);
    let minimal = minimize(
        &FailureOracle::new(&m),
        &mut engine,
        &m,
        &start,
        Config {
            budget: 1_500,
            ..Config::default()
        },
    );
    assert!(minimal.verified);
    let case = minimal.case;
    let witnesses = case.witnesses();
    let mark = engine.asked.len();
    let check = case.verify(&FailureOracle::new(&m), &mut engine, 4_000);
    assert!(
        check.holds && !check.over_budget,
        "the reduced case did not survive its own witnesses: {check:?}"
    );
    assert_eq!(
        engine.asked[mark..], witnesses,
        "the points re-asked of the program are not the points the case claims"
    );
    assert_eq!(
        check.executions,
        witnesses.len() as u64,
        "witnesses were counted differently from the answers the program gave"
    );
}

#[test]
fn the_interpreter_shrinks_this_case_into_a_claim_about_nan() {
    // The negative control, and the reason the routing is not cosmetic. Same model, same algorithm,
    // same budget: only the path differs, and the two answers are not the same claim.
    let m = beam();
    let start = failing_load(&m);
    let mut engine = Counted::new(&m);
    let cfg = Config {
        budget: 1_500,
        ..Config::default()
    };
    let through_program = minimize(&FailureOracle::new(&m), &mut engine, &m, &start, cfg);
    let mut interp = aporia_runtime::Interp;
    let through_interp = minimize(&FailureOracle::new(&m), &mut interp, &m, &start, cfg);
    assert!(through_program.verified && through_interp.verified);
    assert_eq!(
        through_interp.case.dimensions(),
        0,
        "expected the interpreter to drop the only parameter, got {}",
        through_interp.case.describe()
    );
    assert_ne!(
        through_program.case.describe(),
        through_interp.case.describe(),
        "two different computations produced the same claim, which means one of them was not consulted"
    );
    // And the program's answer at the reduced witness is a number the interpreter never produced.
    let x = through_program.case.representative();
    let program_evidence = FailureOracle::new(&m).evidence_at(&x, &mut engine);
    let interp_evidence = FailureOracle::new(&m).evidence_at(&x, &mut interp);
    assert!(
        !program_evidence
            .iter()
            .any(|e| e.detail.contains("NaN") || e.detail.contains("nan")),
        "the program reported a non-finite value: {:?}",
        program_evidence
    );
    assert!(
        interp_evidence
            .iter()
            .any(|e| e.detail.contains("NaN") || e.detail.contains("nan")),
        "the interpreter did not report the non-finite value that makes its claim meaningless: {:?}",
        interp_evidence
    );
}

#[test]
fn a_program_path_offers_no_second_precision_and_no_reference() {
    // Pinned from the real adapter rather than from the trait's defaults, because these two flags are
    // what stop `RiskScorer` from asking a single program the same question twice and calling the
    // agreement a conditioning signal.
    let m = beam();
    let mut engine = Counted::new(&m);
    assert!(!engine.varies_with_precision(), "one program, one precision");
    assert!(
        !engine.has_reference_path(),
        "no A-IR instructions for a reference evaluator to re-do"
    );
    let out = engine.execute(&m, &[10.0], aporia_runtime::ExecConfig::default());
    assert_eq!(out.outputs, vec![2.1], "the beam is upright at 10 N");
    assert_eq!(out.steps, 12, "the program reports its own work");
    let out = engine.execute(&m, &[80.0], aporia_runtime::ExecConfig::default());
    assert_eq!(out.outputs, vec![-1.4], "and deflects backwards at 80 N");
}
