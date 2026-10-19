//! The scale axis as a dimension of a measurement.
//!
//! E1.4 asks whether the fault in a certified leaf is what the leaf contains or how large the leaf is.
//! That makes the certification rule part of what a row means, exactly as E1.3b made its trust rule
//! part of what a row means, and the same three obligations follow: the default must reproduce every
//! committed byte, a non-default must rename itself in the identity and the archive directory, and a
//! row must report the volumes its own partition produced rather than a number a reader would have to
//! re-derive.
//!
//! The fourth obligation is new to this axis and it is the one the experiment turns on. Withholding
//! certification must change what the atlas *claims* without changing the atlas: if the arm that trusts
//! less also searched differently, the difference between it and the control would be a difference in
//! trajectories and the frozen comparison would be measuring the wrong thing. `Label::adjacency` is
//! what makes that true, and `a_withholding_run_withholds_volume_and_moves_no_leaf` is what proves it.

use aporia_bench::corpus;
use aporia_bench::corpus_root;
use aporia_bench::harness::{
    Plan, RESULTS_SCHEMA, archive_name, identity_of, plan_json, results_json,
};
use aporia_boundary::{Certify, Label, Resolution};
use aporia_search::Strategy;
use aporia_store::{Environment, Json};

fn entry(id: &str) -> corpus::Entry {
    let entries = corpus::load(&corpus_root()).expect("the corpus loads");
    entries
        .iter()
        .find(|e| e.id() == id)
        .unwrap_or_else(|| panic!("{id} is not in the corpus"))
        .clone()
}

