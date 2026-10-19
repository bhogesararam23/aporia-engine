#!/usr/bin/env python3
"""e15_prerun_check.py — measure a frozen entry against the prediction its own record makes.

E1.5 names `granular/quiet_band` as the case the experiment turns on, and its freeze says the entry's
record carries a prediction: that the default instrument certifies at least 0.05 of the region it
declares. A prediction an entry makes about an instrument is only evidence if somebody checks it, and
checking it after the arms have run would be reading the outcome first — which is the thing a
pre-registration exists to forbid. So this measures the *default policy*, which is arm A-current's own
configuration, over the deciding entry at the plan's frozen budgets and seeds, and writes what it found
next to the sentence that predicted it.

What this is not matters as much. It measures no arm of the comparison, decides no outcome, moves no
threshold, and is not an E1.5 result: the freeze's status line stays true because no E1.5 arm has run.
It is the entry's design claim checked against the instrument before the experiment that will use it,
published so that the amendment it supports is verifiable from committed bytes rather than quoted from
a terminal.

    scripts/e15_prerun_check.py --commit <sha> [--binary PATH] [--entry family/name] [--write]

Exit is non-zero if a row cannot be read, because an artifact that silently dropped a row would report
a mean over a set nobody chose.
"""

import argparse
import hashlib
import io
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROTOCOL = os.path.join(ROOT, "benchmarks", "protocols", "e1-5-spatial-uncertainty-cost.json")
ARTIFACT = os.path.join(ROOT, "benchmarks", "analysis", "e1-5-deciding-entry-prerun.json")
OUT = os.path.join(ROOT, ".scratch", "e15-prerun-results")
ARCHIVE = os.path.join(ROOT, ".scratch", "e15-prerun-archives")

# The columns the artifact copies out of the row, in the order a reader meets them. A row is copied
# rather than re-derived: `certified_over_region_share` is recomputed here only to check that the two
# columns the frozen metric divides still multiply out to the number the freeze's rule reads.
ROW_COLUMNS = [
    "budget",
    "seed",
    "evaluations",
    "instruction_steps",
    "leaf_cells",
    "declared_union_volume",
    "trusted_over_true_union",
    "trusted_volume",
    "unknown_volume",
    "provisional_volume",
    "suspicious_volume",
    "coarse_certified_volume",
    "coarse_certified_share",
    "thin_certified_volume",
    "region_overlap_refusals",
    "detected_regions",
    "localised_regions",
]


