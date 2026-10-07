# APORIA

**A local analysis instrument for numerical and scientific computation. Hand it a computation, its
input domain, and whatever physical or mathematical rules you know. It executes, gathers five kinds of
evidence, decides where the next experiment goes, narrows in on the place where behaviour turns bad,
shrinks the failure down to something a human can read, and emits a Trust Atlas — a map of TRUSTED /
SUSPICIOUS / UNKNOWN over the parameter space in which every region carries replayable proof.**

APORIA is not a bug finder. The product is *where trust stops, and how sharply*. A failure at one
point is an input to the search, not the output.

No cloud, no external API, no model download, no LLM in the runtime loop. Everything that reasons —
the language, the intermediate representation, the interpreters, the evidence model, the search, the
minimiser, the archive and the replay — is implemented in this repository.

## The research question

Stated as a hypothesis to be measured, not a claim. It was rewritten on 2026-10-06 after a prior-work
pass ([`documentation/prior-work.md`](documentation/prior-work.md)), which found that the question this
README used to ask — whether a multi-evidence search localises distrust regions with fewer executions
than simpler exploration — is already the published objective of the excursion-set and
active-learning-reliability literature (AK-MCS; Azzimonti et al.; Raghavan & Johansson). Against that
literature, comparing against random and stratified sampling measures a strawman. Two sharper
questions replace it:

> **H1 — evidence.** Does fusing five heterogeneous signals with correlation-aware, run-fitted
> calibration localise regions that no strict subset of them localises at the same budget?
>
> **H2 — artefact.** Does an atlas whose every region carries re-executable provenance change what a
> downstream reader can *do* with a result — verify it, compare two runs, dispute a label — compared
> with a score or a region list without provenance?

