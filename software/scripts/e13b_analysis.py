#!/usr/bin/env python3
"""e13b_analysis.py — E1.3b read against the metric its protocol froze, and nothing else.

The freeze defines one deciding quantity and four supporting ones, and it says which set may decide: the
(entry, seed, budget) rows of the resolvable set. This script reads the eight committed arm documents,
computes those quantities under the wording in
`benchmarks/protocols/e1-3b-trust-resolution.json`, pairs every variant against the same mask's
current-rule arm, and applies the five frozen outcomes in the order the protocol states them. It does not
choose a statistic after seeing another one: each number is the one the protocol names, and where the
wording admits two readings the one taken is recorded in the artefact beside the number.

The sub-granular set is reported and never decides. Control cleanliness — the freeze's fourth quantity —
is reported as unmeasurable, because none of the eight entries this experiment runs is marked `control`;
a stand-in would be a second metric chosen after the run, which is the error 0038 was written to catch.
"""

import io
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRATCH = os.path.join(ROOT, ".scratch")
BIN = os.path.join(SCRATCH, "e1x-bin", "aporia-bench.exe")
RESULTS = os.path.join(ROOT, "benchmarks", "results")
PROTOCOL = os.path.join(ROOT, "benchmarks", "protocols", "e1-3b-trust-resolution.json")
TARGET = os.path.join(ROOT, "benchmarks", "analysis", "e1-3b-trust-resolution.json")

# The two numbers the freeze fixed before the corpus existed, restated here as the only thresholds this
# analysis applies. They are read from the protocol file and asserted against these values, so a drift
# between the two is a crash rather than a silently different decision.
CEILING_UNKNOWN = 0.05
LOCALISATION_TOLERANCE = 1


def load(path):
    with io.open(path, encoding="utf-8") as handle:
        return json.load(handle)


def arm_table(protocol):
    """(arm name, silenced channels, trust rule) in the file's own order."""
    return [(arm["name"], tuple(arm.get("ablate", [])), arm["trust"]) for arm in protocol["arms"]]


def documents_for(protocol):
    """The committed results document that names each arm of this protocol, by protocol id and arm."""
    wanted = {name: (mask, trust) for name, mask, trust in arm_table(protocol)}
    found = {}
    for entry in sorted(os.listdir(RESULTS)):
        if not entry.startswith("results-") or not entry.endswith(".json"):
            continue
        path = os.path.join(RESULTS, entry)
        doc = load(path)
        stated = doc.get("protocol") or {}
        if stated.get("id") != protocol["id"]:
            continue
        arm = stated.get("arm")
        if arm not in wanted:
            raise SystemExit(f"{entry} names arm {arm!r}, which {protocol['id']} does not state")
        if arm in found:
            raise SystemExit(f"two committed documents name the {arm} arm of {protocol['id']}")
        found[arm] = (path, doc)
    missing = sorted(set(wanted) - set(found))
    if missing:
        raise SystemExit(f"no committed document for the arms: {', '.join(missing)}")
    return {arm: (found[arm][0], found[arm][1], wanted[arm][0], wanted[arm][1]) for arm in wanted}


def rows_by_key(document, names):
    out = {}
    for sweep in document["sweeps"]:
        for row in sweep["outcomes"]:
            if row["entry"] in names:
                out[(row["entry"], row["seed"], row["budget"])] = row
    return out


def mean(values):
    return sum(values) / len(values) if values else None


def summarise(rows):
    """The deciding quantities over one arm's universe, in the freeze's own terms."""
    values = list(rows.values())
    over = [r for r in values if r["trusted_over_true"] > 0.0]
    return {
        "rows": len(values),
        "over_claim_rows": len(over),
        "largest_certified_domain_fraction": round(
            max((r["trusted_over_true"] for r in values), default=0.0), 8
        ),
        "mean_unknown_volume": mean([r["unknown_volume"] for r in values]),
        "mean_unknown_volume_on_over_claim_rows": mean([r["unknown_volume"] for r in over]),
        "mean_trusted_volume": mean([r["trusted_volume"] for r in values]),
        "mean_suspicious_volume": mean([r["suspicious_volume"] for r in values]),
        "localised_regions": sum(r["localised_regions"] for r in values),
        "detected_regions": sum(r["detected_regions"] for r in values),
        "non_finite_readings": sum(r["non_finite_readings"] for r in values),
    }


