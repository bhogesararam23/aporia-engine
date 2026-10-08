#!/usr/bin/env python3
"""e13_row_trace.py — reproduce measured rows and record the atlas cells that produced them.

A results row says what the instrument concluded; it does not say which cell carried the conclusion.
The cells live in the archive, and the harness archives only the largest budget of the first seed, so
the rows worth reading are re-run as single-row plans: `budgets=[B]`, `seeds=[S]`, one entry, every
other field copied byte-for-byte out of the frozen protocol. That is legitimate because the campaign for
one (entry, strategy, seed, budget) does not depend on the rest of the ladder — and it is *checked*
rather than assumed: each reproduced row is compared with the committed document on every measured field
and the whole census before its cells are recorded.

    cd software && python scripts/e13_row_trace.py e1-geometry

Needs an allowed binary (`scripts/r.sh` builds and keeps one, and prints its sha256). Writes
`benchmarks/analysis/<protocol>-row-trace.json`; the archives it writes go to `.scratch/` and are not
committed, because the artifact records the cells themselves.

Honesty note on the numbers recorded: the leaf's share of a region is not re-derived here. Where exactly
one trusted leaf overlaps a region, the instrument's own `trusted_over_true` divided by that leaf's share
of the domain *is* the overlap the instrument used, so the artifact reports that as `derived_overlap_of_
region_in_leaf`. "Samples inside the declared envelope" counts points inside the entry's stated box,
which for a region carrying a `where` is an upper bound on the carved set, not the set itself.
"""

import csv
import glob
import json
import os
import subprocess
import sys

PROTOCOL_DIR = "benchmarks/protocols"
MEASURED_FIELDS = [
    "evaluations",
    "instruction_steps",
    "declared_regions",
    "detected_regions",
    "localised_regions",
    "suspicious_volume",
    "trusted_volume",
    "unknown_volume",
    "findings",
    "unplaced",
    "trusted_over_true",
    "lattice_sensitive",
    "first_true_failure",
    "false_positive_fraction",
    "duplicate_discovery_rate",
]

# The rows worth reading, and why each was chosen.
ROWS = [
    {
        "purpose": "the full instrument's single failing row",
        "entry": "edge/overflow_tail",
        "arm": "full",
        "seed": 4,
        "budget": 1280,
    },
    {
        "purpose": "the passing sibling: same entry, arm and budget, one seed over",
        "entry": "edge/overflow_tail",
        "arm": "full",
        "seed": 1,
        "budget": 1280,
    },
    {
        "purpose": "the plainest failure: an arm whose channel computes nothing certifies the domain",
        "entry": "edge/overflow_tail",
        "arm": "only-behavioral",
        "seed": 4,
        "budget": 1280,
    },
    {
        "purpose": "a half-covered row: the pole sits exactly on a leaf boundary",
        "entry": "edge/pole_at_edge",
        "arm": "no-physical",
        "seed": 4,
        "budget": 1280,
    },
    {
        "purpose": "a curved region that every arm sees and none encloses",
        "entry": "geometry/narrow_oblique",
        "arm": "full",
        "seed": 1,
        "budget": 1280,
    },
    {
        "purpose": "the same region under the arm that reproduces the full instrument",
        "entry": "geometry/narrow_oblique",
        "arm": "only-physical",
        "seed": 1,
        "budget": 1280,
    },
    {
        "purpose": "a curved region that is enclosed, for contrast",
        "entry": "geometry/diagonal_sum",
        "arm": "full",
        "seed": 1,
        "budget": 320,
    },
]


def fail(message):
    sys.exit(f"e13_row_trace: {message}")


def model_axes(model_text):
    """The model's parameter names, in declaration order — the order a decision's `x` vector uses."""
    out = []
    for line in model_text.splitlines():
        parts = line.strip().split()
        if len(parts) >= 4 and parts[0] == "input" and parts[2] == "in":
            out.append(parts[1])
    return out


def envelope(entry_id):
    family, name = entry_id.split("/", 1)
    truth = json.load(open(f"benchmarks/{family}/{name}/truth.json", encoding="utf-8"))
    axes = model_axes(open(f"benchmarks/{family}/{name}/model.ap", encoding="utf-8").read())
    regions = []
    for r in truth.get("regions", []):
        box = []
        for axis in axes:
            bounds = r.get("axes", {}).get(axis)
            if bounds is None:
                return None
            box.append([float(bounds[0]), float(bounds[1])])
        regions.append(box)
    return {"axes": axes, "regions": regions}


def leaf_box(leaf):
    box = [leaf["bounds"]["axis0"]]
    if leaf["bounds"].get("axis1") is not None:
        box.append(leaf["bounds"]["axis1"])
    return box


