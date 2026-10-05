# APORIA Wiki

## Overview

APORIA is a local analysis instrument for numerical and scientific computation.

The core question is not only whether a computation fails, but **where trust stops** in its input space and how sharply that boundary can be localised.

APORIA takes a computation, its parameter domain, and physical or mathematical rules, then:

**explores → challenges → verifies → localises → minimises → explains**

The output is a **Trust Atlas** over the tested parameter space:

- **TRUSTED** — no current evidence of a problem under the tested assumptions.
- **SUSPICIOUS** — enough evidence supports distrust.
- **UNKNOWN** — evidence is insufficient to classify the region.

TRUSTED is not a proof of correctness.

## Problem

Numerical and scientific programs can behave normally over most of their domain while becoming unreliable in narrow regions. Uniform tests may miss those regions; a single failing point also does not describe the boundary.

APORIA treats this as a search for **trust boundaries**, using several forms of evidence and spending evaluation budget where additional information is useful.

## Architecture

```
Aporia DSL
   ↓
Lexer / Parser
   ↓
Typed A-IR
   ↓
Runtime / Evaluators
   ↓
Observation + Evidence
   ↓
Adaptive Search
   ↓
Trust Atlas
   ↓
Findings / Minimisation / Archive / Replay
```

Current workspace crates:

- `aporia-ir` — three-address A-IR, operands, loops, dimension algebra, canonical literals.
- `aporia-dsl` — language frontend, unit checking, lowering to A-IR.
- `aporia-runtime` — scalar interpreter, batch evaluator, f64/f32 modes, traces and flags.
- `aporia-numerics` — double-double arithmetic, numerical metrics and an independent reference evaluator.
- `aporia-properties` — constraints, probes, relations, divergence and sensitivity.
- `aporia-evidence` — five evidence channels, calibration and correlation-aware fusion.
- `aporia-boundary` — Trust Atlas, refinement, boundary bands and CSV output.
- `aporia-search` — sampling, acquisition, UCB policy and campaign execution.
- `aporia-minimize` — verified counterexample reduction.
- `aporia-store` — archives, digests and bit-exact replay.
- `aporia-bench` — corpus, ground truth, strategy comparison and measurements.
- `aporia-cli` — the command-line front door for DSL models.

The repository is local-first: no cloud service, external API, model download or LLM runtime is required.

## How it works

### 1. Describe the computation

A model declares inputs and domains, computation, observations and rules.

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

Inputs carry units and domains. `require` expresses a contract or physical rule; `check` expresses behavioural properties such as monotonicity, scaling, symmetry, conservation or a Lipschitz bound.

### 2. Execute and observe

The runtime evaluates sampled parameter points and records outputs, traces, flags, instruction steps and probe results.

Numerical evidence can compare precision modes. Differential evidence can compare the runtime against an independent reference evaluator.

### 3. Fuse evidence

APORIA calibrates channel strength from observed behaviour and combines evidence without simply counting correlated signals multiple times.

A single weak observation does not automatically become a suspicious region; corroboration and absolute facts matter.

### 4. Search the boundary

The campaign uses an evaluation budget to explore the domain and steer later evaluations toward informative or high-risk regions.

The Trust Atlas refines cells while retaining the measurements that produced their labels.

### 5. Minimise and explain

When a finding supports it, APORIA searches for a smaller reproducible description. Reductions are accepted only after re-verification.

The final finding is about the failure or distrust boundary, not merely the first point that happened to fail.

## Setup

Requirements:

- Rust 1.88 or newer.
- Windows builds currently use the stable MSVC toolchain.

From `software/`:

```sh
cargo build --release
cargo test --release
```

## Usage

Run a DSL model directly:

```sh
cargo run --release -p aporia-cli -- run benchmarks/aerospace/projectile_sign_mutant/model.ap
```

Set the evaluation budget explicitly:

```sh
cargo run --release -p aporia-cli -- run path/to/model.ap --budget 640
```

The CLI reports campaign measurements, calibration, and up to three findings.

Exit status:

- `0` — no suspicious region reported.
- `1` — suspicious region reported.
- `2` — usage error.
- `3` — model could not be read, compiled or verified.

The CLI currently does not write an archive or claim a minimised case.

## Experiments

The repository includes a benchmark corpus covering analytical functions, ODEs, control, linear algebra, aerospace, electromagnetics and synthetic failure regions.

The measured comparison uses three search strategies: adaptive, stratified and random.

The current recorded ladder contains **21 swept corpus entries, 189 sweeps and 945 campaigns**. The measured result is deliberately narrow: adaptive search has an advantage on one entry, is worse on two, and is equal or unresolved on the remainder.

The clearest current adaptive case is `electromagnetics/rlc_resonance`, where a narrow 0.088% resonance band was localised at the largest tested budget while the two baseline strategies did not localise it in the same ladder.

The experiments also document cases where APORIA does not localise well:

- regions too small for the available sample density;
- regions below the atlas resolution;
- curved boundaries against an axis-aligned partition.

Numbers and experiment history live in `software/benchmarks/results/`. Failed experiments are kept rather than removed from the record.

## Limitations

APORIA is still a research/system prototype.

The current evidence model depends on the assumptions and measurements supplied to it. A Trust Atlas is not a formal proof system.

Important current limits include sampling density, atlas resolution, curved boundaries, calibration questions and the effect of evaluation-costing evidence channels on the sampling trajectory.

The current public front door analyses programs expressed in the Aporia DSL. Broad support for arbitrary existing scientific programs is not complete.

CUDA, hand-written x86-64 kernels and Julia reference implementations are deferred until there is a concrete technical reason and a measurable benefit.

Novelty claims are intentionally limited until a proper literature pass is completed.

## Roadmap

The next system-level work is focused on the boundary between APORIA and computations it does not compile itself.

Planned areas include:

- external-program adapter execution;
- subprocess execution and failure handling;
- archive/replay workflows exposed cleanly through the CLI;
- reporting and run comparison;
- benchmark tooling and reproducibility hardening;
- further evidence-channel and calibration research.

The roadmap is driven by measured gaps rather than adding technology for its own sake.

## FAQ

### Is APORIA an AI system?

No. The core search, evidence fusion and analysis are implemented directly in the repository. There is no pretrained model or LLM in the runtime loop.

### Is APORIA a normal bug finder?

Not exactly. A bug can be one source of evidence, but the intended output is the **region where trust becomes questionable** and the evidence behind that conclusion.

### What does TRUSTED mean?

Only that APORIA found no current reason to distrust the region under the tested assumptions and evidence model. It is not proof that the computation is correct.

### Why have an UNKNOWN state?

Because insufficient evidence is different from both evidence of correctness and evidence of failure.

### Does APORIA require cloud services?

No. The core system is designed to run locally.

### Can it analyse arbitrary existing programs?

Not yet through the main public path. Direct analysis currently uses the Aporia DSL; broader foreign-program support is roadmap work.

### Where are the measurements?

See `software/benchmarks/results/README.md` and the committed JSON measurement files.

### Where should implementation history be read?

Git history and the public benchmark records are the source of truth. Internal development notes are intentionally kept outside the public repository.
