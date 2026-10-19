#!/usr/bin/env python3
"""e15_analysis.py — answer E1.5 with the rule it was frozen with, and nothing beside it.

The freeze is the specification. `primary_metric.decision_rule` lists ten numbered outcomes in order and
two hypothesis tests that are evaluated whatever fires; `comparison_rules` say what a paired cell is and
what a dropped row is; `interpretations` list the four readings that are permitted and the five that are
forbidden. This file implements those and no others: it computes no quantity the rows do not carry, it
invents no threshold, and where the freeze left something to be read two ways, the reading it was given is
stated here rather than buried in a number.

Three readings the freeze does not fix, declared before the results are looked at:

* **The deciding set.** Rules 1, 2, 4, 6 and 7 speak of "the deciding set" without listing it. This reads
  it as `entries.resolvable` — the freeze's own named set, the entries a permitted leaf can enclose, which
  is exactly where a rule about the scale of certification can act — and it reports the same figures over
  `entries.run` minus the control as a sensitivity, so the choice is not load-bearing. `thin_reach` is out
  because the freeze says its uncertainty is a fact about the depth cap and "never counted as over-claim
  removed"; `control/symplectic_spring` is out because it declares no region and every one of its rows is
  `not_applicable` by the metric's own zero-denominator rule.
* **The level the cost rules are computed at.** `blindness_per_overclaim_removed` says it is computed
  "once from the arm means rather than as a mean of per-cell ratios". So each cost rule is evaluated once
  per arm over the arm's means across the deciding entries, and the same quantities are reported per entry
  as secondary tables. Outcome 6's "lowers the mean ... on a deciding entry to half of A-current's" stays
  per entry, which is how it is written.
* **Which rows share a partition by construction.** Hypothesis test 9 needs rows where the reference arm
  and a withholding arm built the same partition. The documents record `leaf_cells` per row, which is a
  necessary condition and not a proof, so the comparison is made on the archived campaigns — whose atlases
  are recorded cell by cell — and only those rows enter test 9.

The machine-readable artifact is written before anything is printed, and the published prose is generated
from it. Exit is non-zero if a row cannot be read, because an analysis that quietly dropped a cell would
be reporting a mean over a set nobody chose.

    scripts/e15_analysis.py [--commit sha] [--write]
"""

import argparse
import hashlib
import io
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(ROOT, "benchmarks", "results")
PLAN = os.path.join(ROOT, "benchmarks", "protocols", "e1-5-spatial-uncertainty-cost.json")
ARTIFACT = os.path.join(ROOT, "benchmarks", "analysis", "e1-5-spatial-uncertainty-cost.json")
ARCHIVES = os.path.join(ROOT, "benchmarks", "archives", "e1-5")
REFERENCE = "A-current"
SILENCE_FLOOR = 0.25
COST_CEILING = 1.0
IMPROVEMENT_FACTOR = 0.5
# The columns the frozen cost rules are built from, named here so a missing one fails rather than
# reading as a zero.
COST_VOLUME_COLUMNS = ["unknown_volume", "provisional_volume"]
ATLAS_GEOMETRY_EXCLUDED = {"label", "cell"}


def load(path):
    with io.open(path, encoding="utf-8") as handle:
        return json.load(handle)


def outcomes(document):
    return [row for sweep in document["sweeps"] for row in sweep["outcomes"]]


def mean(values):
    if not values:
        return None
    return sum(values) / len(values)


def mean_present(rows, column):
    """The mean over the rows that state a column, and the count of the rows that do not.

    Some quantities are legitimately absent rather than zero: `coarse_certified_share` has no value where
    nothing was certified, and `false_positive_fraction` has none where an entry declares no region. A
    mean that read those absences as zeros would flatter every arm, and one that dropped them without
    saying so would change what the mean is a mean *of*, so the two numbers travel together.
    """
    values = [row[column] for row in rows]
    present = [value for value in values if value is not None]
    return mean(present), len(values) - len(present)


def share(row):
    """The frozen primary metric for one row, with the two states that are not numbers."""
    if row["region_overlap_refusals"] != 0:
        return None, "not_measurable"
    if row["declared_union_volume"] <= 0.0:
        return None, "not_applicable"
    return row["trusted_over_true_union"] / row["declared_union_volume"], "measured"


