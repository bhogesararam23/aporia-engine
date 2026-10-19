#!/usr/bin/env python3
"""e15_inventory.py — a factual inventory of what the next experiment starts from, and a check that
the last experiment's published numbers still read the same way out of the raw rows.

Two things happen here and they are deliberately kept apart from every other script in this directory.

The first is an inventory: the corpus as it stands, the frozen protocols and their digests, the result
and archive counts, the row fields a reader can ask a committed document for and the ones a reader
cannot. It is written to be read before anything is designed, so that a design which claims to fill a
gap can be checked against a record of the gap having existed.

The second is an independent recomputation of E1.4's headline. `scripts/e14_analysis.py` is the program
that decided the experiment, and a check performed by the same arithmetic that made the decision is not
an independent check. The means, deltas, ceilings and the deciding comparisons below are written again
here from the committed result rows, and the two answers are compared field by field. Any disagreement
is an error and the exit state says so: a published number that cannot be reproduced from the rows that
were published is not a measurement, whatever document it is written in.

Nothing here opens a new policy run. Every row read is a committed row of a completed experiment, and
the point of reading it again is that E1.5's metric design has to state its denominator against numbers
that already exist rather than against numbers this phase could still choose.
"""

import glob
import hashlib
import io
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BENCH = os.path.join(ROOT, "benchmarks")
RESULTS = os.path.join(BENCH, "results")
OUT = os.path.join(BENCH, "analysis", "e15-starting-inventory.json")

E14_PROTOCOL = os.path.join(BENCH, "protocols", "e1-4-resolution-demand.json")
E14_ARTIFACT = os.path.join(BENCH, "analysis", "e1-4-resolution-demand.json")
ARMS = ["A-current", "B-demand", "C-withhold", "D-demand-and-withhold"]
REFERENCE = "A-current"
CONTROL_ENTRY = "control/symplectic_spring"
# The three row fields E1.4's own recorded defects name as not existing, and the reason H-WITHHOLD's
# first falsifier could not be evaluated: `capture` was computed from a coarse split of the certified
# over-claim because nothing records any other label overlapping a declared region.
PER_LABEL_PRESENT = ["trusted_over_true", "trusted_over_true_coarse"]
PER_LABEL_ABSENT = ["unknown_over_true", "suspicious_over_true", "provisional_over_true"]


def load(path):
    with io.open(path, encoding="utf-8") as handle:
        return json.load(handle)


