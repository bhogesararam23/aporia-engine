# Limitations, and the questions that come up

What APORIA does not do, what it has not measured, and what it refuses to claim. This is the page to
read before quoting a number from this repository.

## The research claim

The question is whether a computation-aware, multi-evidence search localises regions of distrust with
fewer executions than simpler exploration. The measured answer is **partial**:

- Adaptive localises `electromagnetics/rlc_resonance`, a band 0.088% of the domain, at a budget where
  neither random nor stratified localises anything at any budget tested.
- It *loses* on two entries where plain coverage is simply the right tool.
- After the fixes that made the baselines better rather than worse, the advantage is **one entry wide**
  out of 21 swept entries, 189 sweeps, 945 campaigns.

One entry is not an answer, and the larger claim — that this combination is novel — cannot be made at
all before a literature pass, which has not happened. Numerical testing, metamorphic testing,
floating-point analysis, falsification, adaptive sampling and boundary discovery each already own a
piece of this; the candidate contribution is the combination and the search objective, and that word
"candidate" is doing real work.

## Scientific limits that are part of the design

- **`TRUSTED` does not mean proven correct.** It means no current evidence of a problem under the
  tested assumptions and the evidence model that produced the map. A clean bill for a region is a
  statement about the samples and channels that region received.
- **Dropping a parameter in a minimised case is a sampling claim.** "The failure survived samples
  across its whole declared domain", not "the failure is independent of it". The witness points are
  listed by `Case::witnesses()` so the basis is visible; making it a proof needs symbolic reasoning
  about the model, which this project does not claim to do.
- **An interval in a case is a boundary in the samples.** Bisection assumes monotonicity between the
  value and the edge. A failure region with a hole in it is reported as the interval the samples
  support, and that is stated rather than hidden.
- **A minimised risk case is not "the same region, smaller".** Measured on `rlc_resonance`, a
  Physical-driven finding's reduced case pinned axis 0 at 1.29898 while the labelled cell spanned
  [2.039219, 2.041658] — because dropping an axis hands that axis back to the whole declared domain,
  which is exactly what the claim says.
- **A finding made only of a declared relation cannot be minimised at all.** Relations are judged over
  the whole record set; rebuilding one from a candidate's three-point neighbourhood would produce a
  louder claim than the report made. The oracle refuses, and a test asserts the refusal.
- **The Behavioral channel's noise floor has a measured cost.** `MIN_SCALE = 1e-12` in calibration is
  four orders of magnitude above the artifact it exists to swallow, so a real violation below roughly
  1e-11 relative scores zero and a single-channel violation never flags — `Policy::suspicious_channels
  = 2` requires corroboration. Both are recorded with their counter-evidence rather than assumed away.
- **Ground truth is rules-and-divergence only.** `corpus::violates` decides a declared region from the
  model's own rules, so a fault that exists only in a measurement channel has no ground truth and
  cannot be scored. That has withdrawn two candidate mutants; the gap is open.
- **Unbounded domains are refused, not analysed.** A parameter extent that is infinite has no defined
  volume fraction, no workable bisection and no sampler; APORIA has no exploration window separate
  from the declared domain, so it asks for the extent that will actually be sampled instead of printing
  `NaN` for coverage.
- **Correlation between channels is measured per run**, from that run's own records. A run with few
  samples has a weak estimate, and `correlation_samples` is archived so the reader can see how weak.

## Engineering status

Working and measured end to end: the language, A-IR, both interpreters, the five channels, calibration
and fusion, the adaptive atlas, minimisation with verified claims, replayable archives, the benchmark
corpus and the strategy comparison, `run`/`replay`/`report`/`compare`/`bench`/`version`.

Working, tested, **not measured as a research result**: the external-program boundary. It is a real
child process over pipes, its failure modes are exercised by spawning it rather than simulating them,
and it has been demonstrated in two languages — but no ladder run has ever used it, so there is no
number for how adaptive search behaves on a foreign computation at scale. That is a measurement nobody
has taken.

Not done, and not faked:

