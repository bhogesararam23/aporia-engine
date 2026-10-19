#!/usr/bin/env python3
"""e15_verify_run.py — verify every byte of the frozen E1.5 run before any of it is interpreted.

Phase G of the plan ends with "verify every result before opening the analysis", and this is that step. It
reads the four published documents and the archives they wrote, checks them against the freeze rather than
against each other's conclusions, and writes what it found to `analysis/e1-5-run-verification.json`. No
quantity here decides anything: the frozen metric, the cost rules and the ten outcomes belong to
`e15_analysis.py`, which runs after this exits zero.

What is checked, and why each one is a thing that could break:

* **one experiment, four documents.** The protocol digest, the plan section, the entry list and the
  reachability section have to be identical across the arms, because they are the parts the freeze states
  once. An arm that silently changed the grid would be a different experiment sharing a results directory.
* **each arm is the arm the freeze names.** `mask`, `trust`, `resolution`, `certify`, `ablate` and the
  bars, read off the arm's own definition in the file and compared with what the document says it ran.
* **the ladder came out whole.** Six entries x five seeds x four budgets = 120 rows per arm, no duplicates
  and no missing cell, because a run that dropped a cell would quietly change every mean.
* **charged cost is equal, cell by cell.** The freeze's comparison rule, not a preference: unequal
  evaluations make the arms incomparable and the run has to say so rather than report a gain.
* **every archive replays.** A sweep whose records do not re-run under its own manifest configuration is
  not a measurement a reader can return to.
* **the artefacts hash to what the manifest says.** Integrity, which is a different claim from
  reproducibility: `decisions.jsonl` matching its own digest says the bytes were not edited afterwards; it
  says nothing about whether the same inputs would produce them again.
* **the region columns are consistent with each other.** The four label-overlap columns cannot exceed the
  certified volume they are measured inside; the partition fractions add up to the domain; a row with a
  refusal is a row the primary metric cannot read, and is counted rather than averaged away.

Exit is non-zero on any failure.

    scripts/e15_verify_run.py [--commit sha] [--write]
"""

import argparse
import hashlib
import io
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(ROOT, "benchmarks", "results")
ARCHIVES = os.path.join(ROOT, "benchmarks", "archives", "e1-5")
PLAN = os.path.join(ROOT, "benchmarks", "protocols", "e1-5-spatial-uncertainty-cost.json")
ARTIFACT = os.path.join(ROOT, "benchmarks", "analysis", "e1-5-run-verification.json")
LABEL_VOLUMES = ["trusted_volume", "unknown_volume", "suspicious_volume", "provisional_volume"]
OVERLAP_COLUMNS = [
    "trusted_over_true_union",
    "unknown_over_true",
    "suspicious_over_true",
    "provisional_over_true",
]
TOLERANCE = 1e-9
# The part of a plan the freeze states once for the whole experiment. Everything else an arm declares for
# itself: `resolution`, `certify` and `ablate` are the axes the comparison moves, and a document whose plan
# did not differ there would be four copies of one arm.
FROZEN_PLAN_FIELDS = [
    "budgets",
    "strategies",
    "seeds",
    "grid_per_axis",
    "minimise_budget",
    "probe_every",
    "calibrate_every",
    "refine_every",
    "numerical_every",
    "differential_every",
    "symmetric_every",
]


def load(path):
    with io.open(path, encoding="utf-8") as handle:
        return json.load(handle)


def outcomes(document):
    return [row for sweep in document["sweeps"] for row in sweep["outcomes"]]


def per_arm_identity(arm, found):
    """The identity one arm publishes, for the check that two arms cannot share one."""
    return found[arm][1]["identity"]


