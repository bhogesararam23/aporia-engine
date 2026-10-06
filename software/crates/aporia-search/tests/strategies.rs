//! The three strategies that were measured before the level-set baseline existed, pinned to the
//! trajectory each one actually followed.
//!
//! These digests were captured at `16fca7f`, over the tree that produced every published number in
//! `software/benchmarks/results/`, by running each campaign and hashing the coordinates it placed in
//! order together with its charged evaluation count and instruction steps. They are here because
//! adding a fourth strategy to the same driver is exactly the change that can perturb the other three
//! by accident — a family index that moves, an array that grows, a tie in the acquisition scores that
//! resolves the other way — and none of those show up as a wrong *conclusion*, only as a comparison
//! where two arms no longer differ in just one thing.
//!
//! If one of these fails, the question is not whether the digest is stale. It is which arm moved, and
//! whether the ladder's published numbers still describe the search they were measured from.

use aporia_dsl::lower::compile;
use aporia_ir::Model;
use aporia_search::{Campaign, Config, Strategy, run};
use aporia_store::digest::sha256_hex;

fn model(text: &str) -> Model {
    let c = compile("t.ap", text);
    assert!(!c.diagnostics.has_errors(), "{}\n{text}", c.diagnostics);
    c.model
}

/// The trajectory and its cost, as one hex digest. Placed points in order, then the two numbers a
/// reader would otherwise have to take on trust: evaluations charged and instruction steps spent.
fn digest(c: &Campaign) -> String {
    let mut bytes: Vec<u8> = Vec::new();
    for d in &c.decisions {
        for v in &d.x {
            bytes.extend(v.to_bits().to_le_bytes());
        }
    }
    bytes.extend(c.evaluations.to_le_bytes());
    bytes.extend(c.instruction_steps.to_le_bytes());
    sha256_hex(&bytes)
}

const SHARP: &str = "model pin_sharp \"\" {\n input x in [0.0, 1.0]\n let y = sqrt(x - 0.4)\n require finite(y)\n}\n";
const COUPLED: &str = "model pin_coupled \"\" {\n input a in [0.0, 10.0]\n input b in [1.0, 40.0]\n let y = a / b - 0.25 * b\n require y >= 0\n}\n";

#[test]
fn a_sharp_one_dimensional_model_follows_the_pinned_trajectory() {
    let m = model(SHARP);
    for (strategy, expected) in [
        (
            Strategy::Random,
            "0210bcb250e752ba927e68bdad058194ce7d3a628eb4e7c6335ff3c278ba9264",
        ),
        (
            Strategy::Stratified,
            "6545e28b67dbb1dc7a05f313fdfd246e16837e17cc475cbfd0c9f4a0703a5e8d",
        ),
        (
            Strategy::Adaptive,
            "eb2a24e4ecf71f10944c5eaedbc8fbd79136190acbe32e377d89e1ca002371cc",
        ),
    ] {
        let c = run(
            &m,
            Config {
                budget: 240,
                seed: 11,
                strategy,
                ..Config::default()
            },
        );
        assert_eq!(
            c.decisions.len(),
            180,
            "{strategy:?} placed a different number of points"
        );
        assert_eq!(digest(&c), expected, "{strategy:?} moved");
    }
}

#[test]
fn a_two_dimensional_model_follows_the_pinned_trajectory() {
    let m = model(COUPLED);
    for (strategy, expected) in [
        (
            Strategy::Random,
            "0972fdf606a081b3a1d07ec64ba518cabda6b7716f215b78608327d443bba93a",
        ),
        (
            Strategy::Stratified,
            "0a4360bcd10716fbc00ec0030823cc01f44f7ec2031c7aa800fbded4b9b236c2",
        ),
        (
            Strategy::Adaptive,
            "0dc5b1aaf6df373162e6e87979fd3b67e298f58737f4e74e83c072f732ff56a4",
        ),
    ] {
        let c = run(
            &m,
            Config {
                budget: 400,
                seed: 12,
                strategy,
                ..Config::default()
            },
        );
        assert_eq!(
            c.decisions.len(),
            200,
            "{strategy:?} placed a different number of points"
        );
        assert_eq!(digest(&c), expected, "{strategy:?} moved");
    }
}
