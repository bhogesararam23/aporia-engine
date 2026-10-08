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
  Measured in **E2 (run across 11 arms, grid 41, 105 sweeps each, all 3,465 outcomes budget-charged
  and census-verified): partially supported on this corpus.** Removing Physical causes 39
  localisations to be missed, but Physical alone matches the full instrument on all 59 localised cases,
  while the other 4 single-channel arms miss all 59. Evidence fusion did not localise something no
  strict subset could on this corpus. Two things this does **not** say, both because the corpus rather
  than the channels set the limit: it does not say Behavioral is unimportant — Behavioral had almost
  nothing to say to test (across the whole full-instrument run it computed 18 readings and spoke on 3
  of the 18 fault entries, because exactly one fault entry declares a `check` relation at all while
  the other two declarations sit on controls, so its null is vacuous rather than empty), and it does
  not say calibration or correlation-aware fusion are pointless — E2 ablated *channels*, not the
  fusion machinery, which is the question E3 and E4 ask and neither has been run.
- **H1, re-measured on geometry the atlas does not partition along (E1.1 / E1.2 / E1.3).** E2 left one
  specific criticism unanswered: its corpus was boxes aligned with the axes the atlas splits, so
  Physical's parity might describe the corpus rather than the instrument. **E1.x tested that and it did
  not move.** The same eleven arms, ladder, rates and seeds ran over a diagonal half-plane, four curved
  regions and three discrete-choice entries: of the 45 (entry, seed) pairs the frozen universe contains,
  the full instrument localises 28, Physical alone localises the same 28 at the same budgets, and the
  difference set the pre-registration decided on is empty — so the outcome is the same "partially
  supported, nothing uniquely required" as E2, now on curved and discrete shape as well as boxes. What
  the new corpus *did* find belongs to the instrument rather than to the channels, and it is listed as a
  limitation below: three of the nine new region-bearing entries are localised by no arm at all, and one
  of them is detected by no arm either, so on the hardest geometry in the corpus the transfer claim is
  resting on entries the atlas can reach.
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
- **A `TRUSTED` label can be earned by silencing the channel that was withholding it.** E2 measured
  this directly: on `aerospace/projectile_clean` at seed 1 and budget 320 the full instrument marks
  **100% of the domain UNKNOWN and none of it suspicious**, while the arm blind to Sensitivity marks
  the same domain **100% TRUSTED** (and so do the arms that keep only one channel). The volume was
  measured either way — Physical applies to every record — but the labelling policy holds a cell at
  UNKNOWN when one channel's reading is loud and nothing corroborates it, and an arm that cannot hear
  that channel is no longer held. So an arm with *less* evidence is not merely less cautious in this
  build; on those pairs it is more decisive. This is the documented cost of the three-label policy
  (a suspicious cell needs two channels; trust needs the channels that ran to stay quiet), not a
  defect E2 fixed: changing the corroboration rule because one channel looks unhelpful would destroy
  the comparison that measured it. What is now true is that `TRUSTED` means *quiet under the channels
  this run was allowed to consult*, and every results row records which those were.
- **`TRUSTED` sitting on a region the model really violates is now measured, not just argued.** E1.3
  pre-registered a pass condition — on its two edge entries every arm must report `trusted_over_true`
  (the fraction of the domain that is both labelled TRUSTED and inside a declared region) of exactly 0.
  It failed as written: 87 of 330 edge rows report 2.5·10⁻⁶, and one row in thirty under the full
  instrument — `edge/overflow_tail`, seed 4, budget 1280, where the trusted cells cover roughly
  eighty percent of a region that is itself 3·10⁻⁶ of the domain. `unplaced` is 0 in all 2475 rows of
  that run, so no measurement fell off the atlas and no non-finite reading reached a label through that
  route; what reached it is the same three-label policy as above. The condition was frozen precisely so
  that this would be a finding instead of a wording choice, so it is recorded as a failed condition and
  the labelling rule is left for the next pre-registration to change, rather than being adjusted now.
- **The atlas cannot enclose a region narrower than its cells, and E1.x measured how badly.** Three of
  the nine new region-bearing entries are localised by *no* arm at any budget: `geometry/narrow_oblique`
  (a band 0.008 wide crossing a diagonal) is detected on all five seeds by the full instrument, by
  Physical alone and by the instrument blind to Physical, and never enclosed; `edge/pole_at_edge` is
  detected by every arm that keeps Physical and by none of the single non-Physical channels;
  `edge/overflow_tail` is detected by nobody. This is a limitation of partitioning axes, not of the
  evidence model — which is also why the geometry experiment cannot say whether fusion would have
  helped there: on parts of the corpus the instrument cannot reach, every arm is equally wrong.

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

Closed since, each with its own commit and its own test: the minimiser now asks the execution path that
produced the finding rather than the interpreter, and a program's finding is verified by that program
end to end; a risk scorer inherits the channels its path can measure and *refuses* rather than answers
less when it cannot; the observation header is parsed once and checked against the records it
summarises; a finding block missing an identity field is refused by name; the public functions nothing
in production called have been removed; and the reference evaluator's `budget_exceeded` / `non_finite`
flags are now read by the channel that consumes its answer — an unfinished reference is reported as
*not compared* rather than as agreement, and a reference that leaves the real numbers where the runtime
does not is named as a divergence instead of being skipped.

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