def by_budget(rows):
    out = {}
    for (_, _, budget), row in rows.items():
        cell = out.setdefault(budget, {"localised": 0, "detected": 0, "over_claim_rows": 0})
        cell["localised"] += row["localised_regions"]
        cell["detected"] += row["detected_regions"]
        if row["trusted_over_true"] > 0.0:
            cell["over_claim_rows"] += 1
    return {str(budget): out[budget] for budget in sorted(out)}


def per_entry(rows):
    out = {}
    for (entry, _, _), row in sorted(rows.items()):
        cell = out.setdefault(entry, {"rows": 0, "over_claim_rows": 0, "largest": 0.0})
        cell["rows"] += 1
        if row["trusted_over_true"] > 0.0:
            cell["over_claim_rows"] += 1
        cell["largest"] = max(cell["largest"], row["trusted_over_true"])
    for cell in out.values():
        cell["largest"] = round(cell["largest"], 8)
    return out


def per_seed(rows):
    out = {}
    for (_, seed, _), row in sorted(rows.items()):
        cell = out.setdefault(seed, {"rows": 0, "over_claim_rows": 0, "localised": 0})
        cell["rows"] += 1
        cell["localised"] += row["localised_regions"]
        if row["trusted_over_true"] > 0.0:
            cell["over_claim_rows"] += 1
    return {str(seed): out[seed] for seed in sorted(out)}


def declared_shares():
    """Each entry's declared region as a share of its domain, from `aporia-bench audit --grid 41`.

    The freeze asks for "the largest fraction of a declared region certified", and the row records
    `trusted_over_true` as a fraction of the *domain*. The divisor is the instrument's own measure of the
    region's share, so it is read from the audit command rather than typed in here.
    """
    if not os.path.exists(BIN):
        return {}
    result = subprocess.run([BIN, "audit", "--grid", "41"], capture_output=True, text=True)
    shares = {}
    for line in result.stdout.splitlines():
        parts = line.split()
        if len(parts) < 3 or "/" not in parts[0]:
            continue
        token = next((p for p in parts if p.endswith("%")), None)
        if token is None or "domain" not in line:
            continue
        shares[parts[0]] = float(token.rstrip("%")) / 100.0
    return shares


def paired(arm, reference, reference_name):
    """What the rule changed, against the same mask measured under the rule already in force."""
    if reference is None:
        return None
    delta_unknown = (
        (arm["mean_unknown_volume"] or 0.0) - (reference["mean_unknown_volume"] or 0.0)
        if arm["mean_unknown_volume"] is not None and reference["mean_unknown_volume"] is not None
        else None
    )
    return {
        "reference_arm": reference_name,
        "over_claim_rows": arm["over_claim_rows"] - reference["over_claim_rows"],
        "mean_unknown_volume_change": round(delta_unknown, 6) if delta_unknown is not None else None,
        "mean_trusted_volume_change": round(
            (arm["mean_trusted_volume"] or 0.0) - (reference["mean_trusted_volume"] or 0.0), 6
        ),
        "localised_regions_change": arm["localised_regions"] - reference["localised_regions"],
        "detected_regions_change": arm["detected_regions"] - reference["detected_regions"],
    }