def digest(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def load(path):
    with io.open(path, encoding="utf-8") as handle:
        return json.load(handle)


def rows_of(document, entry):
    return [
        outcome
        for sweep in document["sweeps"]
        for outcome in sweep["outcomes"]
        if outcome["entry"] == entry
    ]


def share(row):
    """The frozen primary metric for one row, or why this row does not have one.

    `not_applicable` and `not_measurable` are the freeze's own words and they are kept apart here,
    because a mean that quietly treated an entry with no declared region as a zero would be the most
    flattering number the corpus could produce.
    """
    if row["region_overlap_refusals"] != 0:
        return None, "not_measurable"
    declared = row["declared_union_volume"]
    if declared <= 0.0:
        return None, "not_applicable"
    return row["trusted_over_true_union"] / declared, "measured"


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--commit", default="unknown", help="the commit the measured binary was built from")
    parser.add_argument(
        "--binary",
        default=os.path.join(ROOT, ".scratch", "e1x-bin", "aporia-bench.exe"),
        help="the executable whose bytes did the measuring, recorded by sha256",
    )
    parser.add_argument("--entry", default="granular/quiet_band")
    parser.add_argument("--write", action="store_true", help="write the artifact into benchmarks/analysis")
    arguments = parser.parse_args(argv)

    protocol = load(PROTOCOL)
    plan = protocol["plan"]
    for directory in (OUT, ARCHIVE):
        os.makedirs(directory, exist_ok=True)
    if not os.path.exists(arguments.binary):
        raise SystemExit(f"no executable at {arguments.binary}: build it first, never measure with a stale one")

    command = [
        arguments.binary,
        "run",
        "--only",
        arguments.entry,
        "--strategies",
        ",".join(plan["strategies"]),
        "--budgets",
        ",".join(str(budget) for budget in plan["budgets"]),
        "--seeds",
        ",".join(str(seed) for seed in plan["seeds"]),
        "--grid",
        str(plan["grid_per_axis"]),
        "--differential-every",
        str(plan["differential_every"]),
        "--out",
        OUT,
        "--archive",
        ARCHIVE,
    ]
    run = subprocess.run(command, capture_output=True, text=True)
    if run.returncode != 0:
        sys.stderr.write(run.stdout + "\n" + run.stderr + "\n")
        raise SystemExit(f"the default-policy run failed with {run.returncode}")
    documents = sorted(
        os.path.join(OUT, name) for name in os.listdir(OUT) if name.startswith("results-")
    )
    if len(documents) != 1:
        raise SystemExit(f"expected one results document in {OUT}, found {len(documents)}")
    document = load(documents[0])
    rows = rows_of(document, arguments.entry)
    if len(rows) != len(plan["budgets"]) * len(plan["seeds"]):
        raise SystemExit(
            f"{len(rows)} rows measured, {len(plan['budgets']) * len(plan['seeds'])} expected: "
            "an artifact that dropped a row would report a mean over a set nobody chose"
        )

    copied = []
    shares = []
    for row in sorted(rows, key=lambda r: (r["budget"], r["seed"])):
        missing = [column for column in ROW_COLUMNS if column not in row]
        if missing:
            raise SystemExit(f"{arguments.entry} at budget {row['budget']} states none of {missing}")
        value, status = share(row)
        copied.append({column: row[column] for column in ROW_COLUMNS} | {"share": value, "status": status})
        if value is not None:
            shares.append((row["budget"], value))

    by_budget = {}
    for budget, value in shares:
        by_budget.setdefault(budget, []).append(value)
    mean = sum(value for _, value in shares) / len(shares) if shares else None
    nonzero = sorted({budget for budget, value in shares if value > 0.0})
    artifact = {
        "schema": "aporia.prerun-check/1",
        "generated_by": "scripts/e15_prerun_check.py",
        "purpose": (
            "Check the deciding entry's own prediction against the instrument before any arm of the "
            "comparison runs, so that the freeze's claim about it is a measured statement and not an "
            "intention."
        ),
        "entry": arguments.entry,
        "not_an_e1_5_result": (
            "The default policy alone was run: no arm of E1.5's comparison, no outcome of its decision "
            "rule, no threshold, ceiling or set moved. The plan it measured under is the frozen plan, "
            "copied verbatim into this artifact, and the digest it names is the freeze carrying it."
        ),
        "protocol": {
            "id": protocol["id"],
            "file": os.path.relpath(PROTOCOL, ROOT).replace(os.sep, "/"),
            "digest": hashlib.sha256(open(PROTOCOL, "rb").read()).hexdigest()[:16],
        },
        "plan": plan,
        "policy": {
            "trust": document["trust"],
            "resolution": document["resolution"],
            "certify_share": document.get("certify_share"),
            "arm_it_replicates": "A-current",
        },
        "run": {
            "identity": document["identity"],
            "command": " ".join(command[1:]).replace(ROOT, "."),
            "binary": os.path.relpath(arguments.binary, ROOT).replace(os.sep, "/"),
            "binary_sha256": digest(arguments.binary),
            "built_from_commit": arguments.commit,
        },
        "rows": copied,
        "summary": {
            "rows": len(copied),
            "applicable_measurable_rows": len(shares),
            "mean_certified_over_region_share": mean,
            "mean_trusted_over_true_union": (
                sum(row["trusted_over_true_union"] for row in rows) / len(rows)
            ),
            "rows_with_nonzero_share": sum(1 for _, value in shares if value > 0.0),
            "budgets_with_nonzero_share": nonzero,
            "mean_share_by_budget": {
                str(budget): sum(values) / len(values) for budget, values in sorted(by_budget.items())
            },
            "max_row_coarse_certified_share": max(row["coarse_certified_share"] for row in rows),
            "refusals_total": sum(row["region_overlap_refusals"] for row in rows),
        },
        "prediction": {
            "as_frozen": (
                "entries.entry_why says this entry's `expected certified-over-region share of at least "
                "0.05` is stated in its own truth record before any arm runs"
            ),
            "metric_read": (
                "certified_over_region_share as the frozen definition says: trusted_over_true_union / "
                "declared_union_volume, mean over the applicable measurable rows, which is the "
                "aggregation the primary metric names"
            ),
            "measured_mean": mean,
            "holds": mean is not None and mean >= 0.05,
            "where_the_signal_is": (
                f"{sum(1 for _, value in shares if value > 0.0)} of {len(shares)} rows are nonzero, "
                f"at budget(s) {', '.join(str(b) for b in nonzero)}"
            ),
        },
    }
    text = json.dumps(artifact, indent=1, sort_keys=True) + "\n"
    if arguments.write:
        with io.open(ARTIFACT, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        print(f"wrote {ARTIFACT}")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