H1 is answerable with the ablation the harness supports (`aporia bench run --ablate …`, which drops
a channel's *readings* while charging its evaluations, so the arms cost the same). H2 is a claim about
this repository's own behaviour, checked by the replay, report and compare tests — and the honest risk
is that H2 is engineering rather than research.

H1 has now been measured in E2 across 11 ablation arms and is **partially supported on this corpus**;
the measured answer to the older search-advantage question is **a negative** (E1), both detailed in
[`software/benchmarks/results/README.md`](software/benchmarks/results/README.md). In short: the
three-arm ladder localises `electromagnetics/rlc_resonance`, a band 0.088% of the domain, where neither
random nor stratified localises anything at any budget, and loses on two entries where plain coverage is
the right tool — one entry wide. The comparison that could have changed that verdict has now been run.
A fourth arm, `levelset` — a budgeted single-threshold bracket search in the shape the excursion-set
literature uses, needing none of the evidence instrument — reaches as many entries as the five-channel
search (13 of 18) and resolves more of them across seeds, and the two arms tie or fail together on 15 of
18 entries. By the criterion fixed before the run, **the search is not separated from the baseline**, so
the claim this repository can defend is the evidence model and the artefact, not the algorithm. Numbers
are recorded only from runs that actually happened, and `TRUSTED` never means *proven correct* — it means
no current evidence of a problem under the tested assumptions and evidence model.

## What is here

```
software/crates/
  aporia-ir         A-IR: three-address instructions, operands, counted loops, dimension algebra,
                    canonical text form with bit-exact literals
  aporia-dsl        the Aporia language: lexer, parser, unit checker, lowering to A-IR
  aporia-runtime    scalar interpreter (the reference meaning) and lane-major batch evaluator,
                    f64/f32 modes, traces, flags
  aporia-numerics   double-double arithmetic, ulp/relative/cancellation metrics, splitmix64,
                    a reference evaluator that shares no arithmetic with the runtime
  aporia-properties probes, constraints, divergence, declared relations, inferred patterns,
                    sensitivity
  aporia-evidence   five evidence channels, calibration by excess over typical, correlation-aware
                    noisy-OR fusion with provenance discounting
  aporia-boundary   the Trust Atlas: adaptive bisection whose cells keep their measurements,
                    labelling policy, boundary bands, CSV
  aporia-search     acquisition families, Halton sampling, UCB meta-policy, the budgeted campaign
                    driver, two-stage risk, findings
  aporia-minimize   counterexample minimisation: ddmin over parameters, per-axis interval bisection,
                    significant-digit reduction — each accepted only after re-verification
  aporia-store      run archives: manifest, SHA-256 digests, fixed-record observations, findings,
                    atlas CSV, bit-exact replay against the archive's own A-IR, and the comparison
                    of two stored runs
  aporia-bench      the measurement harness: corpus registry, ground truth, metrics, strategy
                    comparison, verdict, explanation
  aporia-adapter    the external-computation boundary: a child program, its line protocol, its
                    failure modes, and an example solver to copy
  aporia-cli        the command line: `aporia run <model.ap>` (optionally `--program`, `--archive`),
                    `aporia replay`, `aporia report`, `aporia compare`, `aporia bench`
software/benchmarks/  22 corpus entries with declared ground truth, and the committed measurements
software/examples/    external programs in other languages and the models they answer for —
                      `beam.py` (Python 3, standard library only) and `beam.ap`
software/scripts/     dev-env, test runner, lint gate, and the gate that build-verifies every
                      committed tree
documentation/        architecture, adapter protocol, reproducibility, portability, prior work,
                      contributor guide, limitations — the developer-facing docs, tracked and public
```

## Build and run

Rust 1.88 or newer (edition 2024; everything published here was built with 1.99.0) and the C toolchain
your target needs — MSVC plus the Windows SDK on `*-pc-windows-msvc`, `cc` on Linux, the Xcode command
line tools on macOS. Python 3 and git are optional and each used by one thing: the language-boundary
fixture and three checkout tests. There is no other dependency, nothing to download and no service to
configure. From `software/`:

```sh
cargo build --release                 # the workspace
cargo test --release                  # 622 tests across 13 crates
cargo fmt --all --check && cargo clippy --workspace --all-targets
cargo run --release -p aporia-cli -- run benchmarks/aerospace/projectile_sign_mutant/model.ap
cargo run --release -p aporia-bench -- list      # what the corpus contains
cargo run --release -p aporia-bench -- verify    # ground truth against direct evaluation
cargo run --release -p aporia-bench -- run --budgets 40,80,160,320,640 --seeds 1,2,3
cargo run --release -p aporia-bench -- explain analytic/sqrt_domain --budget 640
```

`aporia help` and `aporia --help` print the command list and every exit status; `aporia --version`
prints which build produced the number you are quoting — the same value an archive records as
`tool_version`. Each command answers `--help` too, rather than reading the flag as a filename.
[`documentation/portability.md`](documentation/portability.md) states which platforms have actually
been measured and which are only expected; the short version is that Windows x86_64 is the only one
where a ladder run has happened.

`aporia run <model.ap>` is the instrument's own front door: it compiles the file with the DSL
pipeline, spends an evaluation budget on it with the same campaign driver the benchmark uses, and
prints the atlas summary, the calibration it measured against, and up to three findings with the
loudest piece of evidence behind each. Exit status is `0` when nothing was flagged, `1` when
SUSPICIOUS regions were reported, `2` for bad usage, `3` when the model could not be read, compiled or
verified, and `4` when an external program stopped answering — so a CI job can fail on the difference
between those. `--budget N` sets the evaluation budget (default 640); no minimised case is reported,
because that is a separate oracle question (see `software/benchmarks/results/README.md`).

A model whose arithmetic lives elsewhere is named with `--program`:

```sh
aporia run beam.ap --program "./my-solver --steady"   # beam.ap declares `output deflection : mm`
```

The program is a child process, spoken to as one JSON request and one JSON response per line, and its
answers go to the same campaign as any other model's — same sampling, same five channels, same budget
accounting. The contract is [`documentation/adapter-protocol.md`](documentation/adapter-protocol.md),
and it is the whole interface: nothing else about your program is visible to APORIA. The report prints
`execution program \`…\`` so no reader has to guess who did the arithmetic. `software/crates/aporia-adapter`
ships `aporia-example-solver`, a worked example in about forty lines, and `software/examples/beam.py` is
the same contract satisfied in Python with its standard library alone — which is the version that has to
get the number encoding right by itself, because it shares no serialiser with the caller. A program that
exits, prints a banner, answers the wrong number of values, refuses a point or hangs is reported as what
it is (exit status `4`, and the map labelled incomplete) rather than filled in with plausible numbers.
`--timeout MS` bounds one answer, default 5000.

`--archive DIR` writes the run out as an archive — model text, A-IR, every execution, the atlas
table, the boundary bands, the search decisions, the fitted calibration and a manifest that digests
each file — and `aporia replay <archive-dir>` reads it back. Those are two different questions and they
get two different exit statuses:

```sh
aporia run model.ap --archive run-0001     # 14 files, digested
aporia replay run-0001                     # integrity, then reproduction
```

If a byte no longer matches the manifest, that is an integrity failure (status 5): the archive is not
the one that was written. If every digest matches and re-executing the archived A-IR at the archived
points disagrees, that is a reproduction failure (status 6): the archive is fine and the arithmetic
moved. Collapsing the two into "something is wrong with the archive" is the mistake this split exists
to prevent. A run that was executed by an external program is checked for integrity and reported as
*not replayed* — re-running a program this command does not have is not the same experiment, and
printing "0 mismatches" over nothing would be a pass earned by not trying.

Two more commands read an archive, and neither executes anything:

```sh
aporia report run-0001                     # what the stored run says
aporia compare run-0001 run-0002           # what differs between two stored runs
```

`report` prints the archived configuration, counts, calibration, bands, decisions and finding blocks —
every number read from the files, none recomputed. `compare` puts two archives side by side and reports
each field as the same, changed with both values (`config.budget 40 -> 80`), or held by only one of
them. Findings are paired by the region they describe rather than by atlas cell id, because a cell id is
an index into one run's own partition and means nothing in the other's; a region one run did not find is
reported as missing there, not as a disagreement. Two archives whose models have different parameter
counts are not paired at all — those regions are coordinates in different spaces — and the command says
which sections it refused.

The three endings are three exit statuses: `0` identical, `8` the archives differ, `9` nothing disagreed
but a section could not be compared. `8` is deliberately not `1`: a difference between two runs is not a
claim that either found a region worth trusting less. A corrupt archive on either side stops the
comparison at `5` rather than producing a diff out of bytes that may have been edited.

`aporia compare --json` renders the same comparison for a caller rather than a reader: the verdict, a
tally, and one entry per section saying whether it could be compared at all — because the human form
prints only what moved, and "nothing printed" would be an ambiguous thing for a pipeline to conclude
agreement from. `report` has no JSON flag on purpose: `manifest.json` and `summary.json` inside the
archive already are its machine-readable form, and a third copy would be two things to reconcile.

`aporia-bench run` refuses to produce numbers when a declared region does not hold against direct
evaluation of the model's own rules, archives every top-of-ladder run, and replays each archive before
reporting. A benchmark that cannot be replayed is not a measurement.

The same commands are reachable from the instrument's own front door as `aporia bench <command>`, and
they are the same code: `aporia-bench` is a thin binary over `aporia_bench::cli::dispatch`, and so is
`aporia bench`. There is no second argument parser, corpus reader, sweep or results writer, which is the
only reason the two cannot drift into reporting different numbers for one plan.

```sh
aporia bench list                             what the corpus holds and what each entry claims
aporia bench verify --grid 40                 check every declaration against direct evaluation
aporia bench run --budgets 40,80 --seeds 1    sweep, archive, and write results-<identity>.json
aporia bench run --strategies adaptive,levelset,stratified,random   the four arms, one identity
aporia bench run --ablate differential        same points, same cost, one channel blind
aporia bench verdict benchmarks/results/results-1791153844.json
```

Four strategies, one driver: `random` and `stratified` are the naive arms, `levelset` is a budgeted
single-threshold bracket search over the model's own declared rules and needs none of the evidence
instrument, and `adaptive` is the multi-evidence search. The strategy list is part of the plan, so a run
with a fourth arm gets a fourth identity and cannot overwrite a measurement made with three. The
four-arm comparison has been run, and its result — that `adaptive` and `levelset` are not separated on
this corpus — is in
[`software/benchmarks/results/README.md`](software/benchmarks/results/README.md).

`--ablate` silences a channel's *readings*, not its evaluations: the ablated arm is charged the same
executions and samples the same trajectory, so a difference between two arms is a difference in what
was concluded rather than in what was paid for. The list of silenced channels is part of the plan, so
it is part of the measurement identity — an ablation arm writes its own results file and cannot
overwrite the arm it was measured against. Silencing all five channels is refused at the command line;
with no evidence a campaign has nothing to search on, and it would report a domain of cells that were
never really assessed.

For `bench` the statuses are the harness's own, so a caller can check them from either name: `0` the
command did what was asked, `1` nothing was measured because a declared region did not hold, `2` the
command was refused. `1` is not `aporia run`'s status `1` — a corpus whose ground truth has drifted is a
different problem from a model that flagged a region, and the README says so rather than letting the
number carry both meanings.

On Windows with Smart App Control enabled, freshly linked test binaries can be refused by policy
(`os error 4551`) until Microsoft's cloud verdict arrives. `scripts/test.sh` runs the release profile,
forces a genuine re-link on retry, and prints why it is waiting. That is a machine policy, not a
project failure.

The same policy blocks the `cargo-clippy` executable, so `cargo clippy` cannot be started here at all.
`cargo clippy` is a thin driver that sets `RUSTC_WORKSPACE_WRAPPER` to `clippy-driver` and re-enters
`cargo check`; `clippy-driver` is allowed, so `scripts/clippy.sh` performs that substitution and reports
clippy's own findings on the same command line. It is the gate the workspace lints in `Cargo.toml` are
written against, not a weakened stand-in.

## A model looks like this

```
model euler_decay "explicit Euler on exponential decay, one free step size" {
  input dt in [0.001, 0.6]
  let k = 2
  state e = 1.0
  loop 41 {
    advance e = e - k * e * dt
    watch e
  }
  let final = e
  require final > 0
}
```

That is `software/benchmarks/ode/euler_decay/model.ap`, one of the twenty-one swept corpus entries the
numbers above are measured on, so the syntax shown is the syntax that runs.

Inputs carry units and declared domains, `state` and `advance` are integration, `watch` produces a
trace, `require` is physical evidence against the author's own contract and `check` is behavioural —
monotonicity, scaling as a power, symmetry, conservation within a tolerance, a Lipschitz bound. Units
are checked as dimension *and* exact SI scale, so `km^2/h^2 / (m/s^2)` compared against a bare zero is
rejected before anything executes.

## Status

Working and measured: the language, A-IR and both interpreters; five evidence channels with
experiment-calibrated strengths and fusion that refuses to double count; the adaptive atlas and search;
counterexample minimisation with verified claims; replayable archives; the benchmark corpus and the
strategy comparison.

Working and tested end to end, but not measured as a research result: the command line (`run`, `replay`,
`report`, `compare`, `bench`) and the adapter for programs APORIA does not compile, which is a real child
process over pipes — its five failure modes are exercised by spawning it, not by simulating them. A model
that declares `output` values is analysed through that program by the same campaign as any interpreted
model, in either of the two example languages; that is a demonstration that the boundary works, not a
benchmark of it, and no ladder run has ever used a program.

Two things the instrument now refuses rather than mis-measures, both found by running them and looking at
what came out: a model whose parameter domain is unbounded (its coverage fractions are `NaN`, its
bisection loses a third of the run's evaluations outside every leaf), and a benchmark entry whose values
come from a program the harness has no way to run.

Not done: hand-written x86-64 kernels, which are only admissible with a measured
advantage over compiler output and are not yet written; a CUDA backend, which cannot be compiled or
measured on the machine this was built on because it has no NVIDIA device — stated rather than hidden,
and the design is deferred with its trigger recorded; and Julia reference implementations.
The evidence ablation that H1 asks for has now been run (E2, across 11 arms, showing Physical is
essential while fusion does not improve on Physical alone on this corpus). The search comparison
against an adaptive-learning baseline has also been run: `levelset` is implemented, measured across the
corpus, and the result is that the multi-evidence search is not separated from it on this corpus (E1,
above). The prior-work pass has been done and is written up in
[`documentation/prior-work.md`](documentation/prior-work.md); it narrowed the question rather than
answering it.

Everything above is described as it is: the repository keeps its failed experiments and its corrected
documentation in the open rather than presenting only what worked.
[`documentation/limitations.md`](documentation/limitations.md) is the full list of what this cannot yet
support a claim about.

## License

MIT — see [LICENSE](LICENSE).