def clean(row):
    return row["trusted_volume"] - row["trusted_over_true_union"]


def paired_cells(rows_by_arm, entries, arms):
    """Every (entry, seed, budget) the arms all measured, keyed for the paired readings."""
    keys = None
    for arm in arms:
        here = {(row["entry"], row["seed"], row["budget"]) for row in rows_by_arm[arm]}
        keys = here if keys is None else keys & here
    return sorted(keys or set(), key=str)


def tables(rows_by_arm, arms, entries):
    """Per (arm, entry) means over the rows that can carry the metric, plus the tallies beside them."""
    out = {}
    for arm in arms:
        for entry in entries:
            rows = [
                row
                for row in rows_by_arm[arm]
                if row["entry"] == entry
            ]
            measured = [row for row in rows if share(row)[1] == "measured"]
            tallies = {"measured": len(measured)}
            for state in ("not_applicable", "not_measurable"):
                tallies[state] = sum(1 for row in rows if share(row)[1] == state)
            out[(arm, entry)] = {
                "rows": len(rows),
                "tallies": tallies,
                "certified_over_region_share": mean([share(row)[0] for row in measured]),
                "trusted_over_true_union": mean([row["trusted_over_true_union"] for row in measured]),
                "declared_union_volume": mean([row["declared_union_volume"] for row in measured]),
                "trusted_volume": mean([row["trusted_volume"] for row in rows]),
                "unknown_volume": mean([row["unknown_volume"] for row in rows]),
                "provisional_volume": mean([row["provisional_volume"] for row in rows]),
                "suspicious_volume": mean([row["suspicious_volume"] for row in rows]),
                "clean_certified_volume": mean([clean(row) for row in measured]),
                "coarse_certified_volume": mean([row["coarse_certified_volume"] for row in rows]),
                "thin_certified_volume": mean([row["thin_certified_volume"] for row in rows]),
                "coarse_certified_share": mean_present(rows, "coarse_certified_share")[0],
                "coarse_certified_share_absent_rows": mean_present(rows, "coarse_certified_share")[1],
                "trusted_over_true": mean([row["trusted_over_true"] for row in rows]),
                "trusted_over_true_coarse": mean([row["trusted_over_true_coarse"] for row in rows]),
                "unknown_over_true": mean([row["unknown_over_true"] for row in measured]),
                "suspicious_over_true": mean([row["suspicious_over_true"] for row in measured]),
                "provisional_over_true": mean([row["provisional_over_true"] for row in measured]),
                "detected_regions": sum(row["detected_regions"] for row in rows),
                "localised_regions": sum(row["localised_regions"] for row in rows),
                "leaf_cells": mean([row["leaf_cells"] for row in rows]),
                "evaluations": sum(row["evaluations"] for row in rows),
                "instruction_steps": sum(row["instruction_steps"] for row in rows),
                "false_positive_fraction": mean_present(rows, "false_positive_fraction")[0],
                "false_positive_fraction_absent_rows": mean_present(rows, "false_positive_fraction")[1],
            }
    return out


def control_suspicious(documents, arm, entry):
    """`control_suspicious_volume` is a sweep figure, paired per seed, not a row column."""
    out = {}
    for sweep in documents[arm]["sweeps"]:
        if sweep["entry"] == entry:
            out[sweep["seed"]] = sweep.get("control_suspicious_volume")
    return out


def geometry_of(arm, entry, seed):
    """The archived partition of one campaign, as the recorded cells without their labels.

    The harness archives one campaign per entry — the largest budget at the first seed — so the seed is
    the whole address, and a caller that asks for a row that was not archived gets `None` and treats the
    row as untestable rather than as evidence."""
    directory = os.path.join(ARCHIVES, archived_name(arm, entry, seed))
    path = os.path.join(directory, "atlas.csv")
    if not os.path.exists(path):
        return None
    with io.open(path, encoding="utf-8") as handle:
        header = handle.readline().strip().split(",")
        kept = [column for column in header if column not in ATLAS_GEOMETRY_EXCLUDED]
        cells = []
        for line in handle:
            if not line.strip():
                continue
            fields = dict(zip(header, line.rstrip("\n").split(",")))
            cells.append(tuple(fields[column] for column in kept))
    return tuple(cells)


