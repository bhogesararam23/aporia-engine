#!/usr/bin/env python3
"""e14_repro_gate.py — re-measure every committed experiment and prove the scale axis moved nothing.

E1.4's control arm is the semantics that already produced every committed row in this repository, so the
experiment means something only if that semantics still produces them. This runs all twenty-three published
arms — eleven of the frozen `e1-geometry` protocol, eight of `e1-3b-trust-resolution` and four of
`e1-4-resolution-demand` — one at a time, exactly as the scripts that measured them ran them (`--plan` with
`--arm`, and no flag that shapes what is measured), into a scratch namespace, and compares each replayed
document with the committed one it has to be indistinguishable from.

The first two protocols are the gate the freeze asks for: an arm of an *earlier* experiment reproduced by
*this* build. The four E1.4 arms are in here for a different reason, and it is a provenance reason. The
runner refuses to write a second copy of a measurement it can see is the same one, so the published E1.4
rows can only be re-measured into a scratch namespace and compared — which is what binds them to the bytes
this gate names rather than to the hour of the day they were first written. An E1.4 arm that replayed
different numbers would be a determinism defect in the run that is about to be reported.

The comparison is the strong one, and it has to be: this phase added a fourth label, a policy axis, five
row columns, two top-level document fields and an archive identity note. Any one of those touching the
default path would show up here as a committed number moving.

What must be identical: the identity (which is also the filename), the plan section, the entry list and
its digests, the protocol block, the refusals, the comparison block, every sweep summary field, and every
declared field of every row.

What is allowed to differ, and is reported rather than skipped:
  - `wall_ms`, which the protocol declares a property of the machine.
  - Fields this build writes and those documents predate, listed below by name. A document that gained a
    column is not a document whose numbers changed; a document that gained a column *not* on this list is
    a change of meaning, and the gate refuses it.
  - The top-level `reachability` section, which recomputes what the instrument could enclose. It sits
    outside the identity, and the witness ranking changed under it — `granular/fit_band` is the only entry
    whose class moved, and it is an E1.4 entry, not a row either experiment scored.

A second check runs on the archives the replay writes: every manifest must name the measurement identity
its own document publishes. That is the archive-identity change proved on real bytes rather than on a
fixture, and it is what lets the pre-run guard compare identities instead of inferring them.

Exit is non-zero if any arm differs or any manifest fails to name its measurement, and the frozen rule is
that a failed gate stops the experiment before any E1.4 arm is read.
"""

import argparse
import hashlib
import io
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, ".scratch", "e1x-bin", "aporia-bench.exe")
RESULTS = os.path.join(ROOT, "benchmarks", "results")
OUT = os.path.join(ROOT, ".scratch", "e14-repro-results")
ARCHIVE = os.path.join(ROOT, ".scratch", "e14-repro-archives")
ARTIFACT = os.path.join(ROOT, "benchmarks", "analysis", "e1-4-reproduction-gate.json")

PROTOCOLS = ["e1-geometry", "e1-3b-trust-resolution", "e1-4-resolution-demand"]

# Written by this build and predating the committed documents. Every one of these is outside the
# measurement identity, which is why adding it cannot rename a measurement — and the reason the list is
# closed is that "a document gained a column" is otherwise unfalsifiable.
ADDED_TOP = {"trust", "reachability", "resolution", "certify_share"}
ADDED_ROW = {
    "non_finite_readings",
    "provisional_volume",
    "coarse_certified_volume",
    "coarse_certified_share",
    "trusted_over_true_coarse",
    "leaf_cells",
    "thin_certified_volume",
    # The region decomposition, added after E1.4 ran. Six columns, none of them inside the identity,
    # and none of them a reading any committed document carries: a replayed arm that changed the
    # twelve fields beside these would still fail this gate, which is the only reason the six are safe
    # to add.
    "declared_union_volume",
    "trusted_over_true_union",
    "unknown_over_true",
    "suspicious_over_true",
    "provisional_over_true",
    "region_overlap_refusals",
}
# Declared not part of a measurement before any of this ran, in the protocols' own words.
NON_IDENTITY_ROW = {"wall_ms"}


def canonical(value):
    return json.dumps(value, sort_keys=True)


