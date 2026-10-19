//! The E1.3b record, recomputed from the published documents rather than restated from prose.
//!
//! E1.4 is built on top of E1.3b's result, and its freeze cites that result as the reason the trust
//! axis is closed: requiring more channels made certified-over-true volume *larger*, the non-finite rule
//! never fired, and the only arm that reached zero got there by trusting nothing at all. If any of that
//! stopped being true of these documents — an edit to a row, a rerun under a changed default, an
//! analysis that quietly recomputed a mean — the follow-up would be citing something that no longer
//! says it. So the readings are recomputed here from the bytes, and the analysis artefact is checked
//! against them.
//!
//! What this does *not* do is re-decide E1.3b. The decision rule it fired under belongs to that freeze
//! and is read back from it; nothing here applies E1.4's metric to the earlier rows, and nothing here
//! is a new measurement.

use aporia_bench::corpus_root;
use aporia_store::Json;
use std::path::PathBuf;

const PROTOCOL: &str = "e1-3b-trust-resolution";
const RESOLVABLE: [&str; 5] = [
    "granular/rail_ceiling",
    "granular/stroke_reference",
    "granular/match_return",
    "granular/diagonal_count",
    "granular/tolerance_stack",
];
const SUB_GRANULAR: [&str; 3] = [
    "edge/overflow_tail",
    "edge/pole_at_edge",
    "geometry/narrow_oblique",
];

fn documents() -> Vec<(String, Json)> {
    let dir = corpus_root().join("results");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut names: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("results-") && n.ends_with(".json"))
        })
        .map(|p| (p.display().to_string(), p))
        .collect();
    names.sort();
    for (_, path) in names {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = Json::parse(&text) else {
            continue;
        };
        let cited = value
            .get("protocol")
            .and_then(|p| p.get("id"))
            .and_then(Json::as_str)
            .unwrap_or_default();
        if cited == PROTOCOL {
            let arm = value
                .get("protocol")
                .and_then(|p| p.get("arm"))
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string();
            out.push((arm, value));
        }
    }
    out
}

