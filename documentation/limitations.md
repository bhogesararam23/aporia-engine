# Limitations, and the questions that come up

What APORIA does not do, what it has not measured, and what it refuses to claim. This is the page to
read before quoting a number from this repository.

## The research claim

The question used to be whether a computation-aware, multi-evidence search localises regions of
distrust with fewer executions than simpler exploration. A prior-work pass
([`prior-work.md`](prior-work.md)) found that this is already the published objective of the
excursion-set and active-learning-reliability literature — AK-MCS labels a domain safe / failure /
uncertain under a simulation budget, and Azzimonti et al. produce an inside / outside / inconclusive
partition with error control and a stopping rule. Against that work, beating random and stratified
sampling is not the interesting comparison.

The question has therefore been split in two. The search half has now been measured and came back
unseparated, so what is left is the evidence and the artefact:

- **H1 — evidence.** Does fusing five heterogeneous signals, calibrated and correlation-discounted
  against the same run, localise regions that no strict subset of them localises at the same budget?
  This is the only remaining question that can move the claim, it is expressible as an experiment
  (`aporia bench run --ablate …`), and it has not been run.
- **H2 — artefact.** Does an atlas whose every region carries re-executable provenance change what a
  reader can do with a result? Parts of this are tested in this repository; whether it counts as
  research rather than engineering is open.
- **E1 — search.** Adaptive versus a level-set baseline. **Run, and not separated** (see the list
  below). The arm stays in the default plan whatever it measures.

The measured answer to the *old* question is **a negative, and it has now been measured against the
baseline the literature actually uses**:

- Adaptive localises `electromagnetics/rlc_resonance`, a band 0.088% of the domain, at a budget where
  neither random nor stratified localises anything at any budget tested.
- It *loses* on two entries where plain coverage is simply the right tool.
- After the fixes that made the baselines better rather than worse, the advantage is **one entry wide**
  out of 21 swept entries, 189 sweeps, 945 campaigns.
- **E1 (run):** a fourth arm, `levelset` — a budgeted single-threshold bracket search over the model's
  own declared rules, needing none of the evidence instrument — reaches the same number of entries as
  adaptive (13 of 18) and resolves *more* of them across seeds (65 seed-rows to 60). The two arms tie or
  fail together on 15 of 18 entries; adaptive is cheaper on three and never dearer. By the criterion
  fixed before the run, **the search is not separated from the baseline**, so nothing in this repository
  may claim a search contribution on this corpus. What the instrument buys is earlier localisation on
  three entries, not reach.

One entry, and a baseline that matches the method's reach, is not an answer. And since the pass, no
claim of novelty appears anywhere in this repository: the metamorphic relations are standard,
minimisation is standard, dependence-aware fusion is standard outside testing, calibrated scores are
standard, and the three-label map is a known shape.
What survives is narrower and is listed at the end of
[`prior-work.md`](prior-work.md) — a five-signal score fitted from the run it grades, provenance under
every labelled region, a minimised case re-executed at its own witnesses, and honest refusals.

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
- A literature pass: done on 2026-10-06, and published as
  [`prior-work.md`](prior-work.md). It removed four claims this repository had been making about
  itself and replaced the research question with two narrower ones. What it did *not* do is produce a
  comparison against any named tool: no experiment against an adaptive-learning baseline has been run,
  and the pass is a positioning of the question, not an answer to it.
- CI: no workflow exists. Deliberate — a hosted configuration nobody has executed is documentation
  pretending to be a gate. The gates are the four commands in the developer guide, plus
  `scripts/verify-commits.sh`.

Known defects still open, each recorded with how it was found:

1. The batch evaluator zero-seeds its environment, so a node that was never computed reads as a
   plausible `0.0` where the scalar path yields NaN, and its lane padding writes a literal NaN that no
   input produced. It is dormant rather than wrong-in-production: `run_batch` has no caller, and the
   Differential channel compares the scalar runtime against the independent double-double evaluator,
   not against the batch path, so no published number depends on it. Closing it means either wiring it
   into a channel that would actually want a second path sharing the interpreter's arithmetic — it
   would not — or removing it.
2. Smaller silent defaults that are still there, each with no input that reaches it today: a
   zero-defaulted calibration scale in a manifest reader, `method` defaulting to `"analytic"` when a
   `truth.json` omits it, and a `u16::MAX` sentinel in one inferred-pattern subject that leaks
   `p65535` into a report key (harmless to calibration, which cannot collide with 65,535 parameters,
   and unreadable to a human). The finding-block defaults were the dangerous ones — they attached
   evidence to execution 0 of cell 0 — and are closed: a `.apx` missing a field it always writes is now
   refused with the file and the field named.
3. `aporia-numerics`' reference evaluator computes `budget_exceeded` and `non_finite` and no caller
   reads them, so a reference path that ran out of its step guard is currently indistinguishable from a
   clean one where it happens to produce finite values.

Closed since, each with its own commit and its own test: the minimiser now asks the execution path that
produced the finding rather than the interpreter, and a program's finding is verified by that program
end to end; a risk scorer inherits the channels its path can measure and *refuses* rather than answers
less when it cannot; the observation header is parsed once and checked against the records it
summarises; a finding block missing an identity field is refused by name; and the public functions
nothing in production called have been removed.

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