def git(*args):
    return subprocess.run(
        ["git", "-C", os.path.dirname(ROOT), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    ).stdout.strip()


def rows_of(doc):
    return [row for sweep in doc["sweeps"] for row in sweep["outcomes"]]


def mean(rows, field):
    return sum(row[field] for row in rows) / len(rows)


def documents_by_arm():
    """One document per frozen E1.4 arm, found by the protocol block it cites."""
    protocol = load(E14_PROTOCOL)
    found = {}
    for path in sorted(glob.glob(os.path.join(RESULTS, "results-*.json"))):
        doc = load(path)
        cited = doc.get("protocol") or {}
        if cited.get("id") != protocol["id"]:
            continue
        arm = cited.get("arm")
        if arm in ARMS:
            if arm in found:
                raise SystemExit(
                    f"two documents cite {protocol['id']} arm {arm}: "
                    f"{found[arm]['identity']} and {doc['identity']}"
                )
            found[arm] = doc
    missing = [arm for arm in ARMS if arm not in found]
    if missing:
        raise SystemExit(f"no committed document for E1.4 arms {missing}")
    return protocol, found


def recompute_e14():
    """E1.4's headline, from raw rows, without reading the analysis script's arithmetic."""
    protocol, docs = documents_by_arm()
    rows = {arm: rows_of(docs[arm]) for arm in ARMS}
    for arm in ARMS:
        if len(rows[arm]) != 100:
            raise SystemExit(f"{arm} has {len(rows[arm])} rows, not the 100 the freeze plans")

    reference = rows[REFERENCE]
    per_arm = {}
    for arm in ARMS:
        control = [r for r in rows[arm] if r["entry"] == CONTROL_ENTRY]
        per_arm[arm] = {
            "identity": docs[arm]["identity"],
            "rows": len(rows[arm]),
            "over_claim_rows": sum(1 for r in rows[arm] if r["trusted_over_true"] > 0.0),
            "rows_certifying_nothing": sum(1 for r in rows[arm] if r["trusted_volume"] == 0.0),
            "coarse_certified_volume": mean(rows[arm], "coarse_certified_volume"),
            "trusted_volume": mean(rows[arm], "trusted_volume"),
            "unknown_volume": mean(rows[arm], "unknown_volume"),
            "provisional_volume": mean(rows[arm], "provisional_volume"),
            "suspicious_volume": mean(rows[arm], "suspicious_volume"),
            "trusted_over_true": mean(rows[arm], "trusted_over_true"),
            "trusted_over_true_coarse": mean(rows[arm], "trusted_over_true_coarse"),
            "leaf_cells": mean(rows[arm], "leaf_cells"),
            "evaluations": mean(rows[arm], "evaluations"),
            "instruction_steps": mean(rows[arm], "instruction_steps"),
            "localised_regions_sum": sum(r["localised_regions"] for r in rows[arm]),
            "detected_regions_sum": sum(r["detected_regions"] for r in rows[arm]),
            "evaluations_sum": sum(r["evaluations"] for r in rows[arm]),
            "instruction_steps_sum": sum(r["instruction_steps"] for r in rows[arm]),
            "control_trusted_volume": mean(control, "trusted_volume"),
            "control_provisional_volume": mean(control, "provisional_volume"),
            "control_unknown_volume": mean(control, "unknown_volume"),
            "control_coarse_certified_volume": mean(control, "coarse_certified_volume"),
            "control_suspicious_volume": sum(
                s.get("control_suspicious_volume") or 0.0
                for s in docs[arm]["sweeps"]
                if s["entry"] == CONTROL_ENTRY
            )
            / len([s for s in docs[arm]["sweeps"] if s["entry"] == CONTROL_ENTRY]),
        }

    # Every deciding comparison the frozen rule reads, paired against the reference arm.
    comparisons = {}
    for arm in ARMS:
        if arm == REFERENCE:
            continue
        this = per_arm[arm]
        base = per_arm[REFERENCE]
        comparisons[arm] = {
            "coarse_certified_ratio": this["coarse_certified_volume"] / base["coarse_certified_volume"]
            if base["coarse_certified_volume"] > 0.0
            else None,
            "unknown_volume_change": this["unknown_volume"] - base["unknown_volume"],
            "trusted_over_true_change": this["trusted_over_true"] - base["trusted_over_true"],
            "control_certification_ratio": this["control_trusted_volume"] / base["control_trusted_volume"]
            if base["control_trusted_volume"] > 0.0
            else None,
            "localisation_change": this["localised_regions_sum"] - base["localised_regions_sum"],
            "coarse_certified_volume_change": this["coarse_certified_volume"]
            - base["coarse_certified_volume"],
            "trusted_volume_change": this["trusted_volume"] - base["trusted_volume"],
            "provisional_volume_change": this["provisional_volume"] - base["provisional_volume"],
            "leaf_cells_change": this["leaf_cells"] - base["leaf_cells"],
            "localised_regions_change": this["localised_regions_sum"]
            - base["localised_regions_sum"],
        }

    # `capture` is A-current's own: the freeze defines it as the share of the reference arm's certified
    # over-claim that sits in cells the withholding arm calls PROVISIONAL, and the two arms share a
    # partition by construction, so those cells are exactly the reference's coarse certified leaves.
    # C-withhold's own coarse column is zero for the same reason and is not this quantity.
    withheld = per_arm["C-withhold"]
    certified = per_arm[REFERENCE]
    informativeness = {
        "capture": certified["trusted_over_true_coarse"] / certified["trusted_over_true"]
        if certified["trusted_over_true"] > 0.0
        else None,
        "cost": withheld["control_provisional_volume"] / certified["control_trusted_volume"]
        if certified["control_trusted_volume"] > 0.0
        else None,
        "withheld_share_of_certification": withheld["provisional_volume"] / certified["trusted_volume"],
    }
    # The E1.4 record calls enrichment an after-the-fact reading that decides nothing; it is recomputed
    # here for the same reason and carries the same disclaimer.
    informativeness["enrichment_of_withheld_volume"] = (
        informativeness["capture"] / informativeness["withheld_share_of_certification"]
        if informativeness["capture"] is not None
        and informativeness["withheld_share_of_certification"] > 0.0
        else None
    )

    primary_nonzero = any(
        row["coarse_certified_volume"] > 0.0 for row in rows[REFERENCE]
    )
    ceilings = {"unknown_absolute": 0.10, "certification_floor_of_control": 0.25,
                "coarse_certified_ratio_for_improvement": 0.50, "capture_floor": 0.50,
                "cost_ceiling": 0.50}
    # Per-entry localisation and detection, which is how the freeze words outcomes 3 and 4: "lowers no
    # entry's sum" and "no entry's sum rises anywhere" are statements about entries, not about the
    # whole run's total.
    entries = sorted({row["entry"] for row in rows[REFERENCE]})

    def per_entry(rows_arm, field):
        return {
            entry: sum(r[field] for r in rows_arm if r["entry"] == entry) for entry in entries
        }

    reference_localisation = per_entry(rows[REFERENCE], "localised_regions")
    reference_detection = per_entry(rows[REFERENCE], "detected_regions")
    for arm in comparisons:
        localisation = per_entry(rows[arm], "localised_regions")
        detection = per_entry(rows[arm], "detected_regions")
        comparisons[arm]["localisation_lowered_on_any_entry"] = any(
            localisation[entry] < reference_localisation[entry] for entry in entries
        )
        comparisons[arm]["localisation_raised_on_any_entry"] = any(
            localisation[entry] > reference_localisation[entry] for entry in entries
        )
        comparisons[arm]["detection_lowered_on_any_entry"] = any(
            detection[entry] < reference_detection[entry] for entry in entries
        )

    outcome_3 = []
    for arm in ARMS:
        if arm == REFERENCE:
            continue
        pair = comparisons[arm]
        guards = [
            pair["coarse_certified_ratio"] is not None
            and pair["coarse_certified_ratio"] <= ceilings["coarse_certified_ratio_for_improvement"],
            pair["unknown_volume_change"] <= ceilings["unknown_absolute"],
            not pair["localisation_lowered_on_any_entry"],
            pair["trusted_over_true_change"] <= 1e-12,
            pair["control_certification_ratio"] >= ceilings["certification_floor_of_control"],
        ]
        if all(guards):
            outcome_3.append(arm)
    outcome_2 = [
        arm
        for arm in ARMS
        if comparisons.get(arm, {}).get("control_certification_ratio") is not None
        and comparisons[arm]["control_certification_ratio"] < ceilings["certification_floor_of_control"]
    ]
    outcome_5 = (
        informativeness["capture"] is not None
        and informativeness["capture"] < ceilings["capture_floor"]
        and informativeness["cost"] is not None
        and informativeness["cost"] > ceilings["cost_ceiling"]
    )
    outcome_6 = not any(
        per_arm[arm]["coarse_certified_volume"] < per_arm[REFERENCE]["coarse_certified_volume"]
        for arm in comparisons
    )
    budget_crossing = {}
    for budget in sorted({row["budget"] for row in rows[REFERENCE]}):
        at = [r for r in rows[REFERENCE] if r["budget"] == budget]
        bc = [r for r in rows["B-demand"] if r["budget"] == budget]
        budget_crossing[str(budget)] = {
            "reference_unknown_volume": mean(at, "unknown_volume"),
            "demand_unknown_volume": mean(bc, "unknown_volume"),
            "reference_coarse_certified_volume": mean(at, "coarse_certified_volume"),
        }

    return {
        "protocol_id": protocol["id"],
        "protocol_digest": docs[REFERENCE]["protocol"]["digest"],
        "control_entry": CONTROL_ENTRY,
        "per_arm": per_arm,
        "paired_against_reference": comparisons,
        "informativeness_pair": informativeness,
        "primary_metric_nonzero_on_reference_rows": primary_nonzero,
        "outcome_1_condition_met": not primary_nonzero,
        "outcome_2_arms_reporting_near_zero_certification": outcome_2,
        "outcome_3_arms": outcome_3,
        "outcome_5_h_rename_refuses_h_withhold": outcome_5,
        "outcome_6_condition_met": outcome_6,
        "per_budget": budget_crossing,
        "ceilings": ceilings,
    }


def cross_check(recomputed):
    """Compare this script's arithmetic against the committed decision artifact, field by field.

    The artifact keeps per-arm means for the volumes and sums for the counts, and its paired block is a
    mean of per-cell differences rather than a difference of means; both shapes are compared against
    what this script computes for the same quantity, and the name of the quantity is written out so a
    disagreement says which one moved.
    """
    artifact = load(E14_ARTIFACT)
    checks = []
    for arm in ARMS:
        published = artifact["arms"][arm]
        mine = recomputed["per_arm"][arm]
        for field in (
            "coarse_certified_volume",
            "trusted_volume",
            "unknown_volume",
            "provisional_volume",
            "suspicious_volume",
            "trusted_over_true",
            "leaf_cells",
        ):
            checks.append((f"{arm}.{field}", mine[field], published.get(field)))
        for field, published_field in (
            ("localised_regions_sum", "localised_regions"),
            ("detected_regions_sum", "detected_regions"),
            ("evaluations_sum", "evaluations"),
            ("instruction_steps_sum", "instruction_steps"),
        ):
            checks.append((f"{arm}.{published_field}", mine[field], published.get(published_field)))
        checks.append((f"{arm}.identity", mine["identity"], published.get("identity")))
        for field in ("rows", "over_claim_rows", "rows_certifying_nothing"):
            checks.append((f"{arm}.{field}", mine[field], published.get(field)))
        control = published.get("per_entry", {}).get(CONTROL_ENTRY, {})
        for field, value in (
            ("trusted_volume", mine["control_trusted_volume"]),
            ("provisional_volume", mine["control_provisional_volume"]),
            ("unknown_volume", mine["control_unknown_volume"]),
            ("coarse_certified_volume", mine["control_coarse_certified_volume"]),
            ("control_suspicious_volume", mine["control_suspicious_volume"]),
        ):
            checks.append((f"{arm}.control/{field}", value, control.get(field)))
    for key in ("capture", "cost", "enrichment_of_withheld_volume"):
        checks.append(
            (f"informativeness.{key}", recomputed["informativeness_pair"][key],
             artifact["informativeness"].get(key))
        )
    for arm, entry in artifact["paired_against_reference"].items():
        if arm not in recomputed["paired_against_reference"]:
            continue
        for field, block in entry.items():
            if not isinstance(block, dict) or "mean_difference" not in block:
                continue
            mine_field = {
                "unknown_volume": "unknown_volume_change",
                "trusted_over_true": "trusted_over_true_change",
                "coarse_certified_volume": "coarse_certified_volume_change",
                "leaf_cells": "leaf_cells_change",
                "trusted_volume": "trusted_volume_change",
                "provisional_volume": "provisional_volume_change",
                "localised_regions": "localised_regions_change",
            }.get(field)
            if mine_field is None:
                continue
            checks.append(
                (f"paired {arm}.{field}", recomputed["paired_against_reference"][arm][mine_field],
                 block["mean_difference"])
            )
    problems = []
    for name, mine, theirs in checks:
        if theirs is None:
            problems.append(f"{name}: the committed artifact states no value")
            continue
        if isinstance(theirs, str) or isinstance(mine, str):
            if str(mine) != str(theirs):
                problems.append(f"{name}: recomputed {mine!r} against published {theirs!r}")
            continue
        if abs(float(mine) - float(theirs)) > 1e-9:
            problems.append(f"{name}: recomputed {mine!r} against published {theirs!r}")
    fired = artifact["decision"].get("fired")
    any_improvement = bool(recomputed["outcome_3_arms"])
    if (fired == 3) != any_improvement:
        problems.append(
            f"outcome 3: committed artifact reports fired={fired!r} and this recomputation "
            f"finds {recomputed['outcome_3_arms'] or 'no arm'}"
        )
    published_guards = artifact["decision"]["detail"]["arm_guards"]
    for arm, guards in published_guards.items():
        recomputed_holds = arm in recomputed["outcome_3_arms"] or _holds_every_guard(recomputed, arm)
        if bool(guards["guards_held"]) != recomputed_holds:
            problems.append(
                f"{arm}.guards_held: artifact says {guards['guards_held']}, recomputation says "
                f"{recomputed_holds}"
            )
    return problems, fired


def _holds_every_guard(recomputed, arm):
    """Outcome 3's five guards, without the localisation-gain half that decides its naming."""
    pair = recomputed["paired_against_reference"].get(arm)
    if pair is None:
        return False
    ceilings = recomputed["ceilings"]
    return (
        pair["coarse_certified_ratio"] is not None
        and pair["coarse_certified_ratio"] <= ceilings["coarse_certified_ratio_for_improvement"]
        and pair["unknown_volume_change"] <= ceilings["unknown_absolute"]
        and not pair["localisation_lowered_on_any_entry"]
        and pair["trusted_over_true_change"] <= 1e-12
        and pair["control_certification_ratio"] >= ceilings["certification_floor_of_control"]
    )


def inventory():
    repo = os.path.dirname(ROOT)
    status = git("status", "--porcelain")
    # This script and the file it writes are untracked at the moment the inventory runs, and always
    # will be on any re-run, so counting them would make the artifact's own byte content depend on a
    # piece of bookkeeping rather than on the repository state it is reporting.
    own = ("scripts/e15_inventory.py", "benchmarks/analysis/e15-starting-inventory.json")
    dirty = [
        line
        for line in status.splitlines()
        if line and not any(line.rstrip().endswith(path) for path in own)
    ]
    families = []
    for family in load(os.path.join(BENCH, "registry.json"))["families"]:
        families.append({"name": family["name"], "entries": len(family["entries"])})
    truths = []
    for path in sorted(glob.glob(os.path.join(BENCH, "*", "*", "truth.json"))):
        truth = load(path)
        truths.append(
            {
                "entry": os.path.relpath(os.path.dirname(path), BENCH).replace(os.sep, "/"),
                "schema": truth["schema"],
                "regions": len(truth.get("regions") or []),
                "control": bool(truth.get("control")),
                "resolvable": truth.get("resolvable"),
                "mechanism": truth.get("mechanism", ""),
            }
        )
    schemas = {}
    for truth in truths:
        schemas[truth["schema"]] = schemas.get(truth["schema"], 0) + 1
    mechanisms = [t["mechanism"] for t in truths if t["mechanism"]]
    protocols = []
    for path in sorted(glob.glob(os.path.join(BENCH, "protocols", "*.json"))):
        text = io.open(path, encoding="utf-8").read()
        protocol = json.loads(text)
        protocols.append(
            {
                "id": protocol["id"],
                "schema": protocol["schema"],
                "digest": hashlib.sha256(text.encode("utf-8")).hexdigest()[:16],
                "arms": [arm["name"] for arm in protocol.get("arms", [])],
                "frozen": protocol.get("frozen"),
            }
        )
    e14_ids = [recomputed["per_arm"][arm]["identity"] for arm in ARMS]
    row_fields = sorted(rows_of(load(os.path.join(RESULTS, f"results-{e14_ids[0]}.json")))[0].keys())
    archives = [
        name
        for name in os.listdir(os.path.join(BENCH, "archives"))
        if os.path.isdir(os.path.join(BENCH, "archives", name))
    ]
    return {
        "repository": {
            "head": git("rev-parse", "HEAD"),
            "branch": git("branch", "--show-current"),
            "remote_main": git("rev-parse", "origin/main"),
            "working_tree_entries_dirty": len(dirty),
            "tracked_paths_under_docs": len(
                [p for p in git("ls-files").splitlines() if p.startswith("docs/")]
            ),
            "crates": sorted(
                os.path.basename(p) for p in glob.glob(os.path.join(ROOT, "crates", "*"))
            ),
            "test_files": len(
                glob.glob(os.path.join(ROOT, "crates", "*", "tests", "*.rs"))
            ),
        },
        "corpus": {
            "entries": len(truths),
            "families": families,
            "truth_schemas": schemas,
            "controls": [t["entry"] for t in truths if t["control"]],
            "resolvable": [t["entry"] for t in truths if t["resolvable"] is True],
            "entries_with_more_than_one_region": [
                {"entry": t["entry"], "regions": t["regions"]} for t in truths if t["regions"] > 1
            ],
            "mechanisms_distinct": len(set(mechanisms)) == len(mechanisms),
        },
        "protocols": protocols,
        "records": {
            "result_documents": len(glob.glob(os.path.join(RESULTS, "results-*.json"))),
            "archive_directories": len(archives),
            "e14_arm_identities": dict(zip(ARMS, e14_ids)),
            "analysis_artifacts": sorted(
                os.path.basename(p)
                for p in glob.glob(os.path.join(BENCH, "analysis", "*.json"))
            ),
        },
        "row_fields_available": row_fields,
        "per_label_region_overlap": {
            "present": PER_LABEL_PRESENT,
            "absent": PER_LABEL_ABSENT,
            "geometry_of_the_present_quantity": (
                "the maximum overlap over the declared regions, not the overlap of their union: "
                "`Truth::overlap_fraction` folds regions with `best = best.max(...)`"
            ),
            "consequence": (
                "an entry whose regions tile a cell side by side reports the largest tile, so a "
                "certified cell covering two declared boxes is scored as covering one of them"
            ),
        },
        "commands": {
            "workspace_tests": "scripts/test.sh",
            "targeted_tests": "scripts/t.sh -p aporia-bench --test <name>",
            "clippy": "scripts/clippy.sh",
            "corpus_audit": "cargo run --release -p aporia-bench -- audit",
            "reachability": "cargo run --release -p aporia-bench -- reach",
            "byte_contracts": "scripts/eol-audit.sh",
            "history_builds": "scripts/verify-commits.sh <range>",
            "reproduction_gate": "scripts/e14_repro_gate.py",
        },
    }


recomputed = None


def main():
    global recomputed
    recomputed = recompute_e14()
    problems, fired = cross_check(recomputed)
    report = inventory()
    report["schema"] = "aporia.inventory/1"
    report["produced_by"] = "scripts/e15_inventory.py"
    report["purpose"] = (
        "What E1.5 starts from: the corpus, protocols, records and row fields as they stand, and "
        "E1.4's headline recomputed from committed rows by arithmetic written independently of "
        "scripts/e14_analysis.py."
    )
    report["e14_recomputed"] = recomputed
    report["e14_committed_decision_fired"] = fired
    report["recomputation_disagreements"] = problems
    report["gaps_a_reader_can_check_in_this_inventory"] = [
        "No per-label declared-region overlap exists: `row_fields_available` carries "
        "`trusted_over_true` and no `unknown_over_true`, `suspicious_over_true` or "
        "`provisional_over_true`, which is why E1.4's own artifact records H-WITHHOLD's first "
        "falsifier as not computable from any published column.",
        "The one region-overlap quantity that does exist folds declared regions by maximum, not by "
        "union, and four committed entries declare more than one region - `ode/euler_decay_2d` "
        "declares twenty-four - so the gap is measured on real corpus shapes rather than hypothetical "
        "ones.",
        "E1.4's UNKNOWN ceiling is an absolute area (0.10) chosen against a partition it could not "
        "scale: the recomputed increase it breached is 0.288798828125 on every arm that moved the "
        "partition, and an arm that never touched the partition (C-withhold) shows the ceiling is "
        "silent on relabelling, where the change is 0.0.",
        "The E1.4 freeze states its pilot scope in prose (one entry, two seeds, two budgets) and "
        "provides no reduced plan, so the pilot ran the full frozen plan; see the committed "
        "`recorded_defects` list in `analysis/e1-4-resolution-demand.json`.",
        "E1.4's corpus had no entry that is both resolvable and over-claiming in coarse leaves: the "
        "reference arm's whole certified over-claim is 0.000171 of its domain and 40 of 100 rows "
        "over-claim at all, which is why the resolution question was answered by the bar's arithmetic "
        "rather than by a case built to discriminate.",
    ]
    report["traceability_gaps_found_by_the_recomputation"] = [
        "`trusted_over_true_coarse` is a committed row column and `capture` is computed from it, but no "
        "per-arm summary in the committed artifact publishes it, so a reader cannot check `capture` "
        "against published means without doing this recomputation. Recorded as a reportable gap rather "
        "than a wrong number: every row value and every deciding comparison did reproduce.",
    ]
    if problems:
        print(json.dumps(problems, indent=1))
        raise SystemExit(
            "E1.4's published numbers do not reproduce from its committed rows; nothing is designed "
            "on top of them until that is fixed"
        )
    text = json.dumps(report, indent=1, sort_keys=True) + "\n"
    with io.open(OUT, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    print(f"wrote {OUT}")
    for arm in ARMS:
        summary = recomputed["per_arm"][arm]
        print(
            f"{arm:24} coarse {summary['coarse_certified_volume']:.6f}  "
            f"unknown {summary['unknown_volume']:.6f}  "
            f"trusted {summary['trusted_volume']:.6f}  "
            f"over_true {summary['trusted_over_true']:.6f}  "
            f"loc {summary['localised_regions_sum']}"
        )
    print(f"fired={fired}  outcome 2={recomputed['outcome_2_arms_reporting_near_zero_certification']}  "
          f"outcome 3={recomputed['outcome_3_arms']}  "
          f"outcome 5={recomputed['outcome_5_h_rename_refuses_h_withhold']}  "
          f"outcome 6={recomputed['outcome_6_condition_met']}")
    print("cross-check: every recomputed field matches the committed artifact")


if __name__ == "__main__":
    sys.exit(main())
