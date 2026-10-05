# Measurements

This directory holds the output of `aporia-bench run`, and this file explains what those numbers say —
including where they say the method does not work, and where an earlier version of this file said
something the data does not support.

Twelve files are committed. Each one is the record of what a change did, and only the last is current.

| file | what it is |
|---|---|
| `results-1791114389.json` | the first measurement, from the pipeline as of `7b9e777` |
| `results-1791123062.json` | after the evidence-semantics and atlas-labelling work in `8d188e6` |
| `results-1791125825.json` | after the `suspicious_channels` corroboration rule |
| `results-1791126020.json` | a single-budget repeat of the corroboration build (`--budgets 640` only), kept because the timing note near the end of this file rests on it |
| `results-1791127362.json` | after findings describing one region were merged into one claim |
| `results-1791142780.json` | differential channel wired, reference evaluations charged, no noise floor |
| `results-1791144581.json` | as above with a `1e-13` differential floor — **byte-identical to the previous row**, which is what exposed the floor as a no-op |
| `results-1791145006.json` | as above with the floor at `1e-9` — again byte-identical, so the floor is not what moved the controls |
| `results-1791146048.json` | differential wired and floored, ladder run at `differential_every = 0` |
| `results-1791150251.json` | declared symmetry wired as swap probes, plus a new symmetry control. Every one of the previous run's 180 sweeps reproduced outcome-for-outcome (see "Wiring declared symmetry") |
| `results-1791153844.json` | the risk-threshold counterexample oracle wired — the run whose figures are quoted throughout. Across all 189 sweeps the only measured field that differs from the previous run is `counterexamples` (see "The risk-threshold oracle") |
| `results-1791158752.json` | **current**: the campaign rewired onto an `Executor` seam. Every measured field of all 945 campaigns is identical to the row above, `wall_ms` except — kept because "this refactor changed no measurement" is a claim that needs its own run rather than an assertion |

Every `plan` block now records the rates that cost evaluations (`probe_every`, `numerical_every`,
`differential_every`, `symmetric_every`, `refine_every`, `calibrate_every`): a measurement whose
sampling costs are not recorded cannot be compared against one that ran under different ones. The
figures quoted throughout this file are from `results-1791153844.json`, the current run; it differs
from `results-1791150251.json` in the counterexample rows and nothing else, and from
`results-1791146048.json` in those rows plus the one corpus entry the symmetry control added. The
older rows are evidence about what each change did, not separate citable results.

### The differential channel, and a sampling lesson

Wiring the fifth channel — the runtime against the independent double-double reference — produced two
findings worth keeping.

A guard bug of my own. The channel's first noise floor, 1e-13, sat *below* the 1e-12 clamp in
`Calibrator::fit`, so it could not change anything, and that stayed invisible until two full ladder
runs came out identical to the last digit. `MIN_SCALE` is now a named constant, and a test rejects any
declared floor sitting between zero and the guard.

And a result about how this project should read its own numbers. Turning the channel on moved control
cleanliness from 73 of 90 suspicion-free campaigns to 49 of 90, and it did **not** do so through its
evidence: differential items on the controls sit near 1e-13 and calibrate to strength 0.000 at any
floor. The A/B, same seeds, same budgets, only the rate changing, 9 sweeps per control:

| rate | spring sweeps clean at some budget | projectile sweeps clean at some budget |
|---|---|---|
| 0 | 3 of 9 | 7 of 9 |
| 11 | 0 of 9 | 2 of 9 |

The mechanism is the budget. Every reference evaluation is charged, so the same seed walks a different
sample path, and which cell holds the single loud numerical reading moves with it. Control
cleanliness in this build is sensitive to sampling trajectory, not only to evidence semantics: the
flags are 0.39% of a domain — one cell — and one cell moves when the points move.