def boxes_overlap(a, b):
    return all(x[0] < y[1] and x[1] > y[0] for x, y in zip(a, b))


def point_in_box(point, box):
    return all(lo <= v <= hi for v, (lo, hi) in zip(point, box))


def read_atlas(directory, entry_id):
    path = sorted(glob.glob(f"{directory}/{entry_id.replace('/', '-')}-*/atlas.csv"))
    if not path:
        fail(f"no atlas.csv under {directory}")
    rows = list(csv.DictReader(open(path[-1], encoding="utf-8")))
    return [
        {
            "cell": int(r["cell"]),
            "depth": int(r["depth"]),
            "label": r["label"],
            "samples": int(r["samples"]),
            "mean_risk": float(r["mean_risk"]),
            "max_risk": float(r["max_risk"]),
            "facts": int(r["facts"]),
            "measured": int(r["measured"]),
            "channels": int(r["channels"]),
            "share_of_domain": float(r["relative_size"]),
            "bounds": {
                "axis0": [float(r["lo0"]), float(r["hi0"])],
                "axis1": [float(r["lo1"]), float(r["hi1"])] if "lo1" in r else None,
            },
        }
        for r in rows
    ]


def read_samples(directory, entry_id):
    path = sorted(glob.glob(f"{directory}/{entry_id.replace('/', '-')}-*/decisions.jsonl"))
    if not path:
        return []
    out = []
    for line in open(path[-1], encoding="utf-8"):
        record = json.loads(line)
        out.append({"evaluation": record["evaluation"], "x": record["x"], "risk": record["risk"]})
    return out


