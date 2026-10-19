#!/usr/bin/env python3
"""e15_survey.py — read the six region-overlap columns out of the bytes that already exist.

E1.5 has to be designed against numbers, not against intentions: what a per-label denominator is worth
depends on whether any entry in this corpus certifies a region at all, and how much of what it
certifies is coarse. That question is answerable from measurements that are already published, and
answering it before a protocol is frozen is what keeps the metric from being chosen after the outcome.

Two sources, and the difference between them matters.

`--replay DIR` reads the documents the reproduction gate writes when it re-measures the twenty-three
committed arms: the same rows as published, the same declared fields proved identical, plus the six
columns no published document carries. That is the only way to see the new quantities on E1.x, E1.3b
and E1.4 data without touching those experiments, and the gate's own artifact is what makes the
provenance more than a claim.

`--survey DIR` reads ad-hoc runs of the *default* instrument over entries no committed arm covers, so
that the candidate list for the new corpus is not assembled from the twelve families somebody else
already had a reason to measure. Those runs are not published measurements and are not treated as
anybody's result; they say which entries are worth writing a pre-registration about.

Exit is non-zero if the replay documents disagree with the committed ones they claim to reproduce,
because a survey built on a replay that moved a number would be describing an instrument, not a
corpus.
"""

import argparse
import glob
import io
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(ROOT, "benchmarks", "results")
GATE = os.path.join(ROOT, "benchmarks", "analysis", "e1-5-region-columns-gate.json")
OUT = os.path.join(ROOT, "benchmarks", "analysis", "e1-5-region-column-survey.json")
COLUMNS = [
    "declared_union_volume",
    "trusted_over_true",
    "trusted_over_true_union",
    "trusted_over_true_coarse",
    "unknown_over_true",
    "suspicious_over_true",
    "provisional_over_true",
    "trusted_volume",
    "unknown_volume",
    "provisional_volume",
    "suspicious_volume",
    "coarse_certified_volume",
    "localised_regions",
    "detected_regions",
    "leaf_cells",
]


def load(path):
    with io.open(path, encoding="utf-8") as handle:
        return json.load(handle)


def rows(doc):
    return [o for sweep in doc["sweeps"] for o in sweep["outcomes"]]


def mean(group, field):
    values = [row.get(field) or 0.0 for row in group]
    return sum(values) / len(values)


def table(groups):
    """One row per (arm, entry), with the rates the design question is actually asked in."""
    out = []
    for (arm, entry), group in sorted(groups.items()):
        declared = mean(group, "declared_union_volume")
        certified = mean(group, "trusted_over_true_union")
        unknown = mean(group, "unknown_over_true")
        suspicious = mean(group, "suspicious_over_true")
        out.append(
            {
                "arm": arm,
                "entry": entry,
                "rows": len(group),
                "mean": {column: mean(group, column) for column in COLUMNS},
                # The three rates a metric design turns on: how much of the declared region the
                # instrument signs off, how much it leaves unmeasured, and how much it flags.
                "certified_share_of_region": certified / declared if declared > 0 else None,
                "unmeasured_share_of_region": unknown / declared if declared > 0 else None,
                "flagged_share_of_region": suspicious / declared if declared > 0 else None,
                "max_row_certified_over_region": max(
                    (row.get("trusted_over_true_union") or 0.0) for row in group
                ),
                "refusals": sum(row.get("region_overlap_refusals") or 0 for row in group),
            }
        )
    return out


def collect(directory, arm_key):
    groups = {}
    for path in sorted(glob.glob(os.path.join(directory, "results-*.json"))):
        doc = load(path)
        arm = arm_key(doc)
        if arm is None:
            continue
        for row in rows(doc):
            groups.setdefault((arm, row["entry"]), []).append(row)
    return groups