- Hand-written x86-64 kernels: admissible only with a measured advantage over compiler output; not
  written.
- A CUDA backend: the machine this was built on has no NVIDIA device, so it can be neither compiled nor
  measured here. Deferred with its trigger recorded.
- Julia reference implementations: same status.
- A literature pass: must precede any novelty claim.
- CI: no workflow exists. Deliberate — a hosted configuration nobody has executed is documentation
  pretending to be a gate. The gates are the four commands in the developer guide, plus
  `scripts/verify-commits.sh`.

Known defects still open, each recorded with how it was found:

1. `aporia-bench`'s `RiskScorer` hard-wires the interpreter and the reference evaluator instead of
   asking the `Executor` whose run it is scoring. Latent, because the harness only archives interpreted
   runs — and it will bite the first time a program-executed finding is minimised, in the execution
   accounting specifically, because a `Program` answers without ever running the interpreter.
2. An archive reports two irreconcilable instruction-step totals: the manifest's (the campaign's,
   including charged reference and f32 evaluations) and `observations.bin`'s header (recorded
   observations only). Nothing compares them; `compare` diffs only the manifest copy.
3. `aporia-numerics::reference` has no `state` initialisation pass, so `state q = <expression>` reads
   NaN/zero there while the runtime initialises it. Non-finite pairs are filtered, so this is a silent
   coverage gap in the Differential channel rather than a wrong number. Its own `budget_exceeded` and
   `non_finite` flags are computed and never read.
4. The batch evaluator zero-seeds its environment, so a node that was never computed reads as a
   plausible `0.0` where the scalar path yields NaN, and its lane padding writes a literal NaN that no
   input produced. Latent because `run_batch` has no production caller — which is itself the older
   "built and never wired" defect.
5. A list of smaller silent defaults with no input that reaches them today: zero-defaults for missing
   finding fields, a zero-defaulted calibration scale in a manifest, an invented `tol = 1e-9` when a
   `within` tolerance is written as 0, a `u16::MAX` sentinel that leaks `p65535` into report keys, a
   domain-centre default for a missing axis coordinate, `method` defaulting to `"analytic"`.
6. Some public functions exist only because a test calls them. Each one is a place where the crate
   advertises a capability nothing in production uses.

## Questions that come up

**Why not just fuzz it?** Random search is in the comparison as a baseline (the Random family), and the
measured result is that it is sometimes better. Fuzzing asks "does it break anywhere"; APORIA's output is
a map with a sharpness and a coverage statement, so the budget is spent on boundaries, and the
difference between "we found one bad point" and "trust ends here" is the whole instrument.

**Why five channels instead of one score?** Because one score cannot say what kind of wrong it is. A
NaN, a violated `require`, a precision-dependent answer, a disagreement between two implementations and
an implausible slope are different facts, and fusing them without measuring their correlation
double-counts one measurement as several findings.

**Why does an archive distinguish integrity from reproduction?** They are different conclusions with
different remedies: a digest mismatch means these bytes are not the ones that were written; a
reproduction failure with every digest matching means the arithmetic moved. Collapsing them turns a
build regression into a file-corruption story.

**Why is `wall_ms` in the results at all, if it is not comparable?** Because it is what happened, and
omitting a measured field while keeping a table of timings would be a cleaner-looking document with the
same content. It is labelled non-comparable in the results README, and 627 of 945 rows moved in a run
that changed no measurement.

**Why no LLM, no cloud, no API in the loop?** Not ideology: a component whose behaviour is not
specified cannot be replayed bit-for-bit, and this instrument's whole claim is that a trust map can be
re-executed and checked. Everything that reasons here — language, IR, interpreters, evidence model,
search, minimiser, archive, replay — is in this repository.

**Can I trust a number in this repository?** Every number in `software/benchmarks/results/README.md` is
derived from a committed JSON file in the same directory, and the file that produced it is named in the
text. That README opens by saying it records the places where an earlier version of itself said something
the data does not support. Where a number cannot be derived, it does not appear.
