# Prior work: where APORIA sits, and what it therefore cannot claim

APORIA's research question has to be stated inside a literature, not next to one. This page is the
positioning pass for that. It was performed on 2026-10-06 against primary sources — conference and
journal records reachable through Crossref, and arXiv — and it changes what this repository is allowed
to say. The full working notes, including the searches that came back empty, are kept internally; what
is published here is limited to what a reader can check from the citation.

## What is verified, and what is not

Every citation below was resolved to its Crossref or arXiv record and read back: title, authors, venue
and year. Where a search reported something that could not be resolved this way it is listed under
**not verified** and is not used to support any statement.

This is a positioning pass, not a systematic review. Several routes (ACM Digital Library, DBLP, IEEE
Xplore, Semantic Scholar) were bot- or rate-limited from the machine this was run on, so an absence
recorded here means "not found by these searches", never "does not exist".

## The nearest prior work, by the dimension it occupies

APORIA's product has three parts — what counts as evidence, how evaluations are spent, and what a
reader receives — and prior work is strong in different places on each.

### Budgeted search over an input domain, producing a labelled partition

This is the part of APORIA that is **not** new in shape, and it is the part the research question used
to rest on.

- B. Echard, N. Gayton, M. Lemaire, *AK-MCS: An active learning reliability method combining Kriging
  and Monte Carlo Simulation*, Structural Safety, 2011, [doi:10.1016/j.strusafe.2011.01.002](https://doi.org/10.1016/j.strusafe.2011.01.002).
  Adaptive, simulation-budget-limited learning that labels the domain safe / failure / uncertain.
- D. Azzimonti, D. Ginsbourger, C. Chevalier, J. Bect, Y. Richet, *Adaptive Design of Experiments for
  Conservative Estimation of Excursion Sets*, Technometrics, 2021,
  [doi:10.1080/00401706.2019.1693427](https://doi.org/10.1080/00401706.2019.1693427)
  ([preprint](https://arxiv.org/abs/1611.07256)). The three-part partition — inside, outside,
  inconclusive — is the explicit objective, evaluations are the scarce resource, and there is error
  control and a stopping rule.
- D. Bolin, F. Lindgren, *Calculating Probabilistic Excursion Sets and Related Quantities Using
  `excursions`*, Journal of Statistical Software 86(5), 2018,
  [doi:10.18637/jss.v086.i05](https://doi.org/10.18637/jss.v086.i05) — a published implementation.
- A. Raghavan, K. H. Johansson, *An Active Parameter Learning Approach to the Identification of Safe
  Regions*, 2024, [arXiv:2412.10627](https://arxiv.org/abs/2412.10627). Per-region binary labels,
  budget-limited active visits, safe/unsafe partition.

**Consequence.** "Spend a fixed evaluation budget finding where behaviour crosses a threshold, and
report which side of it is trustworthy" is an established objective with more theory than APORIA's
atlas has. APORIA's quadtree bisection and acquisition families are a discrete, evidence-driven
instance of it, not a new search algorithm. The comparison this repository should have been making was
against an adaptive-learning baseline of that shape rather than against random and stratified sampling,
**and it has now been made**: `levelset` — APORIA's minimal level-set-shaped baseline, one scalar
reading per point and the widest bracket cut in the coarsest leaf straddling it, with no calibration,
no fusion and no surrogate — reaches as many entries as the multi-evidence search across the corpus and
resolves more of them across seeds, and the two arms tie or fail together on 15 of 18 region-bearing
entries. Measured, recorded and **not separated**
([`../software/benchmarks/results/README.md`](../software/benchmarks/results/README.md)). It is not
AK-MCS and claims no reproduction of it: it has no Gaussian process, no uncertainty model and no
stopping rule with guarantees, so this result says the multi-evidence search buys earlier localisation
on three entries and no more reach than a single-threshold bracket search — it does not measure the
literature's best instrument against anything.

### Metamorphic and property-based testing

- S. Segura, G. Fraser, A. B. Sánchez, A. Ruiz-Cortés, *A Survey on Metamorphic Testing*, IEEE
  Transactions on Software Engineering, 2016, [doi:10.1109/TSE.2016.2532875](https://doi.org/10.1109/TSE.2016.2532875).
  The relation vocabulary APORIA declares — additive, multiplicative, permutative, invertive,
  inclusive, exclusive — is standard, and the survey names metamorphic-relation *generation* as the
  open problem.

**Consequence.** `check monotone / scales_as / symmetric / conserved / lipschitz` are metamorphic
relations. The unit-and-dimension checking attached to them is APORIA's own, but the relations
themselves are not a contribution and are not claimed as one. APORIA's answer to the survey's open
problem is to require the author to declare them, which is a design stance, not a solution.

### Floating-point analysis and disagreement

- D. Zou, M. Zeng, Y. Xiong, Z. Fu, L. Zhang, Z. Su, *Detecting Floating-Point Errors via Atomic
  Conditions*, Proc. ACM Program. Lang. (POPL), 2020, [doi:10.1145/3371128](https://doi.org/10.1145/3371128).
  Detects error-inducing inputs through per-operation atomic conditions, and explicitly argues against
  depending on a high-precision oracle.
- P. Panchekha, A. Sanchez-Stern, J. R. Wilcox, Z. Tatlock, *Automatically improving accuracy for
  floating point expressions* (Herbie), PLDI, 2015,
  [doi:10.1145/2737924.2737959](https://doi.org/10.1145/2737924.2737959). Expression rewriting against
  sampled points, with a fixed sample budget.
- D. Monniaux, *The pitfalls of verifying floating-point computations*, ACM TOPLAS, 2008,
  [doi:10.1145/1353445.1353446](https://doi.org/10.1145/1353445.1353446).

**Consequence, twice over.** First: APORIA's Numerical channel — the same coordinates at f64 and f32 —
is a weak signal next to a sound round-off bound, and is presented in this repository as one of five
calibrated signals rather than as error analysis. Second: Monniaux is the primary source for the limit
[already documented here](portability.md): a run's bit pattern is not promised to reproduce across
compilers and platforms, so APORIA's replay is scoped to a pinned toolchain and says so. Zou et al. is
the published objection to APORIA's Differential channel, and the channel stood or fell on being
*one calibrated signal among five*, not on being an oracle. **E2 has now measured what that standing
is worth on this corpus, and it is small:** a Differential-only arm localises nothing anywhere, and
removing the channel costs one outcome in the whole experiment — one detection on
`electromagnetics/rlc_resonance` seed 3, which also requires Numerical to survive, so the two
precision channels jointly achieve that one thing and neither achieves it alone. That is the measured
extent of the claim; the argument that a comparison of two implementations is a legitimate signal
rather than a weaker oracle still holds, and it was never the part at issue.

### Counterexample minimisation

- A. Zeller, R. Hildebrandt, *Simplifying and Isolating Failure-Inducing Input*, IEEE Transactions on Software
  Engineering, 2002,
  [doi:10.1109/32.988498](https://doi.org/10.1109/32.988498) — ddmin, the algorithm APORIA runs over
  parameters.

**Consequence.** Minimisation, and re-running the oracle on each candidate, are standard. What APORIA
adds is narrower than it sounds and unverified as a distinction: a dropped parameter is an
interval-or-point claim re-executed at *its own witness set*, and the case's cost is reported in oracle
calls **and** model executions
([measured here](../software/benchmarks/results/README.md)).

### Correlated evidence and its discounting

- J. C. Knight, N. G. Leveson, *An experimental evaluation of the assumption of independence in
  multiversion programming*, IEEE TSE, 1986, [doi:10.1109/TSE.1986.6312924](https://doi.org/10.1109/TSE.1986.6312924).
- J. Kittler, M. Hatef, R. P. W. Duin, J. Matas, *On combining classifiers*, IEEE TPAMI, 1998,
  [doi:10.1109/34.667881](https://doi.org/10.1109/34.667881).

**Consequence.** "Do not count correlated detectors twice" is decades old, and combining heterogeneous
detector output is a named field. APORIA's noisy-OR with a run-fitted correlation matrix is an
application of that idea inside a testing instrument, not a new fusion rule. The claimable part is
where the correlation comes from: it is estimated from the same campaign's own observations, per
channel pair, rather than assumed.

### Reproducible artifacts

Artifact-evaluation practice distinguishes an artifact that is available, an artifact that was
evaluated as functional, and a result that was reproduced or validated by someone else. That
distinction is not this project's, and APORIA's split between *integrity failure* (status 5: this
archive is not the one that was written) and *reproduction failure* (status 6: the archive is fine,
the arithmetic moved) is a two-exit-code spelling of part of it. **The published definitions were not
re-readable from this machine** — the ACM policy page returned HTTP 403 to every fetch — so this
sentence is deliberately about the vocabulary rather than a citation: anyone checking the claim should
read the ACM Artifact Review and Badging policy directly. What no source checked here, including that
one, states as a requirement is the specific mechanism APORIA uses: a manifest that digests every
artefact so a checkout can be verified byte-for-byte, and a replay that re-executes the archived
intermediate representation rather than re-running the tool that produced it.

## What APORIA can say about itself, given the above

Not "novel", "first", or "unique". What survives is specific and mostly about the artefact; the
evidence model is now measured as well, and E2 narrowed what can be said about it:

- A single instrument in which **five heterogeneous signal kinds** — a declared rule with exact SI
  dimension checking, precision disagreement, disagreement with an independent double-double
  evaluator, metamorphic relations, and probe-measured local sensitivity — are treated as one
  calibrated score whose correlation is estimated from the same run, and that score is what the search
  optimises. Each signal exists elsewhere; among the systems checked here, no single one of them is
  the report's whole measure of distrust — a description of the instrument, and after E2 not a claim
  that all five earn their cost: on this corpus four of them do not change what is localised.
- **Every labelled region carries its own proof**: the observations its evidence names, the archived
  A-IR, and a replay that distinguishes "the archive was edited" from "the numbers moved".
- **A minimised case whose claim is re-executed at its own witnesses**, reported with the number of
  model executions it cost — a column that was wrong in this repository's own published numbers until
  it was measured and corrected.
- **Analysis of a computation APORIA does not implement**, over a pipe, with the execution path's
  capabilities deciding which evidence may even be asked for, so an external program's map is honestly
  weaker than an interpreted one instead of looking identical.
- Refusals that are the result: an unverified ground truth stops a measurement, an unsampleable domain
  is rejected before a budget is spent, an unanswerable oracle question yields no verdict, and a
  program-executed run is reported as *not replayed* rather than as zero mismatches.

## What this pass makes unsupportable, and it is therefore not said anywhere in this repository

- That budgeted boundary discovery is APORIA's contribution as an algorithm.
- That metamorphic relations, counterexample minimisation, calibrated scores, or dependence-aware
  fusion are contributions.
- That the three-label map is a new representation: AK-MCS and the excursion-set literature produce
  the same shape.
- That APORIA is more accurate, faster, or better than any named tool. No such comparison has been
  run; the only numbers in this repository are about APORIA's own runs.
- That bit-exact replay holds across platforms.
- That the search beats random or stratified sampling in general. It beats them on one measured entry
  out of 21, and that is recorded as one entry.
- That the five evidence channels are each load-bearing. E2 removed them one at a time at equal charged
  cost: four of the five change no localisation anywhere on this corpus, and an arm kept to the Physical
  channel alone reproduces every localisation the full instrument reaches, at the same budget, with the
  same boundary tolerance. What the other four demonstrably buy is one detection event, jointly, on one
  entry — recorded in `../software/benchmarks/results/README.md` rather than argued away here.
- That the search beats a level-set-shaped single-threshold baseline. Measured on the whole corpus:
  same reach, fewer seeds resolved, three entries cheaper, fifteen ties or joint failures. Not
  separated, and no claim rests on the search.

## Not verified, and not used above

Reported by the searches but not resolved to a primary record from this machine, so they appear in no
sentence above: TestDS, Fornet, the S-TaLiRo / getF-ATO / Breach family's exact venues, MLf, S-Taylor,
EGORA, "Soot" safe-set estimation, SimDiff, SFUDB, HAMRret, NNFM as a floating-point competition,
Daisy's `--dynamic` option (the Daisy documentation was unreachable; the one Daisy paper that could be
read describes static sound error bounds over user-supplied ranges and does not mention cross-precision
comparison), Peregrine's venue, in-toto's CCS record, and "CARI". Anyone extending this page should
resolve those first: several are certainly real, and the list is a record of this machine's network
limits rather than of the literature.
