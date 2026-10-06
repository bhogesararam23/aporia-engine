# Architecture

What APORIA does, in the order it does it, with the decisions that make each step mean something. This
is the current design, not the original plan; where a step exists but has not been measured broadly, it
says so.

```
model.ap ──▶ A-IR ──▶ Executor ──▶ Observation ──▶ Evidence ──▶ Atlas ──▶ Finding ──▶ Archive
              (language)  (who does    (what was       (what it     (where      (what to     (proof)
                            the math)    seen)           means)      trust ends)  report)
```

## The language and A-IR

`aporia-dsl` lexes, parses, checks and lowers; `aporia-ir` is the only interchange format after that.
Three-address instructions, counted loops, `state`/`advance` for integration, `watch` for traces,
`require` for the author's own contract and `check` for declared behaviour (monotonicity, scaling as a
power, symmetry, conservation, a Lipschitz bound).

Two things about it are load-bearing:

- **Units are dimension *and* exact SI scale.** `km^2/h^2 / (m/s^2)` compared against a bare zero is
  rejected before anything executes. This is not pedantry: a scale mismatch is the most common way a
  scientific model is wrong and the easiest to miss, because the numbers still come out.
- **A-IR has a canonical text form with bit-exact literals.** An archive stores that form and replay
  re-parses it, so "the same model" means the same bytes, and a float in a model cannot drift through
  a round-trip.

`aporia-ir::verify` is the gate every backend trusts. It refuses what cannot be executed or sampled
rather than computing something meaningless: a model with no blocks, an operand that points outside
the instruction list, a rule on a value that was never declared, a parameter domain that is unbounded,
inverted, empty or holds a non-finite choice. The unbounded-domain refusal is recent and measured: an
`input x in [0, inf]` used to produce a coverage line of `NaN`, a third of the run's evaluations
outside every leaf, and a SUSPICIOUS finding printed at risk 0.000 with no evidence behind it.

## Execution

`aporia-runtime`'s `Executor` trait is the boundary: `execute(&mut self, model, x, cfg) -> Outcome`,
plus two capability questions — `varies_with_precision()` and `has_reference_path()`. Three
implementations ship:

| executor | what it is | capabilities |
|---|---|---|
| `Interp` | the scalar interpreter, the reference meaning of A-IR | precision varies, reference path exists |
| `batch` | lane-major evaluation of the same instructions | same model, different arithmetic order |
| `Program` | a child process, one JSON request and response per line | neither: asking it about precision would measure whether it ignores a field it was never sent |

The capability flags describe the *execution path*, not the model. A model the interpreter cannot run
at all — one containing `Opaque`, the instruction that means "some program answers this" — is caught at
the input boundaries that can report a refusal: `aporia run` before spending a budget, and
`aporia-bench`'s corpus gate. `aporia_search::run` does not check, deliberately: it has no way to
decline a budget it was handed, and returning a half-spent campaign as a refusal would be inventing a
result.

An `Outcome` carries outputs, traces, flags (`nan`, `inf`, step-limit) and the instruction steps
executed. A program that stops answering produces non-answers — every declared output NaN — which the
divergence channel reads as a fact about that point, and the exit status reports as an incomplete map
rather than as a result.

## Evidence

`aporia-properties` turns executions into evidence items; `aporia-evidence` decides what they mean.

Five channels, each with its own noise floor: **Behavioral** (how outputs move when inputs move —
declared relations, inferred patterns), **Physical** (did a declared rule hold, did anything leave the
reals), **Numerical** (does the answer depend on rounding — f64 against f32 at the same coordinates),
**Differential** (do two independent execution paths agree), **Sensitivity** (did a tiny input change
produce an implausible output change).

Every item names the observations it was made from. That is not bookkeeping: the cell a finding is
charged to is decided by which observations it names, and a relation measured as "every observation in
the experiment" once put a correct model's whole domain into SUSPICIOUS because one power-law exponent
came out 0.02 from the declared value.

Then two steps that make the numbers commensurable:

- **Calibration by excess over typical.** `scale = median(magnitudes)` per *claim stratum*, clamped by
  a noise floor and a `MIN_SCALE` guard, and `strength = 1 − exp(−(m/scale − 1))`. Absolute evidence —
  a violated `require`, a NaN — is strength 1.0 by construction and skips calibration. A channel whose
  readings are pure noise therefore gets ~0 strength rather than a loud opinion.
- **Correlation-aware noisy-OR fusion.** Two channels that fire together on the same observations are
  not two findings; the correlation matrix is measured from the run, and provenance discounting stops a
  single measurement being counted through several sensors.

Fusion is a calibrated noisy-OR over measured correlation; a new channel that is not in that matrix is
a channel *assumed* independent, which is a claim, not a default.