The ladder default is therefore `differential_every = 0`, which is why the current figures reproduce
the pre-channel row exactly (134 detecting and 97 localising sweeps, 579 boundary rows with a band,
238 inside tolerance, 60 of 60 archives replaying 35,712 executions). The channel is wired, floored
and demonstrated — at rate 11 on `linear_algebra/quadratic_small_root` it reports four real
implementation disagreements, worst `rel=1.111e-1` on the small root, the cancellation that entry
exists to contain — and `--differential-every` lets anyone measure its cost in one command. What has
not been settled is whether it earns that budget: at rate 11 it bought +3 localising sweeps, cost −1
detecting sweep, and carried the control-cleanliness cost above. That stays open, and recorded as
open, rather than being resolved by choosing the rate that flatters the current tables.

The first three were produced by the same command:

```
aporia-bench run --budgets 40,80,160,320,640 --seeds 1,2,3
```

on 2026-10-04 on an Intel Core Ultra 5 125H, Rust 1.99.0 release build, no GPU involved.
20 corpus entries × 3 strategies × 3 seeds × 5 budgets = 900 campaign runs, 180 sweeps.
The current run is the same command on 2026-10-05 with one more entry: 21 swept entries (22
registered, minus `mutants/unit_mistake`, which is caught before anything executes) × 3 × 3 × 5 = 945
campaign runs, 189 sweeps.

### Wiring declared symmetry

`check symmetric(y wrt (a, b))` lowered into a `RelationKind::Symmetric`, and `swap_distance` could
compare two executions, and no campaign ever produced the second execution: `Probes::swaps` was
declared, cloned forward by the search, and never appended to. A symmetry could be written into a
model and silently never tested. The campaign now re-runs a base point with the declared pair
exchanged, one evaluation per pair per base point, charged to the budget like every other probe, and
the Behavioral channel gets one evidence item per swapped pair rather than one per relation.

**What it cost the measurement: nothing, measured.** The 180 sweeps that existed before the wiring
reproduce outcome-for-outcome in `results-1791150251.json`: every field of every one of their 900
campaigns is identical — evaluations, instruction steps, detected and localised region counts,
suspicious/trusted/unknown volumes, findings, boundary rows, counterexamples — and the per-sweep
summary fields (best detecting budget, best localising budget, cleanliness, replay) match too. The one
field that differs anywhere is `wall_ms`, which this file already documents as not comparable.
Compared field by field rather than by totals, so "nothing changed" is a measurement and not an
impression: a model that declares no symmetry pays no evaluations for the machinery, so its sample
path is untouched. Headline totals for those 180 sweeps are therefore the same as the previous
row (134 detect at some budget, 97 localise; 1125 boundary rows, 813 carrying a band, 238 inside the
declared tolerance, 579 campaigns with at least one band).

**What it added**: `electromagnetics/coupled_coils`, a two-winding reactance whose declared symmetry
holds everywhere. 9 sweeps, 45 campaigns, every one of them reporting zero suspicious volume and zero
findings, trusted fraction 1.0000, clean already at the smallest budget in the ladder (40 evaluations).
Control cleanliness across the corpus goes from 73 of 90 campaigns to 118 of 135 — the two original
controls are unchanged at 73 of 90, and the new one is clean in all 45 of its campaigns. Archived
replay goes from 60 of 60 archives and 35,712 executions to 63 of 63 and 37,506.

Corpus-wide means at the top of the ladder move, and only because the denominator gained a clean
entry: suspicious volume adaptive 0.1982 → 0.1888, stratified 0.2110 → 0.2009, random 0.1980 → 0.1886
(63 runs averaged instead of 60 — multiply the old mean by 60/63 and you get the new one), and the
mean suspicious volume of the control campaigns at budget 640 from 0.15% of a domain to 0.10% (27
campaigns instead of 18). The unjustified-suspicion column moves the same way (0.341 → 0.324,
0.308 → 0.292, 0.354 → 0.335) and for a second reason worth knowing: a control with nothing suspicious
reports `false_positive_fraction = Some(0.0)` while a non-control run with nothing suspicious reports
null, so the new entry's zeros are averaged in and the earlier ones are not. That is a property of how
the metric is defined, not evidence that suspicion got better.

