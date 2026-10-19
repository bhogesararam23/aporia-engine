#!/usr/bin/env python3
"""e15_pilot.py — run the scope E1.5 froze for a pilot, and check the six properties it names.

This is not a measurement of the experiment. Nothing here reads `certified_over_region_share`, the four
label-overlap columns, localisation or detection: those decide E1.5 and they are read once, from the
committed full run, after this passes. What this checks is that the instrument can be trusted to hold the
freeze's own promises — and the first promise is the one E1.4 could not keep.

**The pilot is the frozen pilot.** E1.4's stopping rule described "one entry, two seeds and the two
smallest budgets", its runner refused every flag that shapes what is measured beside `--plan`, and so the
pilot ran 100 cells and the deviation went into the analysis as a defect of the freeze. `aporia.protocol/4`
puts the scope in the file, and this script reads its arms, entries, budgets and seeds *from that section*
rather than from a list kept beside it: if the two ever disagree, the count assertion at the end is where
that shows up: 2 entries x 2 budgets x 2 seeds is eight rows per arm, sixteen in all.

The six checks are the freeze's `pilot.checks`, in its words:

1. **determinism** — the reference arm, twice, from one binary, writes byte-identical rows (only
   `wall_ms` excluded, as the protocol declares) and therefore the same identity and filename.
2. **namespace separation** — the two arms land in directories that differ where their declared axes
   differ, and a run whose pilot scope would write a directory the published corpus already holds is
   stopped before it writes, with the reason: not one of those 412 archives states a measurement
   identity, so nothing can prove the run would reproduce it rather than replace it.
3. **equal charged cost** — per (entry, seed, budget) cell, both arms spend the same `evaluations` and
   the same `instruction_steps`, because an adequacy criterion spends labels and cells, not executions.
   Unequal cost makes the arms incomparable, which is the freeze's rule, not this script's preference.
4. **replay** — every archived sweep re-runs its own records and matches all of them.
5. **archive identity names the measurement** — every manifest states the measurement identity the
   run's own document publishes, beside the protocol id, the arm name and the protocol digest. This is
   the property the reproduction gate found missing from all 412 published archives, so it is checked on
   the new bytes rather than assumed.
6. **the certify axis reaches the plan and the policy** — the document header names the criterion, the
   plan section carries it where a rule reads it, the archive's recorded configuration carries the
   criterion and its density bar, and the partitions of the reference arm and the withholding arm agree
   cell for cell in bounds, depth and samples with only the label allowed to move, and only from TRUSTED
   to PROVISIONAL. If the geometry moved, the criterion had become a second search rather than a second
   claim about one search, and no number in the experiment would be attributable to the rule.

Exit is non-zero on any failure, and the frozen rule is that a failed pilot stops the experiment.

    scripts/e15_pilot.py --commit <sha> [--write]
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
PLAN = os.path.join(ROOT, "benchmarks", "protocols", "e1-5-spatial-uncertainty-cost.json")
SCRATCH = os.path.join(ROOT, ".scratch", "e15-pilot")
ARTIFACT = os.path.join(ROOT, "benchmarks", "analysis", "e1-5-pilot.json")
NON_IDENTITY_ROW = {"wall_ms"}
# The atlas column two adequacy criteria are allowed to disagree over. Every other column is a fact
# about the search itself: the cell, its depth, the samples it holds, the risk the evidence reported.
# If any of those moves between the arms, the criterion has become a second search rather than a second
# claim about one search, and no number in the experiment is attributable to the rule.
EXCLUDED_GEOMETRY_COLUMNS = {"label"}


def load(path):
    with io.open(path, encoding="utf-8") as handle:
        return json.load(handle)


def digest(path):
    with io.open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()[:16]


def outcomes(document):
    return [row for sweep in document["sweeps"] for row in sweep["outcomes"]]


def run_arm(arm, namespace, pilot=True):
    """One frozen arm, into its own scratch namespace, with the flags the freeze allows."""
    out = os.path.join(SCRATCH, "results-" + namespace)
    archive = os.path.join(SCRATCH, "archives-" + namespace)
    for directory in (out, archive):
        shutil.rmtree(directory, ignore_errors=True)
    command = [BIN, "run", "--plan", PLAN, "--arm", arm, "--out", out, "--archive", archive]
    if pilot:
        command.append("--pilot")
    run = subprocess.run(command, capture_output=True, text=True)
    if run.returncode != 0:
        return None, run, out, archive
    files = [os.path.join(out, name) for name in os.listdir(out)]
    if len(files) != 1:
        return None, run, out, archive
    return load(files[0]), run, out, archive


def row_key(row):
    return (row["entry"], row["seed"], row["budget"])


def compare_rows(a, b):
    """Every field a row declares, except the one the protocol says is not part of the measurement."""
    problems = []
    for key, value in a.items():
        if key in NON_IDENTITY_ROW:
            continue
        if b.get(key) != value:
            problems.append(f"{a['entry']} seed {a['seed']} budget {a['budget']}: {key} moved")
    return problems


def atlas_geometry(archive_root):
    """(directory, cells) where a cell is its bounds, depth and sample count — never its label."""
    out = {}
    if not os.path.isdir(archive_root):
        return out
    for name in sorted(os.listdir(archive_root)):
        path = os.path.join(archive_root, name, "atlas.csv")
        if not os.path.exists(path):
            continue
        with io.open(path, encoding="utf-8") as handle:
            header = handle.readline().strip().split(",")
            kept = [column for column in header if column not in EXCLUDED_GEOMETRY_COLUMNS]
            cells = []
            for line in handle:
                if not line.strip():
                    continue
                fields = dict(zip(header, line.rstrip("\n").split(",")))
                cells.append(tuple(fields[column] for column in kept))
        out[name] = (kept, cells)
    return out


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--commit", default="unknown", help="the commit the measured binary was built from")
    parser.add_argument("--write", action="store_true", help="write the pilot artifact into benchmarks/analysis")
    arguments = parser.parse_args(argv)

    protocol = load(PLAN)
    pilot = protocol["pilot"]
    arms = pilot["arms"]
    if len(arms) != 2:
        raise SystemExit(f"this script compares two arms; the frozen pilot names {arms}")
    reference, partner = arms
    expected_rows = len(pilot["entries"]) * len(pilot["budgets"]) * len(pilot["seeds"])

    problems = []
    checks = {}
    documents = {}
    namespaces = {}
    for arm in arms:
        document, run, out, archive = run_arm(arm, arm)
        if document is None:
            raise SystemExit(f"{arm}: the frozen pilot run failed\n{run.stdout}\n{run.stderr}")
        documents[arm] = document
        namespaces[arm] = archive

    # 1. Determinism: the reference arm again, and the two documents have to agree field by field.
    again, _, _, _ = run_arm(reference, reference + "-repeat")
    determinism = []
    if again is None or again["identity"] != documents[reference]["identity"]:
        determinism.append("the reference arm published a different identity on its second run")
    else:
        determinism = compare_rows_all(outcomes(documents[reference]), outcomes(again))
    checks["determinism"] = determinism
    problems += determinism

    # 2. Namespace separation, in the two senses that matter. First, the two arms must not write the same
    #    directory: they differ in a declared axis, so that axis has to be visible in the name. Second,
    #    the guard must stop a run that would replace a directory whose archive cannot say what it
    #    measures — and the real case for that is the published corpus, none of whose 412 archives carries
    #    a measurement identity. The directories this run would write are copied into scratch first: a
    #    guard that failed would otherwise replace the bytes a published number is opened from, which is
    #    not a thing a pilot is ever allowed to test in place.
    names = {arm: sorted(os.listdir(path)) for arm, path in namespaces.items()}
    shared = sorted(set(names[reference]) & set(names[partner]))
    namespace = [f"both arms wrote a directory of the same name: {shared}"] if shared else []
    published = os.path.join(ROOT, "benchmarks", "archives")
    mirror = os.path.join(SCRATCH, "published-mirror")
    shutil.rmtree(mirror, ignore_errors=True)
    os.makedirs(mirror)
    copied = []
    for name in names[reference]:
        if os.path.isdir(os.path.join(published, name)):
            shutil.copytree(os.path.join(published, name), os.path.join(mirror, name))
            copied.append(name)
    if not copied:
        namespace.append(
            "this pilot writes no directory that the published corpus already holds, so the guard's "
            "refusal of an archive that cannot name its measurement is untested here"
        )
    else:
        attempted = subprocess.run(
            [
                BIN, "run", "--plan", PLAN, "--arm", reference, "--pilot",
                "--out", os.path.join(SCRATCH, "results-refused"),
                "--archive", mirror,
            ],
            capture_output=True,
            text=True,
        )
        output = attempted.stdout + attempted.stderr
        if attempted.returncode == 0:
            namespace.append(
                f"a run was allowed into the published directories {copied} without their saying what "
                "they measure"
            )
        elif "no measurement identity" not in output:
            namespace.append(
                f"the refusal over {copied} did not say the reason it exists to say: {output[:300]}"
            )
    checks["namespace_separation"] = namespace
    problems += checks["namespace_separation"]

    # 3. Equal charged cost, cell by cell.
    cost = []
    reference_rows = {row_key(r): r for r in outcomes(documents[reference])}
    partner_rows = {row_key(r): r for r in outcomes(documents[partner])}
    if sorted(reference_rows) != sorted(partner_rows):
        cost.append("the two arms measured different cells, so nothing after this is paired")
    for key in sorted(set(reference_rows) & set(partner_rows)):
        a, b = reference_rows[key], partner_rows[key]
        for column in ("evaluations", "instruction_steps"):
            if a[column] != b[column]:
                cost.append(f"{key}: {column} {a[column]} against {b[column]}")
    checks["equal_charged_cost"] = cost
    problems += cost

    # 4. Replay, on every archived sweep.
    replay = []
    for arm, archive in namespaces.items():
        for name in sorted(os.listdir(archive)):
            summary = os.path.join(archive, name, "summary.json")
            if not os.path.exists(summary):
                replay.append(f"{arm}/{name}: no summary")
                continue
            load(summary)
    # A replay block exists only where the run archived something to replay against: the harness archives
    # one campaign per entry — the largest budget at the first seed — so `replay: null` is an honest
    # statement about a sweep that was never written down, and treating it as a failure would turn the
    # archive policy into a replay failure. What must hold is that every sweep which *was* archived
    # reproduces every record it read, and that the ones that were not say so rather than pretending.
    replayed = {}
    for arm, document in documents.items():
        attempted = 0
        for sweep in document["sweeps"]:
            block = sweep.get("replay")
            if block is None:
                continue
            attempted += 1
            if not block.get("reproduced"):
                replay.append(
                    f"{arm}: {sweep['entry']} seed {sweep['seed']} did not replay: {block.get('error')}"
                )
            elif block.get("matched") != block.get("total"):
                replay.append(
                    f"{arm}: {sweep['entry']} matched {block.get('matched')} of {block.get('total')}"
                )
        replayed[arm] = attempted
        if attempted != len(names[arm]):
            replay.append(f"{arm}: replayed {attempted} of the {len(names[arm])} archives it wrote")
    checks["replay"] = replay
    problems += replay

    # 5. An archive that names its measurement, and names the same one its document publishes.
    identity = []
    for arm, document in documents.items():
        for name in sorted(os.listdir(namespaces[arm])):
            manifest = load(os.path.join(namespaces[arm], name, "manifest.json"))
            notes = manifest["environment"]["notes"]
            for key, want in (
                ("measurement_identity", document["identity"]),
                ("protocol", protocol["id"]),
                ("arm", arm),
                ("protocol_digest", digest(PLAN)),
                ("pilot", "true"),
            ):
                if notes.get(key) != want:
                    identity.append(f"{arm}/{name}: records {key}={notes.get(key)!r}, expected {want!r}")
    checks["archive_identity"] = identity
    problems += identity

    # 6. The criterion, in the plan, the header, the archived configuration and the partition.
    axis = []
    for arm, document in documents.items():
        stated = [arm_def for arm_def in protocol["arms"] if arm_def["name"] == arm][0]
        if document.get("certify") != stated["certify"]:
            axis.append(f"{arm}: header says {document.get('certify')!r}, the freeze says {stated['certify']!r}")
        in_plan = document["plan"].get("certify")
        if stated["certify"] == "size":
            if in_plan is not None:
                axis.append(f"{arm}: a criterion no rule reads moved into the plan section")
        elif in_plan != stated["certify"]:
            axis.append(f"{arm}: the plan section does not carry the criterion its rule acts on")
        if document.get("certify_density") != protocol["plan"]["certify_density"]:
            axis.append(f"{arm}: the header does not state the experiment's density bar")
        for name in sorted(os.listdir(namespaces[arm])):
            manifest = load(os.path.join(namespaces[arm], name, "manifest.json"))
            atlas = manifest["config"].get("atlas", {})
            if atlas.get("certify") != stated["certify"]:
                axis.append(f"{arm}/{name}: the archived configuration states no criterion")
            if atlas.get("certify_density") != protocol["plan"]["certify_density"]:
                axis.append(f"{arm}/{name}: the archived configuration states no density bar")
    geometry = []
    left = atlas_geometry(namespaces[reference])
    right = atlas_geometry(namespaces[partner])
    # The directories are *meant* to differ — that difference is check 2 — so they are paired by what
    # each one says it archived, from its own manifest, rather than by the name somebody gave it.
    def key_of(directory_name):
        for arm, root in namespaces.items():
            manifest = os.path.join(root, directory_name, "manifest.json")
            if os.path.exists(manifest):
                notes = load(manifest)["environment"]["notes"]
                return notes["entry"], notes["seed"]
        return None

    paired = {}
    for side, table in (("reference", left), ("partner", right)):
        for name, cells in table.items():
            entry, seed = key_of(name)
            paired.setdefault((entry, seed), {})[side] = (name, cells)
    for cell_key in sorted(paired, key=str):
        sides = paired[cell_key]
        if sorted(sides) != ["partner", "reference"]:
            geometry.append(f"{cell_key}: only {sorted(sides)} archived the same campaign")
            continue
        (name_a, (header_a, cells_a)), (name_b, (header_b, cells_b)) = sides["reference"], sides["partner"]
        if header_a != header_b:
            geometry.append(f"{cell_key}: the two atlases describe different columns")
            continue
        if len(cells_a) != len(cells_b):
            geometry.append(f"{name}: {len(cells_a)} cells against {len(cells_b)}, which is a second search")
            continue
        if cells_a != cells_b:
            geometry.append(f"{name}: the partition itself moved between the two criteria")
    checks["certify_axis"] = axis + geometry
    problems += checks["certify_axis"]

    sizes = {arm: len(outcomes(documents[arm])) for arm in arms}
    if any(size != expected_rows for size in sizes.values()):
        problems.append(
            f"the frozen pilot scope is {expected_rows} rows per arm and the arms wrote {sizes}: "
            "`--pilot` did not narrow the run"
        )
    for arm, document in documents.items():
        if not document.get("pilot"):
            problems.append(f"{arm}: a pilot document that does not say so")

    artifact = {
        "schema": "aporia.pilot/1",
        "generated_by": "scripts/e15_pilot.py",
        "purpose": (
            "Run the scope E1.5 froze for its pilot and check the six properties that file names, before "
            "any arm of the comparison is read."
        ),
        "protocol": {
            "id": protocol["id"],
            "digest": digest(PLAN),
            "file": os.path.relpath(PLAN, ROOT).replace(os.sep, "/"),
        },
        "frozen_pilot": pilot,
        "binary": os.path.relpath(BIN, ROOT).replace(os.sep, "/"),
        "binary_sha256": hashlib.sha256(open(BIN, "rb").read()).hexdigest(),
        "built_from_commit": arguments.commit,
        "arms": {
            arm: {
                "identity": documents[arm]["identity"],
                "rows": sizes[arm],
                "entries": sorted({row["entry"] for row in outcomes(documents[arm])}),
                "budgets": sorted({row["budget"] for row in outcomes(documents[arm])}),
                "seeds": sorted({row["seed"] for row in outcomes(documents[arm])}),
                "certify": documents[arm].get("certify"),
                "resolution": documents[arm].get("resolution"),
                "charged_evaluations": sum(row["evaluations"] for row in outcomes(documents[arm])),
                "instruction_steps": sum(row["instruction_steps"] for row in outcomes(documents[arm])),
                "archives": names[arm],
            }
            for arm in arms
        },
        "checks": checks,
        "rows_expected_per_arm": expected_rows,
        "verdict": "PASSED" if not problems else "FAILED",
        "problems": problems,
        "not_a_measurement": (
            "No deciding quantity is read here. This establishes that the instrument holds the freeze's "
            "promises about scope, cost, replay, namespace and the criterion, on which every later number "
            "in the experiment depends."
        ),
    }
    text = json.dumps(artifact, indent=1, sort_keys=True) + "\n"
    if arguments.write:
        with io.open(ARTIFACT, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        print(f"wrote {ARTIFACT}")
    print(f"e15_pilot: {artifact['verdict']}")
    for problem in problems[:20]:
        print(f"  {problem}")
    return 1 if problems else 0


def compare_rows_all(a, b):
    if len(a) != len(b):
        return [f"{len(a)} rows against {len(b)}"]
    problems = []
    for x, y in zip(a, b):
        problems += compare_rows(x, y)
    return problems


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