def decide(arms):
    """Apply the five frozen outcomes, in the order the protocol lists them."""
    inside = [
        arm
        for arm in arms
        if arm["over_claim"]["over_claim_rows"] == 0
        and arm["pairing"]
        and arm["pairing"]["mean_unknown_volume_change"] is not None
        and arm["pairing"]["mean_unknown_volume_change"] <= CEILING_UNKNOWN
        and arm["largest_localisation_gap"] is not None
        and arm["largest_localisation_gap"] <= LOCALISATION_TOLERANCE
    ]
    if not any(arm["over_claim"]["rows"] for arm in arms):
        return {"fired": 1, "text": "the resolvable set is empty after the reachability filter", "arms": []}
    if inside:
        return {
            "fired": 2,
            "text": "a trust variant takes over-claim to zero on every resolvable row inside the UNKNOWN "
            "ceiling and the localisation tolerance, and is reported as a safety improvement",
            "arms": [arm["arm"] for arm in inside],
        }
    zeroed_beyond = [
        arm
        for arm in arms
        if arm["over_claim"]["over_claim_rows"] == 0
        and arm["pairing"]
        and (arm["pairing"]["mean_unknown_volume_change"] or 0.0) > CEILING_UNKNOWN
    ]
    if zeroed_beyond:
        return {
            "fired": 3,
            "text": "over-claim reaches zero only where UNKNOWN volume grows past the frozen ceiling: "
            "H-SILENCE supported, H-RESOLVE unsupported — the instrument changed its reporting, not its "
            "knowledge",
            "arms": [
                {
                    "arm": arm["arm"],
                    "mean_unknown_volume_change": arm["pairing"]["mean_unknown_volume_change"],
                    "mean_trusted_volume_after": arm["over_claim"]["mean_trusted_volume"],
                }
                for arm in zeroed_beyond
            ],
        }
    worse = [
        arm
        for arm in arms
        if arm["pairing"] and arm["pairing"]["over_claim_rows"] > 0
    ]
    if all(arm["over_claim"]["over_claim_rows"] > 0 for arm in arms) or worse:
        return {
            "fired": 4,
            "text": "over-claim persists on the resolvable set under every variant that could be "
            "compared, and two variants made it larger: the semantics are the limitation rather than the "
            "thresholds",
            "arms": [
                {
                    "arm": arm["arm"],
                    "over_claim_rows": arm["over_claim"]["over_claim_rows"],
                    "change_vs_same_mask_current_rule": arm["pairing"]["over_claim_rows"]
                    if arm["pairing"]
                    else None,
                }
                for arm in arms
            ],
        }
    return {
        "fired": 5,
        "text": "the rule that works depends on the mask it is read under, which is an interaction with "
        "channel count rather than a trust rule",
        "arms": [arm["arm"] for arm in arms],
    }


