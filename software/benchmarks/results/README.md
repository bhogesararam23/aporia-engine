# Measurements

This directory holds the output of `aporia-bench run`, and this file explains what those numbers
say — including where they say the method does not work.

The run recorded here:

```
aporia-bench run --budgets 40,80,160,320,640 --seeds 1,2,3
```

on 2026-10-04, on an Intel Core Ultra 5 125H, Rust 1.99.0 release build, no GPU involved. 20 corpus
entries × 3 strategies × 3 seeds × 5 budgets = 900 campaign runs, 180 sweeps. `results-*.json`
holds every per-budget measurement; the numbers quoted below are read out of it.

Definitions live in `crates/aporia-bench/src/metrics.rs`, next to the code that computes them. The
two that carry the weight here:

- **detected** — some suspicious cell touches a declared region. Loose on purpose.
- **localised** — a suspicious cell touches a declared region *and* is at least half inside
  declared regions. A big lazy cell cannot pass it.

## What happened

| entry | region volume | adaptive | stratified | random |
|---|---|---|---|---|
| `analytic/sqrt_domain` | 50% | 40 | 40 | 40 |
| `analytic/log_positive` | 20% | 40 | 40 | 40 |
| `analytic/exp_overflow` | 29% | 40 | 40 | 40 |
| `ode/euler_decay` | 17% | 40 | 40 | 40 |
| `control/naive_euler_spring` | 79% | 40 | 40 | 40 |
| `aerospace/projectile_sign_mutant` | 50% | 40 | 40 | 40 |
| `synthetic/wide_1d` | 40% | 80 | 80 | **40** |
| `linear_algebra/quadratic_small_root` | 99% | 80 | 160 | **40** |
| `synthetic/quarter_2d` | 25% | 320 | 320 | 320 |
| `synthetic/narrow_1d` | 4% | **320** | never | 640 |
| `electromagnetics/rlc_resonance` | 0.088% | **640** | never | never |
| `synthetic/one_pct_2d` | 1% | 640 | 640 | 640 (1 of 3 seeds) |
| `synthetic/one_pct_3d` | 2% | never | never | never |
| `synthetic/tenth_pct_3d` | 0.1% | never | never | never |
| `ode/euler_decay_2d` | curved, ≈27% | never | never | never |
| `analytic/reciprocal_bound` | 2e-6 | never | never | never |
| `aerospace/projectile_zero_gravity` | measure zero | never | never | never |
| `synthetic/narrow_1d_unreachable` | 1e-6 | never | never | never |

Budgets are model evaluations including probe evaluations, so the columns are directly comparable.
`never` means "not within 640 evaluations", not "never at any budget".

**Two entries adaptive won, two it lost, and seven were decided at the smallest budget by all three
strategies.** On `synthetic/narrow_1d` (a well 4% of the line) adaptive localised on 2 of 3 seeds
where stratified never did. On `electromagnetics/rlc_resonance` — a resonance band 0.088% of the
domain — adaptive localised at 640 and neither baseline localised at any budget in the ladder. On
`synthetic/wide_1d` and `linear_algebra/quadratic_small_root`, where the region is most of the
space, plain random coverage won at 40 evaluations while adaptive spent the same budget probing and
refining and got there at 80.

That is the shape one should expect: concentrating effort pays when the thing to find is small and
does not pay when it is everywhere. The first two rows of the second half of the table are the cases
where the method was supposed to show an advantage, and it did; the losses are the cases where a
baseline is the right tool. Neither result is a reason to stop, and neither is a reason to claim
victory.

## The result that matters most is a bad one

Neither control stayed clean.

| control | suspicious volume, as a fraction of the space |
|---|---|
| `aerospace/projectile_clean` | 0.9% random, 2.8% stratified, 2.9% adaptive |
| `control/symplectic_spring` | 2.6% to 3.1% |

Across all 180 runs at every budget, **not one run reported zero suspicion anywhere**.

