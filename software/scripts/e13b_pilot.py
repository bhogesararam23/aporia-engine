#!/usr/bin/env python3
"""e13b_pilot.py — E1.3b's infrastructure pilot, on the frozen plan and nothing else.

This is not a measurement of the experiment. It runs the promoted protocol's own arms into scratch
namespaces and checks the four properties the freeze said the instrument has to have before a variant
number is worth reading:

1. **Determinism.** The same arm, twice, from the same binary, writes the same rows. `wall_ms` and the
   environment block are excluded because the protocol declares them non-identity; everything else,
   including the identity and therefore the filename, has to match.
2. **Separate namespaces, no clobbering.** A variant arm shares the entry, the seed, the budget and the
   strategy with the reference arm, so the rule is the only thing that can tell their bytes apart. Its
   results document has to carry a different identity, its archives have to land in directories named
   for its rule, and writing them has to leave the reference arm's archives byte-identical.
3. **Equal charged cost.** The comparison is only fair if both arms ran the same points at the same
   price, so every (entry, seed, budget) pair has to carry the same `evaluations` and the same
   `instruction_steps` in both arms.
4. **Replay.** The sweeps report their own archive replay; a replay that does not match every record it
   read is a run that cannot be opened later.

Nothing here reads a deciding quantity: over-claim, UNKNOWN volume and localisation are E1.3b's primary
metric and they are read from the committed run, not from a pilot. The one column this script does look
at is `non_finite_readings`, because the freeze asked for the non-finite variant's effect to be
attributable, and a pilot that never showed the column would leave that question blind either way.
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
PLAN = os.path.join(ROOT, "benchmarks", "protocols", "e1-3b-trust-resolution.json")
SCRATCH = os.path.join(ROOT, ".scratch")
REFERENCE = "full-current-trust"
VARIANT = "full-min-channels-2"
VARIANT_RULE = "min-channels-2"
# Declared non-identity by the frozen protocol: properties of the machine that ran, not of the run.
NON_IDENTITY_ROW = {"wall_ms"}


def canonical(value):
    return json.dumps(value, sort_keys=True)


def sha256_of(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def run_arm(arm, out_dir, archive_dir):
    """One frozen arm, by the protocol's own definition, into the namespaces the pilot owns."""
    for directory in (out_dir, archive_dir):
        os.makedirs(directory, exist_ok=True)
    result = subprocess.run(
        [BIN, "run", "--plan", PLAN, "--arm", arm, "--out", out_dir, "--archive", archive_dir],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SystemExit(f"{arm}: the runner exited {result.returncode}\n{result.stdout}\n{result.stderr}")
    written = [
        os.path.join(out_dir, name)
        for name in sorted(os.listdir(out_dir))
        if name.startswith("results-") and name.endswith(".json")
    ]
    if len(written) != 1:
        raise SystemExit(f"{arm}: expected one results document in {out_dir}, found {written}")
    with io.open(written[0], encoding="utf-8") as handle:
        return written[0], json.load(handle)


def rows(document):
    return [row for sweep in document["sweeps"] for row in sweep["outcomes"]]


def archive_digests(root):
    """Every byte under every archive directory, keyed by path, so a later run can be proven not to
    have touched a directory it did not own."""
    out = {}
    for directory, _, files in os.walk(root):
        for name in files:
            path = os.path.join(directory, name)
            with open(path, "rb") as handle:
                out[os.path.relpath(path, root)] = hashlib.sha256(handle.read()).hexdigest()
    return out


def compare_determinism(first, second, problems):
    """Two runs of one arm: same identity, and every declared field of every row equal."""
    if os.path.basename(first[0]) != os.path.basename(second[0]):
        problems.append(
            f"determinism: one arm wrote {os.path.basename(first[0])} and {os.path.basename(second[0])}"
        )
    for key in sorted(set(first[1]) | set(second[1])):
        if key in ("sweeps", "environment"):
            continue
        if canonical(first[1].get(key)) != canonical(second[1].get(key)):
            problems.append(f"determinism: the top-level field {key!r} moved between two identical runs")
    if len(rows(first[1])) != len(rows(second[1])):
        problems.append("determinism: the two runs wrote different numbers of rows")
        return 0
    fields = 0
    for a, b in zip(rows(first[1]), rows(second[1])):
        for key in sorted(set(a) | set(b)):
            if key in NON_IDENTITY_ROW:
                continue
            if canonical(a.get(key)) != canonical(b.get(key)):
                problems.append(
                    f"determinism: {a['entry']} seed {a['seed']} budget {a['budget']} field {key!r} is "
                    f"{a.get(key)!r} in one run and {b.get(key)!r} in the other"
                )
            fields += 1
    return fields


def compare_cost(reference, variant, problems):
    """Every paired row has to have run the same points at the same price."""
    left = {(r["entry"], r["seed"], r["budget"]): r for r in reference}
    right = {(r["entry"], r["seed"], r["budget"]): r for r in variant}
    if set(left) != set(right):
        problems.append("cost: the two arms measured a different set of (entry, seed, budget) rows")
    shared = 0
    for name in sorted(set(left) & set(right)):
        shared += 1
        for column in ("evaluations", "instruction_steps"):
            if left[name][column] != right[name][column]:
                problems.append(
                    f"cost: {name} charges {column} {left[name][column]} under the reference arm and "
                    f"{right[name][column]} under the variant"
                )
    return shared


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--commit",
        default="unknown",
        help="the commit the measured binary was built from, as `scripts/r.sh` prints it",
    )
    arguments = parser.parse_args(argv)
    commit = arguments.commit
    binary_digest = sha256_of(BIN)
    problems = []
    a_out = os.path.join(SCRATCH, "e13b-pilot-a")
    b_out = os.path.join(SCRATCH, "e13b-pilot-b")
    v_out = os.path.join(SCRATCH, "e13b-pilot-variant")
    archives = os.path.join(SCRATCH, "e13b-pilot-archives")
    # A pilot reads best from an empty namespace: archives left from a previous run would be counted as
    # if this run had written them, and a byte that was already there cannot be proven untouched.
    for directory in (a_out, b_out, v_out, archives):
        if os.path.isdir(directory):
            shutil.rmtree(directory)
        os.makedirs(directory)
    print(f"binary   {BIN}")
    print(f"         sha256 {binary_digest}  built from commit {commit}")
    print(f"protocol {PLAN}")
    print(f"arms     {REFERENCE} twice, then {VARIANT} once\n")

    print(f"running {REFERENCE} ...")
    first = run_arm(REFERENCE, a_out, archives)
    before = archive_digests(archives)
    print(f"running {REFERENCE} again ...")
    second = run_arm(REFERENCE, b_out, archives)
    fields = compare_determinism(first, second, problems)
    after_reference = archive_digests(archives)
    if after_reference != before:
        touched = sorted(set(after_reference) ^ set(before)) or [
            key for key in before if after_reference.get(key) != before[key]
        ]
        problems.append(f"no-clobber: re-running the same arm changed {len(touched)} archive files, first {touched[:3]}")
    print(
        f"  identity {first[1]['identity']}  rows {len(rows(first[1]))}  "
        f"{fields} row fields compared across the two runs"
    )
    print(f"  the reference arm's {len(before)} archive files are byte-identical after the replay")

    print(f"running {VARIANT} ...")
    variant = run_arm(VARIANT, v_out, archives)
    print(f"  identity {variant[1]['identity']}  rows {len(rows(variant[1]))}")
    if variant[1]["identity"] == first[1]["identity"]:
        problems.append(
            f"namespace: {VARIANT} and {REFERENCE} share the identity {variant[1]['identity']}, so the "
            "rule reached nothing that tells their measurements apart"
        )
    if variant[1]["trust"] != VARIANT_RULE:
        problems.append(f"namespace: the variant document mirrors {variant[1]['trust']!r}")
    if first[1]["trust"] != "current":
        problems.append(f"namespace: the reference document mirrors {first[1]['trust']!r}")
    suffix = f"-trust-{VARIANT_RULE}"
    identities_differ = variant[1]["identity"] != first[1]["identity"]
    after_variant = archive_digests(archives)
    written_by_variant = {
        key for key in after_variant if key.split(os.sep)[0].endswith(suffix)
    }
    reference_keys = {key for key in before if key.split(os.sep)[0] not in written_by_variant}
    changed = [key for key in reference_keys if after_variant.get(key) != before[key]]
    missing = [key for key in reference_keys if key not in after_variant]
    print(
        f"  archives now {len(after_variant)} files: {len(reference_keys)} the reference arm wrote, "
        f"{len(written_by_variant)} carrying the suffix {suffix}"
    )
    if not written_by_variant:
        problems.append("namespace: the variant wrote no archive directory named for its rule")
    if changed or missing:
        problems.append(
            f"no-clobber: the variant changed {len(changed)} and removed {len(missing)} of the "
            f"reference arm's archive files, first {changed[:2] + missing[:2]}"
        )

    paired = compare_cost(rows(first[1]), rows(variant[1]), problems)
    print(f"  cost     {paired} (entry, seed, budget) pairs, same evaluations and instruction steps")

    replays = []
    for sweep in first[1]["sweeps"]:
        if sweep.get("replay"):
            record = sweep["replay"]
            replays.append((sweep["entry"], sweep["seed"], record))
            if not (record["reproduced"] and record["matched"] == record["total"] > 0):
                problems.append(f"replay: {sweep['entry']} seed {sweep['seed']} replayed {record}")
    print(f"  replay   {len(replays)} sweeps replayed their archives, all of them matching every record"
          if len(replays) else "  replay   no sweep reported a replay")

    nonzero = [
        (row["entry"], row["seed"], row["budget"], row["non_finite_readings"])
        for row in rows(first[1])
        if row["non_finite_readings"]
    ]
    print(
        f"  attribution  non_finite_readings is nonzero on {len(nonzero)} of {len(rows(first[1]))} rows"
        + (f", first {nonzero[:3]}" if nonzero else "")
    )

    checks = {
        "determinism": {
            "identity": first[1]["identity"],
            "identity_repeated": second[1]["identity"],
            "rows": len(rows(first[1])),
            "row_fields_compared": fields,
            "excluded": sorted(NON_IDENTITY_ROW) + ["environment"],
            "passed": first[1]["identity"] == second[1]["identity"] and fields > 0,
        },
        "namespaces": {
            "reference_arm": REFERENCE,
            "reference_identity": first[1]["identity"],
            "variant_arm": VARIANT,
            "variant_identity": variant[1]["identity"],
            "variant_trust_mirror": variant[1]["trust"],
            "reference_trust_mirror": first[1]["trust"],
            "archive_suffix": suffix,
            "reference_archive_files": len(reference_keys),
            "variant_archive_files": len(written_by_variant),
            "reference_files_touched_by_the_variant": len(changed) + len(missing),
            "passed": bool(identities_differ and written_by_variant and not changed and not missing),
        },
        "cost": {
            "pairs": paired,
            "columns": ["evaluations", "instruction_steps"],
            "passed": paired == len(rows(first[1])) and paired > 0,
        },
        "replay": {
            "sweeps_replayed": len(replays),
            "records_matched": sum(record["matched"] for _, _, record in replays),
            "records_total": sum(record["total"] for _, _, record in replays),
            "passed": len(replays) > 0,
        },
        "attribution": {
            "column": "non_finite_readings",
            "rows": len(rows(first[1])),
            "rows_nonzero": len(nonzero),
            "note": "a count of zero is the honest reading of a corpus whose risks are fused into "
            "[0, 1]; the variant arm of the run will say what it changed with nothing to act on",
        },
    }
    artifact = {
        "schema": "aporia.pilot/1",
        "generated_by": "software/scripts/e13b_pilot.py",
        "purpose": "E1.3b's infrastructure pilot: the properties the freeze required of the instrument "
        "before a variant number is worth reading. No deciding quantity is measured here.",
        "binary": {
            "path": "software/.scratch/e1x-bin/aporia-bench.exe",
            "sha256": binary_digest,
            "built_from_commit": commit,
        },
        "protocol": "benchmarks/protocols/e1-3b-trust-resolution.json",
        "protocol_digest": first[1]["protocol"]["digest"],
        "namespaces": {
            "results": [
                "software/.scratch/e13b-pilot-a",
                "software/.scratch/e13b-pilot-b",
                "software/.scratch/e13b-pilot-variant",
            ],
            "archives": "software/.scratch/e13b-pilot-archives",
            "why": "a pilot is not the experiment, and its bytes must not land in the namespace the "
            "reported numbers are read from",
        },
        "checks": checks,
        "verdict": "PASSED" if not problems else "FAILED",
        "problems": problems,
    }
    target = os.path.join(ROOT, "benchmarks", "analysis", "e1-3b-pilot.json")
    with io.open(target, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(artifact, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(f"wrote {os.path.relpath(target, ROOT)}")

    if problems:
        print(f"\n{len(problems)} problem(s):")
        for line in problems:
            print("  PROBLEM", line)
        print("\nTHE PILOT FAILED: fix the instrument before measuring the experiment.")
        return 1
    print("\nTHE PILOT PASSED: deterministic, namespaced, equally charged, and replayable.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