**A finding about the swap itself.** The declared symmetry in `coupled_coils` is mathematically exact,
but the swap compares two *executions*, and `0.3 * turns_a * turns_b` is left-associated: the base
point evaluates `(0.3*Na)*Nb` and the swap evaluates `(0.3*Nb)*Na`, which round differently. Measured
on that same model as a campaign at a budget of 640 with the library's default rates, 159 of the 160
swaps came back bit-identical and one came back 1.1677e-16 apart — a last-ulp artifact of the
evaluator, pinned by the test
`round_off_in_a_true_symmetry_is_measured_and_stays_round_off`. The corpus entry is deliberately left
in that form rather than parenthesised to `0.3 * (turns_a * turns_b)`, because the point of a control
is to be tested, not arranged. Consequence worth recording: under this ladder's own rates the entry
records 256 swaps, and a single last-ulp reading among them is enough to make the Behavioral
channel's fitted scale leave its default and land on the `MIN_SCALE` guard (1e-12), which is what
`explain` prints as `B=0.000000000001` for this entry. The atlas is unaffected — 0 findings, 1.0000
trusted — because the item is calibrated against a scale two orders above it and one channel cannot
corroborate anything.
What has *not* been settled is whether a channel-wide scale pinned at the guard is the right reference
for a different Behavioral claim in the same campaign that has fewer than eight of its own
measurements; that is a calibration question, and this unit did not touch calibration.

**A corpus gap the wiring exposed again.** The natural pair for this control is a mutant that breaks a
declared symmetry, and it cannot be scored: `corpus::violates` decides ground truth from the model's
own rules and divergence, so a fault that exists only in a measurement channel has no declared region
and the ground-truth gate refuses the entry. The mutant was withdrawn rather than the oracle stretched
around it. It is the second symptom of the missing risk-threshold counterexample oracle recorded in
"Counterexamples, replay, and the cost of finding" above.

`aporia-bench explain` now prints what the campaign actually executed:
`probes: 86 perturbation pairs, 256 symmetry swaps` on `coupled_coils`, and `0 symmetry swaps` on a
model that declares none.

### The risk-threshold oracle

Minimising a counterexample needs a predicate that still answers "is this a failure?" as the case
shrinks. There was one predicate, `FailureOracle`, and it asks *does a declared rule fail here*. For
a finding made by the measurement channels — a cell flagged because an output moves twenty-five times
further than usual along an axis — no rule fails anywhere, so ddmin had no criterion to preserve and
the harness recorded no smaller description. Those 54 rows of 341 sat in nine entries, and three
entries never verified at all: `control/symplectic_spring`, `aerospace/projectile_clean` and
`aerospace/projectile_zero_gravity`.

`aporia-bench::risk::RiskScorer` is the second predicate: the campaign's own evidence model, **frozen**
— its fitted `Calibrator`, its measured `ChannelCorrelation`, its per-(output, axis) median slopes, and
`Policy::suspicious_mean` as the bar — asked about coordinates the search never sampled. The
freezing is the design. Recomputing the ordinary slope from a candidate's own three-point star makes
every candidate typical of itself (ratio 1, channel silent), and refitting the calibrator on the
candidates would let the reduction grade itself by a standard the report never used.

It is deliberately narrower than the report, and says so. Four channels are measurable at a point:
Physical (the model's rules and its divergence), Sensitivity (a probe slope against the frozen
median), Numerical (f64 against f32 at the same coordinates) and Differential (the runtime against the
independent reference). The Behavioral channel is not in the oracle at all, because a declared
relation — monotonicity, a scaling exponent, a symmetry — is judged over the whole record set, and
rebuilding it from one step would produce a *louder* claim than the report made. A finding whose risk
comes only from a relation is not minimisable here; there is a test that asserts the oracle refuses
rather than improvises.

What was measured before trusting it:

- **It reproduces the report at the flagged points.** On three corpus campaigns, every finding's own
  representative scores identically under the frozen scorer and under the report: 1.000 → 1.000,
  0.997 → 0.997, 0.740 → 0.740, 0.998 → 0.998. The fallback is only used for a finding the scorer
  reproduces (`agrees_with`), and only after the rule oracle has failed.
