# Measurements

This directory holds the output of `aporia-bench run`, and this file explains what those numbers say —
including where they say the method does not work, and where an earlier version of this file said
something the data does not support.

Four files are committed, plus the repeat that follows each step.

| file | what it is |
|---|---|
| `results-1791114389.json` | the first measurement, from the pipeline as of `7b9e777` |
| `results-1791123062.json` | after the evidence-semantics and atlas-labelling work in `8d188e6` |
| `results-1791125825.json` | after the `suspicious_channels` corroboration rule |
| `results-1791126020.json` | a single-budget repeat of the corroboration build (`--budgets 640` only), kept because the timing note near the end of this file rests on it |
| `results-1791127362.json` | **current**, after findings that describe one region are merged into one claim |

The first three were produced by the same command:

```
aporia-bench run --budgets 40,80,160,320,640 --seeds 1,2,3
```

on 2026-10-04 on an Intel Core Ultra 5 125H, Rust 1.99.0 release build, no GPU involved.
20 corpus entries × 3 strategies × 3 seeds × 5 budgets = 900 campaign runs, 180 sweeps.

Definitions live in `crates/aporia-bench/src/metrics.rs`, next to the code that computes them. The
two that carry the weight here:

- **detected** — some suspicious cell touches a declared region. Loose on purpose.
- **localised** — a suspicious cell touches a declared region *and* is at least half inside declared
  regions. A big lazy cell cannot pass it.

Everything quoted below was recomputed from the JSON in this directory; if a number here cannot be
derived from those files, it does not belong here.

## What happened, per entry

`loc N` is the best budget at which the strategy localised, with how many of its three seeds got
there; `det N` is detection without localisation; `nothing` is neither within 640 evaluations. The
median column is wall time at the top of the ladder — read the compute-cost note below before drawing
any conclusion from those numbers.

| entry | fault | adaptive | stratified | random | med ms |
|---|---|---|---|---|---|
| `analytic/sqrt_domain` | domain assumption | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 8 |
| `analytic/log_positive` | domain assumption | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 5 |
| `analytic/exp_overflow` | overflow divergence | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 7 |
| `analytic/reciprocal_bound` | overflow divergence | det 40, never loc | det 40, never loc | det 640 (1/3) | 3 |
| `ode/euler_decay` | time step sensitivity | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 14 |
| `ode/euler_decay_2d` | time step, curved boundary | det 40, never loc | det 40, never loc | det 40, never loc | 19 |
| `control/naive_euler_spring` | state update ordering | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 34 |
| `linear_algebra/quadratic_small_root` | cancellation | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 14 |
| `aerospace/projectile_sign_mutant` | sign error | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 7 |
| `synthetic/wide_1d` | 40% of a line | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 6 |
| `synthetic/quarter_2d` | 25% of a plane | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 6 |
| `synthetic/narrow_1d` | 4% of a line | loc 320 (3/3) | loc **160** (3/3) | loc **160** (3/3) | 3 |
| `synthetic/one_pct_2d` | 1% of a plane | loc 640 (1/3) | det 160, never loc | loc 320 (1/3) | 5 |
| `synthetic/one_pct_3d` | 2% of a cube | loc 640 (1/3) | loc 640 (3/3) | det 80, never loc | 4 |
| `electromagnetics/rlc_resonance` | 0.088% band | loc 640 (1/3) | det 320, never loc | det 640 (1/3) | 6 |
| `synthetic/tenth_pct_3d` | 0.1% of a cube | nothing | nothing | nothing | 4 |
| `synthetic/narrow_1d_unreachable` | 1e-6 of a line | det 40, never loc | det 40, never loc | nothing | 3 |
| `aerospace/projectile_zero_gravity` | measure-zero region | nothing | nothing | nothing | 7 |
| `control/symplectic_spring` | none (control) | nothing | nothing | nothing | 28 |
| `aerospace/projectile_clean` | none (control) | nothing | nothing | nothing | 10 |

Across all 180 sweeps: **134 detect at some budget, 97 localise.** The first run was 153 and 92, the
second 140 and 97. The localisation count held exactly where the corroboration rule was applied, and
six sweeps of detection were given up — "Where localisation still stops working" explains why those
six were never detections.

## The strategy comparison, and why it is narrower than hoped

`aporia-bench verdict` says: **adaptive better on 1 entry, worse on 2, equal or unresolved on 15.** In
the first run it was 2 better, 2 worse.

The shape of the result is what a rule like this should produce. Adaptive wins where the region is
small relative to the space and nothing else finds it — `electromagnetics/rlc_resonance`, a resonance
band 0.088% of the domain, localised at 640 by adaptive on one seed and by neither baseline at any
budget — and loses where the region is large enough that plain coverage walks into it anyway:
`synthetic/narrow_1d` and `synthetic/one_pct_3d`, both resolved by stratified at the same or a smaller
budget. Between the first and second runs the baselines gained (`narrow_1d` went from 3 resolving seeds
to 9, `one_pct_3d` from 0 localising sweeps to 4, `quarter_2d`'s best localising budget from 320 to
40) and adaptive's advantage narrowed as a result.

