"""Derive the committed artifacts' headline claims and print them as a table.

This is the check that the prose in `benchmarks/results/README.md` still matches the JSON in
`benchmarks/analysis/`: the numbers below are read out of the artifact, which is itself generated from
the committed result documents. Nothing here is typed in by hand.

    cd software && python scripts/e13_recheck.py
"""

import json
import sys

ARTIFACT = "benchmarks/analysis/e1-geometry-postmortem.json"

d = json.load(open(ARTIFACT, encoding="utf-8"))
m = d["primary_metric"]
u = d["universe"]

print(f"protocol {d['inputs']['protocol']}  digest {d['inputs']['protocol_digest']}")
print(f"documents: {len(d['inputs']['result_documents'])}  arms: {', '.join(sorted(d['arms']))}")
print(f"cost per arm: {d['cost']['evaluations_per_arm']} evaluations, "
      f"{d['cost']['instruction_steps_per_arm']} instruction steps, equal={d['cost']['arms_cost_equal']}")
print(f"universe: {len(u['entries'])} entries x {len(u['seeds'])} seeds = {m['F'] + m['O'] and u['pairs']} pairs")
print(f"  membership rule: {u['membership_rule']}")
print(f"F={m['F']} O={m['O']} |F\\\\O|={len(m['full_only'])} |O\\\\F|={len(m['only_physical_only'])} "
      f"shared={m['shared']} same budget={m['shared_at_same_budget']} unresolved={len(m['unresolved_pairs'])}")
print(f"decision rule satisfied: {m['decision_rule_satisfied']}")
print(f"  {m['decision_rule_text'][:150]}")
print("\ncorpus-wide, all swept entries:")
for arm in sorted(d["corpus_wide"]):
    c = d["corpus_wide"][arm]
    print(f"   {arm:<18} localised {c['localised_pairs']:>3}/{c['pairs']}   detected {c['detected_pairs']:>3}")
print("\nsingle removals and single-channel arms on the frozen universe:")
for group in ("per_single_removal", "per_single_channel_arm"):
    for arm in sorted(d[group]):
        v = d[group][arm]
        print(f"   {arm:<18} lost {len(v['localisation_lost_from_full']):>2} localisations, "
              f"{len(v['detection_lost_from_full']):>2} detections")
s = d["safety_condition"]
print(f"\nsafety: {s['failing_rows']} of {s['edge_rows']} edge rows over-claim; values {s['distinct_values']}")
counts = {}
for row in s["rows"]:
    key = (row["arm"], row["entry"])
    counts[key] = counts.get(key, 0) + 1
for (arm, entry), n in sorted(counts.items()):
    print(f"   {arm:<18} {entry:<20} {n:>2} rows")
print("\nover-claim by entry (fraction of the declared envelope the full instrument certifies):")
for row in d["over_claim_by_entry"]:
    full = row["by_arm"]["full"]
    print(f"   {row['entry']:<26} envelope {row['declared_envelope_share']:<10.6g} "
          f"full max {full['max']:<10.6g} ({full['nonzero_rows']}/{full['rows']} rows) "
          f"fraction {row['fraction_of_envelope_certified_by_full']}")
print("\npartition:")
for row in d["testability_partition"]:
    g = row["declared"]
    print(
        f"   {row['entry']:<26} {row['experiment']:<6} full {row['full_localised']}/{row['pairs']} "
        f"op {row['only_physical_localised']}/{row['pairs']} det {row['full_detected']}/{row['pairs']} "
        f"{row['class']:<40} envelope {g['envelope_share_of_domain']:.3e} vs finest leaf "
        f"{g['finest_permitted_leaf_share_of_domain']:.3e} -> enclosable {row['leaf_could_ever_enclose_region']}"
    )
print("\ntraced rows:")
for row in d["testability_partition"]:
    for t in (row.get("traced_rows") or []):
        leaf = t.get("leaf_touching_the_region")
        print(
            f"   {row['entry']:<26} {t.get('arm', ''):<16} {t.get('purpose', '')}"
        )
        if leaf:
            print(
                f"      leaf {leaf['cell']} depth {leaf['depth']} {leaf['label']} "
                f"samples {leaf['samples']} mean {leaf['mean_risk']} max {leaf['max_risk']} "
                f"facts {leaf['facts']} measured {leaf['measured']} "
                f"bounds {leaf['bounds']['axis0']} share {leaf['share_of_domain']}"
            )
            if t.get("derived_overlap_of_region_in_leaf") is not None:
                print(f"      derived overlap of region in that leaf: "
                      f"{t['derived_overlap_of_region_in_leaf']:.6g}")
            if t.get("samples_inside_declared_envelope") is not None:
                print(
                    f"      samples inside the declared envelope: "
                    f"{t['samples_inside_declared_envelope']}"
                    f" (largest sample anywhere {t.get('largest_sample_anywhere')})"
                )
print("\nimplementation checks:")
for c in d["implementation_checks"]:
    print(f"   [{c['result'][:40]}] {c['check']}")
return_code = 0
if not (m["F"] == 28 and m["O"] == 28 and s["failing_rows"] == 87 and d["cost"]["arms_cost_equal"]):
    print("UNEXPECTED: the artifact does not carry the headline claims the prose cites", file=sys.stderr)
    return_code = 1
sys.exit(return_code)
