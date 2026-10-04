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
  <family>/<name>/model.ap      the computation, in the Aporia DSL
  <family>/<name>/truth.json    the ground-truth region, its boundaries, and the derivation
```

## What a `truth.json` says

`regions` is a union of axis-aligned boxes in the model's own parameter names. A point inside one of
them is a point where the model is genuinely wrong, by the derivation quoted in the file. Boxes were
chosen over an arbitrary predicate because a box is checkable: `aporia-bench verify` evaluates the
model on a dense grid and refuses a corpus entry whose declared boxes do not match what the model
actually does. The declared boundary values come next to them, with a tolerance, which is what the
boundary-precision metric is measured against.

Three shapes are deliberately included because each breaks something:

- **A control** (`aerospace/projectile_clean`, `control/symplectic_spring`) has no region at all.
  Anything APORIA calls suspicious there is a false positive, and that is the only way to find out.
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