One entry where the method shows an advantage, and two where a baseline is the right tool, is not an
answer. It is a reason the ladder needs more entries of the resonance shape. Adding them is the next
experiment, not a rhetorical claim that the shape does not matter.

## The controls, and a correction to this file

The first version of this file reported the controls as "0.9% random, 2.8% stratified, 2.9% adaptive"
and concluded that no run was clean. The conclusion was right; the table was only half the data. Those
numbers were the mean suspicious volume at the *top* of the ladder. Computed the same way from the same
JSON at every budget, the first run was far worse than this file admitted. Mean suspicious volume over
the 18 control campaigns at each budget:

| budget | 40 | 80 | 160 | 320 | 640 |
|---|---|---|---|---|---|
| first run | 38.9% | 46.5% | 45.1% | 12.0% | 2.55% |
| second run | 0.22% | 0.82% | 0.78% | 1.22% | 2.30% |
| current (corroboration) | 0.07% | 0.02% | 0.09% | 0.11% | **0.15%** |

That a table in this file understated the problem is a documentation failure, not a measurement one:
the numbers were in the JSON the file claims to explain, and the summary was read from the interesting
row instead of the whole column.

Where the controls are now:

- **73 of 90 control campaigns report no suspicion at all** — 10 of 90 in the first run, 29 of 90 in
  the second.
- **7 of 18 control sweeps are clean at every budget in the ladder.** This was the headline bad number
  in the first version of this file, "not one run reported zero suspicion anywhere", and it is now
  false in the good direction.
- **11 of 18 still flag something at some budget**, and what remains is 0.39% of the domain at a time —
  a cell or two in an atlas of dozens.

What those remaining flags are made of, from `aporia-bench explain`: cells where two channels did
agree, or where a rule really did fire at a point the ground truth calls correct. The rule that
addresses the rest is now in place — `Policy::suspicious_channels = 2`, so one channel's opinion holds
a cell at UNKNOWN rather than making it SUSPICIOUS, unless an absolute fact fired there — and what it
did *not* remove is the boundary of what this build can decide without more evidence. It is reported
as that, rather than tuned further.

The cost of the rule is ignorance, and it is visible: on the controls at the top of the ladder the map
is now roughly half TRUSTED and half UNKNOWN (corpus-wide at 640, adaptive `trusted 0.603, unknown
0.199`), because cells whose only claim was one loud channel are neither suspicious nor trusted. A
Trust Atlas with three labels has to be allowed to use the third one.

## Corpus-wide volume and unjustified suspicion

Mean over the 60 runs at the top of the ladder, per strategy:

| | suspicious volume | of which unjustified | control suspicious |
|---|---|---|---|
| first run — adaptive | 0.0211 | 0.580 | 0.0292 |
| first run — stratified | 0.0209 | 0.613 | 0.0271 |
| first run — random | 0.0176 | 0.599 | 0.0202 |
| second run — adaptive | 0.2163 | 0.451 | 0.0247 |
| second run — stratified | 0.2366 | 0.457 | 0.0254 |
| second run — random | 0.2157 | 0.437 | 0.0189 |
| current — adaptive | 0.1982 | 0.341 | 0.0013 |
| current — stratified | 0.2110 | 0.308 | 0.0000 |
| current — random | 0.1980 | 0.354 | 0.0033 |

Suspicious volume rose an order of magnitude between the first and second runs while the
*unjustified* share fell, and that is not a contradiction: in the first run most cells that should
have been suspicious were not labelable at all (`trusted_volume: 0`, everything UNKNOWN), and the
suspicious volume that did exist was slivers produced by a refinement bug. 0.021 of a broken atlas is
not a better false-positive rate than 0.216 of a working one. The columns that matter are the third
and the fourth, and both improved again under corroboration.

## Boundary precision

`boundaries` is measured against the declared transition in each entry's `truth.json`, normalised by
the axis width, and only exists where the atlas produced a band — two labelled cells that disagree.

| | first run | second run | current |
|---|---|---|---|
| boundary-checked rows | 810 | 810 | 810 |
| rows with at least one band | 585 | **627** | 579 |
| bands inside the declared tolerance | 165 | **247** | 238 |
| median normalised error | 0.0746 | 0.1044 | 0.1044 |

Band coverage rose between the first and second runs because TRUSTED became reachable at all: a band
needs two labelled cells, and in the first run many cells could never be labelled trusted, so the
boundary of a known square root reported `"error": "no band on this axis"`. Corroboration gives back
part of that (579 rows, 238 inside tolerance), because cells that used to be suspicious on one
channel's word are now UNKNOWN and an UNKNOWN cell forms no band. The median error did not move, at
0.1044 of the axis width in both later runs — coarse, and the price of an axis-aligned partition with
a `min_samples` floor. All of these movements are reported, including the ones worse than the
previous build. A precise-looking 0.0746 measured on an atlas that could not label anything is not a
better result than a coarse 0.1044 measured on one that can.