- **A scorer given *every* sensor over-flags, so it is not given every sensor.** The first version
  judged a candidate with all the channels the campaign ran anywhere. On `control/symplectic_spring`
  it flagged 34 of 200 recorded points where the report flagged 4 — because the report probes every
  seventh round and compares precisions every eleventh, so most of its own records were never measured
  that way at all. Over-flagging is the unsafe direction: ddmin would verify a reduction at coordinates
  the atlas would not have flagged. Now a scorer is built per finding and restricted to the channels
  that finding was actually made of.
- **A verified reduction is a claim about the failure's shape, not a zoom into the flagged cell.**
  Measured on `rlc_resonance`, a Physical-driven finding's reduced case pinned axis 0 at 1.29898 while
  the labelled cell spanned [2.039219, 2.041658]. That is what dropping a parameter *means*: the
  failure does not depend on it, and the witness set for a dropped axis is the whole declared domain.
  Every witness of the reduced case is re-checked by the same oracle, so the row is verifiable — but it
  is not "the same region, smaller", and an earlier version of this file would have implied that.
- **100% verified does not mean 100% shrunk.** Of the 54 rows now attributed to the risk oracle, 15
  drop a parameter outright; the other 39 reduce span or significant digits (descriptions like
  `x in [0.4181, 0.5818]` against a full-precision pinned start). None is the untouched starting case.
  `dimensions`, `digits` and `description` are in every row, so this can be re-checked rather than
  taken on faith.
- **Cost is reported, not hidden — but read it per oracle.** A row that needed both predicates records
  `minimisation_evaluations` for the rule attempt *and* the risk attempt together, and
  `oracle: "rule" | "risk"` says which question the row answers. The two answers are not the same
  claim: a rule failure says the model is wrong at those coordinates; a risk hit says the instrument
  would still flag them. On a control, that distinction is the entire content of the measurement.
- **That summed number counts queries, not executions, and the two oracles differ per query.**
  `FailureOracle` asks one question and runs the model once. `RiskScorer` rebuilds the evidence the
  report used, so one query runs the model at the candidate point, once per axis with that axis
  perturbed, once again at reduced precision, and once through the double-double reference —
  `arity + 3` executions. So a `"risk"` row's `minimisation_evaluations` is a lower bound on what it
  cost, by that factor, and a `"rule"` row's is exact. Stated rather than left to be assumed: the
  numbers in this column are comparable within an oracle kind and not across kinds. Closing the gap
  means having the oracle report its own executions and re-measuring the column, which is open.

Everything else in the ladder is untouched: comparing the new run against the previous one sweep by
sweep, all 189 sweeps and all 945 campaigns agree on every measured field except `counterexamples`
(and `wall_ms`, which is not comparable). Detections 134, localisations 97, control cleanliness 118 of
135, boundary rows 1125 with 813 banded and 238 inside tolerance, 63 of 63 archives replaying 37,506
executions — identical.

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
| `electromagnetics/coupled_coils` | none (control, declared symmetry) | nothing | nothing | nothing | 4 |

Across all 180 sweeps: **134 detect at some budget, 97 localise.** The first run was 153 and 92, the
second 140 and 97. The localisation count held exactly where the corroboration rule was applied, and
six sweeps of detection were given up — "Where localisation still stops working" explains why those
six were never detections. The current run measures 189 sweeps and the same two numbers: the nine
added sweeps are the `coupled_coils` control, which detects nothing because nothing is wrong with it.

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
the control campaigns at each budget (18 per row until the third control arrived):

| budget | 40 | 80 | 160 | 320 | 640 |
|---|---|---|---|---|---|
| first run | 38.9% | 46.5% | 45.1% | 12.0% | 2.55% |
| second run | 0.22% | 0.82% | 0.78% | 1.22% | 2.30% |
| current (corroboration) | 0.07% | 0.02% | 0.09% | 0.11% | **0.15%** |
| symmetry run (three controls) | 0.04% | 0.01% | 0.06% | 0.07% | **0.10%** |

The last row is the same measurement with `electromagnetics/coupled_coils` added, so it averages 27
control campaigns per budget instead of 18; the two original controls are unchanged campaign for
campaign.

That a table in this file understated the problem is a documentation failure, not a measurement one:
the numbers were in the JSON the file claims to explain, and the summary was read from the interesting
row instead of the whole column.

Where the controls are now:

