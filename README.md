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

Stated as a hypothesis to be measured, not a claim:

> Can a computation-aware, multi-evidence search strategy discover and localise regions of numerical
> distrust using fewer executions than simpler exploration strategies?

Invariant detection, floating-point analysis, metamorphic testing and falsification each already own
pieces of this. The candidate contribution is the combination plus the search objective: heterogeneous
evidence treated as one calibrated, non-double-counting signal, and an evaluation budget spent on the
boundary.

The current measured answer is **partial, and the honest version is in
[`software/benchmarks/results/README.md`](software/benchmarks/results/README.md)**. In short: on a
21-entry measurement — 189 sweeps, 945 campaigns — adaptive localises
`electromagnetics/rlc_resonance`, a band 0.088% of the domain, where neither random nor stratified
localises anything at any budget; it *loses* on two entries where plain coverage is the right tool;
and after the fixes that made the baselines better the advantage is one entry wide. One entry is not
an answer. Numbers are recorded only from runs that actually happened, and `TRUSTED` never means
*proven correct* — it means no current evidence of a problem under the tested assumptions and evidence
model.

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
                    atlas CSV, and bit-exact replay against the archive's own A-IR
  aporia-bench      the measurement harness: corpus registry, ground truth, metrics, strategy
                    comparison, verdict, explanation
  aporia-adapter    the external-computation boundary: a child program, its line protocol, its
                    failure modes, and an example solver to copy
  aporia-cli        the command line: `aporia run <model.ap>`, optionally `--program <solver>`
software/benchmarks/  22 corpus entries with declared ground truth, and the committed measurements
software/scripts/     dev-env, test runner, and the gate that build-verifies every committed tree
```

## Build and run

Rust 1.88 or newer (`stable-msvc` on Windows; the workspace builds with MSVC 14.44 and Windows SDK
10.0.26100). From `software/`:

```sh
cargo build --release                 # the workspace
cargo test --release                  # 393 tests
cargo run --release -p aporia-cli -- run benchmarks/aerospace/projectile_sign_mutant/model.ap
cargo run --release -p aporia-bench -- list      # what the corpus contains
cargo run --release -p aporia-bench -- verify    # ground truth against direct evaluation
cargo run --release -p aporia-bench -- run --budgets 40,80,160,320,640 --seeds 1,2,3
cargo run --release -p aporia-bench -- explain analytic/sqrt_domain --budget 640
```

`aporia run <model.ap>` is the instrument's own front door: it compiles the file with the DSL
pipeline, spends an evaluation budget on it with the same campaign driver the benchmark uses, and
prints the atlas summary, the calibration it measured against, and up to three findings with the
loudest piece of evidence behind each. Exit status is `0` when nothing was flagged, `1` when
SUSPICIOUS regions were reported, `2` for bad usage, `3` when the model could not be read, compiled or
verified, and `4` when an external program stopped answering — so a CI job can fail on the difference
between those. It writes no archive and reports no minimised case; `--budget N` sets the evaluation
budget (default 640).

A model whose arithmetic lives elsewhere is named with `--program`:

```sh
aporia run beam.ap --program "./my-solver --steady"   # beam.ap declares `output deflection : mm`
```

The program is a child process, spoken to as one JSON request and one JSON response per line, and its
answers go to the same campaign as any other model's — same sampling, same five channels, same budget
accounting. The report prints `execution program \`…\`` so no reader has to guess who did the
arithmetic. `aporia-adapter` ships `aporia-example-solver`, a worked example in about forty lines; a
program that exits, prints a banner, answers the wrong number of values, refuses a point or hangs is
reported as what it is (exit status `4`, and the map labelled incomplete) rather than filled in with
plausible numbers. `--timeout MS` bounds one answer, default 5000.

`aporia-bench run` refuses to produce numbers when a declared region does not hold against direct
evaluation of the model's own rules, archives every top-of-ladder run, and replays each archive before
reporting. A benchmark that cannot be replayed is not a measurement.

On Windows with Smart App Control enabled, freshly linked test binaries can be refused by policy
(`os error 4551`) until Microsoft's cloud verdict arrives. `scripts/test.sh` runs the release profile,
forces a genuine re-link on retry, and prints why it is waiting. That is a machine policy, not a
project failure.

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

Not done: the command-line front end and the adapter interface for programs APORIA does not compile
(in-process DSL today); hand-written x86-64 kernels, which are only admissible with a measured
advantage over compiler output and are not yet written; a CUDA backend, which cannot be compiled or
measured on the machine this was built on because it has no NVIDIA device — stated rather than hidden,
and the design is deferred with its trigger recorded; Julia reference implementations; a literature
pass, which must precede any claim of novelty.

Everything above is described as it is: the repository keeps its failed experiments and its corrected
documentation in the open rather than presenting only what worked.

## License

MIT — see [LICENSE](LICENSE).