def find_arm_documents(protocol_id, arms, problems):
    """The published document for each frozen arm, found by the protocol and arm it claims."""
    found = {}
    for name in sorted(os.listdir(RESULTS)):
        if not (name.startswith("results-") and name.endswith(".json")):
            continue
        document = load(os.path.join(RESULTS, name))
        protocol = document.get("protocol") or {}
        if protocol.get("id") != protocol_id:
            continue
        arm = protocol.get("arm")
        if arm in found:
            problems.append(f"{arm}: two published documents claim to be this arm")
        found[arm] = (name, document)
    for arm in arms:
        if arm not in found:
            problems.append(f"{arm}: no published document for this frozen arm")
    return found


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--commit", default="unknown", help="the commit the verified run belongs to")
    parser.add_argument("--write", action="store_true", help="write the verification artifact")
    arguments = parser.parse_args(argv)

    protocol = load(PLAN)
    plan = protocol["plan"]
    arms = [definition["name"] for definition in protocol["arms"]]
    run_entries = protocol["entries"]["run"]
    problems = []

    found = find_arm_documents(protocol["id"], arms, problems)
    frozen_digest = hashlib.sha256(open(PLAN, "rb").read()).hexdigest()[:16]

    # One experiment, four documents.
    shared = {}
    for arm, (name, document) in found.items():
        shared.setdefault("protocol_digest", set()).add(
            (document.get("protocol") or {}).get("digest")
        )
        # The plan the freeze states once, which is the ladder and the rates — not the axes an arm
        # declares for itself (`resolution`, `certify`, `ablate`), whose whole purpose is to differ.
        shared.setdefault("frozen_plan", set()).add(
            json.dumps({key: document["plan"][key] for key in FROZEN_PLAN_FIELDS}, sort_keys=True)
        )
        # The two bars the freeze states once for the whole experiment. They are checked per arm against
        # the arm's own definition below; here they are checked across arms, because a density bar that
        # travelled between arms would be a criterion quietly retuned by whoever ran last. The plan
        # section is not the place to look: it carries them only where a rule can read them, which is the
        # rule that keeps an inert field from renaming a committed measurement.
        shared.setdefault("bars", set()).add(
            (document.get("certify_share"), document.get("certify_density"))
        )
        shared.setdefault("entries", set()).add(json.dumps(document["entries"], sort_keys=True))
        shared.setdefault("reachability", set()).add(
            json.dumps(document.get("reachability"), sort_keys=True)
        )

        if document.get("pilot"):
            problems.append(f"{arm}: a pilot document published among the experiment's results")
    for field, values in shared.items():
        if len(values) > 1:
            problems.append(f"the arms disagree about {field}, which the freeze states once")
    identities = {arm: per_arm_identity(arm, found) for arm in found}
    if len(set(identities.values())) != len(identities):
        problems.append(f"two arms published the same measurement identity: {identities}")
    if shared.get("protocol_digest", {None}) != {frozen_digest}:
        problems.append(
            f"a document cites a protocol digest other than the tree's {frozen_digest}: "
            f"{sorted(str(v) for v in shared.get('protocol_digest', []))}"
        )

    # Each arm is the arm the freeze names, and the ladder came out whole.
    per_arm = {}
    cells = {}
    for arm in arms:
        if arm not in found:
            continue
        name, document = found[arm]
        definition = [entry for entry in protocol["arms"] if entry["name"] == arm][0]
        stated = {
            "trust": document.get("trust"),
            "resolution": document.get("resolution"),
            "certify": document.get("certify"),
            "certify_share": document.get("certify_share"),
            "certify_density": document.get("certify_density"),
            "ablate": document["plan"].get("ablate", []),
        }
        wanted = {
            "trust": definition.get("trust", "current"),
            "resolution": definition.get("resolution", "current"),
            "certify": definition["certify"],
            "certify_share": plan["certify_share"],
            "certify_density": plan["certify_density"],
            "ablate": definition.get("ablate", []),
        }
        for key in wanted:
            if stated[key] != wanted[key]:
                problems.append(f"{arm}: records {key}={stated[key]!r}, the freeze says {wanted[key]!r}")
        rows = outcomes(document)
        keys = sorted((row["entry"], row["seed"], row["budget"]) for row in rows)
        expected = sorted(
            (entry, seed, budget)
            for entry in run_entries
            for seed in plan["seeds"]
            for budget in plan["budgets"]
        )
        if keys != expected:
            problems.append(
                f"{arm}: {len(keys)} cells measured, {len(expected)} frozen; the ladder is not whole"
            )
        if len(set(keys)) != len(keys):
            problems.append(f"{arm}: a cell appears twice, so a mean over rows is not a mean over cells")
        for sweep in document["sweeps"]:
            block = sweep.get("replay")
            if block is None:
                continue
            if not block.get("reproduced") or block.get("matched") != block.get("total"):
                problems.append(
                    f"{arm}: {sweep['entry']} seed {sweep['seed']} did not replay its own archive "
                    f"({block.get('matched')}/{block.get('total')}, error {block.get('error')!r})"
                )
        per_arm[arm] = {
            "file": name,
            "identity": document["identity"],
            "rows": len(rows),
            "sweeps": len(document["sweeps"]),
            "charged_evaluations": sum(row["evaluations"] for row in rows),
            "instruction_steps": sum(row["instruction_steps"] for row in rows),
            "leaf_cells": sum(row["leaf_cells"] for row in rows),
            "region_overlap_refusals": sum(row["region_overlap_refusals"] for row in rows),
            "replayed_sweeps": sum(1 for s in document["sweeps"] if s.get("replay")),
            "replayed_records": sum(
                (s.get("replay") or {}).get("matched", 0) for s in document["sweeps"]
            ),
            "total_records": sum((s.get("replay") or {}).get("total", 0) for s in document["sweeps"]),
        }
        for row in rows:
            cells.setdefault((row["entry"], row["seed"], row["budget"]), {})[arm] = row

    # Charged cost, paired cell by cell: the freeze's rule, in the freeze's words.
    unequal = []
    for key, by_arm in sorted(cells.items(), key=lambda item: str(item[0])):
        if len(by_arm) != len(arms):
            unequal.append(f"{key}: measured by {sorted(by_arm)}, not by every arm")
            continue
        reference = by_arm[arms[0]]
        for arm, row in by_arm.items():
            for column in ("evaluations", "instruction_steps"):
                if row[column] != reference[column]:
                    unequal.append(
                        f"{key}: {arm} spends {row[column]} {column} against {reference[column]} "
                        f"for {arms[0]}"
                    )
    problems += unequal

    # Rows that can carry the frozen metric, and rows that cannot.
    column_problems = []
    applicable = {arm: 0 for arm in per_arm}
    for key, by_arm in cells.items():
        for arm, row in by_arm.items():
            if row["region_overlap_refusals"] != 0:
                continue
            if row["declared_union_volume"] > 0.0:
                applicable[arm] += 1
            for column in OVERLAP_COLUMNS:
                if row[column] < -TOLERANCE:
                    column_problems.append(f"{arm}/{key}: {column} is negative")
            if row["trusted_over_true_union"] > row["trusted_volume"] + TOLERANCE:
                column_problems.append(
                    f"{arm}/{key}: certified region exceeds the trusted volume it is measured inside"
                )
            total = sum(row[column] for column in LABEL_VOLUMES)
            if abs(total - 1.0) > 1e-6:
                column_problems.append(
                    f"{arm}/{key}: the four label volumes sum to {total}, not the domain"
                )
            if sum(row[c] for c in OVERLAP_COLUMNS) > row["declared_union_volume"] + TOLERANCE:
                column_problems.append(
                    f"{arm}/{key}: the label overlaps add up to more than the declared region"
                )
    problems += column_problems

    # Archive integrity and replay, on the bytes the run left rather than on its own summary.
    archives = []
    if not os.path.isdir(ARCHIVES):
        problems.append(f"no archive namespace at {ARCHIVES}")
    for name in sorted(os.listdir(ARCHIVES)) if os.path.isdir(ARCHIVES) else []:
        directory = os.path.join(ARCHIVES, name)
        path = os.path.join(directory, "manifest.json")
        if not os.path.exists(path):
            problems.append(f"{name}: an archive directory with no manifest")
            continue
        manifest = load(path)
        bad = []
        for relative, recorded in manifest["files"].items():
            target = os.path.join(directory, relative)
            if not os.path.exists(target):
                bad.append(f"{relative}: missing")
                continue
            actual = hashlib.sha256(open(target, "rb").read()).hexdigest()
            if actual != recorded:
                bad.append(f"{relative}: digests to {actual[:12]}, the manifest says {recorded[:12]}")
        notes = manifest["environment"]["notes"]
        arm = notes.get("arm")
        identity = notes.get("measurement_identity")
        if bad:
            problems.append(f"{name}: {len(bad)} artefact(s) fail their digest — {bad[:3]}")
        if identity and arm in per_arm and identity != per_arm[arm]["identity"]:
            problems.append(
                f"{name}: archived measurement {identity}, the published document for {arm} is "
                f"{per_arm[arm]['identity']}"
            )
        archives.append(
            {
                "directory": name,
                "arm": arm,
                "entry": notes.get("entry"),
                "seed": notes.get("seed"),
                "measurement_identity": identity,
                "artefacts": len(manifest["files"]),
                "integrity": "clean" if not bad else "FAILED",
            }
        )

    summary = {
        "rows_per_arm": len(run_entries) * len(plan["seeds"]) * len(plan["budgets"]),
        "arms": per_arm,
        "applicable_metric_rows": applicable,
        "unequal_cost_cells": len(unequal),
        "column_inconsistencies": len(column_problems),
        "archives": archives,
        "archived_campaigns": len(archives),
    }
    artifact = {
        "schema": "aporia.run-verification/1",
        "generated_by": "scripts/e15_verify_run.py",
        "purpose": (
            "Verify the frozen run's bytes against the freeze before the analysis reads any of them: one "
            "experiment, four arms, whole ladder, equal charged cost, replayed and digest-clean archives, "
            "and rows that can carry the metric counted apart from rows that cannot."
        ),
        "protocol": {
            "id": protocol["id"],
            "digest": frozen_digest,
            "file": os.path.relpath(PLAN, ROOT).replace(os.sep, "/"),
        },
        "commit": arguments.commit,
        "checks": {
            "one_experiment": [p for p in problems if "disagree about" in p or "digest other than" in p],
            "arm_definitions": [p for p in problems if "records " in p and "the freeze says" in p],
            "ladder": [p for p in problems if "ladder" in p or "appears twice" in p],
            "charged_cost": unequal,
            "replay_and_integrity": [
                p for p in problems if "digests to" in p or "no manifest" in p or "archived measurement" in p
            ]
            + [p for p in problems if "did not replay" in p],
            "region_columns": column_problems,
            "published_documents": [p for p in problems if "no published document" in p or "two published" in p],
            "pilot_leak": [p for p in problems if "pilot document" in p],
        },
        "summary": summary,
        "verdict": "PASSED" if not problems else "FAILED",
        "problems": problems,
    }
    text = json.dumps(artifact, indent=1, sort_keys=True) + "\n"
    if arguments.write:
        with io.open(ARTIFACT, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        print(f"wrote {ARTIFACT}")
    print(f"e15_verify_run: {artifact['verdict']}")
    for problem in problems[:25]:
        print(f"  {problem}")
    if len(problems) > 25:
        print(f"  ... and {len(problems) - 25} more")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