## Where localisation still stops working

Three distinct limits, and they are not the same problem.

- **Sample density.** `synthetic/tenth_pct_3d` is a well of half-width 0.0464 on each of three axes, so
  its volume is 1.0e-4 of the domain and 640 evaluations *expect* 0.064 hits inside it — fewer than one
  run in fifteen touches the fault at all, whatever the strategy. It detected in 9 of 9 sweeps in the
  first run and in 0 of 9 now. Those detections were noise: cells flagged by the elasticity artifact
  and the invented monotonicity claim, tinting so much of the space — up to 87.5% of a control's
  domain at a budget of 80 — that one of the resulting slivers brushed the true box. This is the
  clearest example in the corpus of a "detection" that was never a detection, and it accounts for most
  of the drop from 153 to 134.
- **Partition resolution.** `analytic/reciprocal_bound` is 2e-6 wide and `narrow_1d_unreachable` is
  2e-6 across; `Policy::max_depth` is 12, so a unit axis cannot be narrower than about 2.4e-4. They
  are beyond what the representation expresses at any budget in the ladder, and they stay in the
  corpus so that fact is visible rather than rhetorically buried.
- **Curvature.** `ode/euler_decay_2d`'s boundary is the hyperbola `k*dt = 1`, declared as 24
  rectangular slices each a strict subset of the true region. Detection succeeds at 40 on every seed
  and localisation never does: the cost of an axis-aligned partition against a curved transition, paid
  in cells.

## Counterexamples, replay, and the cost of finding

- **Replay: 60 of 60 archived runs reproduced bit-for-bit, 35,712 executions, zero mismatches**, in all
  three full runs. Every archive was written by the harness at the largest budget for seed 1, opened
  with `Loaded::open`, checked against its manifest digests, and re-executed against its own recorded
  A-IR; outputs, traces, raised flags and instruction-step counts are compared as bit patterns. This
  is the one number in this file with no asterisk on it.
- **Minimisation produced a verified smaller description for 16 of 20 entries** (14 in the first run,
  15 in the second) on 287 of 341 individual findings. The count of attempts fell when findings were
  merged, because there are fewer claims to minimise, and the verified share held at 84.2% (88.8%
  before merging). The four entries that never produce one still fail for the reason recorded in
  decision 0012: the counterexample oracle asks whether a *declared rule* fails, and a finding
  produced only by the measurement channels violates no rule, so ddmin has nothing to preserve. Those
  findings need a risk-threshold oracle with a frozen calibrator, which the harness does not wire up
  yet. That is open work, not a result.
- **Duplicate discovery rate: 0.447 → 0.625 → 0.677 → 0.469**, averaged over the runs at the top of the
  ladder that have a declared region and at least one finding (162 rows in the first run, 149, 148 and
  148 in the later ones). The metric counts a finding as a duplicate when it is assigned to a region
  the report has already claimed (`metrics.rs::duplicate_rate`). It rose with the labelling fixes —
  one region now produces many findings where it produced a few narrow ones — and the rise was then
  partly undone by `merge_findings`, which folds suspicious cells into one claim when they carry the
  same loud claims and their boxes abut into a box. Mean findings at the top of the ladder went
  53.0 → 80.2 → 73.5 → **15.4** for the same detections: nothing about the atlas changed (suspicious
  volume, band coverage, detection and localisation counts are identical to the previous run), only
  how many sentences the report writes about it. A merged finding still names every constituent cell
  and sums their samples, so the claim can be checked against the evaluations it was built from.
  The remaining 0.469 is region-level duplication across cells that do not abut, which a box merge
  cannot reach; the honest next step is reporting per region rather than per connected component.
- **Compute cost should be read from `instruction_steps`, not `wall_ms`.** `instruction_steps` is
  deterministic and identical across runs for a given entry, strategy, seed and budget (640 evaluations
  of `sqrt_domain` is 640 steps; of `naive_euler_spring`, 525,440). `wall_ms` is not: the same
  evaluations with the same finding counts took 58-70 ms per campaign in the second measurement and
  7-9 ms in the current one, and a third run on the current build (`results-1791126020.json`)
  reproduced the lower figure. Nothing in the change accounts for eightfold on identical work shapes —
  the label rule does reduce refinement, but `sqrt_domain`'s atlas size and finding count are
  unchanged between the two. It is reported as measured with the cause not established, which is
  exactly why this file quotes steps whenever it needs a cost.

## Reading this table later

`aporia-bench run` regenerates the JSON, and every archive it writes replays. The corpus verifies
before any measurement runs — `run` refuses to produce numbers when a declared region does not hold
against direct evaluation of the model's own rules — so a change in these columns means the method
changed, not that a ground truth drifted. The reasoning behind the changes between the runs is in
`docs/decisions/0014`, `0015` and `0016` (internal), and the code for each claim is in the commit
named against its file in the table at the top of this one.
