#!/usr/bin/env python3
"""e14_analysis.py — read E1.4's four arms against the metric the freeze fixed, in the order it fixed it.

Nothing here chooses a quantity after seeing a result. Every reading is named in
`benchmarks/protocols/e1-4-resolution-demand.json`: the primary metric, its denominator and aggregation,
the paired comparison, the informativeness pair that decides the new state, the two ceilings, and the
seven deciding outcomes in order. This script computes those and applies that rule; it prints what fired
and what did not.

Three properties of the run are checked before any number is read, because the freeze makes them
preconditions rather than courtesies: the reproduction gate has to have passed, the infrastructure pilot
has to have passed, and both have to cite the same binary and the same protocol bytes as the documents
being analysed. A result produced by different bytes than the gate checked is not that result.

The rule's ordering matters and is preserved: the null outcome is evaluated first, then the silence
guard, then the improvement, and only then the favourable readings. An arm that certifies almost nothing
cannot be rescued by a later clause.

Two output artefacts: the machine-readable JSON committed under `benchmarks/analysis`, and the table the
results README quotes. The numbers are never copied by hand from one to the other.
"""

import argparse
import io
import json
import os
import sys
from collections import defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(ROOT, "benchmarks", "results")
PROTOCOL = os.path.join(ROOT, "benchmarks", "protocols", "e1-4-resolution-demand.json")
GATE = os.path.join(ROOT, "benchmarks", "analysis", "e1-4-reproduction-gate.json")
PILOT = os.path.join(ROOT, "benchmarks", "analysis", "e1-4-pilot.json")
ARTIFACT = os.path.join(ROOT, "benchmarks", "analysis", "e1-4-resolution-demand.json")

ARMS = ["A-current", "B-demand", "C-withhold", "D-demand-and-withhold"]
REFERENCE = "A-current"
CEILING_UNKNOWN = 0.10
CERTIFICATION_FLOOR = 0.25
IMPROVEMENT_RATIO = 0.50
CAPTURE_FLOOR = 0.50
COST_CEILING = 0.50


def load(path):
    with io.open(path, encoding="utf-8") as handle:
        return json.load(handle)


def rows_of(doc):
    return [row for sweep in doc["sweeps"] for row in sweep["outcomes"]]


def find_documents(protocol_id, problems):
    """One document per frozen arm, found by the protocol block it cites rather than by filename."""
    found = {}
    for name in sorted(os.listdir(RESULTS)):
        if not name.startswith("results-") or not name.endswith(".json"):
            continue
        doc = load(os.path.join(RESULTS, name))
        cited = (doc.get("protocol") or {}).get("id")
        if cited != protocol_id:
            continue
        arm = (doc.get("protocol") or {}).get("arm")
        if arm in found:
            problems.append(f"two documents cite {protocol_id} arm {arm}")
        found[arm] = doc
    missing = [arm for arm in ARMS if arm not in found]
    if missing:
        problems.append(f"no document cites arm(s) {missing}")
    return found


def mean(values):
    return sum(values) / len(values) if values else 0.0


def per_arm(doc, key):
    return mean([row.get(key) or 0.0 for row in rows_of(doc)])


def by_entry(doc, key):
    out = defaultdict(float)
    for row in rows_of(doc):
        out[row["entry"]] += row.get(key) or 0.0
    return dict(out)


def paired(direct, other):
    """Mean paired difference over the shared (entry, seed, budget) cells, and the cells compared."""
    keys = set(direct) & set(other)
    if not keys:
        return 0.0, 0
    return mean([direct[k] - other[k] for k in keys]), len(keys)


