# Measurements

This directory holds the output of `aporia-bench run`, and this file explains what those numbers
say — including where they say the method does not work, and where an earlier version of this file
said something the data does not support.

Two runs are committed.

| file | what it is |
|---|---|
| `results-1791114389.json` | the first measurement, from the pipeline as of `7b9e777` |
| `results-1791123062.json` | the current measurement, after the evidence-semantics and atlas-labelling work in `8d188e6` |

Both were produced by the same command:

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

Best budget at which each strategy resolved, and how many of its three seeds got there. `—` means not
within 640 evaluations, not "never at any budget". Median column is wall time at the top of the
ladder, which is the compute-cost figure.

| entry | fault | adaptive | stratified | random | med ms |
|---|---|---|---|---|---|
| `analytic/sqrt_domain` | domain assumption | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 60 |
| `analytic/log_positive` | domain assumption | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 35 |
| `analytic/exp_overflow` | overflow | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 34 |
| `analytic/reciprocal_bound` | overflow | det 40, never loc | det 40, never loc | det 80, never loc | 28 |
| `ode/euler_decay` | time step | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 92 |
| `ode/euler_decay_2d` | time step, curved | det 40, never loc | det 40, never loc | det 40, never loc | 131 |
| `control/naive_euler_spring` | state ordering | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 238 |
| `linear_algebra/quadratic_small_root` | cancellation | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 46 |
| `aerospace/projectile_sign_mutant` | sign error | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 69 |
| `synthetic/wide_1d` | 40% of a line | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 37 |
| `synthetic/quarter_2d` | 25% of a plane | loc 40 (3/3) | loc 40 (3/3) | loc 40 (3/3) | 45 |
| `synthetic/narrow_1d` | 4% of a line | loc 320 (3/3) | loc **160** (3/3) | loc **160** (3/3) | 21 |
| `synthetic/one_pct_2d` | 1% of a plane | loc 640 (1/3) | never loc, det 160 | loc 320 (1/3) | 25 |
| `synthetic/one_pct_3d` | 2% of a cube | loc 640 (1/3) | loc 640 (3/3) | never loc, det 80 | 28 |
| `electromagnetics/rlc_resonance` | 0.088% band | loc 640 (1/3) | never loc, det 40 | never loc, det 320 | 39 |
| `synthetic/tenth_pct_3d` | 0.1% of a cube | nothing | nothing | nothing | 25 |
| `synthetic/narrow_1d_unreachable` | 1e-6 of a line | det 40, never loc | det 40, never loc | nothing | 26 |
| `aerospace/projectile_zero_gravity` | measure-zero region | nothing | nothing | nothing | 58 |
| `control/symplectic_spring` | none (control) | nothing, as it should be | — | — | 171 |
| `aerospace/projectile_clean` | none (control) | nothing, as it should be | — | — | 61 |

Across all 180 sweeps: **140 detect at some budget, 97 localise.** In the first run those were 153
and 92. The two movements mean different things and both are reported below.

## The strategy comparison got weaker, and that is the honest result

`aporia-bench verdict` now says: **adaptive better on 1 entry, worse on 2, equal or unresolved on
15.** In the first run it was 2 and 2.

What changed is that the baselines got better, not that adaptive got worse: fixing the labelling
bugs moved `synthetic/narrow_1d` from 3 resolving seeds to 9, `synthetic/one_pct_3d` from 0
localising sweeps to 4, and `synthetic/quarter_2d`'s best localising budget from 320 to 40 — and
plain coverage and stratification took those wins at the same or a smaller budget than adaptive did.
`narrow_1d` and `one_pct_3d` are now entries where stratified *beats* adaptive.

The one place adaptive still wins is the one it won before: `electromagnetics/rlc_resonance`, a
resonance band 0.088% of the domain, localised at 640 by adaptive on one seed and by neither
baseline at any budget in the ladder. The research question is about exactly that case, and one
entry is not an answer. It is a reason the ladder needs more entries of that shape, not a result to
stop on.

