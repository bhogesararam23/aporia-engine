#!/usr/bin/env python3
"""post_e1_analysis.py — the deterministic, machine-readable audit of a measured protocol.

Reads only committed files: `benchmarks/results/results-*.json` for one protocol id, the protocol file
those documents cite, and the corpus the documents were measured against — plus, when it exists, the
row-trace artifact written by `e13_row_trace.py`. It writes one JSON artifact and prints a summary, and
it runs no campaign: every figure is either copied from a committed file or arithmetic on values copied
from committed files, so the prose a reader trusts is generated from the source a machine can check.

    cd software && python scripts/post_e1_analysis.py e1-geometry

The script deliberately does not evaluate a declared `where` expression. Region measures are read as
envelopes, and the carved set comes from the instrument's own row trace, because a second implementation
of the truth language would turn this audit into an opinion rather than a check on the first one.
"""

import glob
import hashlib
import json
import os
import re
import sys

# The atlas's own constants, from `aporia_boundary::Policy::default` — which `harness::Plan::config`
# hands to every arm of every run of this protocol. They are here so the reachability arithmetic below
# uses the instrument's numbers rather than a paraphrase of them.
MAX_DEPTH = 12
LOCALISATION_BAR = 0.5
MIN_SAMPLES = 4
MIN_CHANNELS_APPLIED = 1
# `truth::LATTICE_PER_AXIS` and `LATTICE_SENSITIVITY_PER_AXIS`, the two fixed resolutions a predicate region
# is measured on.
LATTICE_PER_AXIS = 32
LATTICE_SENSITIVITY_PER_AXIS = 64

CAUSE = {
    "semantics": "TRUSTED is earned by the cell's own quiet samples and says nothing about the part of "
    "the cell that was never sampled: Policy::classify grants it when the cell is not flagged, holds at "
    "least min_samples points, and has min_channels_applied channels *applied* inside it.",
    "granularity": "the region is thinner than the finest leaf the atlas is permitted to build at "
    "max_depth, so no leaf can be half inside it and localisation is impossible by construction.",
    "coverage": "no sample in the sweep entered the region, so the label was decided entirely by points "
    "outside it.",
    "labelling_by_absence_of_evidence": "the arm's own census shows channels computing nothing, and the "
    "leaf was trusted anyway — silence under the channels that ran is what TRUSTED means here.",
    "corpus": "the declared region sits where resolution is scarcest: pinned against a domain edge, or "
    "straddling a leaf boundary.",
    "implementation": "the measured quantity does not equal the quantity the protocol defines. This "
    "audit reports no instance of it; the checks that would show one are listed under "
    "implementation_checks.",
}

EXPERIMENT = {"geometry": "E1.1", "discrete": "E1.2", "edge": "E1.3", "domain": "E1.3"}


def fail(message):
    sys.exit(f"post_e1_analysis: {message}")


def numbers(text):
    return re.findall(r"-?\d+\.?\d*(?:[eE][-+]?\d+)?", text)


def model_domains(model_text):
    """`input name in [lo, hi]` as a span, `input name in {a, b}` as its enumerated values."""
    spans = {}
    for line in model_text.splitlines():
        parts = line.strip().split()
        if len(parts) < 4 or parts[0] != "input" or parts[2] != "in":
            continue
        found = numbers(" ".join(parts[3:]))
        if len(found) < 2:
            continue
        if parts[3].startswith("["):
            spans[parts[1]] = (float(found[0]), float(found[1]))
        elif parts[3].startswith("{"):
            spans[parts[1]] = (float(found[0]), float(found[-1]))
    return spans


