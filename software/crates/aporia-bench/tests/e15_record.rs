//! The published E1.5 answer, checked against the bytes it says it read.
//!
//! An analysis artifact is a claim about other files, so the useful tests are the ones that open those
//! files again: the protocol digest it cites, the documents it counts, the per-entry metric it reports,
//! and the cost rate whose ceiling decides the outcome. This recomputes all four from the committed
//! results documents and ignores the artifact's own summary wherever the rows can be reached directly —
//! a generator bug cannot agree with itself that way.
//!
//! What is deliberately *not* here: the decision rule's prose, or any reading of what the numbers mean.
//! Those belong to the artifact and to the report. A test that re-derived the outcome from a second copy
//! of the rule would only prove that the same mistake was made twice.

use aporia_bench::corpus_root;
use aporia_store::Json;
use std::path::PathBuf;

const PROTOCOL: &str = "e1-5-spatial-uncertainty-cost";
const ARMS: [&str; 4] = ["A-current", "B-size", "C-density", "D-density-and-demand"];
/// The entry the freeze calls the deciding case, and the one its pre-run measurement located the whole
/// effect on. Pinned by name because a report that quietly changed its central entry would otherwise
/// change its conclusion with it.
const DECIDING: &str = "granular/quiet_band";

fn analysis() -> Json {
    let path = corpus_root()
        .join("analysis")
        .join(format!("{PROTOCOL}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Json::parse(&text).expect("the analysis artifact is JSON")
}

fn results(arm: &str) -> (PathBuf, Json) {
    let artifact = analysis();
    let file = artifact
        .get("documents")
        .and_then(|d| d.get(arm))
        .and_then(|d| d.get("file"))
        .and_then(Json::as_str)
        .unwrap_or_else(|| panic!("the artifact records no file for {arm}"))
        .to_string();
    let path = corpus_root().join("results").join(&file);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    (
        path,
        Json::parse(&text).expect("a results document is JSON"),
    )
}

fn rows(document: &Json) -> Vec<Json> {
    document
        .get("sweeps")
        .and_then(Json::as_array)
        .expect("sweeps")
        .iter()
        .flat_map(|sweep| {
            sweep
                .get("outcomes")
                .and_then(Json::as_array)
                .expect("a sweep has outcomes")
                .iter()
                .cloned()
        })
        .collect()
}

fn number(row: &Json, key: &str) -> f64 {
    row.get(key)
        .and_then(Json::as_f64)
        .unwrap_or_else(|| panic!("a row states no {key}"))
}

/// The frozen metric for one row, with the two states that are not numbers kept apart.
fn share(row: &Json) -> Option<f64> {
    if number(row, "region_overlap_refusals") != 0.0 {
        return None;
    }
    let denominator = number(row, "declared_union_volume");
    if denominator <= 0.0 {
        return None;
    }
    Some(number(row, "trusted_over_true_union") / denominator)
}

#[test]
fn the_artifact_cites_the_bytes_it_read_and_they_have_not_moved() {
    let cited = analysis()
        .get("protocol")
        .and_then(|p| p.get("digest"))
        .and_then(Json::as_str)
        .expect("the artifact cites the freeze by digest")
        .to_string();
    let path = corpus_root()
        .join("protocols")
        .join(format!("{PROTOCOL}.json"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let digest = aporia_store::digest::sha256_hex(&bytes)[..16].to_string();
    assert_eq!(
        cited, digest,
        "the artifact cites {cited} while the tree holds {digest}: either the analysis is stale or the \
         frozen pre-registration moved after the run it reports"
    );
}

#[test]
fn every_arm_the_artifact_reports_is_a_published_document_of_its_own_identity() {
    let artifact = analysis();
    for arm in ARMS {
        let (path, document) = results(arm);
        let recorded = artifact
            .get("documents")
            .and_then(|d| d.get(arm))
            .and_then(|d| d.get("identity"))
            .and_then(Json::as_str)
            .unwrap_or_else(|| panic!("the artifact records no identity for {arm}"));
        assert_eq!(
            document.get("identity").and_then(Json::as_str),
            Some(recorded),
            "{arm}: the artifact reports identity {recorded} for {}",
            path.display()
        );
        let protocol = document
            .get("protocol")
            .expect("a frozen run names its protocol in its document");
        assert_eq!(
            protocol.get("arm").and_then(Json::as_str),
            Some(arm),
            "{arm}: the document it cites belongs to another arm"
        );
        let counted = artifact
            .get("documents")
            .and_then(|d| d.get(arm))
            .and_then(|d| d.get("rows"))
            .and_then(Json::as_u64);
        assert_eq!(
            counted,
            Some(rows(&document).len() as u64),
            "{arm}: the artifact counts a different number of rows than the document holds"
        );
    }
}

#[test]
fn the_deciding_entrys_metric_recomputes_from_the_rows_the_artifact_divides() {
    let artifact = analysis();
    for arm in ARMS {
        let (_, document) = results(arm);
        let measured: Vec<f64> = rows(&document)
            .iter()
            .filter(|row| row.get("entry").and_then(Json::as_str) == Some(DECIDING))
            .filter_map(share)
            .collect();
        let recomputed = measured.iter().sum::<f64>() / measured.len() as f64;
        let reported = artifact
            .get("primary_metric")
            .and_then(|m| m.get("per_entry"))
            .and_then(|entries| entries.get(DECIDING))
            .and_then(|entry| entry.get("arms"))
            .and_then(|arms| arms.get(arm))
            .and_then(|one| one.get("share"))
            .and_then(Json::as_f64)
            .unwrap_or_else(|| panic!("the artifact reports no share for {arm} on {DECIDING}"));
        assert!(
            (recomputed - reported).abs() < 1e-12,
            "{arm} on {DECIDING}: the rows say {recomputed} and the artifact says {reported}"
        );
        assert_eq!(
            artifact
                .get("primary_metric")
                .and_then(|m| m.get("per_entry"))
                .and_then(|entries| entries.get(DECIDING))
                .and_then(|entry| entry.get("arms"))
                .and_then(|arms| arms.get(arm))
                .and_then(|one| one.get("measured"))
                .and_then(Json::as_u64),
            Some(measured.len() as u64),
            "{arm} on {DECIDING}: the artifact averaged a different number of rows than the ones that \
             can carry the metric"
        );
    }
}

#[test]
fn exactly_one_outcome_fires_and_the_rate_that_decided_it_is_the_quotient_it_claims() {
    // The freeze's ordering promise: one outcome, in order, and the branch that fired decided on a
    // quantity a reader can recompute. Which outcome fired is a finding, not a property of the code, so
    // this asserts the shape of the decision and the arithmetic under it rather than its content.
    let artifact = analysis();
    let rule = artifact
        .get("decision_rule")
        .expect("the artifact records the decision");
    let evaluated = rule.get("evaluated").expect("every outcome was evaluated");
    let Json::Obj(evaluated) = evaluated else {
        panic!("the evaluated section was written as something other than an object");
    };
    let true_count = evaluated
        .iter()
        .filter(|(_, value)| value.as_bool() == Some(true))
        .count();
    assert_eq!(
        true_count, 1,
        "the decision rule is supposed to leave exactly one outcome true, and {true_count} are"
    );
    let fired = rule
        .get("fired")
        .and_then(Json::as_str)
        .expect("an outcome fired");
    let fired_is_true = evaluated
        .iter()
        .find(|(name, _)| name.as_str() == fired)
        .and_then(|(_, value)| value.as_bool())
        == Some(true);
    assert!(
        fired_is_true,
        "`fired` names {fired}, which the evaluated section does not hold true"
    );

    let costs = artifact
        .get("cost_rules")
        .expect("the cost rules are recorded");
    for arm in ARMS.iter().skip(1) {
        let entry = costs.get(arm).expect("an arm's cost rule");
        let rate = entry.get("rate").and_then(Json::as_f64);
        let blindness = entry
            .get("blindness_rise")
            .and_then(Json::as_f64)
            .expect("the numerator is recorded");
        let removed = entry
            .get("over_claim_removed")
            .and_then(Json::as_f64)
            .expect("the denominator is recorded");
        if removed > 0.0 {
            assert!(
                (blindness / removed - rate.expect("a rate beside a positive denominator")).abs()
                    < 1e-9,
                "{arm}: the published rate is not the quotient of the two published volumes"
            );
        }
        assert_eq!(
            entry.get("rate_satisfied").and_then(Json::as_bool),
            Some(rate.is_some_and(|value| value <= 1.0)),
            "{arm}: the ceiling of 1.0 is the freeze's, so a rate cannot be reported as satisfying it \
             on a different rule"
        );
    }
}
