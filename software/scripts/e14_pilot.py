#!/usr/bin/env python3
"""e14_pilot.py — E1.4's infrastructure pilot: five properties, no deciding quantity.

This is not a measurement of the experiment. It runs the frozen protocol's own arms into scratch
namespaces and checks the properties the freeze said the instrument has to have before any variant number
is worth reading. Nothing here looks at `coarse_certified_volume`, `trusted_over_true`, `unknown_volume`
or localisation: those decide E1.4 and they are read from the committed run, once, after this passes.

1. **Determinism.** The control arm, twice, from the same binary, writes the same rows. `wall_ms` is
   excluded because the protocol declares it non-identity; everything else, including the identity and so
   the filename, has to match.
2. **Four namespaces, none of them shared.** The arms differ in one declared axis, so that axis has to be
   visible in the identity and in the archive directory name.
3. **The guard refuses across a measurement.** A run pointed at a directory another arm already filled
   is stopped, and stopped with a reason. That is the archive-identity change exercised on real bytes
   rather than on a fixture.
4. **Equal charged cost.** Four arms over the same (entry, seed, budget) cell carry the same `evaluations`
   and `instruction_steps`, because a resolution demand spends cells, not executions.
5. **Replay, and an archive that names its measurement.** Every archived sweep reports a replay that
   matched every record it read, and every manifest records the identity the run's own document publishes.

**Where this departs from the freeze, stated here rather than left to be noticed.** Stopping rule 2 says the
pilot is "one entry, two seeds and the two smallest budgets". This script runs the full frozen plan — five
entries, five seeds, four budgets, 100 charged cells per arm. The cause is mechanical rather than rhetorical:
with `--plan` the runner refuses every flag that shapes what is measured, so a reduced pilot needs a reduced
frozen plan, and `e1-4-resolution-demand.json` does not contain one. What rule 2 actually protects is the
property list above and the sentence "must not move a threshold, a tolerance or a criterion", and neither is
touched: nothing in this file reads a deciding quantity. What was exceeded is the cost bound, and the analysis
artefact carries that as one of its `recorded_defects` rather than as a footnote here.

One property is new to this experiment and is the reason the control arm and the withholding arm are both
run here: the two must produce the **same partition**. The archived `atlas.csv` is the record of that, so
the pilot compares geometry -- cell, depth, samples -- and allows exactly one column to differ, the
label, and only in the direction `TRUSTED` to `PROVISIONAL`. If the two partitions differ, arm C is a
different search rather than a different claim and the frozen comparison would be measuring the wrong
thing.

Exit is non-zero if any property fails, and the frozen rule is that a failed pilot stops the experiment.
"""

import argparse
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, ".scratch", "e1x-bin", "aporia-bench.exe")
PLAN = os.path.join(ROOT, "benchmarks", "protocols", "e1-4-resolution-demand.json")
SCRATCH = os.path.join(ROOT, ".scratch")
ARMS = ["A-current", "B-demand", "C-withhold", "D-demand-and-withhold"]
REFERENCE = "A-current"
PARTNER = "C-withhold"
NON_IDENTITY_ROW = {"wall_ms"}
ARTIFACT = os.path.join(ROOT, "benchmarks", "analysis", "e1-4-pilot.json")