- **73 of 90 control campaigns report no suspicion at all** — 10 of 90 in the first run, 29 of 90 in
  the second. Measured the same way over three controls it is 118 of 135: `electromagnetics/coupled_coils`
  is clean in all 45 of its campaigns.
- **7 of 18 control sweeps are clean at every budget in the ladder.** This was the headline bad number
  in the first version of this file, "not one run reported zero suspicion anywhere", and it is now
  false in the good direction. Across the three controls it is 16 of 27.
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

Mean over the runs at the top of the ladder, per strategy (60 runs per row until the third control,
63 in the last three):

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
| symmetry run — adaptive | 0.1888 | 0.324 | 0.0009 |
| symmetry run — stratified | 0.2009 | 0.292 | 0.0000 |
| symmetry run — random | 0.1886 | 0.335 | 0.0022 |

The last three rows are the same campaigns as the previous three plus one clean control, so their
means are lower for arithmetic reasons rather than behavioural ones: multiply a `current` suspicious
volume by 60/63 and you get the symmetry-run figure. The unjustified column moves for that reason and
one other, recorded in the symmetry section: a clean control reports `false_positive_fraction =
Some(0.0)` while a clean non-control run reports null, so the new entry's zeros are averaged and the
old ones were not.

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

The symmetry run reproduces all four of the *current* column exactly (810, 579, 238, 0.1044): the new
entry declares no transition, so it contributes no boundary rows, and the entries that do are measured
by campaigns that did not change.

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

- **Replay: 60 of 60 archived runs reproduced bit-for-bit, 35,712 executions, zero mismatches**, in the
  three 900-campaign runs, and 63 of 63 with 37,506 executions in the current 945-campaign run. Every
  archive was written by the harness at the largest budget for seed 1, opened
  with `Loaded::open`, checked against its manifest digests, and re-executed against its own recorded
  A-IR; outputs, traces, raised flags and instruction-step counts are compared as bit patterns. This
  is the one number in this file with no asterisk on it. The three new archives are the symmetry
  control's, so the swapped executions are inside what replay re-checks.
- **Minimisation produced a verified smaller description for 287 of 341 individual findings across the
  three 20-entry runs** (14 entries in the first, 15, then 16) — and for **341 of 341 in the current
  run**, with all 19 entries that produced findings represented. The count of attempts fell when
  findings were merged, because there are fewer claims to minimise, and the verified share held at
  84.2% (88.8% before merging) until the risk-threshold oracle landed. The entries that never produced
  one failed for the reason recorded in decision 0012: the counterexample oracle asked whether a
  *declared rule* fails, and a finding produced only by the measurement channels violates no rule, so
  ddmin had nothing to preserve. That is now a second oracle rather than an open hole — see "The
  risk-threshold oracle" below, including what its 100% does and does not prove.
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

## What the Behavioral channel's scale is actually made of

`coupled_coils` declares `check symmetric(reactance wrt (turns_a, turns_b))` and leaves the product
un-parenthesised on purpose: the base point evaluates `(0.3*Na)*Nb` and the swap evaluates
`(0.3*Nb)*Na`, which round differently at isolated points. One such reading, at 1.1677e-16 relative,
was the only Behavioral evidence in that campaign — fewer than eight of its own measurements, so the
relation gets no stratum of its own and the channel-wide reference is what its claims are scored
against. `Calibrator::fit` clamps that reference to `MIN_SCALE` = 1e-12. The question this section
answers is whether that guard is protecting the report from the evaluator's rounding or deciding
which real violations get believed. It was answered by measuring, not by choosing a constant that
flatters a table (`aporia-bench/tests/behavioral_scale.rs`, budget 400, seed 1, every base point
swapped; five models differing only in the size of the term that breaks the symmetry).

