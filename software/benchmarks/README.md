# Benchmark corpus

APORIA's claims are measurements, and a measurement needs a known answer. This corpus is the set of
answers: small scientific models written in the Aporia DSL, each with a `truth.json` that states
where the model really stops being trustworthy and why.

Nothing here is downloaded. Every model was written for this project, so the licence is the
repository's MIT and there is no attribution obligation. External families (SciMLBenchmarks,
OpenMDAO examples, NASA NTRS, NIST Matrix Market) are the intended source for later entries and are
recorded as *not yet used* rather than credited in advance.

## Layout

```
benchmarks/
  registry.json                 families, entries, fault classes covered and not covered
  protocols/<id>.json           an experiment specified in full before its corpus or its arms existed
  <family>/<name>/model.ap      the computation, in the Aporia DSL
  <family>/<name>/truth.json    the ground-truth region, its boundaries, and the derivation
```

## Experiment protocols

A results file says what was measured. A protocol file says what was *going* to be measured, and it
has to exist first: a metric chosen after the arms are read is not a metric. `protocols/` holds them:
the arms, the entry list, the budgets, the seeds, the rates, the deciding statistic and the decision
rule, written down before the corpus they name exists.

The runner reads them, which is what makes them more than a note. `aporia-bench run --plan
protocols/<id>.json --arm <name>` takes its whole definition from the file, refuses any command-line
flag that would shape what is measured (`--budgets`, `--seeds`, `--only`, `--ablate`, the sampling
rates), requires the arm to be named rather than guessed, and writes the protocol id and arm into the
results document. A protocol file with no `primary_metric` is refused outright: without a written
decider it is a plan, not a pre-registration.

`protocols/e1-geometry.json` is E1.1/E1.2/E1.3 — whether E2's finding that one channel suffices
transfers to corpus entries whose geometry does not match the atlas's axis-aligned partition. It was
committed on 2026-10-08, and the entries its `entries` list names did not exist when it was written
and are authored against it, not the other way round. Its arms, budgets, seeds and rates are E2's,
unchanged on purpose: the only variable is the shape of the truth.

`protocols/e1-3b-trust-resolution.json` is E1.3b — whether a truth-blind tightening of the trust rule
stops `TRUSTED` covering a violated region, and what that costs in UNKNOWN area and in localisation. Its
schema is `aporia.protocol/draft-1`, deliberately: the rule variants it names need an instrument
dimension that does not exist yet, and its `entries` section describes sets rather than naming files, so
a runner asked to use it refuses it by schema name. A freeze that could be run early is a freeze that can
be quietly amended by the first run; this one cannot be run until the commit that adds its entries
promotes it, and that commit is not allowed to contain a measurement.

## What a `truth.json` says

`regions` is a union of axis-aligned boxes in the model's own parameter names. A point inside one of
them is a point where the model is genuinely wrong, by the derivation quoted in the file. Boxes were
chosen over an arbitrary predicate because a box is checkable: `aporia-bench verify` evaluates the
model on a dense grid and refuses a corpus entry whose declared boxes do not match what the model
actually does. The declared boundary values come next to them, with a tolerance, which is what the
boundary-precision metric is measured against.

Three shapes are deliberately included because each breaks something:

- **A control** (`aerospace/projectile_clean`, `control/symplectic_spring`,
  `electromagnetics/coupled_coils`) has no region at all. Anything APORIA calls suspicious there is a
  false positive, and that is the only way to find out. `coupled_coils` controls a specific sensor: its
  `check symmetric(reactance wrt (turns_a, turns_b))` holds mathematically everywhere, so it is the
  entry that the swap probe runs on, and it is written left-associated on purpose — the probe has to
  survive the last-ulp differences the evaluator produces (measured on this model: 159 of 160 swaps
  bit-identical, one differing by 1.1677e-16) without turning them into suspicion.
- **A narrow band** (`analytic/reciprocal_bound` at 2e-6 wide, `electromagnetics/rlc_resonance`)
  tests whether the search spends evaluations where it matters. A coarse atlas should report
  UNKNOWN here, not TRUSTED.
- **A curved boundary** (`ode/euler_decay_2d`, where instability needs `k*dt > 1`) is a hyperbola that
  an axis-aligned partition can only stair-step. The corpus declares the inner approximation as
  boxes and says so, so the reported cell count is the price of that representation rather than a
  mystery.

## Fault classes

The specification's fault library (§20) is the checklist. Covered today: sign error, boundary
condition, unit mistake, cancellation, time-step sensitivity, state-update ordering, overflow,
incorrect domain assumption. Not covered yet: incorrect constant, incorrect stopping condition,
normalisation, scalar/SIMD and CPU/GPU divergence — the last two need the CUDA backend, which
cannot be built or measured on this machine.

`mutants/unit_mistake` is the interesting one of the bunch, because the correct answer is that
APORIA never executes it: the unit checker rejects `km^2/h^2 / (m/s^2)` compared against a bare zero
at compile time. A fault class that becomes a type error should not be counted as a detection
success by a runtime search.

## Reproducing a run

`aporia-bench` writes an experiment archive for every run (see `software/crates/aporia-store`), and
the recorded numbers in `results/` came from those archives, so a claim in this repository can be
replayed rather than trusted.