def main(protocol_id, binary):
    if not os.path.exists(binary):
        fail(f"no binary at {binary}; run scripts/r.sh first")
    frozen = json.load(open(f"{PROTOCOL_DIR}/{protocol_id}.json", encoding="utf-8"))
    committed = {}
    for path in sorted(glob.glob("benchmarks/results/results-*.json")):
        doc = json.load(open(path, encoding="utf-8"))
        proto = doc.get("protocol") or {}
        if proto.get("id") == protocol_id:
            committed[proto["arm"]] = doc
    if not committed:
        fail(f"no committed document cites {protocol_id}")

    workspace = ".scratch/e13-row-trace"
    os.makedirs(workspace, exist_ok=True)
    artifact_rows = []
    for spec in ROWS:
        entry, arm, seed, budget = spec["entry"], spec["arm"], spec["seed"], spec["budget"]
        tag = f"{arm}-{entry.replace('/', '-')}-seed{seed}-budget{budget}"
        attempt = 1
        while os.path.exists(f"{workspace}/{tag}-out{attempt}"):
            attempt += 1
        out_dir, arch_dir = (f"{workspace}/{tag}-out{attempt}", f"{workspace}/{tag}-arch{attempt}")
        os.makedirs(out_dir, exist_ok=True)

        # The frozen plan, narrowed to one row. Everything that shapes a measurement is copied.
        derived = json.loads(json.dumps(frozen))
        derived["id"] = f"{protocol_id}-trace"
        derived["status"] = (
            "NOT A PRE-REGISTRATION AND NOT A RESULT. One (entry, arm, seed, budget) of "
            f"{protocol_id}, reproduced so its archive can be read; every other field of the plan is "
            "the frozen one."
        )
        derived["plan"] = dict(frozen["plan"], budgets=[budget], seeds=[seed])
        derived["entries"] = dict(frozen["entries"], run=[entry])
        plan_path = f"{workspace}/{tag}.json"
        with open(plan_path, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(derived, handle, indent=2, ensure_ascii=False)
            handle.write("\n")

        result = subprocess.run(
            [binary, "run", "--plan", plan_path, "--arm", arm, "--out", out_dir, "--archive", arch_dir],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            fail(f"{tag}: run refused (exit {result.returncode})\n{result.stdout[-600:]}")
        produced = [f for f in os.listdir(out_dir) if f.endswith(".json")]
        if len(produced) != 1:
            fail(f"{tag}: expected one results file, found {produced}")
        fresh = json.load(open(f"{out_dir}/{produced[0]}", encoding="utf-8"))
        got = {
            (o["entry"], o["seed"], o["budget"]): o
            for s in fresh["sweeps"]
            for o in s["outcomes"]
        }[(entry, seed, budget)]
        old = {
            (o["entry"], o["seed"], o["budget"]): o
            for s in committed[arm]["sweeps"]
            for o in s["outcomes"]
        }[(entry, seed, budget)]
        differences = [f for f in MEASURED_FIELDS if got.get(f) != old.get(f)]
        census_same = got["census"] == old["census"]
        if differences or not census_same:
            fail(
                f"{tag}: reproduced row differs from the committed row in {differences}"
                + ("" if census_same else " and the census")
            )

        leaves = read_atlas(arch_dir, entry)
        samples = read_samples(arch_dir, entry)
        declared = envelope(entry)
        if declared is None:
            fail(f"{entry} declares an axis the model does not have")
        regions = declared["regions"]
        # Every leaf whose box touches a declared envelope, and every sample that landed inside one.
        touching = [l for l in leaves if any(boxes_overlap(leaf_box(l), r) for r in regions)]
        inside = [s for s in samples if any(point_in_box(s["x"], r) for r in regions)]
        row = {
            "purpose": spec["purpose"],
            "arm": arm,
            "entry": entry,
            "seed": seed,
            "budget": budget,
            "reproduces_committed_row": True,
            "fields_compared": MEASURED_FIELDS + ["census"],
            "committed_trusted_over_true": old["trusted_over_true"],
            "committed_trusted_volume": old["trusted_volume"],
            "committed_unknown_volume": old["unknown_volume"],
            "committed_suspicious_volume": old["suspicious_volume"],
            "committed_detected_regions": old["detected_regions"],
            "committed_localised_regions": old["localised_regions"],
            "committed_findings": old["findings"],
            "committed_unplaced": old["unplaced"],
            "declared_axes": declared["axes"],
            "declared_envelopes": regions,
            "leaves_total": len(leaves),
            "deepest_leaf": max(l["depth"] for l in leaves),
            "finest_leaf_share_of_domain": min(l["share_of_domain"] for l in leaves),
            "leaves_touching_a_declared_envelope": len(touching),
            "leaf_touching_the_region": None,
            "samples_inside_declared_envelope": len(inside),
            "samples_total_recorded": len(samples),
            "widest_sample_inside_envelope": max((s["x"][0] for s in inside), default=None),
            "largest_sample_anywhere": max((s["x"][0] for s in samples), default=None),
            "trusted_leaves": sum(1 for l in leaves if l["label"] == "TRUSTED"),
            "unknown_leaves": sum(1 for l in leaves if l["label"] == "UNKNOWN"),
            "suspicious_leaves": sum(1 for l in leaves if l["label"] == "SUSPICIOUS"),
            "best_leaf_inside_fraction_of_region": None,
        }
        # The leaf whose trusted share carries the over-claim, when there is exactly one trusted leaf
        # overlapping the region: `trusted_over_true` / leaf share is then the instrument's own overlap.
        if old["trusted_over_true"] > 0.0:
            candidates = [l for l in touching if l["label"] == "TRUSTED"]
            if len(candidates) == 1:
                leaf = candidates[0]
                row["leaf_touching_the_region"] = leaf
                if leaf["share_of_domain"] > 0:
                    row["derived_overlap_of_region_in_leaf"] = (
                        old["trusted_over_true"] / leaf["share_of_domain"]
                    )
            elif len(candidates) > 1:
                row["trusted_leaves_touching_the_region"] = candidates
        # For an enclosed region: the leaf that decided it, from the suspicious side.
        if old["localised_regions"] > 0:
            best = max(
                (l for l in leaves if l["label"] == "SUSPICIOUS"),
                key=lambda l: l["share_of_domain"],
                default=None,
            )
            row["deciding_suspicious_leaf"] = best
        artifact_rows.append(row)
        print(
            f"{tag}: reproduced ({len(MEASURED_FIELDS)} fields + census identical to the committed row), "
            f"{len(leaves)} leaves, deepest {row['deepest_leaf']}, trusted {row['trusted_leaves']}, "
            f"samples inside the envelope {row['samples_inside_declared_envelope']}"
        )

    out_path = f"benchmarks/analysis/{protocol_id}-row-trace.json"
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    artifact = {
        "schema": "aporia.row-trace/1",
        "generated_by": "software/scripts/e13_row_trace.py",
        "protocol": protocol_id,
        "binary": binary,
        "binary_sha256": sha256(binary),
        "method": (
            "single-row plans derived from the frozen protocol (budgets=[B], seeds=[S], one entry), each "
            "reproduced row compared with the committed document on every measured field and the whole "
            "census before its cells were read; archives written under .scratch and not committed"
        ),
        "rows": artifact_rows,
    }
    with open(out_path, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(artifact, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
    print(f"wrote {out_path} ({len(artifact_rows)} rows)")
    return 0


def sha256(path):
    import hashlib

    return hashlib.sha256(open(path, "rb").read()).hexdigest()


if __name__ == "__main__":
    protocol_id = sys.argv[1] if len(sys.argv) > 1 else "e1-geometry"
    binary = sys.argv[2] if len(sys.argv) > 2 else ".scratch/e1x-bin/aporia-bench.exe"
    sys.exit(main(protocol_id, binary))