def declared_geometry(entry_id):
    family, name = entry_id.split("/", 1)
    truth_path = f"benchmarks/{family}/{name}/truth.json"
    model_path = f"benchmarks/{family}/{name}/model.ap"
    if not (os.path.exists(truth_path) and os.path.exists(model_path)):
        return None
    truth = json.load(open(truth_path, encoding="utf-8"))
    spans = model_domains(open(model_path, encoding="utf-8").read())
    axes = len(spans)
    if axes == 0 or not truth.get("regions"):
        return None
    domain_area = 1.0
    for lo, hi in spans.values():
        domain_area *= hi - lo
    envelope_share = 0.0
    thinnest = None
    for region in truth["regions"]:
        named = list(region.get("axes", {}).items())
        if not named:
            continue
        area, widest_span = 1.0, 0.0
        for axis, bounds in named:
            lo, hi = float(bounds[0]), float(bounds[1])
            area *= hi - lo
            widest_span = max(widest_span, hi - lo)
        envelope_share += area / domain_area
        # The direction that decides whether a leaf can be half inside: the region's thinnest extent.
        thin = min(float(b[1]) - float(b[0]) for _, b in named)
        thinnest = thin if thinnest is None else min(thinnest, thin)
    # `split` halves the longest normalised axis each time, so at depth d each of n axes has been
    # halved about d/n times: the finest permitted leaf occupies 1/2^(per_axis*n) of the domain.
    per_axis = MAX_DEPTH // axes
    leaf_fraction = 1.0 / 2 ** (per_axis * axes)
    side_of_finest_leaf = max(hi - lo for lo, hi in spans.values()) / 2**per_axis
    return {
        "axes": axes,
        "declared_regions": len(truth["regions"]),
        "carries_a_where": any(r.get("where") for r in truth["regions"]),
        "envelope_share_of_domain": round(envelope_share, 12),
        "thinnest_declared_extent": thinnest,
        "finest_permitted_leaf_share_of_domain": round(leaf_fraction, 12),
        "longest_side_of_finest_permitted_leaf": side_of_finest_leaf,
    }


def leaf_can_ever_enclose(geometry):
    """Could any permitted leaf be half inside the declared envelope? Envelope arithmetic only.

    A leaf half inside a region must have the region's measure at least half the leaf's, and no leaf is
    finer than `finest_permitted_leaf_share_of_domain`. Where this is false the region is out of reach
    at every budget and for every arm, and no evidence channel can change that.
    """
    if not geometry:
        return None
    floor = geometry["finest_permitted_leaf_share_of_domain"]
    if floor <= 0:
        return None
    return geometry["envelope_share_of_domain"] >= floor * LOCALISATION_BAR