| the model's symmetry is | swaps sampled | differing swaps | largest relative asymmetry | fitted Behavioral scale | loudest calibrated strength | peak risk | SUSPICIOUS volume | findings |
|---|---|---|---|---|---|---|---|---|
| exactly true, parenthesised | 100 | 0 | 0 | 1.000e0 (unfitted default) | 0.000 | 0.000 | 0.0000 | 0 |
| exactly true, re-associated | 100 | 0 | 0 | 1.000e0 | 0.000 | 0.000 | 0.0000 | 0 |
| violated by ~1e-14 | 100 | 74 | 6.226e-15 | **1.000e-12 (the guard)** | 0.000 | 0.000 | 0.0000 | 0 |
| violated by ~1e-10 | 100 | 97 | 6.186e-11 | 4.141e-12 (its own median) | 1.000 | 1.000 | 0.0000 | 0 |
| violated by ~1e-3 | 100 | 97 | 6.184e-4 | 4.141e-5 | 1.000 | 1.000 | 0.0000 | 0 |

**The guard is the right order of magnitude for the artifact, and no change to it is supported.** The
reassociation artifact is rare — at this seed and budget it did not appear at all — and where the
corpus has measured one it was 1.1677e-16, four orders below the 1e-12 the guard pins the scale to.

**The guard is also load-bearing in a direction this file had not stated.** The ~1e-14 row is not an
artifact: that model's declared symmetry is genuinely violated wherever the difference shows. Its own
median sits below the guard, so its stratum is clamped the same way the channel is and every item
scores zero excess. A real violation of a declared relation is invisible below roughly 1e-11 relative.
That is defensible — double precision promises little across a three-term expression — but it is now a
measured consequence of the constant rather than an assumption about it.

**Strength saturates, so the dose-response is read in the magnitudes.** `1 - exp(-excess)` pins the
1e-10 and 1e-3 cases both at 1.000, differing only in the last digits. An earlier draft of this test
asserted the scores had to be strictly ordered and failed on that; it would have been testing the
exponential rather than the instrument.

**The result that matters is not about the scale.** A model whose only defect is that its declared
symmetry fails — at 97 of 100 swapped points, evidence strength saturating, peak fused risk 1.000 —
reports `suspicious 0.0000` and zero findings. A relation violation is recorded as a *measurement*, and
`Policy::suspicious_channels` requires two channels to corroborate a measurement, whereas a *fact* (a
rule that fired, an output that left the real numbers) needs none. So `require` can flag a cell alone
and `check symmetric` cannot, however often or however loudly it fails. The corroboration rule is
working as designed and this section reports its cost: a pure symmetry defect is currently something
APORIA measures and refuses to call suspicious.

Whether a violated *declaration* should count as a fact rather than a measurement is open, and it is
not settled here. The counter-evidence is this corpus's own control: `coupled_coils` is a control
precisely because its symmetry evidence must not become a finding, so promoting relations to facts
would need the promotion gated on calibrated strength — nonzero, which the guard already decides —
and measured against the full ladder before it could be claimed. Until that A/B exists, the behaviour
is pinned by test rather than changed by preference.

## Reading this table later

`aporia-bench run` regenerates the JSON, and every archive it writes replays. The corpus verifies
before any measurement runs — `run` refuses to produce numbers when a declared region does not hold
against direct evaluation of the model's own rules — so a change in these columns means the method
changed, not that a ground truth drifted. The reasoning behind the changes between the runs is in
`docs/decisions/0014`, `0015` and `0016` (internal), and the code for each claim is in the commit
named against its file in the table at the top of this one.

The files above are named for the second they finished, which made a measurement's identity a fact about
the clock: re-running one plan produced a second artefact that could not be recognised as the same
question, and "have I already measured this?" had no answer short of diffing two files by hand. New runs
are named for the measurement instead — a 12-character digest of the `plan` block and the entries it
covered, recorded inside the file as `identity`. A rerun of one plan is therefore refused rather than
duplicated (`pass --out DIR to keep both`), and a change to a budget, a strategy, a seed, an
evaluation-costing rate, the entry selection or a `truth.json` claim yields a different name, because it
is a different measurement. Where the run happened is not part of the name: that is provenance, and it
stays in the document's `environment` block.

Nothing here was renamed. The twelve timestamped files keep their names and their place in the history,
and `aporia-bench verdict <file>` prints the identity each one implies from its own recorded plan and
entries, so an old measurement can still be addressed by the experiment it was:

```
identity  e8a100bbf082  (derived from the plan and entries it records)
```