## The controls, and a correction to this file

The first version of this file reported the controls as "0.9% random, 2.8% stratified, 2.9%
adaptive" and concluded that no run was clean. The conclusion was right; the table was only half the
data. Those numbers were the mean suspicious volume at the *top* of the ladder. Computed the same way
from the same JSON at every budget, the first run was far worse than this file admitted:

| budget | 40 | 80 | 160 | 320 | 640 |
|---|---|---|---|---|---|
| first run, mean control suspicious volume | 38.9% | 46.5% | 45.1% | 12.0% | 2.55% |
| current run | 0.22% | 0.82% | 0.78% | 1.22% | 2.30% |

That table understating the problem is a documentation failure, not a measurement one: the numbers
were in the file this one claims to explain, and the summary was read from the interesting row
instead of the whole column.

Where the controls are now:

- **29 of 90 control campaigns report no suspicion at all** (was 10 of 90).
- **16 of 18 control sweeps are clean at at least one budget** (was 6 of 18).
- **0 of 18 are clean at every budget.** Both controls, all three strategies, all three seeds still
  flag *something* somewhere in the ladder, and the amount grows with the budget for adaptive: the
  spring goes 0.00% → 0.00% → 0.26% → 2.47% → 3.52% as the budget doubles from 40 to 640.

What the remaining flags are made of, read from `aporia-bench explain control/symplectic_spring`:
almost entirely the sensitivity channel — `o1 moved 782x further than usual along p0`. For a symplectic
integrator observed over a long horizon that is a *true* statement about that part of its step-size
range: the answer genuinely swings far more there than elsewhere. It is not a defect, and the model
violates no rule there. That is the open question this build has not answered: **a single channel's
opinion, at maximum strength, is currently enough to make a cell SUSPICIOUS.** The candidate answer
is corroboration — one channel holds a cell at UNKNOWN and appears in the report as "delicate", while
SUSPICIOUS requires two channels surviving the provenance discount, or an absolute fact such as a
fired rule or a NaN. `Policy.min_channels` is not the knob for that and must not be used as one: it
guards TRUSTED, and after the fix below it now means "channels *measured*", not "channels that spoke".

## Corpus-wide volume and unjustified suspicion

Mean over the 60 runs at the top of the ladder, per strategy:

| | suspicious volume | of which unjustified | control suspicious |
|---|---|---|---|
| first run — adaptive | 0.0211 | 0.580 | 0.0292 |
| first run — stratified | 0.0209 | 0.613 | 0.0271 |
| first run — random | 0.0176 | 0.599 | 0.0202 |
| current — adaptive | 0.2163 | 0.451 | 0.0247 |
| current — stratified | 0.2366 | 0.457 | 0.0254 |
| current — random | 0.2157 | 0.437 | 0.0189 |

Suspicious volume went up by an order of magnitude while the *unjustified* share of it went down.
That is not a contradiction and it is the important reading of this change: in the first run most
cells that should have been suspicious were not labelable at all (`trusted_volume: 0`, everything
UNKNOWN), and the suspicious volume that did exist was slivers produced by a refinement bug. 0.021
of a broken atlas is not a better false-positive rate than 0.216 of a working one; the useful column
is the third and the fourth.

## Boundary precision, which had no data before

`boundaries` is measured against the declared transition in each entry's `truth.json`, normalised by
the axis width, and only exists where the atlas produced a band — two labelled cells that disagree.

| | first run | current |
|---|---|---|
| boundary-checked rows | 810 | 810 |
| rows with at least one band | 585 | **627** |
| bands inside the declared tolerance | 165 | **247** |
| median normalised error | 0.0746 | 0.1044 |