/// Every row of one arm's document, flattened.
fn rows(doc: &Json) -> Vec<Json> {
    doc.get("sweeps")
        .and_then(Json::as_array)
        .map(|sweeps| {
            sweeps
                .iter()
                .flat_map(|sweep| {
                    sweep
                        .get("outcomes")
                        .and_then(Json::as_array)
                        .map(|rows| rows.to_vec())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn number(row: &Json, key: &str) -> f64 {
    row.get(key).and_then(Json::as_f64).unwrap_or(0.0)
}

fn count(row: &Json, key: &str) -> u64 {
    row.get(key).and_then(Json::as_u64).unwrap_or(0)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

#[test]
fn eight_arms_eight_entries_twenty_rows_each_and_their_sets_are_the_freezes() {
    let documents = documents();
    assert_eq!(documents.len(), 8, "E1.3b published eight arms");
    for (arm, doc) in &documents {
        let rows = rows(doc);
        assert_eq!(
            rows.len(),
            160,
            "{arm}: five seeds x four budgets x eight entries"
        );
        assert_eq!(
            doc.get("entries")
                .and_then(Json::as_array)
                .map_or(0, |items: &[Json]| items.len()),
            8,
            "{arm}"
        );
        assert_eq!(
            doc.get("trust").and_then(Json::as_str),
            Some(if arm.contains("min-channels-2") {
                "min-channels-2"
            } else if arm.contains("nonfinite") {
                "non-finite-blocks-trust"
            } else if arm.contains("both-trust-rules") {
                "both-trust-rules"
            } else {
                "current"
            }),
            "{arm}: the document's rule is not the one its name states"
        );
        // The defect the follow-up closes: no entry in this run list is a control, so the freeze's
        // fourth quantity had nothing to be computed from.
        assert!(
            rows.iter()
                .all(|row| !row.get("control").and_then(Json::as_bool).unwrap_or(false)),
            "{arm}: a control appeared in E1.3b's run list, and the record says none did"
        );
        assert!(
            rows.iter()
                .all(|row| count(row, "non_finite_readings") == 0),
            "{arm}: a non-finite reading appeared, and the committed finding is that the rule never \
             had anything to act on"
        );
    }
}

#[test]
fn the_reading_that_decided_e1_3b_still_reads_the_same_way() {
    let documents = documents();
    let arm = |wanted: &str| -> Json {
        documents
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, doc)| doc.clone())
            .unwrap_or_else(|| panic!("no committed {wanted} document"))
    };
    let in_set = |row: &Json, set: &[&str]| -> bool {
        set.contains(&row.get("entry").and_then(Json::as_str).unwrap_or_default())
    };

    let current = arm("full-current-trust");
    let min_two = arm("full-min-channels-2");
    let non_finite = arm("full-nonfinite-blocks-trust");
    let silence = arm("only-physical-both-trust-rules");

    let over_rows = |doc: &Json, set: &[&str]| -> usize {
        rows(doc)
            .iter()
            .filter(|row| in_set(row, set) && number(row, "trusted_over_true") > 0.0)
            .count()
    };
    let unknown_mean = |doc: &Json, set: &[&str]| -> f64 {
        mean(
            &rows(doc)
                .iter()
                .filter(|row| in_set(row, set))
                .map(|row| number(row, "unknown_volume"))
                .collect::<Vec<_>>(),
        )
    };
    let trusted_mean = |doc: &Json, set: &[&str]| -> f64 {
        mean(
            &rows(doc)
                .iter()
                .filter(|row| in_set(row, set))
                .map(|row| number(row, "trusted_volume"))
                .collect::<Vec<_>>(),
        )
    };
    let localised = |doc: &Json, set: &[&str]| -> u64 {
        rows(doc)
            .iter()
            .filter(|row| in_set(row, set))
            .map(|row| count(row, "localised_regions"))
            .sum()
    };

    // The resolvable set: nine rows of a hundred certified volume inside a declared region under the
    // rule already in force, and requiring a second applied channel made that *worse*, not better.
    assert_eq!(
        over_rows(&current, &RESOLVABLE),
        9,
        "the committed over-claim count moved"
    );
    assert_eq!(over_rows(&min_two, &RESOLVABLE), 11);
    assert_eq!(over_rows(&non_finite, &RESOLVABLE), 9);
    assert!(
        unknown_mean(&min_two, &RESOLVABLE) > unknown_mean(&current, &RESOLVABLE),
        "the tighter rule stopped trading UNKNOWN area for nothing"
    );
    assert_eq!(
        localised(&current, &RESOLVABLE),
        localised(&min_two, &RESOLVABLE),
        "localisation moved under the channel rule, and the record says it did not"
    );

    // The sub-granular control: 18 of 60 rows over-claim, which is fewer than the freeze expected, and
    // the discrepancy is recorded rather than repaired.
    assert_eq!(over_rows(&current, &SUB_GRANULAR), 18);
    assert_eq!(over_rows(&silence, &SUB_GRANULAR), 0);

    // Rule 3's arithmetic: the only arm at zero over-claim reaches it by certifying nothing, and its
    // UNKNOWN rise is measured the way the freeze measures every arm -- against the same ablation mask
    // under the rule already in force, not against a different mask. Pairing the silence arm with
    // `full-current-trust` instead compares two point-placement policies and one trust rule at once,
    // and gives 0.453 where the published record says 0.522.
    let silence_reference = arm("only-physical-current-trust");
    assert_eq!(over_rows(&silence, &RESOLVABLE), 0);
    assert_eq!(over_rows(&silence_reference, &RESOLVABLE), 9);
    assert_eq!(trusted_mean(&silence, &RESOLVABLE), 0.0);
    let rise = unknown_mean(&silence, &RESOLVABLE) - unknown_mean(&silence_reference, &RESOLVABLE);
    assert!(
        rise > 0.05,
        "the silence arm's UNKNOWN rise over its own mask is {rise}, and rule 3 fired on it exceeding \
         the frozen 0.05 ceiling"
    );
    assert_eq!(
        localised(&silence, &RESOLVABLE),
        localised(&silence_reference, &RESOLVABLE)
    );
    assert_eq!(
        localised(&silence, &RESOLVABLE),
        localised(&current, &RESOLVABLE)
    );

    // And the analysis artefact agrees with the documents it claims to summarise.
    let path = corpus_root()
        .join("analysis")
        .join("e1-3b-trust-resolution.json");
    let text = std::fs::read_to_string(path).expect("the analysis artefact is committed");
    let artifact = Json::parse(&text).expect("it is JSON");
    assert_eq!(
        artifact
            .get("decision")
            .and_then(|d| d.get("fired"))
            .and_then(Json::as_u64),
        Some(3),
        "the recorded decision was rule 3"
    );
    let recorded = artifact
        .get("decision")
        .and_then(|d| d.get("arms"))
        .and_then(Json::as_array)
        .and_then(|arms| {
            arms.iter().find(|entry| {
                entry.get("arm").and_then(Json::as_str) == Some("only-physical-both-trust-rules")
            })
        })
        .and_then(|entry| entry.get("mean_unknown_volume_change"))
        .and_then(Json::as_f64);
    assert!(
        recorded.is_some_and(|value| (value - rise).abs() < 1e-6),
        "the artefact's {recorded:?} no longer matches the {rise} the documents give"
    );
    assert!(
        artifact
            .get("secondary")
            .and_then(|s| s.get("control_cleanliness"))
            .and_then(|c| c.get("measured"))
            .and_then(Json::as_bool)
            == Some(false),
        "the unmeasurable fourth quantity has become measurable, which means it was retro-fitted"
    );
}