def digest(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def committed_arms():
    """Every published arm of the frozen experiments, ordered by file name."""
    out = []
    for name in sorted(os.listdir(RESULTS)):
        # The directory's README is prose about the namespace, not a document in it.
        if not name.startswith("results-") or not name.endswith(".json"):
            continue
        path = os.path.join(RESULTS, name)
        with io.open(path, encoding="utf-8") as handle:
            doc = json.load(handle)
        protocol = doc.get("protocol") or {}
        if protocol.get("id") in PROTOCOLS:
            out.append((protocol["id"], protocol["arm"], path, doc))
    return out


def compare(protocol, arm, committed, replayed, problems):
    """Field-by-field equality over everything the committed document declares. Returns the row count."""
    where = f"{protocol}/{arm}"
    if replayed.get("identity") != committed["identity"]:
        problems.append(
            f"{where}: identity {replayed.get('identity')} is not the committed {committed['identity']}"
        )
    for key in sorted(set(committed) | set(replayed)):
        if key in ("sweeps", "identity"):
            continue
        if key not in committed:
            if key not in ADDED_TOP:
                problems.append(f"{where}: the replay added the top-level field {key!r}")
            continue
        if key not in replayed:
            problems.append(f"{where}: the replay lost the top-level field {key!r}")
            continue
        if key == "reachability":
            # Recomputed from the partition rule, which this phase changed for a thin carve. Listed
            # by kind below -- section added, class changed, or figures moved with the class held --
            continue
        if canonical(committed[key]) != canonical(replayed[key]):
            problems.append(f"{where}: the top-level field {key!r} differs")
    if len(committed["sweeps"]) != len(replayed["sweeps"]):
        problems.append(
            f"{where}: {len(committed['sweeps'])} committed sweeps, {len(replayed['sweeps'])} replayed"
        )
        return 0
    rows = 0
    for sweep_old, sweep_new in zip(committed["sweeps"], replayed["sweeps"]):
        for key in sweep_old:
            if key == "outcomes":
                continue
            if canonical(sweep_old[key]) != canonical(sweep_new.get(key)):
                problems.append(
                    f"{where}: sweep {sweep_old['entry']} seed {sweep_old['seed']} field {key!r} "
                    f"committed {sweep_old[key]!r} replayed {sweep_new.get(key)!r}"
                )
        if len(sweep_old["outcomes"]) != len(sweep_new["outcomes"]):
            problems.append(f"{where}: sweep {sweep_old['entry']} holds a different number of rows")
            continue
        for row_old, row_new in zip(sweep_old["outcomes"], sweep_new["outcomes"]):
            rows += 1
            if (row_old["entry"], row_old["budget"]) != (row_new["entry"], row_new["budget"]):
                problems.append(f"{where}: rows are out of order at {row_old['entry']}")
                continue
            for key in sorted(set(row_old) | set(row_new)):
                if key not in row_old:
                    if key not in ADDED_ROW:
                        problems.append(
                            f"{where}: row {row_new['entry']} carries {key!r}, which no committed row has"
                        )
                    continue
                if key not in row_new:
                    problems.append(f"{where}: row {row_old['entry']} lost the field {key!r}")
                    continue
                if key in NON_IDENTITY_ROW:
                    continue
                if canonical(row_old[key]) != canonical(row_new[key]):
                    problems.append(
                        f"{where}: row {row_old['entry']} budget {row_old['budget']} field {key!r} "
                        f"committed {row_old[key]!r} replayed {row_new[key]!r}"
                    )
    return rows


def reachability_movement(committed, replayed):
    """What the recomputed `reachability` section says differently, in the three kinds that mean things.

    The section sits outside the measurement identity and is recomputed from the partition rule rather
    than recorded from the run, so it is the one part of a replay this phase was allowed to change
    without that being a changed number. "Changed" is three claims and they read differently:

      - `section_added`: the committed document predates the section, so there was no old answer. Every
        E1.x document is like this, and listing its entries as "moved" would be the error this function
        exists to avoid.
      - `class_changed`: an entry the instrument used to call unresolvable it now calls resolvable, or
        the reverse. This is the only kind that could touch a frozen claim.
      - `figures_changed`: same entry, same class, different arithmetic behind it -- the share a witness
        reached, how many candidates were measured. Reported because the witness ranking is what moved
        them, and a reader deserves to know the figure under a published class is not the figure that
        would be written today.
    """
    out = {"section_added": [], "class_changed": [], "figures_changed": []}
    if not committed.get("reachability"):
        out["section_added"] = sorted(row["entry"] for row in replayed.get("reachability", []))
        return out
    old = {row["entry"]: row for row in committed["reachability"]}
    new = {row["entry"]: row for row in replayed.get("reachability", [])}
    for entry in sorted(set(old) & set(new)):
        if old[entry].get("class") != new[entry].get("class"):
            out["class_changed"].append(
                {
                    "entry": entry,
                    "committed": old[entry].get("class"),
                    "replayed": new[entry].get("class"),
                }
            )
            continue
        figures = ("best_declared_share", "best_envelope_share", "candidates", "witness_depth")
        before = [
            [region.get(key) for key in figures] for region in old[entry].get("regions", [])
        ]
        after = [
            [region.get(key) for key in figures] for region in new[entry].get("regions", [])
        ]
        if before != after:
            out["figures_changed"].append(
                {"entry": entry, "class": new[entry].get("class"), "committed": before, "replayed": after}
            )
    return out


def archive_identities(arm_directory):
    """Every measurement identity an archive in this tree names, and any that names none."""
    named = set()
    missing = []
    for root, _dirs, files in os.walk(arm_directory):
        if "manifest.json" not in files:
            continue
        path = os.path.join(root, "manifest.json")
        with io.open(path, encoding="utf-8") as handle:
            manifest = json.load(handle)
        notes = ((manifest.get("environment") or {}).get("notes")) or {}
        identity = notes.get("measurement_identity")
        if identity:
            named.add(identity)
        else:
            missing.append(os.path.relpath(root, arm_directory))
    return named, missing


def wall_ms(doc):
    return sum(row["wall_ms"] for sweep in doc["sweeps"] for row in sweep["outcomes"])


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--commit", default="unknown", help="the commit the measured binary was built from"
    )
    parser.add_argument(
        "--recompute-report",
        action="store_true",
        help=(
            "read the replayed documents from the previous gate's scratch directory instead of "
            "measuring again, for re-describing bytes already produced. The verdict is computed from "
            "the same documents either way; this never re-decides an arm."
        ),
    )
    parser.add_argument(
        "--artifact",
        default=ARTIFACT,
        help=(
            "where to write the gate's own record. It defaults to the published E1.4 gate file, and "
            "a later phase that re-runs this gate to certify *its* binary has to point it somewhere "
            "else: the published file is the record of the run that preceded E1.4's arms, and "
            "overwriting it with a different binary's verdict would revise a completed experiment."
        ),
    )
    parser.add_argument(
        "--purpose",
        default=(
            "prove that the scale axis E1.4 measures left every committed measurement it does not name "
            "exactly as published, and that the bytes doing that also write the four E1.4 arms now being "
            "read -- which is what binds a published row to a commit rather than to a time of day"
        ),
        help="what this run of the gate was for, written into the record it produces",
    )
    arguments = parser.parse_args(argv)
    for directory in (OUT, ARCHIVE):
        os.makedirs(directory, exist_ok=True)
    binary_sha = digest(BIN)
    print(f"binary   {BIN} sha256 {binary_sha}")
    print(f"built from commit {arguments.commit}")
    print(f"results  {OUT}")
    print(f"archives {ARCHIVE}\n")
    print(f"{'protocol/arm':<40} {'identity':<14} {'swp':>4} {'rows':>5} {'ms':>6}  verdict")
    problems = []
    total_rows = 0
    records = []
    for protocol, arm, path, committed in committed_arms():
        where = f"{protocol}/{arm}"
        replayed_path = os.path.join(OUT, os.path.basename(path))
        if os.path.exists(replayed_path):
            os.remove(replayed_path)
        archive_root = os.path.join(ARCHIVE, protocol, arm)
        before = len(problems)
        if arguments.recompute_report and os.path.exists(replayed_path):
            # The bytes this gate already produced, described differently and not re-measured.
            run = subprocess.CompletedProcess([BIN], 0, "", "")
        else:
            run = subprocess.run(
                [
                    BIN,
                    "run",
                    "--plan",
                    os.path.join(ROOT, "benchmarks", "protocols", f"{protocol}.json"),
                    "--arm",
                    arm,
                    "--out",
                    OUT,
                    "--archive",
                    archive_root,
                ],
                capture_output=True,
                text=True,
            )
        if run.returncode != 0:
            problems.append(f"{where}: the runner exited {run.returncode}: {run.stdout}{run.stderr}")
            print(f"{where:<40} {'-':<14} {'-':>4} {'-':>5} {'-':>6}  RUN FAILED")
            continue
        if not os.path.exists(replayed_path):
            problems.append(f"{where}: the replay wrote no {os.path.basename(path)}")
            print(f"{where:<40} {'-':<14} {'-':>4} {'-':>5} {'-':>6}  NO FILE")
            continue
        with io.open(replayed_path, encoding="utf-8") as handle:
            replayed = json.load(handle)
        rows = compare(protocol, arm, committed, replayed, problems)
        total_rows += rows
        movement = reachability_movement(committed, replayed)
        named, nameless = archive_identities(archive_root)
        if nameless:
            problems.append(
                f"{where}: {len(nameless)} archive(s) record no measurement identity: "
                + ", ".join(sorted(nameless)[:3])
            )
        if named and named != {committed["identity"]}:
            problems.append(
                f"{where}: archives name {sorted(named)} while the document publishes "
                f"{committed['identity']}"
            )
        records.append(
            {
                "protocol": protocol,
                "arm": arm,
                "document": os.path.basename(path),
                "identity": committed["identity"],
                "identity_replayed": replayed.get("identity"),
                "sweeps": len(committed["sweeps"]),
                "rows": rows,
                "wall_ms_committed": wall_ms(committed),
                "wall_ms_replayed": wall_ms(replayed),
                "archives_naming_this_measurement": sorted(named),
                "reachability": movement,
                "verdict": "identical" if len(problems) == before else "DIFFERS",
            }
        )
        print(
            f"{where:<40} {committed['identity']:<14} {len(committed['sweeps']):>4} {rows:>5} "
            f"{wall_ms(replayed):>6}  {records[-1]['verdict']}"
        )
    added = sorted(
        {entry for record in records for entry in record["reachability"]["section_added"]}
    )
    classes = sorted(
        {
            item["entry"]
            for record in records
            for item in record["reachability"]["class_changed"]
        }
    )
    figures = sorted(
        {
            item["entry"]
            for record in records
            for item in record["reachability"]["figures_changed"]
        }
    )
    print(
        f"reachability: {len(added)} entries in documents that predate the section, "
        f"{len(classes)} classes changed {classes or '-'}, {len(figures)} entries whose figures "
        f"moved with the class held {figures or '-'}"
    )
    artifact = {
        "schema": "aporia.reproduction-gate/2",
        "generated_by": "scripts/e14_repro_gate.py",
        "purpose": arguments.purpose,
        "binary_sha256": binary_sha,
        "built_from_commit": arguments.commit,
        "protocols": PROTOCOLS,
        "compared": [
            "identity",
            "plan",
            "entries",
            "protocol",
            "question",
            "refused",
            "comparison",
            "sweep summaries",
            "every declared field of every row",
        ],
        "excluded": [
            {
                "field": "wall_ms",
                "why": "declared a property of the machine, not of the measurement, by the protocols",
            },
            {
                "field": "reachability",
                "why": (
                    "recomputed from the partition rule rather than recorded from the run, and outside "
                    "the identity; the witness ranking changed under it, so the movement is listed per "
                    "entry rather than compared away"
                ),
            },
        ],
        "allowed_added_fields": {
            "top_level": sorted(ADDED_TOP),
            "row": sorted(ADDED_ROW),
            "rule": (
                "any other added or missing key is a problem: the list is closed so that a document "
                "gaining a column stays a claim that can fail"
            ),
        },
        "archive_identity_rule": (
            "every manifest the replay writes must name the measurement identity its own document "
            "publishes; an archive that names none is a problem, because the pre-run guard compares "
            "identities and can only be exact if the bytes it reads carry one"
        ),
        "arms": records,
        "rows_compared": total_rows,
        "reachability_summary": {
            "section_added_for_entries": added,
            "classes_changed": classes,
            "figures_changed_with_class_held": figures,
            "why_reported": (
                "the section is recomputed from the partition rule and sits outside the identity; the "
                "witness ranking changed under it, so the movement is listed by kind rather than "
                "compared away or left silent"
            ),
        },
        "problems": problems,
        "verdict": "PASSED" if not problems else "FAILED",
    }
    os.makedirs(os.path.dirname(os.path.abspath(arguments.artifact)), exist_ok=True)
    with io.open(arguments.artifact, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(artifact, handle, indent=1, sort_keys=True)
        handle.write("\n")
    print(f"artifact {arguments.artifact}: {artifact['verdict']}")
    for problem in problems[:20]:
        print(f"  {problem}")
    if len(problems) > 20:
        print(f"  ... and {len(problems) - 20} more")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