/// Every committed results document, so the byte-identity claim is about the published record and not
/// about one convenient file.
fn documents() -> Vec<(Json, std::path::PathBuf)> {
    let dir = corpus_root().join("results");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for file in entries.flatten() {
        let path = file.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if !name.starts_with("results-") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a committed document is readable");
        let value = Json::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        out.push((value, path));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

#[test]
fn a_default_plan_writes_the_bytes_that_named_every_committed_arm() {
    // The reproduction gate's precondition, stated for the new axis: a plan read back from a committed
    // document holds the inert scale rule, and serialises to the same plan section it was read from.
    // Had `resolution` or `certify_share` been written unconditionally, this is the assertion that
    // would fail — and with it the claim that arm A is the same experiment as the arms already
    // published, rather than a new one wearing their numbers.
    //
    // E1.4's documents are in scope for the sharpest version of this: three of its four arms withheld
    // or demanded at a scale rule, so their plan sections *do* carry the axis — and must not carry the
    // criterion that was not invented yet, which would rename a published measurement without changing
    // one number in it. That is not a hypothetical: the reproduction gate found exactly this, and the
    // count below is the assertion that keeps it found.
    let mut republished = 0usize;
    for (doc, path) in documents() {
        // The three experiments whose arms this phase has to reproduce, and nothing else: the earliest
        // documents in this directory predate the strict plan section, so their plan is not an object
        // this reader can parse and their identity belongs to a results schema of its own.
        let protocol = doc
            .get("protocol")
            .and_then(|p| p.get("id"))
            .and_then(Json::as_str)
            .unwrap_or_default();
        if !matches!(
            protocol,
            "e1-geometry" | "e1-3b-trust-resolution" | "e1-4-resolution-demand"
        ) {
            continue;
        }
        let Some(recorded) = doc.get("plan") else {
            panic!("{} records no plan", path.display());
        };
        let plan = Plan::from_json(recorded).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        match protocol {
            "e1-geometry" | "e1-3b-trust-resolution" => {
                assert_eq!(
                    plan.resolution,
                    Resolution::Current,
                    "{} was measured under a rule the document does not name",
                    path.display()
                );
                assert_eq!(
                    plan.certify_share,
                    2f64.powi(-8),
                    "{} carries a bar no committed arm could have acted on",
                    path.display()
                );
            }
            // E1.4's arms are the committed cases where the axis is *not* inert, and the criterion is
            // not one of them: it did not exist when they ran. A plan section that gained a field the
            // run could not have read would be a renamed measurement, so this is the assertion the
            // reproduction gate's identity check rests on.
            _ => {
                assert_eq!(
                    plan.certify,
                    aporia_boundary::Certify::Size,
                    "{} withheld on a criterion it never stated",
                    path.display()
                );
            }
        }
        assert_eq!(
            plan_json(&plan).to_compact(),
            recorded.to_compact(),
            "a committed plan section no longer round-trips: {}",
            path.display()
        );
        let recomputed = identity_of(
            RESULTS_SCHEMA,
            recorded,
            doc.get("entries").expect("a document records its entries"),
        );
        assert_eq!(
            doc.get("identity").and_then(Json::as_str),
            Some(recomputed.as_str()),
            "{}",
            path.display()
        );
        republished += 1;
    }
    // Eleven E1.x arms, eight E1.3b arms and four E1.4 arms: the twenty-three documents this phase's
    // reproduction gate re-measures. A count that fell short would mean a published arm stopped
    // reading back — which is what the gate's identity comparison is for, and what it found once.
    assert_eq!(
        republished, 23,
        "the three committed experiments are twenty-three documents, and {republished} round-tripped"
    );
}

#[test]
fn the_scale_axis_moves_the_identity_and_the_default_does_not() {
    let base = Plan {
        budgets: vec![320, 640],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        ..Plan::default()
    };
    let entries = Json::Arr(vec![Json::object(vec![(
        "entry",
        Json::text("analytic/sqrt_domain"),
    )])]);
    let id = |plan: &Plan| identity_of(RESULTS_SCHEMA, &plan_json(plan), &entries);

    // Inert: a bar the rule never reads cannot move a measurement. This is the half that keeps the
    // reproduction gate honest — an arm that names `Current` and a different threshold is still the
    // committed experiment, because nothing in the instrument consults the number.
    let mut inert = base.clone();
    inert.certify_share = 0.25;
    assert_eq!(
        id(&base),
        id(&inert),
        "an axis that is off still wrote its bar into the identity"
    );
    assert!(
        !plan_json(&inert).to_compact().contains("resolution"),
        "the default axis wrote a field no committed document carries: {}",
        plan_json(&inert).to_compact()
    );

    // Live: each rule states itself, the bar travels with it, and no two of the four collapse.
    let mut sections = Vec::new();
    for (rule, share) in [
        (Resolution::Demand, 0.003_906_25),
        (Resolution::Withhold, 0.003_906_25),
        (Resolution::DemandAndWithhold, 0.003_906_25),
        (Resolution::Demand, 0.0625),
    ] {
        let mut plan = base.clone();
        plan.resolution = rule;
        plan.certify_share = share;
        let json = id(&plan);
        assert_ne!(
            json,
            id(&base),
            "{rule:?} at {share} is arm A under another name"
        );
        sections.push(json);
    }
    let mut distinct = sections.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        sections.len(),
        "the four live arms digest to {} identities, not {}",
        distinct.len(),
        sections.len()
    );
}

#[test]
fn a_withholding_run_withholds_volume_and_moves_no_leaf() {
    // The freeze's control claim, measured on a campaign rather than on a hand-built atlas: arms A and
    // C run the same search over the same evidence and differ only in what the finished partition
    // says about itself.
    let anchor = entry("granular/fit_band");
    let plan = |resolution| Plan {
        budgets: vec![640],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        resolution,
        ..Plan::default()
    };
    let model = anchor.model.as_ref().expect("the band compiles to a model");
    let run =
        |resolution| aporia_search::run(model, plan(resolution).config(Strategy::Adaptive, 640, 1));
    let current = run(Resolution::Current);
    let withheld = run(Resolution::Withhold);

    assert_eq!(
        current.atlas.cells.len(),
        withheld.atlas.cells.len(),
        "withholding certification changed the partition"
    );
    let mut moved = 0usize;
    for (a, b) in current.atlas.cells.iter().zip(&withheld.atlas.cells) {
        assert_eq!(a.bounds, b.bounds, "a cell moved in space");
        assert_eq!(a.depth, b.depth, "a cell moved in depth");
        assert_eq!(a.samples, b.samples, "a cell changed its evidence");
        if a.label != b.label {
            assert_eq!(
                a.label,
                Label::Trusted,
                "only trusted cells may change label"
            );
            assert_eq!(b.label, Label::Provisional);
            moved += 1;
        }
    }
    assert!(
        moved > 0,
        "arm C relabelled nothing, which means the bar is above every leaf the run reached"
    );

    let a = aporia_bench::metrics::Outcome::measure(&anchor, &current, 0, Strategy::Adaptive, 640);
    let c = aporia_bench::metrics::Outcome::measure(&anchor, &withheld, 0, Strategy::Adaptive, 640);
    assert!(
        a.coarse_certified_volume > 0.0,
        "arm A certified nothing coarser than the bar, so there is nothing for E1.4 to decide about"
    );
    assert_eq!(
        c.provisional_volume, a.coarse_certified_volume,
        "the withheld volume is not the volume A certified too coarsely, which is the pair the \
         informativeness test is built on"
    );
    assert_eq!(
        c.coarse_certified_volume, 0.0,
        "a withholding arm cannot certify a coarse leaf"
    );
    assert_eq!(
        c.trusted_over_true_coarse, 0.0,
        "and cannot over-claim from one either"
    );
    // What withholding can remove is exactly what it withholds: the coarse certified volume. On this
    // entry at this budget that turns out to be nothing of the over-claim — the leaves crossing the
    // band's edges were refined by the risk the band itself produces, so their certified volume is
    // under the bar and arm C leaves it alone. That is a measurement, not a property of the fixture,
    // and the frozen metric is built to report it per entry: a policy cannot reduce an over-claim it
    // never certified coarsely.
    assert_eq!(
        a.trusted_over_true - c.trusted_over_true,
        a.trusted_over_true_coarse,
        "the reduction in over-claim is not the volume moved out of certification: A {} C {} coarse {}",
        a.trusted_over_true,
        c.trusted_over_true,
        a.trusted_over_true_coarse
    );
    assert!(
        c.trusted_over_true <= a.trusted_over_true,
        "withholding certification somehow certified more: A {} C {}",
        a.trusted_over_true,
        c.trusted_over_true
    );
    // UNKNOWN is untouched: the withheld volume went to a state of its own, which is the difference
    // between this axis and the one E1.3b measured.
    assert_eq!(
        c.unknown_volume, a.unknown_volume,
        "withholding certification became an uncertainty statistic instead"
    );
}

#[test]
fn a_demanding_run_pays_for_resolution_in_the_columns_the_ceiling_reads() {
    // Arm B changes the partition, so its gains have to be distinguishable from arm C's relabelling.
    // What it costs is in these columns: cells, and the sample floor that a split can starve.
    let anchor = entry("granular/fit_band");
    let config = |resolution| {
        Plan {
            budgets: vec![640],
            strategies: vec![Strategy::Adaptive],
            seeds: vec![1],
            resolution,
            ..Plan::default()
        }
        .config(Strategy::Adaptive, 640, 1)
    };
    let model = anchor.model.as_ref().expect("the band compiles to a model");
    let current = aporia_bench::metrics::Outcome::measure(
        &anchor,
        &aporia_search::run(model, config(Resolution::Current)),
        0,
        Strategy::Adaptive,
        640,
    );
    let demanded = aporia_bench::metrics::Outcome::measure(
        &anchor,
        &aporia_search::run(model, config(Resolution::Demand)),
        0,
        Strategy::Adaptive,
        640,
    );

    assert_eq!(
        demanded.evaluations, current.evaluations,
        "a resolution demand must not cost evaluations: it re-arranges evidence already paid for"
    );
    assert!(
        demanded.leaf_cells > current.leaf_cells,
        "the demand built no extra cells: {} against {}",
        demanded.leaf_cells,
        current.leaf_cells
    );
    assert!(
        demanded.coarse_certified_volume <= current.coarse_certified_volume,
        "looking closer certified more volume than not looking: {} against {}",
        demanded.coarse_certified_volume,
        current.coarse_certified_volume
    );
    assert_eq!(
        demanded.provisional_volume, 0.0,
        "arm B withholds nothing; its only state changes are Trusted, Suspicious and UNKNOWN"
    );
    // The price, stated as a measurement and not as a defect: a leaf split past the sample floor stops
    // being certifiable, so a demand can convert coarse trust into UNKNOWN rather than into knowledge.
    assert!(
        demanded.unknown_volume >= current.unknown_volume,
        "the demand took no volume from anywhere, so it went looking in no cell: {} against {}",
        demanded.unknown_volume,
        current.unknown_volume
    );
}

#[test]
fn the_archive_directory_names_the_scale_rule_and_not_otherwise() {
    // An archive is the bytes a reported number is later opened from. Two arms that are not the same
    // measurement cannot share a directory, and the default arm must keep the directory every committed
    // archive already lives in.
    let e = entry("granular/fit_band");
    let plan = |resolution| Plan {
        resolution,
        ..Plan::default()
    };
    let committed = archive_name(&e, &plan(Resolution::Current), Strategy::Adaptive, 1);
    assert!(
        !committed.contains("-res-"),
        "the default rule renamed the directory every committed archive lives in: {committed}"
    );
    assert_eq!(committed, "granular-fit_band-adaptive-seed1");
    for rule in [
        Resolution::Demand,
        Resolution::Withhold,
        Resolution::DemandAndWithhold,
    ] {
        let name = archive_name(&e, &plan(rule), Strategy::Adaptive, 1);
        assert!(
            name.ends_with(&format!("-res-{}", rule.name())),
            "{rule:?} wrote an archive directory that does not name itself: {name}"
        );
        assert_ne!(name, committed);
    }
}

#[test]
fn a_row_reports_the_volumes_its_own_partition_produced() {
    // Each column is read by the analysis out of the row, so a row that disagreed with its own atlas
    // would make an arm's verdict unattributable — the failure mode `non_finite_readings` was added to
    // close in E1.3b.
    let anchor = entry("granular/stroke_reference");
    for rule in [Resolution::Current, Resolution::Withhold] {
        let plan = Plan {
            budgets: vec![640],
            strategies: vec![Strategy::Adaptive],
            seeds: vec![1],
            resolution: rule,
            ..Plan::default()
        };
        let model = anchor
            .model
            .as_ref()
            .expect("the entry compiles to a model");
        let campaign = aporia_search::run(model, plan.config(Strategy::Adaptive, 640, 1));
        let row =
            aporia_bench::metrics::Outcome::measure(&anchor, &campaign, 0, Strategy::Adaptive, 640);
        let coverage = campaign.atlas.coverage();
        let policy = campaign.atlas.policy();
        let leaves: Vec<&aporia_boundary::Cell> = campaign
            .atlas
            .leaf_ids()
            .iter()
            .filter_map(|id| campaign.atlas.cell(*id))
            .collect();
        let root = campaign.atlas.root();
        let volume = |c: &aporia_boundary::Cell| c.relative_size(root);

        assert_eq!(row.leaf_cells, coverage.cells as u64, "{rule:?}");
        assert_eq!(row.provisional_volume, coverage.provisional, "{rule:?}");
        let coarse_trusted: f64 = leaves
            .iter()
            .filter(|c| c.label == Label::Trusted && policy.coarse(c))
            .map(|c| volume(c))
            .sum();
        assert!(
            (row.coarse_certified_volume - coarse_trusted).abs() < 1e-12,
            "{rule:?}: the row's coarse certified volume is not its own partition's"
        );
        assert!(
            row.trusted_over_true_coarse <= row.trusted_over_true + 1e-12,
            "{rule:?}: a part of the over-claim is larger than the whole of it"
        );
        let json = row.to_json();
        for key in [
            "provisional_volume",
            "coarse_certified_volume",
            "trusted_over_true_coarse",
            "leaf_cells",
        ] {
            assert!(
                json.get(key).is_some(),
                "{rule:?}: the analysis reads {key} out of the row and the row does not carry it"
            );
        }
        let share = json
            .get("coarse_certified_share")
            .cloned()
            .unwrap_or(Json::Null);
        if row.trusted_volume > 0.0 {
            assert!(
                matches!(share, Json::Num(_)),
                "{rule:?}: a row that certified something reports no share of it"
            );
        } else {
            assert!(
                matches!(share, Json::Null),
                "{rule:?}: a row that certified nothing reports a share of zero certification"
            );
        }
    }
}

#[test]
fn a_negative_zero_is_a_signed_absence_and_this_record_says_so_rather_than_hiding_it() {
    // E1.3b reported that `trusted_over_true` prints as `-0.0` on rows where nothing was certified, and
    // handled it as a display question: equal to zero in every comparison, and not worth rewriting the
    // bytes of published rows for. Writing it up as "the sign of a clamp" was half wrong, and the
    // arithmetic here found the other half: a sum over no cells is itself negative zero on this
    // toolchain, before any clamp touches it. That is worth pinning, because every volume column in this
    // report is a sum over the leaves that carry a label, so an arm that labelled nothing produces -0.0
    // as its *natural* answer -- and a reader who takes `provisional_volume: -0.0` to mean "the state
    // fired with a negative volume" would be reading a sign that carries no information.
    let empty: Vec<f64> = Vec::new();
    let nothing: f64 = empty.iter().copied().sum();
    assert!(
        nothing == 0.0 && nothing.is_sign_negative(),
        "a sum over nothing stopped being negative zero, so the claim below needs restating: bits {:#018x}",
        nothing.to_bits()
    );
    assert_eq!(
        nothing, 0.0,
        "and it is still equal to zero, which is what every comparison uses"
    );
    assert!(
        Json::number(nothing).to_compact().contains("-0.0"),
        "the writer prints it as -0.0; the analysis reads it as zero"
    );

    let anchor = entry("granular/fit_band");
    for rule in [
        Resolution::Current,
        Resolution::Demand,
        Resolution::Withhold,
    ] {
        let plan = Plan {
            budgets: vec![320],
            strategies: vec![Strategy::Adaptive],
            seeds: vec![1],
            resolution: rule,
            ..Plan::default()
        };
        let model = anchor.model.as_ref().expect("the band compiles to a model");
        let campaign = aporia_search::run(model, plan.config(Strategy::Adaptive, 320, 1));
        let row =
            aporia_bench::metrics::Outcome::measure(&anchor, &campaign, 0, Strategy::Adaptive, 320);
        // What the columns may not be: a genuinely negative volume. A signed zero is a representation, a
        // negative number would be an arithmetic error, and the two are only distinguishable this way.
        for (name, value) in [
            ("provisional_volume", row.provisional_volume),
            ("coarse_certified_volume", row.coarse_certified_volume),
            ("trusted_over_true_coarse", row.trusted_over_true_coarse),
            ("trusted_volume", row.trusted_volume),
            ("unknown_volume", row.unknown_volume),
            ("suspicious_volume", row.suspicious_volume),
        ] {
            assert!(
                value >= 0.0 || value == 0.0,
                "{rule:?}: {name} is {value}, which is not a volume anyone could certify"
            );
        }
        // The three columns this axis adds carry the same convention as the ones beside them, and the
        // same absence: under `Current` no leaf is provisional, so the sum is empty and signed.
        if rule == Resolution::Current {
            assert_eq!(row.provisional_volume, 0.0, "{rule:?}");
            assert!(
                row.provisional_volume.is_sign_negative(),
                "arm A's provisional column stopped being an empty sum, so the -0.0 claim needs                  re-checking against the code that changed"
            );
        }
    }

    // And the history is untouched: the committed documents still contain the text they were published
    // with, which can only be tested against the bytes and not against a parsed number.
    let rewritten = documents()
        .into_iter()
        .filter(|(_, path)| {
            std::fs::read_to_string(path)
                .unwrap_or_default()
                .contains("-0.0")
        })
        .count();
    assert!(
        rewritten > 0,
        "E1.3b records that committed documents print `-0.0` in their volume columns; none does, so          either the history was rewritten or this assertion is stale"
    );
}

#[test]
fn a_document_names_the_criterion_it_certified_under_even_when_that_criterion_was_inert() {
    // E1.5's four arms differ by one field, so the field has to be readable from a document on its own.
    // The plan section deliberately does *not* carry a criterion that no rule could act on — an inert
    // criterion must not rename a committed measurement, which is what `plan_json` guards and
    // `a_default_plan_writes_the_bytes_that_named_every_committed_arm` proves — so without a header
    // statement a reader of arm A would see `certify_share` and no way to know what it was the bar of.
    let entries = vec![entry("granular/fit_band")];
    let environment = {
        let mut e = Environment::current();
        e.notes.clear();
        e
    };
    let header = |plan: &Plan| results_json(&[], &entries, plan, &environment, None);

    let default = Plan {
        budgets: vec![320],
        seeds: vec![1],
        ..Plan::default()
    };
    let doc = header(&default);
    assert_eq!(
        doc.get("resolution").and_then(Json::as_str),
        Some("current"),
        "the rule is stated even where it is the one every committed arm used"
    );
    assert_eq!(
        doc.get("certify").and_then(Json::as_str),
        Some("size"),
        "a header that states a scale bar without stating what the bar measures is half a rule"
    );
    assert_eq!(
        doc.get("certify_share").and_then(Json::as_f64),
        Some(default.certify_share)
    );
    assert_eq!(
        doc.get("certify_density").and_then(Json::as_f64),
        Some(default.certify_density)
    );
    // The plan section stays byte-identical to what a default plan has always written, which is the
    // half that keeps this addition from renaming anything already published.
    assert_eq!(
        doc.get("plan")
            .expect("a document records its plan")
            .to_compact(),
        plan_json(&default).to_compact()
    );
    assert_eq!(
        doc.get("identity").and_then(Json::as_str),
        Some(
            identity_of(
                RESULTS_SCHEMA,
                &plan_json(&default),
                doc.get("entries").expect("entries"),
            )
            .as_str()
        ),
        "a header field that moved the identity would be a new experiment, not a clearer document"
    );

    let withheld = Plan {
        budgets: vec![320],
        seeds: vec![1],
        resolution: Resolution::Withhold,
        certify: Certify::Density,
        certify_density: 2_048.0,
        ..Plan::default()
    };
    let doc = header(&withheld);
    assert_eq!(
        doc.get("certify").and_then(Json::as_str),
        Some("density"),
        "the arm that withheld on evidence density has to be readable as that arm"
    );
    assert_eq!(
        doc.get("certify_density").and_then(Json::as_f64),
        Some(2_048.0)
    );
    // And the criterion is in the plan section here, because a rule reads it: the same field the
    // default plan omits as inert is the one this plan cannot be re-run without.
    assert!(
        doc.get("plan")
            .and_then(|p| p.get("certify"))
            .and_then(Json::as_str)
            == Some("density"),
        "a criterion a rule acts on belongs in the digest as well as the header"
    );
}

#[test]
fn the_two_adequacy_criteria_relabel_differently_and_move_nothing_else() {
    // E1.5's central comparison is between two criteria, and it is only a comparison if the arms share
    // a search. A leaf is withheld for being large under one and for being thinly measured under the
    // other; neither of those is a statement about where the next evaluation goes, so the partition,
    // the sample counts and the acquisition credit have to come out identical while the *claims* come
    // out different. E1.4 measured what it looks like when the label reaches the credit — three of five
    // entries disagreed on cell counts for no reason but the label — and `Coverage::credit` is the
    // fix; this is the same obligation asserted for the criterion that fix did not exist yet to cover.
    let anchor = entry("granular/fit_band");
    let model = anchor.model.as_ref().expect("the band compiles to a model");
    let plan = |certify, density| Plan {
        budgets: vec![640],
        strategies: vec![Strategy::Adaptive],
        seeds: vec![1],
        resolution: Resolution::Withhold,
        certify,
        certify_density: density,
        ..Plan::default()
    };
    let run = |certify, density| {
        aporia_search::run(
            model,
            plan(certify, density).config(Strategy::Adaptive, 640, 1),
        )
    };
    let by_size = run(Certify::Size, 1_024.0);
    let by_density = run(Certify::Density, 1_024.0);

    assert_eq!(
        by_size.evaluations, by_density.evaluations,
        "the two criteria bought different amounts of evidence, so the arms are not comparable on cost"
    );
    assert_eq!(
        by_size.decisions.len(),
        by_density.decisions.len(),
        "the two searches made a different number of acquisition decisions"
    );
    for (a, b) in by_size.decisions.iter().zip(&by_density.decisions) {
        assert_eq!(
            a.x, b.x,
            "the two criteria placed a different point at evaluation {}",
            a.evaluation
        );
        assert_eq!(
            a.family, b.family,
            "and disagreed about why it was worth buying"
        );
    }
    assert_eq!(
        by_size.atlas.cells.len(),
        by_density.atlas.cells.len(),
        "which criterion certification was measured against changed the partition"
    );
    let mut relabelled = 0usize;
    for (a, b) in by_size.atlas.cells.iter().zip(&by_density.atlas.cells) {
        assert_eq!(a.bounds, b.bounds, "a cell moved in space");
        assert_eq!(a.depth, b.depth, "a cell moved in depth");
        assert_eq!(a.samples, b.samples, "a cell changed its evidence");
        if a.label != b.label {
            // A disagreement between the two criteria is about one thing only: whether this measured
            // leaf is signed off. Neither direction may reach UNKNOWN, which would turn a claim into an
            // uncertainty statistic, and neither may touch SUSPICIOUS, which is decided by the evidence
            // a leaf holds rather than by the bar it is judged against.
            assert!(
                matches!(
                    (a.label, b.label),
                    (Label::Trusted, Label::Provisional) | (Label::Provisional, Label::Trusted)
                ),
                "the two criteria disagreed about something other than signing a leaf off: {:?} \
                 against {:?}",
                a.label,
                b.label
            );
            relabelled += 1;
        }
    }

    let size = by_size.atlas.coverage();
    let density = by_density.atlas.coverage();
    assert!(
        (size.credit() - density.credit()).abs() < 1e-12,
        "the search credited itself differently under two criteria that saw the same leaves: {} vs {}",
        size.credit(),
        density.credit()
    );
    // What the criteria disagree about is visible in the report and invisible to the sampler, which is
    // exactly the separation the experiment needs: `resolved()` is a statement of claims, `credit()` is
    // the signal the next point is placed against.
    assert_eq!(
        size.unknown, density.unknown,
        "a criterion changed how much of the space was unmeasured, which no adequacy rule is allowed \
         to do: {} vs {}",
        size.unknown, density.unknown
    );
    assert_ne!(
        size.resolved(),
        density.resolved(),
        "the two criteria made identical claims on this fixture, so they are one arm measured twice"
    );
    assert_ne!(size.provisional, density.provisional);
    let size_outcome =
        aporia_bench::metrics::Outcome::measure(&anchor, &by_size, 0, Strategy::Adaptive, 640);
    let density_outcome =
        aporia_bench::metrics::Outcome::measure(&anchor, &by_density, 0, Strategy::Adaptive, 640);
    assert_ne!(
        size_outcome.provisional_volume, density_outcome.provisional_volume,
        "the two criteria withheld the same volume on this fixture, so nothing here distinguishes \
         them and the arms would be one arm measured twice"
    );
    assert!(
        relabelled > 0,
        "no leaf was relabelled between the criteria, which is the same failure as the line above"
    );
}