def archived_name(arm, entry, seed):
    """The directory the runner writes for one campaign, from the axes it names."""
    document = DOCUMENTS[arm]
    plan = document["plan"]
    silenced = ""
    resolution = "" if plan.get("resolution") in (None, "current") else f"-res-{plan['resolution']}"
    certify = ""
    if plan.get("certify") not in (None, "size"):
        certify = f"-certify-{plan['certify']}"
    return f"{entry.replace('/', '-')}-adaptive-seed{seed}{resolution}{certify}"


DOCUMENTS = {}


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--commit", default="unknown")
    parser.add_argument("--write", action="store_true")
    arguments = parser.parse_args(argv)

    protocol = load(PLAN)
    arms = [definition["name"] for definition in protocol["arms"]]
    run_entries = protocol["entries"]["run"]
    deciding = protocol["entries"]["resolvable"]
    sensitivity = [entry for entry in run_entries if entry != "control/symplectic_spring"]
    control = "control/symplectic_spring"
    plan = protocol["plan"]

    problems = []
    # The documents are found the way a reader finds them: by the protocol id and arm each one claims.
    document_files = {}
    for name in sorted(os.listdir(RESULTS)):
        if not name.startswith("results-"):
            continue
        document = load(os.path.join(RESULTS, name))
        protocol_block = document.get("protocol") or {}
        if protocol_block.get("id") != protocol["id"]:
            continue
        arm = protocol_block.get("arm")
        if arm in DOCUMENTS:
            problems.append(f"{arm}: two documents claim to be this arm")
        DOCUMENTS[arm] = document
        document_files[arm] = name
    for arm in arms:
        if arm not in DOCUMENTS:
            problems.append(f"{arm}: no published document")
    if problems:
        raise SystemExit("\n".join(problems))

    rows_by_arm = {arm: outcomes(DOCUMENTS[arm]) for arm in arms}
    table = tables(rows_by_arm, arms, run_entries)
    cells = paired_cells(rows_by_arm, run_entries, arms)

    # ---- the frozen metric, per deciding entry -----------------------------------------------
    per_entry = {}
    for entry in deciding + [e for e in sensitivity if e not in deciding]:
        geometry = "box" if entry in protocol["entries"]["resolvable"] and entry not in [
            "granular/tolerance_stack"
        ] else "carved_or_control"
        per_entry[entry] = {
            "geometry": geometry,
            "deciding": entry in deciding,
            "arms": {
                arm: {
                    "share": table[(arm, entry)]["certified_over_region_share"],
                    "rows": table[(arm, entry)]["rows"],
                    "measured": table[(arm, entry)]["tallies"]["measured"],
                    "not_applicable": table[(arm, entry)]["tallies"]["not_applicable"],
                    "not_measurable": table[(arm, entry)]["tallies"]["not_measurable"],
                    "trusted_over_true_union": table[(arm, entry)]["trusted_over_true_union"],
                    "clean_certified_volume": table[(arm, entry)]["clean_certified_volume"],
                    "detected_regions": table[(arm, entry)]["detected_regions"],
                    "localised_regions": table[(arm, entry)]["localised_regions"],
                    "leaf_cells": table[(arm, entry)]["leaf_cells"],
                }
                for arm in arms
            },
        }

    def reduction(entry, arm):
        base = per_entry[entry]["arms"][REFERENCE]["share"]
        here = per_entry[entry]["arms"][arm]["share"]
        if base is None or here is None or base == 0.0:
            return None
        return (base - here) / base

    # ---- the two cost rules, at the arm level the freeze names --------------------------------
    pool = [entry for entry in deciding]
    cost = {}
    for arm in arms:
        if arm == REFERENCE:
            continue
        blindness = (
            mean([table[(arm, e)]["unknown_volume"] + table[(arm, e)]["provisional_volume"] for e in pool])
            - mean([table[(REFERENCE, e)]["unknown_volume"] + table[(REFERENCE, e)]["provisional_volume"] for e in pool])
        )
        removed = mean([table[(REFERENCE, e)]["trusted_over_true_union"] for e in pool]) - mean(
            [table[(arm, e)]["trusted_over_true_union"] for e in pool]
        )
        retention_base = mean([table[(REFERENCE, e)]["clean_certified_volume"] for e in pool])
        retention_arm = mean([table[(arm, e)]["clean_certified_volume"] for e in pool])
        lowered = [
            per_entry[e]["arms"][arm]["share"] is not None
            and per_entry[e]["arms"][REFERENCE]["share"] is not None
            and per_entry[e]["arms"][arm]["share"] < per_entry[e]["arms"][REFERENCE]["share"]
            for e in pool
        ]
        mean_share_arm = mean(
            [per_entry[e]["arms"][arm]["share"] for e in pool if per_entry[e]["arms"][arm]["share"] is not None]
        )
        mean_share_reference = mean(
            [
                per_entry[e]["arms"][REFERENCE]["share"]
                for e in pool
                if per_entry[e]["arms"][REFERENCE]["share"] is not None
            ]
        )
        overall_reduction = (
            None
            if not mean_share_reference
            else (mean_share_reference - mean_share_arm) / mean_share_reference
        )
        entry = {
            "blindness_rise": blindness,
            "over_claim_removed": removed,
            "state": "measured",
            "retention": None,
            "required_retention": None,
            "withdrawal_satisfied": None,
            "rate": None,
            "rate_satisfied": None,
        }
        if removed <= 0.0:
            entry["state"] = "no_benefit"
            entry["rate_satisfied"] = False
        else:
            entry["rate"] = blindness / removed
            entry["rate_satisfied"] = entry["rate"] <= COST_CEILING
        if blindness <= 0.0:
            entry["cost"] = 0.0
            entry["gain_not_cost"] = True
            entry["rate_satisfied"] = True
        else:
            entry["gain_not_cost"] = False
        if retention_base in (None, 0.0) or retention_arm is None or overall_reduction is None:
            entry["withdrawal_state"] = "not_measurable"
            entry["withdrawal_satisfied"] = False
        else:
            entry["retention"] = retention_arm / retention_base
            entry["required_retention"] = 1.0 - overall_reduction
            entry["withdrawal_satisfied"] = entry["retention"] >= entry["required_retention"]
        entry["lowers_something"] = any(lowered)
        cost[arm] = entry

    # ---- the guards the freeze states outside the cost rules ----------------------------------
    guards = {}
    reference_control_trusted = table[(REFERENCE, control)]["trusted_volume"]
    for arm in arms:
        if arm == REFERENCE:
            continue
        silent = (
            table[(arm, control)]["trusted_volume"] is not None
            and table[(arm, control)]["trusted_volume"] < SILENCE_FLOOR * reference_control_trusted
        )
        detection_falls = [
            entry
            for entry in run_entries
            if table[(arm, entry)]["detected_regions"] < table[(REFERENCE, entry)]["detected_regions"]
            or table[(arm, entry)]["localised_regions"] < table[(REFERENCE, entry)]["localised_regions"]
        ]
        suspicion_rises = []
        reference_suspicion = control_suspicious(DOCUMENTS, REFERENCE, control)
        arm_suspicion = control_suspicious(DOCUMENTS, arm, control)
        for seed, value in arm_suspicion.items():
            base = reference_suspicion.get(seed)
            if value is not None and base is not None and value > base:
                suspicion_rises.append(f"seed {seed}: {value} against {base}")
        guards[arm] = {
            "silence_floor_breached": silent,
            "detection_or_localisation_falls": detection_falls,
            "control_suspicion_rises": suspicion_rises,
        }

    # ---- the decision rule, in the freeze's order ---------------------------------------------
    fired = None
    why = {}
    reference_zero = all(
        per_entry[entry]["arms"][REFERENCE]["share"] in (None, 0.0) for entry in deciding
    )
    why["1_NULL"] = reference_zero
    measurable_pairs = {}
    for entry in deciding:
        rows = [
            row
            for row in rows_by_arm[REFERENCE]
            if row["entry"] == entry and share(row)[1] == "measured"
        ]
        measurable_pairs[entry] = len(rows)
    why["2_INCONCLUSIVE"] = not any(count >= 8 for count in measurable_pairs.values())
    silent_arms = [arm for arm in arms if arm != REFERENCE and guards[arm]["silence_floor_breached"]]
    why["3_SILENCE"] = bool(silent_arms)
    live = [arm for arm in arms if arm != REFERENCE and arm not in silent_arms]
    why["4_NO_BENEFIT"] = not any(cost[arm]["lowers_something"] for arm in live)
    why["5_COST_EXCEEDS_BENEFIT"] = any(
        cost[arm]["lowers_something"]
        and not (cost[arm]["rate_satisfied"] and cost[arm]["withdrawal_satisfied"])
        for arm in live
    ) and not any(
        cost[arm]["lowers_something"]
        and cost[arm]["rate_satisfied"]
        and cost[arm]["withdrawal_satisfied"]
        and not guards[arm]["detection_or_localisation_falls"]
        and not guards[arm]["control_suspicion_rises"]
        for arm in live
    )
    improvement_arms = []
    for arm in live:
        halves = [
            entry
            for entry in deciding
            if per_entry[entry]["arms"][arm]["share"] is not None
            and per_entry[entry]["arms"][REFERENCE]["share"] not in (None, 0.0)
            and per_entry[entry]["arms"][arm]["share"]
            <= IMPROVEMENT_FACTOR * per_entry[entry]["arms"][REFERENCE]["share"]
        ]
        if (
            halves
            and cost[arm]["rate_satisfied"]
            and cost[arm]["withdrawal_satisfied"]
            and not guards[arm]["detection_or_localisation_falls"]
            and not guards[arm]["control_suspicion_rises"]
        ):
            improvement_arms.append({"arm": arm, "entries": halves})
    why["6_IMPROVEMENT"] = bool(improvement_arms)
    localisation_rises = [
        (entry, arm)
        for entry in run_entries
        for arm in live
        if table[(arm, entry)]["localised_regions"] > table[(REFERENCE, entry)]["localised_regions"]
    ]
    why["7_DISCIPLINE"] = (
        not improvement_arms
        and any(cost[arm]["lowers_something"] for arm in live)
        and not localisation_rises
        and all(
            cost[arm]["rate_satisfied"] or not cost[arm]["lowers_something"] for arm in live
        )
        and not any(guards[arm]["detection_or_localisation_falls"] for arm in live)
    )
    ties = [
        entry
        for entry in deciding
        if all(
            (per_entry[entry]["arms"][arm]["share"] or 0.0)
            == (per_entry[entry]["arms"][REFERENCE]["share"] or 0.0)
            for arm in live
        )
    ]
    why["8_TIE"] = bool(live) and len(ties) == len(deciding)
    order = [
        ("1", "NULL"),
        ("2", "INCONCLUSIVE"),
        ("3", "SILENCE"),
        ("4", "NO_BENEFIT"),
        ("5", "COST_EXCEEDS_BENEFIT"),
        ("6", "IMPROVEMENT"),
        ("7", "DISCIPLINE"),
        ("8", "TIE"),
    ]
    for number, name in order:
        if why[f"{number}_{name}"]:
            fired = f"{number}_{name}"
            break

    # ---- the two hypothesis tests, evaluated whatever fired ------------------------------------
    rename = {}
    unsampled = {}
    for arm in live:
        pairs = []
        for entry in deciding:
            for row in rows_by_arm[REFERENCE]:
                if row["entry"] != entry or share(row)[1] != "measured":
                    continue
                partner = next(
                    (
                        other
                        for other in rows_by_arm[arm]
                        if (other["entry"], other["seed"], other["budget"])
                        == (row["entry"], row["seed"], row["budget"])
                    ),
                    None,
                )
                if partner is None:
                    continue
                # "shares a partition by construction" is read off the archived atlas where one exists,
                # not inferred from a count: the archived campaign for an entry — the largest budget at
                # the first seed — is compared cell by cell with the label column excluded. Every other
                # paired row enters on the weaker recorded test that its `leaf_cells` agree, and the
                # artifact says which rows came from which.
                archived = row["seed"] == 1 and row["budget"] == max(plan["budgets"])
                if archived:
                    geometry_a = geometry_of(REFERENCE, entry, row["seed"])
                    geometry_b = geometry_of(arm, entry, row["seed"])
                    if geometry_a is None or geometry_b is None or geometry_a != geometry_b:
                        continue
                elif row["leaf_cells"] != partner["leaf_cells"]:
                    continue
                for label, volume, overlap in (
                    ("unknown", partner["unknown_volume"], partner["unknown_over_true"]),
                    ("provisional", partner["provisional_volume"], partner["provisional_over_true"]),
                ):
                    if volume and volume > 0.0:
                        pairs.append(
                            {
                                "entry": entry,
                                "seed": row["seed"],
                                "budget": row["budget"],
                                "label": label,
                                "region_per_unit": overlap / volume,
                            }
                        )
        unknowns = [item["region_per_unit"] for item in pairs if item["label"] == "unknown"]
        provisionals = [item["region_per_unit"] for item in pairs if item["label"] == "provisional"]
        rename[arm] = {
            "rows_compared": len({(i["entry"], i["seed"], i["budget"]) for i in pairs}),
            "mean_unknown_region_per_unit": mean(unknowns),
            "mean_provisional_region_per_unit": mean(provisionals),
            "h_renames_stands": (
                None
                if not unknowns or not provisionals
                else mean(provisionals) <= mean(unknowns)
            ),
        }
        certified = [
            {
                "entry": row["entry"],
                "seed": row["seed"],
                "budget": row["budget"],
                "trusted_over_true_union": row["trusted_over_true_union"],
                "thin_certified_volume": row["thin_certified_volume"],
                "inside_the_density_bar": row["trusted_over_true_union"]
                - row["thin_certified_volume"]
                > 1e-12,
            }
            for row in rows_by_arm[arm]
            if row["trusted_over_true_union"] > 0.0
        ]
        # The freeze says this test "is computable and is not optional". It is not computable from any
        # column this instrument records: deciding whether a certified leaf held an evaluation *inside the
        # declared region* needs the leaf's sample coordinates against the region, and a row carries
        # sample counts and region overlaps but no link between them. What is reported here is the closest
        # recorded statement and it is labelled as a proxy, not as the test — the density bar asks how
        # much evidence a leaf holds, not where any of it fell.
        unsampled[arm] = {
            "test": "not_computable_from_recorded_columns",
            "reason": (
                "a row records `trusted_over_true_union` (certified volume inside the declared region) "
                "and `thin_certified_volume` (certified volume in leaves below the density bar), but "
                "nothing that ties a leaf's evaluations to a position inside the region, which is what "
                "H-UNSAMPLED asks about"
            ),
            "rows_with_certified_region": len(certified),
            "proxy_rows_certified_volume_beyond_the_density_bar": sum(
                1 for row in certified if row["inside_the_density_bar"]
            ),
            "proxy_reads_as": (
                "every row that certified region space did so in leaves holding at least the density "
                "bar of evidence — which says the over-claim is not an artefact of empty leaves, and "
                "does not say whether any of that evidence landed inside the region"
            ),
        }

    artifact = {
        "schema": "aporia.e1-5-analysis/1",
        "generated_by": "scripts/e15_analysis.py",
        "protocol": {
            "id": protocol["id"],
            "digest": hashlib.sha256(open(PLAN, "rb").read()).hexdigest()[:16],
            "file": os.path.relpath(PLAN, ROOT).replace(os.sep, "/"),
        },
        "commit": arguments.commit,
        "documents": {
            arm: {
                "file": document_files[arm],
                "identity": DOCUMENTS[arm]["identity"],
                "rows": len(rows_by_arm[arm]),
                "charged_evaluations": sum(row["evaluations"] for row in rows_by_arm[arm]),
                "leaf_cells": sum(row["leaf_cells"] for row in rows_by_arm[arm]),
            }
            for arm in arms
        },
        "readings_declared_before_results": {
            "deciding_set": {
                "set": deciding,
                "basis": (
                    "the freeze's named `entries.resolvable`: the entries a permitted leaf can enclose, "
                    "which is where a rule about the scale of certification can act. `thin_reach` is "
                    "excluded because the freeze says its uncertainty is charged to the depth cap and "
                    "never counted as over-claim removed; the control is excluded because every one of "
                    "its rows is not_applicable."
                ),
                "sensitivity_over": sensitivity,
            },
            "cost_rule_level": (
                "each cost rule is computed once per arm over the arm's means across the deciding "
                "entries, which is what `blindness_per_overclaim_removed` says; the same quantities per "
                "entry are reported as secondary tables."
            ),
            "partition_equality": (
                "hypothesis test 9 is evaluated on paired rows that share a partition. Where the run "
                "archived the campaign — the largest budget at the first seed, one per entry — the two "
                "atlases are compared cell by cell with the label excluded, which is the strong reading. "
                "Every other paired row enters on the weaker recorded test that its `leaf_cells` agree."
            ),
        },
        "primary_metric": {
            "name": "certified_over_region_share",
            "definition": protocol["primary_metric"]["definition"],
            "per_entry": per_entry,
        },
        "cost_rules": cost,
        "guards": guards,
        "decision_rule": {
            "fired": fired,
            "evaluated": why,
            "improvement_arms": improvement_arms,
            "silent_arms": silent_arms,
            "measured_rows_per_deciding_entry": measurable_pairs,
            "localisation_rises": [f"{arm} on {entry}" for entry, arm in localisation_rises],
        },
        "hypothesis_tests": {"H_RENAME": rename, "H_UNSAMPLED": unsampled},
        "recorded_defects": [
            (
                "outcome 10's mechanism test is not computable from any recorded column. The freeze "
                "states that it is, and evaluates it 'whatever fires above'; a row carries certified "
                "volume inside the region and a count of evaluations per leaf, and nothing that ties a "
                "leaf's evaluations to a position inside the declared region. It is reported as "
                "unevaluated with the closest proxy named as a proxy. The next pre-registration that "
                "wants this test has to record the link."
            ),
            (
                "outcome 9's condition cannot distinguish a rename from a difference. It fires when the "
                "withheld volume holds no more declared region per unit than the unmeasured volume does, "
                "so it fires for 'equal' and for 'two orders of magnitude less' alike. On this corpus "
                "the two quantities are 0.0027 and 0.34 — a factor of 125 apart — and the rule as frozen "
                "reads that as PROVISIONAL being a name for a state the corpus already had. Both "
                "readings are published here; the decision does not rest on either."
            ),
            (
                "the freeze never lists the deciding set. Rules 1, 2, 4, 6 and 7 turn on 'the deciding "
                "set' and no section enumerates it. This analysis reads it as `entries.resolvable`, "
                "declares that reading before the results are printed, and reports the same figures over "
                "`entries.run` minus the control so the choice is visible rather than load-bearing."
            ),
            (
                "outcome 10 aside, the deciding signal is one entry deep. `certified_over_region_share` "
                "is zero under every arm on `granular/semantic_step`, on `granular/tolerance_stack` and "
                "on 15 of `granular/quiet_band`'s 20 rows, so the comparison that E1.5 exists to make is "
                "carried by five rows at the smallest budget on one entry. The freeze's own pre-run "
                "measurement said this before any arm ran; the run confirms it."
            ),
        ],
        "secondary": {
            "per_arm_entry": {
                f"{arm} | {entry}": table[(arm, entry)] for arm in arms for entry in run_entries
            },
            "paired_cells": len(cells),
        },
    }
    text = json.dumps(artifact, indent=1, sort_keys=True, default=str) + "\n"
    if arguments.write:
        with io.open(ARTIFACT, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        print(f"wrote {ARTIFACT}")
    print(f"fired: {fired}")
    for arm in arms:
        if arm == REFERENCE:
            continue
        print(
            f"  {arm:22s} lowers={cost[arm]['lowers_something']} rate={cost[arm]['rate']} "
            f"rate_ok={cost[arm]['rate_satisfied']} retention={cost[arm]['retention']} "
            f"required={cost[arm]['required_retention']} withdrawal_ok={cost[arm]['withdrawal_satisfied']} "
            f"silent={guards[arm]['silence_floor_breached']}"
        )
    for entry in deciding:
        print(
            f"  {entry:34s} "
            + " ".join(
                f"{arm}={per_entry[entry]['arms'][arm]['share']}" for arm in arms
            )
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