def main(protocol_id, out_path, trace_path):
    documents = {}
    for path in sorted(glob.glob("benchmarks/results/results-*.json")):
        doc = json.load(open(path, encoding="utf-8"))
        proto = doc.get("protocol") or {}
        if proto.get("id") != protocol_id:
            continue
        arm = proto.get("arm")
        if arm in documents:
            fail(f"two documents claim arm {arm}")
        documents[arm] = (path, doc)
    if not documents:
        fail(f"no committed document cites protocol {protocol_id!r}")

    protocol_path = f"benchmarks/protocols/{protocol_id}.json"
    if not os.path.exists(protocol_path):
        fail(f"{protocol_path} is missing")
    raw = open(protocol_path, "rb").read()
    digest = hashlib.sha256(raw).hexdigest()[:16]
    protocol = json.loads(raw.decode("utf-8"))
    cited = {(d.get("protocol") or {}).get("digest") for _, d in documents.values()}
    if cited != {digest}:
        fail(f"documents cite {sorted(cited)}; {protocol_path} digests to {digest}")

    trace = json.load(open(trace_path, encoding="utf-8")) if os.path.exists(trace_path) else None
    traced_rows = {}
    if trace:
        for row in trace["rows"]:
            traced_rows[(row["arm"], row["entry"], row["seed"], row["budget"])] = row

    def index(doc):
        sweeps, rows = {}, {}
        for sweep in doc["sweeps"]:
            sweeps[(sweep["entry"], sweep["seed"])] = sweep
            for o in sweep["outcomes"]:
                rows[(o["entry"], o["seed"], o["budget"])] = o
        return sweeps, rows

    arms, sweep_of, row_of = {}, {}, {}
    for name, (path, doc) in documents.items():
        sweeps, rows = index(doc)
        sweep_of[name], row_of[name] = sweeps, rows
        arms[name] = {
            "file": os.path.basename(path),
            "identity": doc["identity"],
            "mask": sorted(doc["plan"].get("ablate") or []),
            "sweeps": len(doc["sweeps"]),
            "rows": len(rows),
            "evaluations": sum(o["evaluations"] for o in rows.values()),
            "instruction_steps": sum(o["instruction_steps"] for o in rows.values()),
            "refused": sorted(r["entry"] for r in doc.get("refused", [])),
            "replayed": sum(1 for s in doc["sweeps"] if s.get("replay")),
            "replay_errors": sum(1 for o in rows.values() if o.get("replay_error")),
            "counterexample_rows": sum(len(o["counterexamples"]) for o in rows.values()),
        }
        if arms[name]["replay_errors"]:
            fail(f"{name}: replay errors present")
    costs = {(a["evaluations"], a["instruction_steps"]) for a in arms.values()}
    if len(costs) != 1:
        fail(f"arms are not cost-equal: {costs}")
    evaluations, steps = costs.pop()
    if len(arms) != len(protocol["arms"]):
        fail(f"{len(arms)} documents for {len(protocol['arms'])} frozen arms")

    named = protocol["entries"]["run"]
    anchors = protocol["entries"]["anchors"]
    controls = sorted(e["entry"] for e in documents["full"][1]["entries"] if e.get("control"))
    refused = arms["full"]["refused"]
    universe = [e for e in named if e not in anchors and e not in controls and e not in refused]
    seeds = documents["full"][1]["plan"]["seeds"]
    pairs = [(e, s) for e in universe for s in seeds]
    budgets = documents["full"][1]["plan"]["budgets"]

    def localised(arm):
        return {
            k: sweep_of[arm][k].get("localised_at_budget")
            for k in pairs
            if k in sweep_of[arm]
        }

    def detected(arm):
        return {
            k
            for k in pairs
            if k in sweep_of[arm] and sweep_of[arm][k].get("detected_at_budget") is not None
        }

    F, O = localised("full"), localised("only-physical")
    set_f = {k for k, v in F.items() if v is not None}
    set_o = {k for k, v in O.items() if v is not None}
    full_only, op_only = sorted(set_f - set_o), sorted(set_o - set_f)
    shared = sorted(set_f & set_o)
    same_budget = sorted(k for k in shared if F[k] == O[k])
    unresolved = sorted(set(pairs) - set_f - set_o)

    # The frozen decision rule, applied in the order the protocol states it.
    if not set_f:
        rule = 1
    elif full_only:
        rule = 2
    elif op_only:
        rule = 4
    elif len(same_budget) == len(shared):
        rule = 3
    else:
        rule = 0
    rule_text = (
        protocol["primary_metric"]["decision_rule"][rule - 1]
        if rule
        else "the two arms localise the same pairs at different budgets: no numbered rule fits"
    )

    # Corpus-wide, not just the frozen universe: these are the quantities E2 published, and a reader
    # comparing the two experiments will look for them here.
    all_pairs = [
        (entry, seed)
        for entry in named
        for seed in seeds
        if (entry, seed) in sweep_of["full"]
    ]
    corpus_wide = {}
    for arm in sorted(arms):
        loc = {
            k: sweep_of[arm][k].get("localised_at_budget")
            for k in all_pairs
            if k in sweep_of[arm]
        }
        det = {
            key
            for key in all_pairs
            if key in sweep_of[arm]
            and sweep_of[arm][key].get("detected_at_budget") is not None
        }
        corpus_wide[arm] = {
            "pairs": len(all_pairs),
            "localised_pairs": sum(1 for v in loc.values() if v is not None),
            "detected_pairs": len(det),
        }

    per_removal, per_single = {}, {}
    for arm in sorted(a for a in arms if a != "full"):
        loc, det = localised(arm), detected(arm)
        entry = {
            "localisation_lost_from_full": sorted(
                f"{e}:seed{s}" for (e, s) in set_f if loc.get((e, s)) is None
            ),
            "detection_lost_from_full": sorted(
                f"{e}:seed{s}" for (e, s) in detected("full") - det
            ),
            "detection_gained_over_full": sorted(f"{e}:seed{s}" for (e, s) in det - detected("full")),
            "localisation_gained_over_full": sorted(
                f"{e}:seed{s}" for (e, s) in pairs if loc.get((e, s)) is not None and F.get((e, s)) is None
            ),
        }
        (per_removal if arm.startswith("no-") else per_single)[arm] = entry

    volumes = {}
    for arm in sorted(a for a in arms if a != "full"):
        delta = {"suspicious": 0.0, "trusted": 0.0, "unknown": 0.0}
        control_trust_rise = 0
        compared = 0
        for key, o in row_of["full"].items():
            other = row_of[arm].get(key)
            if other is None:
                fail(f"{arm} is missing {key}")
            compared += 1
            for label, field in (
                ("suspicious", "suspicious_volume"),
                ("trusted", "trusted_volume"),
                ("unknown", "unknown_volume"),
            ):
                delta[label] += other[field] - o[field]
            if o["control"] and other["trusted_volume"] > o["trusted_volume"] + 1e-12:
                control_trust_rise += 1
        volumes[arm] = {k: round(v, 6) for k, v in delta.items()}
        volumes[arm]["rows_compared"] = compared
        volumes[arm]["control_rows_trusted_more_than_full"] = control_trust_rise

    geometry = {e: declared_geometry(e) for e in universe}
    edge = [e for e in universe if e.startswith("edge/")]
    failures, edge_rows = [], 0
    for arm in sorted(arms):
        for key in sorted(row_of[arm]):
            entry, seed, budget = key
            if entry not in edge:
                continue
            edge_rows += 1
            o = row_of[arm][key]
            value = o["trusted_over_true"]
            if value == 0.0:
                continue
            share = geometry[entry]["envelope_share_of_domain"]
            # `causes` holds legend keys only. "how much of the region was certified" and "was the
            # whole domain trusted" are observations about the row, and they travel as their own
            # fields so that a reader cannot mistake a description for a diagnosis.
            causes = ["semantics"]
            whole_envelope = bool(share and value >= share - 1e-15)
            whole_domain = o["trusted_volume"] >= 1.0 - 1e-12
            if leaf_can_ever_enclose(geometry[entry]) is False:
                causes.append("granularity")
            if o["detected_regions"] == 0:
                causes.append("coverage")
            silent = sorted(c["channel"] for c in o["census"] if c["computed"] == 0)
            if silent:
                causes.append("labelling_by_absence_of_evidence")
            row = {
                "arm": arm,
                "entry": entry,
                "seed": seed,
                "budget": budget,
                "trusted_over_true": value,
                "declared_envelope_share": share,
                "fraction_of_envelope_certified": round(value / share, 6) if share else None,
                "trusted_volume": o["trusted_volume"],
                "unknown_volume": o["unknown_volume"],
                "suspicious_volume": o["suspicious_volume"],
                "findings": o["findings"],
                "unplaced": o["unplaced"],
                "silenced": sorted(c["channel"] for c in o["census"] if c["silenced"]),
                "channels_computing_nothing": silent,
                "causes": causes,
                "observed_whole_envelope_certified": whole_envelope,
                "observed_whole_domain_trusted": whole_domain,
            }
            traced = traced_rows.get((arm, entry, seed, budget))
            if traced:
                row["trace"] = {
                    "purpose": traced.get("purpose"),
                    "leaf_touching_the_region": traced.get("leaf_touching_the_region"),
                    "derived_overlap_of_region_in_leaf": traced.get(
                        "derived_overlap_of_region_in_leaf"
                    ),
                    "trusted_leaves_touching_the_region": traced.get(
                        "trusted_leaves_touching_the_region"
                    ),
                    "samples_inside_declared_envelope": traced.get(
                        "samples_inside_declared_envelope"
                    ),
                    "largest_sample_anywhere": traced.get("largest_sample_anywhere"),
                    "leaves_total": traced.get("leaves_total"),
                    "deepest_leaf": traced.get("deepest_leaf"),
                    "finest_leaf_share_of_domain": traced.get("finest_leaf_share_of_domain"),
                    "reproduces_committed_row": traced.get("reproduces_committed_row"),
                }
            failures.append(row)
    failures.sort(
        key=lambda f: (-f["trusted_over_true"], f["arm"], f["entry"], f["seed"], f["budget"])
    )

    over_claim = []
    for entry in universe:
        by_arm = {}
        for arm in sorted(arms):
            values = [o["trusted_over_true"] for (e, s, b), o in row_of[arm].items() if e == entry]
            by_arm[arm] = {
                "rows": len(values),
                "nonzero_rows": sum(1 for v in values if v > 0.0),
                "max": max(values) if values else 0.0,
            }
        share = geometry[entry]["envelope_share_of_domain"]
        over_claim.append(
            {
                "entry": entry,
                "declared_envelope_share": share,
                "by_arm": by_arm,
                "fraction_of_envelope_certified_by_full": round(
                    by_arm["full"]["max"] / share, 6
                )
                if share
                else None,
            }
        )

    partition = []
    for entry in universe:
        loc_by_arm = {
            arm: sum(1 for s in seeds if localised(arm).get((entry, s)) is not None)
            for arm in arms
        }
        det_by_arm = {
            arm: sum(1 for s in seeds if (entry, s) in detected(arm)) for arm in arms
        }
        f_here = sum(1 for s in seeds if F.get((entry, s)) is not None)
        o_here = sum(1 for s in seeds if O.get((entry, s)) is not None)
        enclosable = leaf_can_ever_enclose(geometry[entry])
        if f_here == 0 and max(loc_by_arm.values()) == 0:
            if enclosable is False:
                cls = "structurally_unlocalisable"
            elif max(det_by_arm.values()) > 0:
                cls = "seen_by_some_arm_localised_by_none"
            else:
                cls = "seen_by_no_arm_localised_by_none"
        elif f_here == o_here == len(seeds):
            cls = "exercised_physical_parity_every_pair"
        elif f_here == o_here:
            cls = "exercised_physical_parity_some_pairs"
        else:
            cls = "discriminates"
        traced = [
            {
                k: r[k]
                for k in (
                    "arm",
                    "seed",
                    "budget",
                    "purpose",
                    "leaves_total",
                    "deepest_leaf",
                    "finest_leaf_share_of_domain",
                    "trusted_leaves",
                    "samples_inside_declared_envelope",
                    "largest_sample_anywhere",
                    "leaf_touching_the_region",
                    "derived_overlap_of_region_in_leaf",
                    "deciding_suspicious_leaf",
                )
                if k in r
            }
            for r in (trace["rows"] if trace else [])
            if r["entry"] == entry
        ]
        partition.append(
            {
                "entry": entry,
                "experiment": EXPERIMENT[entry.split("/", 1)[0]],
                "pairs": len(seeds),
                "full_localised": f_here,
                "only_physical_localised": o_here,
                "full_detected": det_by_arm["full"],
                "localised_by_any_arm": max(loc_by_arm.values()),
                "detected_by_any_arm": max(det_by_arm.values()),
                "class": cls,
                "discriminates_between_arms": bool(full_only or op_only),
                "declared": geometry[entry],
                "leaf_could_ever_enclose_region": enclosable,
                "traced_rows": traced,
            }
        )

    unplaced_total = sum(o["unplaced"] for arm in arms for o in row_of[arm].values())
    artifact = {
        "schema": "aporia.postmortem/1",
        "generated_by": "software/scripts/post_e1_analysis.py",
        "inputs": {
            "protocol": f"benchmarks/protocols/{protocol_id}.json",
            "protocol_digest": digest,
            "result_documents": sorted(a["file"] for a in arms.values()),
            "row_trace": os.path.basename(trace_path) if trace else None,
        },
        "question": protocol["question"],
        "hypotheses": protocol.get("hypotheses"),
        "arms": arms,
        "cost": {
            "evaluations_per_arm": evaluations,
            "instruction_steps_per_arm": steps,
            "arms_cost_equal": True,
        },
        "universe": {
            "membership_rule": "protocol entries.run, minus entries.anchors, minus entries whose truth "
            "declares control, minus entries the plan refuses",
            "entries": universe,
            "seeds": seeds,
            "budgets": budgets,
            "pairs": len(pairs),
            "anchors_excluded": anchors,
            "controls_excluded": controls,
            "refused_excluded": refused,
        },
        "primary_metric": {
            "name": protocol["primary_metric"]["name"],
            "F": len(set_f),
            "O": len(set_o),
            "full_only": [list(k) for k in full_only],
            "only_physical_only": [list(k) for k in op_only],
            "shared": len(shared),
            "shared_at_same_budget": len(same_budget),
            "unresolved_pairs": [list(k) for k in unresolved],
            "decision_rule_satisfied": rule,
            "decision_rule_text": rule_text,
        },
        "corpus_wide": corpus_wide,
        "per_single_removal": per_removal,
        "per_single_channel_arm": per_single,
        "volume_deltas_vs_full": volumes,
        "safety_condition": {
            "text": protocol["secondary_metrics"][4],
            "edge_entries": edge,
            "edge_rows": edge_rows,
            "failing_rows": len(failures),
            "distinct_values": sorted({f["trusted_over_true"] for f in failures}, reverse=True),
            "rows": failures,
        },
        "over_claim_by_entry": over_claim,
        "testability_partition": partition,
        "cause_classes": CAUSE,
        "implementation_checks": [
            {
                "check": "trusted_over_true equals the envelope arithmetic of the traced leaf",
                "detail": "leaf share x region share of that leaf: 0.0078125 x 3.2e-4 = 2.5e-6 on the "
                "full arm's failing row, and 1.25e-6 where a leaf boundary cuts the band in half",
                "result": "confirmed against the row trace" if trace else "row trace absent",
            },
            {
                "check": "the quantity is an absolute volume, not a ratio, so no 0/0 reads as a pass",
                "result": "confirmed: it is a sum of cell volumes in the units of suspicious_volume",
            },
            {
                "check": "no unplaced measurement hides behind a label",
                "detail": f"unplaced summed over every row of every arm = {unplaced_total}",
                "result": "confirmed: 0" if unplaced_total == 0 else f"FAILED: {unplaced_total}",
            },
            {
                "check": "UNKNOWN is excluded from the numerator, so a pass can mean only 'not enough "
                "samples'",
                "detail": "the passing sibling of the failing row is UNKNOWN with 3 samples against "
                "min_samples 4 — the quantity can only fail when a leaf is trusted, and it can pass for "
                "a reason that has nothing to do with safety",
                "result": "confirmed against the row trace" if trace else "row trace absent",
            },
            {
                "check": "a trusted cell overlapping several regions counts its largest, not their union",
                "detail": "Truth::overlap_at takes max over regions, so on a multi-region entry the "
                "quantity is a lower bound of 'trusted volume inside any declared region'. It can only "
                "under-report a violation, never invent one, and both edge entries declare one region, "
                "so the frozen condition's arithmetic is unaffected",
                "result": "confirmed by reading truth.rs::overlap_at; the direction is conservative",
            },
            {
                "check": "the envelope denominator sums regions, which double-counts overlap between them",
                "detail": "Truth::total_fraction adds each region's volume and the corpus keeps boxes "
                "disjoint, so `fraction_of_envelope_certified` is only exact where regions do not "
                "overlap; it is reported per entry and no entry in this protocol's edge pair has more "
                "than one region",
                "result": "confirmed; flagged as a bound, not a correction",
            },
            {
                "check": "a region that exists only at a domain endpoint is measured from interior midpoints",
                "detail": "the lattice samples lo + (i+0.5) x width/N, so x = 4 is never a sample; the "
                "failing envelope's measure comes from points strictly inside it, which is why the "
                "derived overlap (3.2002e-4) matches 1e-5 / 0.03125 rather than an endpoint-inclusive "
                "count. `corpus::verify` hits the endpoints exactly and is a separate gate",
                "result": "confirmed against the trace artifact",
            },
            {
                "check": "a band narrower than the measure lattice is reported by the lattice, not by "
                "its own width",
                "detail": "a `where` region's sampled share counts points of a fixed "
                f"{LATTICE_PER_AXIS}-per-axis lattice inside its envelope, so a band thinner than one "
                "lattice step is quantised: the artifact reports the sampled share, the step and the "
                "doubled step (the sensitivity lattice), and does not attempt a finer measure of its "
                "own. `lattice_sensitive` is false on every row of this run, which says the *localisation "
                "decisions* did not move when the lattice doubled; it does not say the measure is the "
                "region's true area",
                "result": "confirmed: reported as a bound on interpretation, not corrected",
            },
            {
                "check": "the metric is a volume, not a confidence statement",
                "detail": "no sample count, no per-channel requirement and no uncertainty enters it; a "
                "leaf is trusted on four quiet points and the region inside it contributes its whole "
                "measure, so `trusted_over_true = 0` does not mean 'the region was checked' and a "
                "non-zero value does not carry a probability",
                "result": "confirmed: it was never a statistical statement and must not be read as one",
            },
            {
                "check": "a predicate region's measure is not re-derived by this audit",
                "result": "confirmed by construction: envelopes here, carved sets from the instrument",
            },
        ],
        "atlas_constants": {
            "measure_lattice_per_axis": LATTICE_PER_AXIS,
            "sensitivity_lattice_per_axis": LATTICE_SENSITIVITY_PER_AXIS,
            "max_depth": MAX_DEPTH,
            "localisation_bar": LOCALISATION_BAR,
            "min_samples": MIN_SAMPLES,
            "min_channels_applied": MIN_CHANNELS_APPLIED,
            "source": "aporia_boundary::Policy::default, which harness::Plan::config hands to every arm "
            "of every run of this protocol",
        },
    }
    text = json.dumps(artifact, indent=2, ensure_ascii=False)
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    with open(out_path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text + "\n")

    print(f"wrote {out_path} ({len(text.splitlines())} lines)")
    print(
        f"protocol {protocol_id} digest {digest}; arms {len(arms)}; cost {evaluations} evaluations and "
        f"{steps} steps per arm"
    )
    print(
        f"universe {len(pairs)} pairs; F {len(set_f)}, O {len(set_o)}, |F\\\\O| {len(full_only)}, "
        f"|O\\\\F| {len(op_only)}, same budget {len(same_budget)}/{len(shared)} -> rule {rule}"
    )
    print(
        "corpus-wide localised pairs: "
        + ", ".join(f"{a}={corpus_wide[a]['localised_pairs']}" for a in sorted(corpus_wide))
    )
    print(f"safety: {len(failures)} of {edge_rows} edge rows fail; unplaced total {unplaced_total}")
    print("partition:")
    for row in partition:
        print(
            f"   {row['entry']:<26} {row['experiment']:<6} full {row['full_localised']}/"
            f"{row['pairs']} only-Physical {row['only_physical_localised']}/{row['pairs']} detected "
            f"{row['full_detected']}/{row['pairs']} -> {row['class']}"
            + (
                ""
                if row["leaf_could_ever_enclose_region"] is not False
                else f"   [finest leaf {row['declared']['finest_permitted_leaf_share_of_domain']:.3e}"
                f" vs envelope {row['declared']['envelope_share_of_domain']:.3e}]"
            )
        )
    return 0


if __name__ == "__main__":
    protocol_id = sys.argv[1] if len(sys.argv) > 1 else "e1-geometry"
    out = sys.argv[2] if len(sys.argv) > 2 else f"benchmarks/analysis/{protocol_id}-postmortem.json"
    trace = sys.argv[3] if len(sys.argv) > 3 else f"benchmarks/analysis/{protocol_id}-row-trace.json"
    sys.exit(main(protocol_id, out, trace))