def index_rows(doc):
    return {(r["entry"], r["seed"], r["budget"]): r for r in rows_of(doc)}


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", action="store_true", help="write the analysis artefact")
    args = parser.parse_args(argv)
    problems = []
    protocol = load(PROTOCOL)
    ceiling = CEILING_UNKNOWN
    documents = find_documents(protocol["id"], problems)
    if problems:
        for problem in problems:
            print(f"  {problem}")
        return 2

    # Preconditions the freeze states as stopping rules, checked before any reading.
    binaries = {}
    for label, path in (("reproduction gate", GATE), ("pilot", PILOT)):
        if not os.path.exists(path):
            problems.append(f"{label} artefact is missing: {path}")
            continue
        artifact = load(path)
        if artifact.get("verdict") != "PASSED":
            problems.append(f"{label} verdict is {artifact.get('verdict')!r}, not PASSED")
        cited_binary = artifact.get("binary_sha256")
        if not cited_binary:
            problems.append(f"{label} names no binary: its PASSED verdict belongs to unknown bytes")
            continue
        binaries.setdefault(cited_binary, []).append(label)
    if len(binaries) > 1:
        problems.append(
            "the gate and the pilot were measured by different bytes: "
            + "; ".join(f"{digest} -> {labels}" for digest, labels in sorted(binaries.items()))
        )
    binary_digest = next(iter(binaries), None)
    plan_digest = None
    with open(PROTOCOL, "rb") as handle:
        plan_digest = __import__("hashlib").sha256(handle.read()).hexdigest()[:16]
    for arm, doc in documents.items():
        cited = (doc.get("protocol") or {}).get("digest")
        if cited != plan_digest:
            problems.append(
                f"{arm}: cites protocol digest {cited}, but the freeze now digests to {plan_digest}"
            )
    if problems:
        for problem in problems:
            print(f"  precondition failed: {problem}")
        return 1

    bar = protocol["plan"]["certify_share"]
    control_entries = protocol["entries"]["control"]
    resolvable = set(protocol["entries"]["resolvable"])

    arms = {}
    for arm in ARMS:
        doc = documents[arm]
        rows = rows_of(doc)
        resolution = doc.get("resolution")
        summary = {
            "resolution_rule": resolution,
            "identity": doc["identity"],
            "rows": len(rows),
            "coarse_certified_volume": per_arm(doc, "coarse_certified_volume"),
            "coarse_certified_share": mean(
                [
                    row["coarse_certified_share"]
                    for row in rows
                    if row.get("coarse_certified_share") is not None
                ]
            ),
            "rows_certifying_nothing": sum(
                1 for row in rows if not (row.get("trusted_volume") or 0.0) > 0.0
            ),
            "trusted_over_true": per_arm(doc, "trusted_over_true"),
            "trusted_volume": per_arm(doc, "trusted_volume"),
            "unknown_volume": per_arm(doc, "unknown_volume"),
            "provisional_volume": per_arm(doc, "provisional_volume"),
            "suspicious_volume": per_arm(doc, "suspicious_volume"),
            "over_claim_rows": sum(1 for row in rows if (row.get("trusted_over_true") or 0.0) > 0.0),
            "localised_regions": sum(row.get("localised_regions") or 0 for row in rows),
            "detected_regions": sum(row.get("detected_regions") or 0 for row in rows),
            "leaf_cells": per_arm(doc, "leaf_cells"),
            "evaluations": sum(row.get("evaluations") or 0 for row in rows),
            "instruction_steps": sum(row.get("instruction_steps") or 0 for row in rows),
            "per_entry": {
                entry: {
                    "coarse_certified_volume": mean(
                        [
                            row["coarse_certified_volume"] or 0.0
                            for row in rows
                            if row["entry"] == entry
                        ]
                    ),
                    "trusted_volume": mean(
                        [row["trusted_volume"] or 0.0 for row in rows if row["entry"] == entry]
                    ),
                    "unknown_volume": mean(
                        [row["unknown_volume"] or 0.0 for row in rows if row["entry"] == entry]
                    ),
                    "provisional_volume": mean(
                        [row["provisional_volume"] or 0.0 for row in rows if row["entry"] == entry]
                    ),
                    "trusted_over_true": mean(
                        [row["trusted_over_true"] or 0.0 for row in rows if row["entry"] == entry]
                    ),
                    "localised_regions": by_entry(doc, "localised_regions").get(entry, 0.0),
                    "detected_regions": by_entry(doc, "detected_regions").get(entry, 0.0),
                    "leaf_cells": mean(
                        [row["leaf_cells"] or 0.0 for row in rows if row["entry"] == entry]
                    ),
                    "control": bool(
                        [row.get("control") for row in rows if row["entry"] == entry][:1]
                    ),
                    "control_suspicious_volume": mean(
                        [
                            sweep["control_suspicious_volume"]
                            for sweep in doc["sweeps"]
                            if sweep["entry"] == entry
                            and sweep.get("control_suspicious_volume") is not None
                        ]
                    ),
                }
                for entry in sorted({row["entry"] for row in rows})
            },
            "per_budget": {
                str(budget): {
                    "coarse_certified_volume": mean(
                        [
                            row["coarse_certified_volume"] or 0.0
                            for row in rows
                            if row["budget"] == budget
                        ]
                    ),
                    "trusted_volume": mean(
                        [row["trusted_volume"] or 0.0 for row in rows if row["budget"] == budget]
                    ),
                    "unknown_volume": mean(
                        [row["unknown_volume"] or 0.0 for row in rows if row["budget"] == budget]
                    ),
                    "provisional_volume": mean(
                        [row["provisional_volume"] or 0.0 for row in rows if row["budget"] == budget]
                    ),
                    "localised_regions": sum(
                        row.get("localised_regions") or 0 for row in rows if row["budget"] == budget
                    ),
                    "leaf_cells": mean(
                        [row["leaf_cells"] or 0.0 for row in rows if row["budget"] == budget]
                    ),
                }
                for budget in sorted({row["budget"] for row in rows})
            },
        }
        arms[arm] = summary

    reference = arms[REFERENCE]
    keyed = {arm: index_rows(documents[arm]) for arm in ARMS}

    # Paired differences against the control arm, per arm and on the deciding quantities.
    comparisons = {}
    for arm in ARMS:
        if arm == REFERENCE:
            continue

        def paired_key(key):
            direct = {k: (v.get(key) or 0.0) for k, v in keyed[arm].items()}
            base = {k: (v.get(key) or 0.0) for k, v in keyed[REFERENCE].items()}
            delta, cells = paired(direct, base)
            return {"mean_difference": delta, "cells": cells}

        rows_arm = rows_of(documents[arm])
        comparisons[arm] = {
            "coarse_certified_volume": paired_key("coarse_certified_volume"),
            "unknown_volume": paired_key("unknown_volume"),
            "provisional_volume": paired_key("provisional_volume"),
            "trusted_volume": paired_key("trusted_volume"),
            "trusted_over_true": paired_key("trusted_over_true"),
            "leaf_cells": paired_key("leaf_cells"),
            "localised_regions": paired_key("localised_regions"),
            "rows_where_partitions_differ": sum(
                1
                for k in set(keyed[arm]) & set(keyed[REFERENCE])
                if (keyed[arm][k].get("leaf_cells") or 0)
                != (keyed[REFERENCE][k].get("leaf_cells") or 0)
            ),
            "per_seed": {
                str(seed): mean(
                    [
                        (keyed[arm][k].get("coarse_certified_volume") or 0.0)
                        - (keyed[REFERENCE][k].get("coarse_certified_volume") or 0.0)
                        for k in keyed[arm]
                        if k in keyed[REFERENCE] and k[1] == seed
                    ]
                )
                for seed in sorted({k[1] for k in keyed[arm]})
            },
        }

    # The informativeness pair the freeze defines for the new state.
    over_reference = sum(
        row.get("trusted_over_true") or 0.0 for row in rows_of(documents[REFERENCE])
    )
    over_coarse = sum(
        row.get("trusted_over_true_coarse") or 0.0 for row in rows_of(documents[REFERENCE])
    )
    capture = (over_coarse / over_reference) if over_reference > 0.0 else None
    withheld_on_control = mean(
        [
            row["provisional_volume"] or 0.0
            for row in rows_of(documents["C-withhold"])
            if row.get("control")
        ]
    )
    certified_on_control = mean(
        [row["trusted_volume"] or 0.0 for row in rows_of(documents[REFERENCE]) if row.get("control")]
    )
    cost = (withheld_on_control / certified_on_control) if certified_on_control > 0.0 else None
    # Descriptive, not a frozen test: `capture` says how much of A's real over-claim sits in the volume
    # the withholding rule would refuse, and says nothing on its own until it is read against the share
    # of certification that volume covers. A set holding 91% of certification and 67% of the over-claim
    # is not enriched for violation, and the freeze's pair cannot see that difference.
    certified_share_of_domain = reference["trusted_volume"]
    withheld_share_of_certification = (
        reference["coarse_certified_volume"] / certified_share_of_domain
        if certified_share_of_domain > 0.0
        else None
    )
    enrichment = (
        capture / withheld_share_of_certification
        if capture is not None and withheld_share_of_certification
        else None
    )
    informativeness = {
        "enrichment_of_withheld_volume": enrichment,
        "enrichment_definition": (
            "capture divided by the share of A-current's certification the withheld volume covers; "
            "1.0 means the withheld set holds exactly its share of the real over-claim, above 1.0 means "
            "coarse certification is where the over-claim actually lives. Derived after the fact from "
            "the two frozen quantities and decides nothing -- no outcome in `decision_rule` reads it."
        ),
        "withheld_share_of_certification": withheld_share_of_certification,
        "capture": capture,
        "capture_definition": (
            "arm A's trusted_over_true volume that sits in leaves coarser than the bar, over its whole "
            "trusted_over_true volume; under a withholding arm that volume is what moves out of "
            "certification, because the two arms share the partition"
        ),
        "cost": cost,
        "cost_definition": (
            "the withholding arm's PROVISIONAL volume on the control entry over the control arm's "
            "TRUSTED volume there: certification withdrawn where the entry declares nothing to find"
        ),
        "control_suspicious_volume_measured": {
            arm: arms[arm]["per_entry"].get(entry, {}).get("control_suspicious_volume")
            for arm in ARMS
            for entry in control_entries
        },
    }

    # The seven outcomes, evaluated in the frozen order. The text of each is the freeze's own.
    rule = protocol["primary_metric"]["decision_rule"]
    fired = None
    detail = {}
    if all(arms[arm]["coarse_certified_volume"] == 0.0 for arm in ARMS):
        fired = 1
        detail = {
            "reading": "no arm certified anything coarser than the bar, including the control",
        }
    else:
        silent = [
            arm
            for arm in ARMS
            if arms[arm]["per_entry"]
            and all(
                (arms[arm]["per_entry"].get(entry, {}).get("trusted_volume") or 0.0)
                < CERTIFICATION_FLOOR
                * (reference["per_entry"].get(entry, {}).get("trusted_volume") or 0.0)
                for entry in control_entries
            )
        ]
        detail["certifies_almost_nothing"] = silent
        # Outcome 2 is a statement about an arm, not the experiment's verdict -- the freeze says such an
        # arm "is not an improvement on any reading of any other metric" -- so it is recorded per arm
        # above and excluded from the outcomes that decide, rather than ending the evaluation.
        def guards(arm):
            """Every clause outcome 3 states, as five separate booleans, plus the one clause that
            separates outcome 3 from outcome 4.
            """
            summary = arms[arm]
            checks = {
                "cuts_coarse_certified_in_half": summary["coarse_certified_volume"]
                <= IMPROVEMENT_RATIO * reference["coarse_certified_volume"],
                "unknown_within_ceiling": (summary["unknown_volume"] - reference["unknown_volume"])
                <= CEILING_UNKNOWN,
                "over_claim_not_raised": summary["trusted_over_true"]
                <= reference["trusted_over_true"] + 1e-12,
                "holds_control_certification": all(
                    (summary["per_entry"].get(entry, {}).get("trusted_volume") or 0.0)
                    >= CERTIFICATION_FLOOR
                    * (reference["per_entry"].get(entry, {}).get("trusted_volume") or 0.0)
                    for entry in control_entries
                ),
                # Outcome 4's exception is carved out rather than folded in: the two outcomes share
                # every guard here, and differ only on whether the arm found anything new.
                "no_localisation_loss": all(
                    (summary["per_entry"].get(entry, {}).get("localised_regions") or 0.0)
                    >= (reference["per_entry"].get(entry, {}).get("localised_regions") or 0.0) - 1e-12
                    for entry in reference["per_entry"]
                ),
            }
            localisation_gained = any(
                (summary["per_entry"].get(entry, {}).get("localised_regions") or 0.0)
                > (reference["per_entry"].get(entry, {}).get("localised_regions") or 0.0) + 1e-12
                for entry in summary["per_entry"]
            )
            held = all(checks.values())
            return {"checks": checks, "guards_held": held, "localisation_gained": localisation_gained}

        eligible = [arm for arm in ARMS if arm != REFERENCE and arm not in silent]
        detail["arm_guards"] = {arm: guards(arm) for arm in eligible}
        improvements = [
            arm for arm in eligible if guards(arm)["guards_held"] and guards(arm)["localisation_gained"]
        ]
        discipline = [
            arm
            for arm in eligible
            if guards(arm)["guards_held"] and not guards(arm)["localisation_gained"]
        ]
        # The shape the freeze did not enumerate: an arm that satisfies every guard except the UNKNOWN
        # ceiling. H-RESOLVE says it "is falsified by any ceiling in `decision_rule` being missed", so
        # such an arm is neither outcome 3 nor outcome 4 -- recording it as discipline would let the
        # intervention's size buy a permitted verdict, which is the thing the ceiling was set for.
        ceiling_only = [
            arm
            for arm in eligible
            if all(v for k, v in guards(arm)["checks"].items() if k != "unknown_within_ceiling")
            and not guards(arm)["checks"]["unknown_within_ceiling"]
        ]
        detail["improvements"] = improvements
        detail["arms_breaching_only_the_unknown_ceiling"] = ceiling_only
        renamed = (
            capture is not None
            and cost is not None
            and capture < CAPTURE_FLOOR
            and cost > COST_CEILING
        )
        nobody_cut = all(
            arms[arm]["coarse_certified_volume"] >= reference["coarse_certified_volume"] - 1e-12
            for arm in eligible
        ) or not eligible
        # The comparison rule about look-alike arms, checked rather than asserted: if demanding
        # resolution leaves nothing coarse to withhold, arm D and arm B are one measurement.
        identical_to_demand = [
            arm
            for arm in ("D-demand-and-withhold",)
            if arm in arms
            and "B-demand" in arms
            and all(
                arms[arm][key] == arms["B-demand"][key]
                for key in (
                    "coarse_certified_volume",
                    "unknown_volume",
                    "provisional_volume",
                    "trusted_volume",
                    "trusted_over_true",
                    "localised_regions",
                    "leaf_cells",
                )
            )
        ]
        detail["arms_identical_to_demand"] = identical_to_demand

        # Outcome 7 states itself as applying "independent of which of the above fires", so it is
        # evaluated as its own check and reported under every verdict instead of being used as the
        # fallback for a verdict that did not fire.
        budgets = sorted(int(key) for key in reference["per_budget"])
        crossing = {}
        for arm in ARMS:
            per_budget = arms[arm]["per_budget"]
            crossing[arm] = {
                str(budget): {
                    "trusted_volume": per_budget[str(budget)]["trusted_volume"],
                    "unknown_volume": per_budget[str(budget)]["unknown_volume"],
                    "coarse_certified_volume": per_budget[str(budget)]["coarse_certified_volume"],
                    "leaf_cells": per_budget[str(budget)]["leaf_cells"],
                }
                for budget in budgets
                if str(budget) in per_budget
            }
        rule_7 = {
            "prediction": (
                "a leaf must hold at least four samples to be split, so a quiet space bisected to "
                "depth d needs about 4 x 2^d evaluations before it can be certified at 2^-d; at the "
                "frozen bar that is roughly 1000 evaluations, so UNKNOWN should give back to TRUSTED "
                "between 640 and 1280"
            ),
            "per_budget": crossing,
            "gives_back": {},
        }
        for arm in ("B-demand", "D-demand-and-withhold"):
            at640, at1280 = crossing[arm].get("640"), crossing[arm].get("1280")
            if at640 is None or at1280 is None:
                continue
            rule_7["gives_back"][arm] = {
                "unknown_falls": at1280["unknown_volume"] < at640["unknown_volume"],
                "trusted_rises": at1280["trusted_volume"] > at640["trusted_volume"],
                "unknown_640": at640["unknown_volume"],
                "unknown_1280": at1280["unknown_volume"],
                "trusted_640": at640["trusted_volume"],
                "trusted_1280": at1280["trusted_volume"],
            }
        rule_7["met"] = bool(rule_7["gives_back"]) and all(
            v["unknown_falls"] and v["trusted_rises"] for v in rule_7["gives_back"].values()
        )
        if not rule_7["met"]:
            rule_7["stated_as_defect"] = (
                "the per-budget readings do not show UNKNOWN giving back to TRUSTED between 640 and "
                "1280, so the arithmetic in `certify_bar_derivation` or the depth accounting behind it "
                "is wrong; outcome 7 requires that this be stated as a defect of the protocol file"
            )
        detail["rule_7"] = rule_7

        if improvements:
            fired = 3
            detail["arms"] = improvements
        elif discipline:
            fired = 4
            detail["arms"] = discipline
        elif renamed:
            fired = 5
            detail["capture"] = capture
            detail["cost"] = cost
        elif nobody_cut:
            fired = 6
        else:
            fired = None
            detail["why_nothing_fired"] = (
                f"no arm satisfies outcome 3 or outcome 4 as written, and neither outcome 5 nor outcome "
                f"6 applies. The arms that came closest are {ceiling_only}: each cuts "
                f"`coarse_certified_volume` to nothing, lowers `trusted_over_true`, loses no "
                f"localisation and holds control-entry certification, and is refused only by the "
                f"+{CEILING_UNKNOWN} UNKNOWN ceiling. The freeze enumerates no outcome for 'cut the "
                f"over-claim, gained no localisation, breached the UNKNOWN ceiling', and H-RESOLVE "
                f"declares itself falsified by any ceiling being missed, so the ceiling is not read as "
                f"outcome 4's exception: reporting such an arm as certification discipline would let "
                f"the size of the intervention buy a permitted verdict, which is what the ceiling was "
                f"set to prevent. The gap is a defect of this decision rule and is reported as one "
                f"rather than resolved by whichever reading is friendlier."
            )

    print(f"protocol   {protocol['id']} digest {plan_digest}")
    print(f"bar        {bar}  ceiling unknown +{ceiling}  certification floor {CERTIFICATION_FLOOR}")
    print(f"rows       {sum(arms[arm]['rows'] for arm in ARMS)} across {len(ARMS)} arms")
    print(
        f"{'arm':<24} {'coarse':>8} {'unk':>7} {'prov':>7} {'trs':>7} {'over':>8} "
        f"{'loc':>5} {'cells':>7}  rule"
    )
    for arm in ARMS:
        summary = arms[arm]
        print(
            f"{arm:<24} {summary['coarse_certified_volume']:>8.5f} "
            f"{summary['unknown_volume']:>7.4f} {summary['provisional_volume']:>7.4f} "
            f"{summary['trusted_volume']:>7.4f} {summary['trusted_over_true']:>8.6f} "
            f"{summary['localised_regions']:>5.0f} {summary['leaf_cells']:>7.1f}"
        )
    print(f"\ndecision rule outcome that fired: {fired}")
    if fired is None:
        print(f"  {detail['why_nothing_fired'][:600]}")
    else:
        print(f"  {rule[fired - 1][:400]}")
    if fired != 1:
        for arm in eligible:
            g = guards(arm)
            held = [k for k, v in g["checks"].items() if not v]
            print(f"  {arm:<24} guards failed: {held or 'none'}  gained: {g['localisation_gained']}")
        print(
            f"  outcome 7 crossing met: {detail['rule_7']['met']}  "
            f"silence rule (outcome 2): {detail.get('certifies_almost_nothing')}"
        )
    if capture is not None:
        print(
            f"capture {capture:.4f}  cost {cost if cost is None else round(cost, 4)}  "
            f"enrichment {enrichment if enrichment is None else round(enrichment, 4)}"
        )
    artifact = {
        "schema": "aporia.e1-4-analysis/1",
        "generated_by": "scripts/e14_analysis.py",
        "protocol": {
            "id": protocol["id"],
            "digest": plan_digest,
            "frozen": protocol["frozen"],
        },
        "metric": {
            "name": protocol["primary_metric"]["name"],
            "denominator": protocol["primary_metric"]["denominator"],
            "aggregation": protocol["primary_metric"]["aggregation"],
            "certify_share": bar,
        },
        "ceilings": {
            "unknown_absolute": CEILING_UNKNOWN,
            "certification_floor_of_control": CERTIFICATION_FLOOR,
            "coarse_certified_ratio_for_improvement": IMPROVEMENT_RATIO,
            "capture_floor": CAPTURE_FLOOR,
            "cost_ceiling": COST_CEILING,
        },
        "preconditions": {
            "reproduction_gate": load(GATE)["verdict"] if os.path.exists(GATE) else None,
            "pilot": load(PILOT)["verdict"] if os.path.exists(PILOT) else None,
            "binary_sha256": binary_digest,
            "built_from_commit": (load(GATE).get("built_from_commit") if os.path.exists(GATE) else None),
            "arms_found": sorted(documents),
        },
        "arms": arms,
        "paired_against_reference": comparisons,
        "informativeness": informativeness,
        "recorded_defects": [
            (
                "the pilot ran the full frozen plan (100 charged cells per arm) where stopping rule 2 "
                "states one entry, two seeds and the two smallest budgets. The cause is mechanical: with "
                "--plan the runner refuses every flag that shapes what is measured, so a reduced pilot "
                "needs a reduced frozen plan, and this freeze did not provide one. No threshold, "
                "tolerance or criterion moved, and the pilot declares every deciding quantity it does not "
                "measure; what was exceeded is the freeze's cost bound on the pilot."
            ),
            (
                "the decision rule enumerates no outcome for an arm that cuts the primary metric to zero, "
                "holds every other guard, gains no localisation and misses only the UNKNOWN ceiling. B and "
                "D are exactly that arm. The gap is reported as a defect of the rule rather than resolved "
                "by reading outcome 4's 'safety holds' as excluding the ceiling, which is the reading that "
                "would have made this a permitted outcome."
            ),
            (
                "H-WITHHOLD's first falsifier -- whether the withheld volume captures more of the real "
                "over-claim than the volume the same partition calls UNKNOWN -- is not computable from any "
                "published column: `trusted_over_true` measures certification overlapping a declared "
                "region, and nothing records UNKNOWN overlapping one. The enrichment ratio reported beside "
                "the frozen pair is the closest computable reading and it is not that test."
            ),
        ],
        "decision": {
            "fired": fired,
            "text": (
                rule[fired - 1]
                if fired
                else "no outcome among 1 to 6 fires as written; see detail.why_nothing_fired"
            ),
            "detail": detail,
            "all_outcomes": rule,
        },
        "not_claims": protocol["not_claims"],
        "limitations_this_does_not_remove": protocol["limitations_this_does_not_remove"],
        "problems": problems,
    }
    if args.write:
        with io.open(ARTIFACT, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(artifact, handle, indent=1, sort_keys=True)
            handle.write("\n")
        print(f"artifact {ARTIFACT}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