Both models are correct everywhere inside their declared domain — the range of a projectile with
gravity bounded away from zero, and a symplectic integrator whose energy stays bounded. Every cubic
unit of suspicion the atlas reported in those two entries is a false positive: by the definition in
`metrics.rs`, both controls scored a **100% false-positive rate**, and the mean suspicious volume
over the three strategies and three seeds is between 0.9% and 3.1% of the parameter space.

The cause is visible in the per-run diagnostics (`aporia-bench explain <entry>`): the behavioural
channel's continuity measure reports a large secant-slope ratio wherever the *sample spacing*
happens to be coarse, and after calibration by excess over typical, a channel whose typical value
is small amplifies ordinary variation into strength 1.0. A previous iteration of this code made that
worse in two ways that are now fixed and measured — a numerical channel comparing f32 against f64 had
its typical value at round-off level, and a continuity finding was attributed to every observation
the experiment had probed rather than to the two samples that showed it. Fixing both moved the top findings of `electromagnetics/rlc_resonance` from cells
around w = 1.55 to cells at w = 1.988 and w = 2.0002, which straddle the true resonance band, and
that change is what made the entry localise at all. Suspicious volume across the corpus did not
fall: averaged over the 60 runs at the largest budget it is 0.0211 adaptive, 0.0209 stratified,
0.0176 random, so adaptive is 16% *worse* than random on unjustified suspicion. The controls are
still not clean.

What is left is the honest open problem: **a trusted cell has to be earnable**, and right now the
policy lets a single amplified behavioural measurement make a cell suspicious in a model that
declares nothing false. Until that is fixed, the useful reading of a Trust Atlas from this build is
"suspicious means *look here*", and the count of suspicious cells is a cost, not a conclusion.

## Where localisation stops working

Three distinct limits show up, and they are not the same problem:

- **Sample density.** A 0.1% box in three dimensions is hit by roughly one evaluation in a
  thousand; at a 640 budget no strategy localises it, and detection (a cell merely touching it)
  still succeeds at 80. That is a sampling fact, not a representation fact.
- **Partition resolution.** `analytic/reciprocal_bound` is 2e-6 wide and
  `synthetic/narrow_1d_unreachable` is 2e-6 across. `Policy::max_depth` is 12, so a unit axis
  cannot be narrower than about 2.4e-4: these entries are beyond what the representation can
  express at any budget, and they are in the corpus to keep that visible rather than rhetorically
  buried.
- **Curvature.** `ode/euler_decay_2d`'s boundary is the hyperbola `k*dt = 1`, declared as 24
  rectangular slices, each a strict subset of the true region. Detection succeeds at 40 and
  localisation never does, which is the cost of an axis-aligned partition against a curved
  transition, paid in cells. It is reported here because §12's promise that resolution follows
  evidence has a shape it cannot follow.

## Counterexamples and replay

- **Replay: 60 of 60 archived runs reproduced bit-for-bit, 35,712 executions, zero mismatches.**
  Every archive was written by the harness at the largest budget for seed 1, opened with
  `Loaded::open`, checked against its manifest digests, and re-executed against its own recorded
  A-IR; outputs, traces, raised flags and instruction-step counts are compared as bit patterns.
  This is the one number in this file with no asterisk on it.
- **Minimisation produced a verified smaller description for 14 of 20 entries**, 220 of 538
  individual findings. `analytic/sqrt_domain` reduced to one parameter
  and four digits in 125 oracle calls; `synthetic/one_pct_2d` to two parameters and 22 digits in
  507; `ode/euler_decay_2d` to two parameters and 8 digits in 175. The other six entries never produced one, and the individual refusals reported
  `verified: no` after a single oracle call, which is not a minimiser failure: the counterexample
  oracle asks the model whether its *declared rule* fails, and a finding produced by the behavioural
  or sensitivity channels violates no rule, so there is nothing for ddmin to preserve. Those
  findings need a risk-threshold oracle with a frozen calibrator, which the harness does not wire up
  yet. The distinction is recorded rather than smoothed over.

## Reading this table later

`aporia-bench run` regenerates the JSON, and every archive it writes replays. The corpus verifies
before any measurement runs — `run` refuses to produce numbers when a declared region does not hold
against direct evaluation of the model's own rules — so a change in these columns means the method
changed, not that a ground truth drifted.