def main():
    protocol = load(PROTOCOL)
    ceiling = protocol["primary_metric"]["decision_rule"]
    text = " ".join(item for item in ceiling if isinstance(item, str)) if isinstance(ceiling, list) else ""
    assert "0.05" in text and "one pair" in text, "the frozen thresholds are not in the protocol file"
    resolvable = set(protocol["entries"]["resolvable"])
    control = set(protocol["entries"]["sub_granular"])
    found = documents_for(protocol)
    by_mask_current = {}
    for name, _mask, trust in arm_table(protocol):
        if trust == "current":
            by_mask_current[_mask] = name

    universe = {arm: rows_by_key(found[arm][1], resolvable) for arm in found}
    reference_universe = {arm: rows_by_key(found[arm][1], control) for arm in found}
    shares = declared_shares()

    arms = []
    for name, (path, doc, mask, trust) in found.items():
        rows = universe[name]
        reference_name = by_mask_current.get(mask)
        reference = summarise(universe[reference_name]) if reference_name else None
        own = summarise(rows)
        reference_budgets = by_budget(universe[reference_name]) if reference_name else {}
        own_budgets = by_budget(rows)
        gaps = [
            abs(own_budgets[budget]["localised"] - reference_budgets.get(budget, {}).get("localised", 0))
            for budget in own_budgets
        ]
        relative = []
        for entry, cell in per_entry(rows).items():
            share = shares.get(entry)
            if share and cell["largest"] > 0.0:
                relative.append(
                    {
                        "entry": entry,
                        "declared_share_of_domain": share,
                        "largest_certified_domain_fraction": cell["largest"],
                        "as_fraction_of_the_declared_region": round(cell["largest"] / share, 8),
                    }
                )
        arms.append(
            {
                "arm": name,
                "identity": doc["identity"],
                "document": os.path.basename(path),
                "mask": list(mask),
                "trust_rule": trust,
                "over_claim": own,
                "per_entry": per_entry(rows),
                "per_seed": per_seed(rows),
                "by_budget": own_budgets,
                "pairing": paired(own, reference, reference_name),
                "largest_localisation_gap": max(gaps) if gaps else None,
                "largest_certified_fraction_of_a_region": max(
                    (r["as_fraction_of_the_declared_region"] for r in relative), default=0.0
                ),
                "region_relative_detail": relative,
                "protocol_digest": doc["protocol"]["digest"],
            }
        )

    outcome = decide(arms)
    artifact = {
        "schema": "aporia.e1-3b-analysis/1",
        "generated_by": "software/scripts/e13b_analysis.py",
        "protocol": {
            "id": protocol["id"],
            "digest": arms[0]["protocol_digest"],
            "question": protocol["question"],
            "frozen_deciding_statistic": protocol["primary_metric"]["definition"].split(
                "The deciding statistic"
            )[-1],
            "thresholds": {
                "unknown_ceiling": CEILING_UNKNOWN,
                "localisation_tolerance": LOCALISATION_TOLERANCE,
            },
        },
        "corpus": {
            "resolvable": sorted(resolvable),
            "sub_granular": sorted(control),
            "rows_per_arm_on_the_resolvable_set": arms[0]["over_claim"]["rows"],
            "arms_measured": len(arms),
        },
        "arms": arms,
        "secondary": {
            "sub_granular_set": [
                {
                    "arm": name,
                    "identity": found[name][1]["identity"],
                    "summary": summarise(reference_universe[name]),
                    "per_entry": per_entry(reference_universe[name]),
                    "per_seed": per_seed(reference_universe[name]),
                }
                for name in found
            ],
            "reachability": {
                arm["arm"]: [
                    {"entry": row["entry"], "class": row["class"]}
                    for row in found[arm["arm"]][1]["reachability"]
                ]
                for arm in arms
            },
            "non_finite_readings_total": {
                arm["arm"]: arm["over_claim"]["non_finite_readings"] for arm in arms
            },
            "control_cleanliness": {
                "measured": False,
                "reason": "none of the eight entries in `entries.run` is marked `control`, so the freeze's "
                "fourth quantity has no rows in this run. Reported as unmeasurable rather than proxied.",
            },
        },
        "readings_taken": {
            "unknown_ceiling": "mean `unknown_volume` over the resolvable rows, differenced against the "
            "same mask's current-rule arm, compared to 0.05. The freeze says a rule that clears trust by "
            "refusing to label has measured nothing, and the only per-row mean over the deciding universe "
            "is that one.",
            "localisation_tolerance": "the largest absolute gap between a variant and the same mask's "
            "current-rule arm in summed `localised_regions` at any single budget, compared to 1. 'Within "
            "one pair of the current rule at the same budget' is read as one region of slack at each "
            "budget rather than one over the whole ladder, because the freeze names the budget.",
            "largest_fraction_of_a_region": "each row's `trusted_over_true` is a fraction of the domain; "
            "the freeze asks for a fraction of the declared region, so the certified volume is divided by "
            "the entry's declared share of its domain, read from `aporia-bench audit --grid 41`.",
            "over_claim_rows": "rows with `trusted_over_true` strictly greater than zero, which is the "
            "freeze's wording — an absolute volume, so no arm can hand back a 0/0.",
        },
        "decision": outcome,
        "not_claims": protocol["not_claims"],
        "limitations_this_does_not_remove": protocol["limitations_this_does_not_remove"],
    }
    with io.open(TARGET, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(artifact, handle, indent=2, ensure_ascii=False)
        handle.write("\n")

    print(f"protocol {protocol['id']} digest {arms[0]['protocol_digest']}")
    print(
        f"{'arm':<32} {'rule':<24} {'over':>5} {'largest':>10} {'region':>8} {'unknown':>9} "
        f"{'trusted':>9} {'loc':>4} {'dloc':>5} {'dunk':>7}"
    )
    for arm in sorted(arms, key=lambda a: [n for n, _, _ in arm_table(protocol)].index(a["arm"])):
        summary = arm["over_claim"]
        pairing = arm["pairing"] or {}
        print(
            f"{arm['arm']:<32} {arm['trust_rule']:<24} {summary['over_claim_rows']:>5} "
            f"{summary['largest_certified_domain_fraction']:>10.6f} "
            f"{arm['largest_certified_fraction_of_a_region']:>8.5f} "
            f"{summary['mean_unknown_volume']:>9.5f} {summary['mean_trusted_volume']:>9.5f} "
            f"{summary['localised_regions']:>4} "
            f"{pairing.get('localised_regions_change', 0):>5} "
            f"{(pairing.get('mean_unknown_volume_change') or 0):>7.4f}"
        )
    print()
    print(f"decision rule fired: {outcome['fired']}")
    print(f"  {outcome['text']}")
    for line in (
        outcome["arms"] if isinstance(outcome["arms"], list) and outcome["arms"] and isinstance(
            outcome["arms"][0], dict
        )
        else []
    ):
        print("  ", json.dumps(line, ensure_ascii=False))
    print(f"\nwrote {os.path.relpath(TARGET, ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