## The Trust Atlas

`aporia-boundary` keeps a forest of axis-aligned cells over the declared domain. Each cell keeps its
own measurements — samples, risk sum and maximum, which channels spoke, which channels were *applied*,
how many absolute facts — because a label that cannot be re-derived from the evidence under it is a
colour on a map.

Bisection happens where it buys something: refinement is driven by risk spread and by
`refine_below`, so cells that are uniformly boring are not subdivided, and a coarse cell is labelled
coarse rather than being allowed to imply resolution it did not pay for.

`Policy` decides the three labels. A cell is SUSPICIOUS only when it looks troubled in aggregate *and*
some single evaluation inside it either holds an absolute fact or was seen the same way by two
independent channels (`suspicious_channels = 2`): a loud single-channel measurement is a lead, not a
finding, and that rule removed the last false positives on the controls. A cell is TRUSTED only when it
has enough samples and enough channels have actually been *applied* to it; everything else is UNKNOWN.
`TRUSTED` never means proven correct — it means no current evidence of a problem under the tested
assumptions and evidence model.

Two recent fixes are worth naming because they were invisible defects: a **discrete choice axis** was
being bounded in index space while the sampler returns option values, so 38 of 120 measurements in a
real model fell outside every leaf; and the loss had no signal, since `locate` answered "leaf 0" for
anything it could not place. Now `leaf_of` returns an `Option`, unplaceable measurements are counted in
`coverage.unplaced`, and that count reaches the report, `summary.json` and `compare`. A cell whose mean
risk is not a number is now labelled UNKNOWN rather than falling through every comparison against NaN
into `Trusted`.

## Adaptive search

`aporia-search` runs the campaign: budget, sampling, acquisition, two-stage risk, findings. Six
acquisition families — Random, Coverage (a Halton sequence), Boundary, Uncertainty, Sensitivity,
Contradiction — and a UCB meta-policy over new information per evaluation. **All three strategies are
the same driver with a different set of families enabled**, which is what makes a difference in outcome
a statement about where points were placed and not about the code that placed them.

Two-stage risk is the reason the map and the report agree: `online_risk` is what the search could see
while it was looking, and `final_risk` is measured again after the whole record set exists, because
some evidence — a declared relation, an inferred pattern, a sensitivity reading against the
experiment's own distribution — only exists at the end. A `remeasure` pass relabels every cell from the
finished evidence and costs no executions, since it re-arranges measurements already paid for.

Sampling is charged honestly: every probe star, precision pair, reference evaluation and symmetry swap
consumes budget. That is why wiring the differential channel moved control cleanliness from 73 to 49 of
90 sweeps *without its evidence ever firing* — the budget moved, so the trajectory moved, so the one
cell holding a marginal reading moved. This project calls that a ladder methodology problem and records
it rather than tuning the rate that flatters the table.

## Minimisation

`aporia-minimize` shrinks a failure to the part that causes it: ddmin over parameters, then per-axis
interval bisection, then significant-digit reduction — in that order, because each step's claim is
stated in terms the previous one produced. Every reduction is accepted only after the resulting case is
re-verified against the oracle, so the output is a description that still fails, or nothing.

A `Case` is three states per parameter — pinned, band, dropped — and each is a claim checked at the
sample points that make it. A dropped parameter means "the failure survived samples across its whole
declared domain", *not* "the failure is independent of it", and `Case::witnesses()` lists exactly what
was executed so a reader can see the basis for the claim. Turning a drop into a proof needs symbolic
reasoning about the model, which this project does not claim to do.

The oracle is an input (`Oracle::query(x) -> Verdict`, where a `Verdict` carries the answer *and the
model executions it cost*). Two ship: `FailureOracle` asks whether a declared rule fails — one execution
per answer — and `aporia-bench`'s `RiskOracle` asks whether the report would still flag the point,
against the campaign's own frozen calibrator, correlation and per-axis slope reference, restricted to
the channels the finding was actually made of. The second costs several executions per answer, and the
published column now reports both units rather than guessing the factor.

A finding made only of a declared relation is not minimisable by the risk oracle: a relation is judged
over the whole record set, and rebuilding it from a candidate's three-point neighbourhood would produce
a *louder* claim than the report made. The oracle refuses, and a test asserts it refuses.

## Where each concern lives, and why that matters

`aporia-cli` contains rendering and exit statuses. `aporia-bench` contains the corpus, the truth gate,
the metrics and the results identity. `aporia-store` contains the byte contract and the comparison.
There is exactly one `StoredFinding` constructor, one campaign configuration serializer, one number
encoding, one strategy vocabulary, one archive layout. Every bug in this project's own correction list
came from a place where two callers described the same thing independently — which is the reason this
sentence is in the architecture page rather than only in the guide.