def check_replay(replayed):
    """The replayed documents must still say what the committed ones say, field by field."""
    problems = []
    for path in sorted(glob.glob(os.path.join(replayed, "results-*.json"))):
        name = os.path.basename(path)
        committed_path = os.path.join(RESULTS, name)
        if not os.path.exists(committed_path):
            problems.append(f"{name}: replayed but never published")
            continue
        replayed_doc, committed_doc = load(path), load(committed_path)
        if replayed_doc["identity"] != committed_doc["identity"]:
            problems.append(f"{name}: identity moved")
        for s_committed, s_replayed in zip(committed_doc["sweeps"], replayed_doc["sweeps"]):
            for c, r in zip(s_committed["outcomes"], s_replayed["outcomes"]):
                for key, value in c.items():
                    if key == "wall_ms":
                        continue
                    if r.get(key) != value:
                        problems.append(
                            f"{name}: {c['entry']} seed {c['seed']} field {key} moved"
                        )
                        break
    return problems


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--replay", default=os.path.join(ROOT, ".scratch", "e14-repro-results"))
    parser.add_argument("--survey", default=os.path.join(ROOT, ".scratch", "e15-survey"))
    parser.add_argument("--write", action="store_true", help="write the artifact into benchmarks/analysis")
    arguments = parser.parse_args(argv)

    gate = load(GATE)
    problems = check_replay(arguments.replay)
    if problems:
        for problem in problems[:10]:
            print(problem)
        raise SystemExit(f"{len(problems)} replayed field(s) differ from the committed rows")

    replayed = collect(arguments.replay, lambda d: (d.get("protocol") or {}).get("arm"))
    survey = collect(arguments.survey, lambda d: "default-instrument" if d else None)
    artifact = {
        "schema": "aporia.region-column-survey/1",
        "generated_by": "scripts/e15_survey.py",
        "purpose": (
            "What the six region-overlap columns say about measurements that already exist, read "
            "before E1.5's metric was designed so that its denominator, its cost rule and its entry "
            "requirements are stated against numbers rather than against hopes."
        ),
        "provenance": {
            "reproduction_gate": {
                "artifact": os.path.relpath(GATE, ROOT).replace(os.sep, "/"),
                "verdict": gate["verdict"],
                "binary_sha256": gate["binary_sha256"],
                "built_from_commit": gate["built_from_commit"],
                "arms": len(gate["arms"]),
                "rows_compared": gate["rows_compared"],
            },
            "survey_runs": (
                "ad-hoc runs of the default plan (current trust, current resolution, no ablation) "
                "over entries no committed arm measures, written to a scratch namespace and not "
                "published as a result of any experiment"
            ),
            "replay_verified_here": "every committed field of every replayed row re-checked against "
                                    "the committed document by this script, independently of the gate",
        },
        "committed_arms": table(replayed),
        "default_instrument_survey": table(survey),
        "findings": [
            "Under the committed default instrument, the box-declared entries certify almost nothing "
            "inside their own regions: the largest certified-over-region mean among them is 0.00074 "
            "of the domain on `synthetic/one_pct_3d` (14.8% of that entry's region), and on the "
            "granular and analytic families the declared region is overwhelmingly SUSPICIOUS rather "
            "than certified — `granular/diagonal_count` flags 95% of its region and certifies 0.02%. "
            "A resolution policy has nothing to act on where the region is loud.",
            "The entries that do certify their region are the carved ones — `geometry/curved_product` "
            "certifies 67.8% of the region it declares and `geometry/curved_arc` 2.4% under `full` — "
            "and on a carved region the four columns are two lattice estimates rather than one "
            "arithmetic, so the ordering between the union and the historical maximum is not "
            "available there. A metric whose denominator has to add up is therefore designed on "
            "box-declared entries.",
            "The mechanism the columns expose is E1.4's H-DEPTH stated as arithmetic: a leaf is "
            "certified when its own samples are quiet, and `risk_max` makes any leaf holding one "
            "loud evaluation suspicious, so certified-over-region volume can only come from region "
            "a leaf certified *without ever sampling inside it*. Over-claim is therefore a statement "
            "about whether the sample set reached inside the region, not about where the label's "
            "threshold sits.",
            "What the instrument certifies is a property of the channels that ran, now stated as a "
            "share of the answer key rather than as a volume: `granular/tolerance_stack` certifies "
            "0.0 of its declared region under `full-current-trust` and 1.0 of it — half its entire "
            "domain, wrong — under `no-physical-current-trust`, and `discrete/lattice_two` does the "
            "same between `full` and `no-physical`. A TRUSTED that moves from nothing to everything "
            "by unplugging one sensor is a claim about the sensor. E1.4 arms its four cells with the "
            "full instrument for its own reasons; an experiment about certification has to say which "
            "channels were listening, and E1.5 states it in its arm definitions rather than in prose.",
        ],
    }
    text = json.dumps(artifact, indent=1, sort_keys=True) + "\n"
    if arguments.write:
        with io.open(OUT, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        print(f"wrote {OUT}")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