def digest(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def archive_root_for(arm):
    if arm == REFERENCE:
        return os.path.join(SCRATCH, "e14-pilot-archives-a1")
    return os.path.join(SCRATCH, f"e14-pilot-archives-{arm}")


def run(arm, out_dir, archive_dir, problems, label):
    os.makedirs(out_dir, exist_ok=True)
    os.makedirs(archive_dir, exist_ok=True)
    result = subprocess.run(
        [BIN, "run", "--plan", PLAN, "--arm", arm, "--out", out_dir, "--archive", archive_dir],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        problems.append(
            f"{label}: the runner exited {result.returncode}: {result.stderr.strip()[:400]}"
        )
        return None
    # The runner names its document after the measurement, so the pilot finds it by listing the
    # directory rather than by guessing a filename it would then be assuming something about.
    written = [f for f in os.listdir(out_dir) if f.startswith("results-")]
    if len(written) != 1:
        problems.append(f"{label}: {out_dir} holds {len(written)} documents, expected one")
        return None
    with io.open(os.path.join(out_dir, written[0]), encoding="utf-8") as handle:
        return json.load(handle)


def rows(doc):
    return [row for sweep in doc["sweeps"] for row in sweep["outcomes"]]


def compare_rows(a, b, what, problems):
    """Every declared field of every row, in order, across two documents of the same measurement."""
    if len(a) != len(b):
        problems.append(f"{what}: {len(a)} rows against {len(b)}")
        return 0
    for left, right in zip(a, b):
        for key in sorted(set(left) | set(right)):
            if key in NON_IDENTITY_ROW:
                continue
            if key not in left or key not in right:
                problems.append(f"{what}: row {left['entry']} {key} present in only one run")
                continue
            if json.dumps(left[key], sort_keys=True) != json.dumps(right[key], sort_keys=True):
                problems.append(
                    f"{what}: row {left['entry']} seed {left['seed']} budget {left['budget']} "
                    f"field {key!r} differs: {left[key]!r} against {right[key]!r}"
                )
    return len(a)


def atlas_geometry(archive_root):
    """Every archived partition, keyed by the entry it belongs to.

    Keyed by the manifest's `entry` note and not by the directory name, because the directory name is
    the thing that differs between the arms being compared -- arm C's suffixes its rule into the name,
    which is the namespace property the pilot also checks. Two ways of naming one sweep must not be
    read as two sweeps.
    """
    out = {}
    for root, _dirs, files in os.walk(archive_root):
        if "atlas.csv" not in files or "manifest.json" not in files:
            continue
        with io.open(os.path.join(root, "manifest.json"), encoding="utf-8") as handle:
            notes = ((json.load(handle).get("environment") or {}).get("notes")) or {}
        entry = notes.get("entry") or os.path.basename(root)
        with io.open(os.path.join(root, "atlas.csv"), encoding="utf-8") as handle:
            header = handle.readline().strip().split(",")
            records = []
            for line in handle:
                cells = line.strip().split(",")
                if len(cells) == len(header):
                    records.append(dict(zip(header, cells)))
        if records:
            out[f"{entry}|seed{notes.get('seed')}"] = records
    return out


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--commit", default="unknown")
    arguments = parser.parse_args(argv)
    problems = []
    # A pilot has to be re-runnable, and the only way to know a document came from this run is to start
    # from an empty namespace: the runner names its file after the measurement, so a rerun of an
    # unchanged plan lands on the same filename and a stale directory would silently hold two of them --
    # or, worse for the partition comparison, the archives of a previous attempt would be read as this
    # one's. Nothing outside .scratch is touched.
    for stale in sorted(os.listdir(SCRATCH)):
        if stale.startswith("e14-pilot"):
            shutil.rmtree(os.path.join(SCRATCH, stale), ignore_errors=True)
    binary_sha = digest(BIN)
    plan_sha = digest(PLAN)
    print(f"binary  {binary_sha}")
    print(f"plan    {plan_sha}")
    print(f"commit  {arguments.commit}\n")

    documents = {}
    properties = {}

    # 1. Determinism: the control arm twice, into separate namespaces.
    first = run(
        REFERENCE,
        os.path.join(SCRATCH, "e14-pilot-a1"),
        archive_root_for(REFERENCE),
        problems,
        f"{REFERENCE} first pass",
    )
    second = run(
        REFERENCE,
        os.path.join(SCRATCH, "e14-pilot-a2"),
        os.path.join(SCRATCH, "e14-pilot-archives-a2"),
        problems,
        f"{REFERENCE} second pass",
    )
    compared = 0
    if first and second:
        if first.get("identity") != second.get("identity"):
            problems.append(
                f"determinism: identities {first.get('identity')} and {second.get('identity')}"
            )
        compared = compare_rows(rows(first), rows(second), "determinism", problems)
    properties["determinism"] = {
        "identity": (first or {}).get("identity"),
        "rows_compared": compared,
    }

    # 2-5: the other three arms, once each.
    for arm in ARMS[1:]:
        document = run(
            arm,
            os.path.join(SCRATCH, f"e14-pilot-{arm}"),
            archive_root_for(arm),
            problems,
            arm,
        )
        if document:
            documents[arm] = document
    if first:
        documents[REFERENCE] = first

    identities = {arm: (documents.get(arm) or {}).get("identity") for arm in ARMS}
    if len({value for value in identities.values()}) != 4:
        problems.append(f"namespaces: four arms did not produce four identities: {identities}")
    sections = {
        arm: json.dumps((documents.get(arm) or {}).get("plan"), sort_keys=True) for arm in ARMS
    }
    if len(set(sections.values())) != 4:
        problems.append("namespaces: two arms wrote the same plan section")
    properties["namespaces"] = {
        "identities": identities,
        "resolution_per_arm": {arm: (documents.get(arm) or {}).get("resolution") for arm in ARMS},
    }

    # 3. The guard, on the bytes the runs just wrote: a second arm pointed at the first arm's archives.
    # The refusal has to be tested on a *collision*, which is the case the guard exists for: one arm's
    # own directory names, filled by a different measurement. Copying the control arm's directories
    # would not do it -- the arms name themselves apart, which is the namespace property checked above,
    # and a run that cannot collide has nothing to be refused for.
    probe_root = os.path.join(SCRATCH, "e14-pilot-guard")
    shutil.rmtree(probe_root, ignore_errors=True)
    os.makedirs(probe_root, exist_ok=True)
    source = archive_root_for(PARTNER)
    control_identity = (documents.get(REFERENCE) or {}).get("identity")
    planted = 0
    for name in sorted(os.listdir(source)) if os.path.isdir(source) else []:
        target = os.path.join(probe_root, name)
        shutil.copytree(os.path.join(source, name), target)
        manifest = os.path.join(target, "manifest.json")
        with io.open(manifest, encoding="utf-8") as handle:
            text = json.load(handle)
        notes = ((text.get("environment") or {}).get("notes")) or {}
        if notes.get("measurement_identity") and notes.get("measurement_identity") != control_identity:
            notes["measurement_identity"] = control_identity
            planted += 1
        with io.open(manifest, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(text, handle)
    if planted == 0:
        problems.append(
            "guard: no colliding archive could be planted, so the refusal was never attempted"
        )
    moved = subprocess.run(
        [
            BIN,
            "run",
            "--plan",
            PLAN,
            "--arm",
            PARTNER,
            "--out",
            os.path.join(SCRATCH, "e14-pilot-guard-out"),
            "--archive",
            probe_root,
        ],
        capture_output=True,
        text=True,
    )
    noise = moved.stdout + moved.stderr
    refused = moved.returncode != 0 and (
        "belongs to measurement" in noise or "no measurement identity" in noise
    )
    if not refused:
        problems.append(
            f"guard: an arm was allowed to write over another arm's archives: exit "
            f"{moved.returncode} {noise.strip()[:300]}"
        )
    last_line = noise.strip().splitlines()[-1][:400] if noise.strip() else ""
    properties["guard_refusal"] = {
        "exit": moved.returncode,
        "message": last_line,
        "passed": refused,
    }

    # 4. Equal charged cost, on every (entry, seed, budget) cell.
    keyed = {
        arm: {(r["entry"], r["seed"], r["budget"]): r for r in rows(doc)}
        for arm, doc in documents.items()
    }
    cells = set()
    for arm in ARMS:
        cells |= set(keyed.get(arm, {}))
    mismatches = []
    for cell in sorted(cells):
        charges = {
            (keyed[arm][cell]["evaluations"], keyed[arm][cell]["instruction_steps"])
            for arm in ARMS
            if cell in keyed.get(arm, {})
        }
        if len(charges) != 1:
            mismatches.append({"cell": list(cell), "charges": [list(c) for c in charges]})
    if mismatches:
        problems.append(
            f"cost: {len(mismatches)} of {len(cells)} cells charged differently across arms, first "
            f"{mismatches[0]}"
        )
    properties["charged_cost"] = {
        "cells": len(cells),
        "mismatched": len(mismatches),
        "evaluations_total_per_arm": {
            arm: sum(r["evaluations"] for r in rows(doc)) for arm, doc in documents.items()
        },
    }

    # 5. Replay, and the identity each archive names.
    replays = []
    for arm, doc in documents.items():
        for sweep in doc["sweeps"]:
            replay = sweep.get("replay") or {}
            if not replay:
                continue
            if not replay.get("reproduced") or replay.get("matched") != replay.get("total"):
                problems.append(
                    f"replay: {arm} {sweep['entry']} seed {sweep['seed']} reports {replay}"
                )
            replays.append(replay)
    named = {}
    nameless = []
    for arm in ARMS:
        seen = set()
        for base, _dirs, files in os.walk(archive_root_for(arm)):
            if "manifest.json" not in files:
                continue
            with io.open(os.path.join(base, "manifest.json"), encoding="utf-8") as handle:
                notes = ((json.load(handle).get("environment") or {}).get("notes")) or {}
            identity = notes.get("measurement_identity")
            if identity:
                seen.add(identity)
            else:
                nameless.append(f"{arm}/{os.path.relpath(base, archive_root_for(arm))}")
        published = identities.get(arm)
        if seen != {published}:
            problems.append(f"archive identity: {arm} named {sorted(seen)}, publishes {published}")
        named[arm] = sorted(seen)
    if nameless:
        problems.append(f"archive identity: {len(nameless)} manifest(s) name no measurement")
    properties["replay_and_identity"] = {
        "archived_sweeps": len(replays),
        "records_matched": sum(r.get("matched", 0) for r in replays),
        "records_total": sum(r.get("total", 0) for r in replays),
        "identities_per_arm": named,
    }

    # The axis property: arm A and arm C, one partition, two claims about it.
    geometry_a = atlas_geometry(archive_root_for(REFERENCE))
    geometry_c = atlas_geometry(archive_root_for(PARTNER))
    shared = sorted(set(geometry_a) & set(geometry_c))
    if not shared:
        problems.append("same partition: no archived partition was written by both arms")
    moved_labels = 0
    total_cells = 0
    for key in shared:
        left_rows, right_rows = geometry_a[key], geometry_c[key]
        if len(left_rows) != len(right_rows):
            problems.append(
                f"same partition: {key} holds {len(left_rows)} cells in A and {len(right_rows)} in C"
            )
            continue
        for left, right in zip(left_rows, right_rows):
            total_cells += 1
            for column in ("cell", "depth", "samples"):
                if left.get(column) != right.get(column):
                    problems.append(
                        f"same partition: {key} cell {left.get('cell')} column {column} "
                        f"{left.get(column)!r} against {right.get(column)!r}"
                    )
            if left.get("label") != right.get("label"):
                moved_labels += 1
                if not (
                    left.get("label") == "TRUSTED" and right.get("label") == "PROVISIONAL"
                ):
                    problems.append(
                        f"same partition: {key} label moved {left.get('label')} to "
                        f"{right.get('label')}, which is not the withheld state's move"
                    )
    properties["same_partition"] = {
        "partitions_compared": len(shared),
        "cells_compared": total_cells,
        "labels_moved": moved_labels,
    }

    verdict = "PASSED" if not problems else "FAILED"
    print(f"arms             {len(documents)} of 4 run")
    print(f"rows per arm     {compared}")
    print(f"charged cells    {properties['charged_cost']['cells']}")
    print(
        f"archived sweeps  {len(replays)}, records matched "
        f"{sum(r.get('matched', 0) for r in replays)} of {sum(r.get('total', 0) for r in replays)}"
    )
    print(
        f"partition compare {properties['same_partition']['cells_compared']} cells, "
        f"{moved_labels} labels moved"
    )
    print(f"\npilot {verdict}")
    artifact = {
        "schema": "aporia.pilot/2",
        "generated_by": "scripts/e14_pilot.py",
        "protocol": os.path.basename(PLAN),
        "protocol_sha256": plan_sha,
        "protocol_digest_prefix": plan_sha[:16],
        "binary_sha256": binary_sha,
        "built_from_commit": arguments.commit,
        "purpose": (
            "the five infrastructure properties E1.4's freeze requires, and the partition identity that "
            "makes its control arm a control rather than a second search"
        ),
        "not_a_measurement_of": [
            "coarse_certified_volume",
            "trusted_over_true",
            "unknown_volume",
            "localisation",
            "any verdict about the four arms",
        ],
        "scratch": {
            "results_root": "software/.scratch/e14-pilot-*",
            "archives_root": "software/.scratch/e14-pilot-archives-*",
        },
        "checks": properties,
        "problems": problems,
        "verdict": verdict,
    }
    os.makedirs(os.path.dirname(ARTIFACT), exist_ok=True)
    with io.open(ARTIFACT, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(artifact, handle, indent=1, sort_keys=True)
        handle.write("\n")
    for problem in problems[:25]:
        print(f"  {problem}")
    if len(problems) > 25:
        print(f"  ... and {len(problems) - 25} more")
    print(f"artifact {ARTIFACT}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