Two of those three movements are good and one is not. Band *coverage* and in-tolerance hits went up
because TRUSTED became reachable at all: a band needs two labelled cells, and in the first run a
large number of cells could never be labelled trusted, so the boundary of a known square root
reported `"error": "no band on this axis"`. The median error got 40% *worse*, from 0.0746 to 0.1044
of the axis width, because the cells that now form bands are honest-sized rather than the drilled
slivers the refinement bug produced. A precise-looking 0.07 measured on an atlas that could not
label anything is not a better result than a coarse 0.10 measured on one that can. Reported both
ways, with the direction of each stated.

## Where localisation still stops working

Three distinct limits, and they are not the same problem.

- **Sample density.** `synthetic/tenth_pct_3d` is a well of half-width 0.0464 on each of three
  axes, so its volume is 1.0e-4 of the domain and 640 evaluations *expect* 0.064 hits inside it —
  fewer than one run in fifteen touches the fault at all, whatever the strategy. It now detects in **0 of 9**
  sweeps, where the first run detected in 9 of 9. The first run's detections were noise: cells
  flagged by the elasticity artifact and the invented monotonicity claim, tinting so much of the
  space — up to 87.5% of a control's domain at a budget of 80 — that one of the resulting slivers
  brushed the true box. That is the clearest example in the corpus of a "detection" that was never a
  detection, and the drop from 153 to 140 is mostly this entry and `narrow_1d_unreachable`.
- **Partition resolution.** `analytic/reciprocal_bound` is 2e-6 wide and `narrow_1d_unreachable` is
  2e-6 across; `Policy::max_depth` is 12, so a unit axis cannot be narrower than about 2.4e-4. They
  are beyond what the representation expresses at any budget in the ladder, and they stay in the
  corpus so that fact is visible rather than rhetorically buried.
- **Curvature.** `ode/euler_decay_2d`'s boundary is the hyperbola `k*dt = 1`, declared as 24
  rectangular slices each a strict subset of the true region. Detection succeeds at 40 on every
  seed and localisation never does: the cost of an axis-aligned partition against a curved
  transition, paid in cells.

## Counterexamples, replay, and the cost of finding

- **Replay: 60 of 60 archived runs reproduced bit-for-bit, 35,712 executions, zero mismatches**, in
  both runs. Every archive was written by the harness at the largest budget for seed 1, opened with
  `Loaded::open`, checked against its manifest digests, and re-executed against its own recorded
  A-IR; outputs, traces, raised flags and instruction-step counts are compared as bit patterns.
  This is the one number in this file with no asterisk on it.
- **Minimisation produced a verified smaller description for 15 of 20 entries** (was 14 of 20), on
  332 of 465 individual findings (was 220 of 538). The six that never produced one still fail for
  the reason recorded in `docs/decisions/0012`: the counterexample oracle asks whether a *declared
  rule* fails, and a finding produced only by the sensitivity channel violates no rule, so ddmin has
  nothing to preserve. Those findings need a risk-threshold oracle with a frozen calibrator, which
  the harness does not wire up yet. That is open work, not a result.
- **Duplicate discovery rate went up, from 0.447 to 0.625** averaged over the runs at the top of the
  ladder that have a declared region and at least one finding (162 such rows in the first run, 149 in
  the current one; the rest have no declared region or nothing to duplicate). The metric counts a
  finding as a duplicate when it is assigned to a declared region the report has already claimed
  (`metrics.rs::duplicate_rate`), so about two thirds of findings on a current run describe a fault
  that had already been described. Mean findings on those rows went 53.0 → 80.2. This is a regression
  and it follows directly from the labelling fix: cells that should be suspicious are now suspicious,
  so one region produces many findings rather than a few slivers. The number is in the JSON; the
  merge rule that would reduce it is not written yet.

## Reading this table later

`aporia-bench run` regenerates the JSON, and every archive it writes replays. The corpus verifies
before any measurement runs — `run` refuses to produce numbers when a declared region does not hold
against direct evaluation of the model's own rules — so a change in these columns means the method
changed, not that a ground truth drifted. The reasoning behind the changes between the two files is
in `docs/decisions/0014` and `0015` (internal), and the code that implements each claim is the
commit named in the table at the top of this file.
