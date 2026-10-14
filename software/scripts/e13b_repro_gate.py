#!/usr/bin/env python3
"""e13b_repro_gate.py — re-measure every committed E1.x arm and prove the trust refactor moved nothing.

E1.3b's reference arm is the rule that already produced the committed files, so the experiment only
means something if that rule still produces them. This runs the frozen `e1-geometry` protocol one arm
at a time, exactly as `scripts/e1x-run.sh` ran it — `--plan` with `--arm`, no overrides — into a scratch
namespace, and compares each replayed document with the committed one it must be indistinguishable from.

What has to be identical, because it is what the identity digests or what the measurement claims: the
identity itself (which is also the filename), the plan section, the entry list and its digests, the
protocol block, the refused entries, the cross-arm comparison block, every sweep's summary fields, and
every row's every declared field.

What is allowed to differ, and is reported rather than hidden:
- `wall_ms`, a property of the machine that ran rather than of the measurement. The protocol says so, and
  the two sums are printed so a reader can see the column was looked at and not merely skipped.
- The fields this build writes and those runs predate: the top-level `trust` mirror, the top-level
  `reachability` section, and each row's `non_finite_readings`. All three sit outside the measurement
  identity, and a document that gained a column is not a document that changed a number — but the claim
  is only as good as the check, so any *other* added or missing key is a problem.

Exit is non-zero if any arm differs, and the gate's own rule is that a failed gate stops the experiment.

The comparison is also written as `benchmarks/analysis/e1-3b-reproduction-gate.json`, in the same
namespace as the other committed analysis artefacts, so a reader can see which sections were declared
comparable and what each arm's verdict was without re-running anything. Pass `--commit` to record which
commit the measured binary was built from; without it the artefact says `unknown` rather than guessing.
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
PROTOCOL = os.path.join(ROOT, "benchmarks", "protocols", "e1-geometry.json")
COMMITTED = os.path.join(ROOT, "benchmarks", "results")
OUT = os.path.join(ROOT, ".scratch", "e13b-repro-results")
ARCHIVE = os.path.join(ROOT, ".scratch", "e13b-repro-archives")

# Written by this build and predating the committed files; each is outside the identity, and the gate's
# job is to say which of the two is true for any given key rather than to assume the answer.
ADDED_TOP = {"trust", "reachability"}
ADDED_ROW = {"non_finite_readings"}
# Declared not part of a measurement before any of this ran, in the protocol's own words.
NON_IDENTITY_ROW = {"wall_ms"}


def canonical(value):
    return json.dumps(value, sort_keys=True)


def committed_arms():
    """Every arm the committed results directory holds for the E1.x protocol, ordered by file name."""
    out = []
    for name in sorted(os.listdir(COMMITTED)):
        # The directory's own README is prose about the namespace, not a document in it.
        if not name.startswith("results-") or not name.endswith(".json"):
            continue
        path = os.path.join(COMMITTED, name)
        with io.open(path, encoding="utf-8") as handle:
            doc = json.load(handle)
        protocol = doc.get("protocol") or {}
        if protocol.get("id") == "e1-geometry":
            out.append((protocol["arm"], path, doc))
    return out


def compare(arm, committed, replayed, problems):
    """Field-by-field equality over everything the committed document declares. Returns the row count."""
    if replayed.get("identity") != committed["identity"]:
        problems.append(
            f"{arm}: identity {replayed.get('identity')} is not the committed {committed['identity']}"
        )
    for key in sorted(set(committed) | set(replayed)):
        if key in ("sweeps", "identity"):
            continue
        if key not in committed:
            if key not in ADDED_TOP:
                problems.append(f"{arm}: the replay added the top-level field {key!r}")
            continue
        if key not in replayed:
            problems.append(f"{arm}: the replay lost the top-level field {key!r}")
            continue
        if canonical(committed[key]) != canonical(replayed[key]):
            problems.append(f"{arm}: the top-level field {key!r} differs")
    if len(committed["sweeps"]) != len(replayed["sweeps"]):
        problems.append(
            f"{arm}: {len(committed['sweeps'])} committed sweeps, {len(replayed['sweeps'])} replayed"
        )
        return 0
    rows = 0
    for sweep_old, sweep_new in zip(committed["sweeps"], replayed["sweeps"]):
        for key in sweep_old:
            if key == "outcomes":
                continue
            if canonical(sweep_old[key]) != canonical(sweep_new.get(key)):
                problems.append(
                    f"{arm}: sweep {sweep_old['entry']} seed {sweep_old['seed']} field {key!r} "
                    f"committed {sweep_old[key]!r} replayed {sweep_new.get(key)!r}"
                )
        if len(sweep_old["outcomes"]) != len(sweep_new["outcomes"]):
            problems.append(f"{arm}: sweep {sweep_old['entry']} holds a different number of rows")
            continue
        for row_old, row_new in zip(sweep_old["outcomes"], sweep_new["outcomes"]):
            rows += 1
            if (row_old["entry"], row_old["budget"]) != (row_new["entry"], row_new["budget"]):
                problems.append(f"{arm}: rows are out of order at {row_old['entry']}")
                continue
            for key in sorted(set(row_old) | set(row_new)):
                if key not in row_old:
                    if key not in ADDED_ROW:
                        problems.append(
                            f"{arm}: row {row_new['entry']} carries {key!r}, which no committed row has"
                        )
                    continue
                if key not in row_new:
                    problems.append(f"{arm}: row {row_old['entry']} lost the field {key!r}")
                    continue
                if key in NON_IDENTITY_ROW:
                    continue
                if canonical(row_old[key]) != canonical(row_new[key]):
                    problems.append(
                        f"{arm}: row {row_old['entry']} budget {row_old['budget']} field {key!r} "
                        f"committed {row_old[key]!r} replayed {row_new[key]!r}"
                    )
    return rows


def wall_ms(doc):
    return sum(row["wall_ms"] for sweep in doc["sweeps"] for row in sweep["outcomes"])


def digest(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--commit", default="unknown", help="the commit the measured binary was built from")
    arguments = parser.parse_args(argv)
    for directory in (OUT, ARCHIVE):
        os.makedirs(directory, exist_ok=True)
    print(f"binary   {BIN} sha256 {digest(BIN)}")
    print(f"built from commit {arguments.commit}")
    print(f"protocol {PROTOCOL}")
    print(f"results  {OUT}")
    print(f"archives {ARCHIVE}\n")
    print(f"{'arm':<18} {'identity':<14} {'sweeps':>7} {'rows':>5} {'ms':>6}  verdict")
    problems = []
    total_rows = 0
    arms = committed_arms()
    records = []
    for arm, path, committed in arms:
        replayed_path = os.path.join(OUT, os.path.basename(path))
        if os.path.exists(replayed_path):
            os.remove(replayed_path)
        before = len(problems)
        run = subprocess.run(
            [BIN, "run", "--plan", PROTOCOL, "--arm", arm, "--out", OUT, "--archive", ARCHIVE],
            capture_output=True,
            text=True,
        )
        if run.returncode != 0:
            problems.append(f"{arm}: the runner exited {run.returncode}: {run.stdout}{run.stderr}")
            print(f"{arm:<18} {'-':<14} {'-':>7} {'-':>5} {'-':>6}  RUN FAILED")
            continue
        if not os.path.exists(replayed_path):
            problems.append(f"{arm}: the replay wrote no {os.path.basename(path)}")
            print(f"{arm:<18} {'-':<14} {'-':>7} {'-':>5} {'-':>6}  NO FILE")
            continue
        with io.open(replayed_path, encoding="utf-8") as handle:
            replayed = json.load(handle)
        rows = compare(arm, committed, replayed, problems)
        total_rows += rows
        verdict = "identical" if len(problems) == before else "DIFFERS"
        records.append(
            {
                "arm": arm,
                "document": os.path.basename(path),
                "identity": committed["identity"],
                "identity_replayed": replayed.get("identity"),
                "sweeps": len(committed["sweeps"]),
                "rows": rows,
                "wall_ms_committed": wall_ms(committed),
                "wall_ms_replayed": wall_ms(replayed),
                "verdict": verdict,
            }
        )
        print(
            f"{arm:<18} {committed['identity']:<14} {len(committed['sweeps']):>7} {rows:>5} "
            f"{wall_ms(replayed):>6}  {verdict}"
        )
    print(f"\n{len(arms)} arms, {total_rows} rows compared field by field, wall_ms excluded by rule")
    artifact = {
        "schema": "aporia.reproduction-gate/1",
        "generated_by": "software/scripts/e13b_repro_gate.py",
        "purpose": "E1.3b's frozen reproduction gate: the trust refactor must relabel nothing that was "
        "already measured, because the experiment's reference arm is the rule that produced these files.",
        "binary": {"path": "software/.scratch/e1x-bin/aporia-bench.exe", "sha256": digest(BIN), "built_from_commit": arguments.commit},
        "protocol": "benchmarks/protocols/e1-geometry.json",
        "namespaces": {
            "results": "software/.scratch/e13b-repro-results",
            "archives": "software/.scratch/e13b-repro-archives",
            "why": "a replay is not a second measurement, and the committed identity is already taken by "
            "the file this gate is checked against",
        },
        "compared": [
            "identity",
            "plan section",
            "entries and their truth digests",
            "protocol block (id, arm, digest)",
            "environment",
            "refused entries",
            "cross-arm comparison block",
            "every sweep's summary fields",
            "every row's every declared field",
        ],
        "excluded": {
            "row_fields": sorted(NON_IDENTITY_ROW),
            "reason": "wall-clock is a property of the machine that ran, declared non-identity in the "
            "frozen protocol before any of this was measured",
            "added_by_this_build": {
                "top_level": sorted(ADDED_TOP),
                "row": sorted(ADDED_ROW),
                "note": "outside the measurement identity; any other added or missing key is a problem "
                "and this gate would have stopped",
            },
        },
        "arms": records,
        "totals": {"arms": len(records), "rows_compared": total_rows, "problems": len(problems)},
        "problems": problems,
        "verdict": "PASSED" if not problems else "FAILED",
    }
    target = os.path.join(ROOT, "benchmarks", "analysis", "e1-3b-reproduction-gate.json")
    with io.open(target, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(artifact, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(f"wrote {os.path.relpath(target, ROOT)}")
    if problems:
        print(f"\n{len(problems)} problem(s):")
        for line in problems:
            print("  PROBLEM", line)
        print("\nTHE REPRODUCTION GATE FAILED — E1.3b must not be measured until it is understood.")
        return 1
    print("\nTHE REPRODUCTION GATE PASSED: every committed field is what the replay wrote.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

