# Contributor and developer guide

Everything a person needs in order to change APORIA, in the order they will need it. No onboarding
theatre, no code-of-conduct boilerplate: this project has one author and one rule — **a claim only
belongs in this repository if something in it made the claim true.**

## What the repository is for

APORIA looks at a scientific computation, spends an evaluation budget finding where its behaviour
stops being trustworthy, and emits a Trust Atlas — TRUSTED / SUSPICIOUS / UNKNOWN over the parameter
space — where every region carries replayable proof. The research question it exists to answer is
whether a computation-aware, multi-evidence search localises regions of distrust with fewer
executions than simpler exploration. Read
[`software/benchmarks/results/README.md`](../software/benchmarks/results/README.md) before anything
else: it contains the measured answer, the places the method does not work, and three corrections of
earlier claims in that same file.

## Layout

```
software/crates/          13 Rust crates, one concern each (the list is in the root README)
software/benchmarks/      the corpus (22 entries, 21 swept), the 13 committed measurements,
                          the archives a ladder run wrote (git-ignored, regenerable)
software/examples/        external programs in other languages, with their models
software/scripts/         shell helpers — convenience and one lint substitution, never semantics
documentation/            this directory: the developer-facing docs, tracked and public
docs/                     INTERNAL. Planning, decision records, the agent canvas. Never published:
                          .gitignore keeps /docs/ out of the repository entirely
```

`docs/` being untracked is not a tidiness rule. It holds the working notes that later became
`documentation/` pages and measurement prose, and publishing half of a reasoning chain is worse than
publishing none. The gate is one command and it has to print `0`:

```sh
git ls-files | grep -c "^docs/"
```

## Build, test, gate

```sh
cd software
cargo build --release
cargo test --release                   # the whole suite; ~550 tests across 13 crates
cargo fmt --all --check
cargo clippy --workspace --all-targets
```

Those four are the real gates and they are portable; the scripts in `software/scripts/` are wrappers
that add things `cargo` cannot do alone:

| script | what it adds |
|---|---|
| `scripts/dev-env.sh` | exports `INCLUDE`/`LIB` for MSVC on Windows; on any other platform it says so and succeeds |
| `scripts/test.sh` | runs the release suite and, when Smart App Control refuses a freshly linked binary, forces a genuine re-link and retries with the reason printed |
| `scripts/clippy.sh` | the clippy gate on a machine where `cargo-clippy.exe` is refused: sets `RUSTC_WORKSPACE_WRAPPER` to the allowed `clippy-driver` and re-enters `cargo check` |
| `scripts/verify-commits.sh` | exports each commit's own tree and builds it, because the test suite runs against the working tree and an uncommitted file can hide a broken commit |

When the machine blocks `cargo.exe` itself rather than only freshly linked test binaries — which is what
Windows Smart App Control did for a whole session in October 2026 — the four gates above cannot be
started, and there is a workaround worth knowing about so it is not rediscovered from scratch:
`rustc.exe`, `rustfmt.exe` and `clippy-driver.exe` in the toolchain directory are unaffected, so a
metadata-only `rustc --emit=metadata` per crate, in dependency order, answers "does it compile",
`clippy-driver` with the same arguments and the manifest's own lint set answers "does it lint", and
linking each `--test` target and running it answers "does it pass". The catch that costs the most time is
that a per-file reputation verdict is keyed on the linked bytes: re-linking with a different
`-C metadata` gets a fresh verdict immediately, while retrying the identical refused binary waits forever,
which is why `scripts/test.sh` re-links rather than merely sleeping. None of that is committed: it is
machine-specific scaffolding around a machine-specific policy, and the portable gates are the four
commands above.

Lints live in `software/Cargo.toml` under `[workspace.lints]`: `unsafe_code = "forbid"`, clippy
`all` + `pedantic` at warn, with four `cast_*` rules allowed because this is index-heavy numeric codeand those rules fire on the shape of the domain rather than on mistakes.

## The crate edges, and why they are that way

The dependency graph is acyclic and every edge is a decision, not a convenience:

- `aporia-dsl` → `aporia-ir`; nothing else knows the DSL's syntax, so A-IR is the only interchange.
- `aporia-runtime` → `aporia-ir`. The `Executor` trait lives here, which is what lets a foreign
  program be an execution path rather than a special case.
- `aporia-properties` / `aporia-evidence` → runtime + ir. Channels produce evidence; only
  `aporia-evidence` decides what evidence means.
- `aporia-boundary` → ir + evidence. The atlas knows cells and labels, not how evidence was made.
- `aporia-search` → all of the above **and** `aporia-store`, because a campaign owes the caller the
  mapping from a finding to its stored record (`Campaign::stored_findings`) — one definition of a
  stored finding, not two callers assembling one by hand. `aporia-store` never depends back on
  search.
- `aporia-minimize` → ir + properties + runtime. Its `Oracle` is an input trait: the minimiser must
  not decide what "still a failure" means.
- `aporia-bench` → search, store, minimize, risk; `aporia-cli` → `aporia-bench`. The CLI is rendering
  and exit statuses. **The CLI is never a second implementation of anything** — if a command needs a
  decision, the decision belongs in a crate and the command asks.

## Changing the language

1. **Lexer** (`aporia-dsl/src/lexer.rs`): a token kind. Strings and comments have escape rules that
   the round-trip tests in `aporia-ir/src/text.rs` depend on; a new literal type needs one of those.
2. **Parser** (`aporia-dsl/src/parser.rs`) and **checker** (`aporia-dsl/src/check.rs`,
   `units.rs`): dimension and SI-scale checking happens here, before lowering.
3. **Lowering** (`aporia-dsl/src/lower.rs`): the construct becomes A-IR instructions.
4. **A-IR** (`aporia-ir/src/ir.rs`): if it is a new shape, it needs a canonical text form
   (`text.rs`) and a `verify.rs` rule. The canonical form is what archives store and replay re-parses,
   so a construct without a text form cannot be archived.
5. **Runtime**: scalar (`interp.rs`), batch (`batch.rs`), reference (`aporia-numerics/reference.rs`)
   — three implementations of the same instruction, and the Differential channel's whole purpose is
   that they can disagree.
6. `aporia-ir/src/verify.rs` is the gate every backend trusts. Refusals belong there when a
   declaration cannot be sampled or executed at all — see `check_domains`, which refuses an unbounded
   interval because volumes, coverage and bisecting are undefined over an infinite extent.

## Adding an evidence channel

Six places, and the compiler catches only some of them:

1. `aporia-evidence/src/channel.rs`: the variant, `ALL`, `index()`, `name()`, and — the part that is
   a *scientific* decision — `noise_floor()`. A relative calibration divides by the experiment's own
   typical value, so a channel whose readings sit near zero needs a floor or it turns noise into
   strength. `MIN_SCALE` in `calibrate.rs` clamps every fitted scale; a declared floor between zero
   and that clamp cannot change anything, and there is a test that says so.
2. The producer, in `aporia-properties` or the campaign: emit `Evidence` with a subject, a magnitude
   and the observation ids it names. **Which observations an item names decides which cell a finding
   is charged to** — a relation that says "every observation in the experiment" makes the whole
   explored space responsible for one comparison, and that bug once put a correct model's entire
   domain in SUSPICIOUS.
3. `aporia-evidence/src/fuse.rs`: `matrix: [[f64; 5]; 5]` and `per_channel: [f64; 5]` widen to 6,
   and the correlation estimator gets a new pair. Fusion is a calibrated noisy-OR over measured
   correlation; a new channel that is not in that matrix is a channel assumed independent, which is
   a claim.
4. `aporia-boundary`: `Cell.channels` / `measured` are `u8` bitmasks, so a 6th channel still fits —
   a 9th would not, and nothing would tell you.
5. `Policy::suspicious_channels` and `min_channels` now describe a space with one more voice in it.
   Re-measure rather than re-justify: the labels every published number carries depend on them.
6. The corpus: a channel with no corpus entry that *should* flag through it is a channel nobody
   verified fires.

## Adding a corpus entry

```
software/benchmarks/<family>/<name>/model.ap
software/benchmarks/<family>/<name>/truth.json      (schema aporia.truth/1)
software/benchmarks/registry.json                   add the name to its family
```

Then `cargo run --release -p aporia-bench -- verify --grid 40`, which checks every declared region
against direct evaluation of the model's own rules and **refuses to measure** an entry whose claim
does not hold. Ground truth is decided by `corpus::violates` — rules plus divergence — so a fault
that exists only in a measurement channel has no declared region and cannot be scored. That has
stopped two candidate mutants; the answer is a different entry, not a stretched oracle.

`truth.json` names axes; if it names one the model does not have, the entry is refused rather than
scored as a missed boundary.

## Adding an external program

Read [`adapter-protocol.md`](adapter-protocol.md) — one JSON object per line each way, non-finite
values as the *strings* `"NaN"`/`"Infinity"`, `{"error": …}` as a refusal rather than a value,
flush after every answer, deterministic answers for identical input. Then copy
`software/examples/beam.py`.

A new language earns its place by demonstrating a *different boundary property*. Python is there
because it shares no serialiser with APORIA and therefore has to get the number encoding right by
itself. A C or C++ program would have to justify itself the same way — e.g. a program that reports
no step count, or one whose runtime environment APORIA must not assume — otherwise it is a checklist
entry, and this repository has decided not to have those. Nothing here is a placeholder: there is no
stub CUDA backend, no unwired assembler kernel, no dormant plugin system.

## Archives, replay and measurement

[`reproducibility.md`](reproducibility.md) covers the format and the four statuses. Two rules apply
to anyone changing them:

- **An archive is a byte contract.** Every artefact is digested by `manifest.json`, so anything that
  changes bytes changes the archive. `.gitattributes` has a rule for every tracked file precisely so
  that a checkout cannot rewrite one; `aporia-cli/tests/byte_contract.rs` asks git what a fresh
  checkout would write and digests *that*, and asserts every tracked file declares its line endings.
  If you add a file type, add its rule — the test will tell you, and a clone will not.
- **Never overwrite a measurement.** Results files are named for their identity: a digest of the
  results schema, the plan and the corpus entries covered. `aporia-bench run` refuses to write a
  second file for the same identity, because two runs of one plan differ only in `wall_ms`. When a
  field's *meaning* changes, bump `harness::RESULTS_SCHEMA` — that is what `aporia.results/2` is, and
  what makes the corrected run a new identity instead of a squatted one. Old files are history and
  stay untouched.
- **Anything a measurement can be compared on belongs in the plan, not in a constant.** The identity is
  a digest of schema, plan and entries, so a knob that changes what a number means but is not in the
  plan lets two different experiments share a filename — which the harness then refuses, looking like a
  bug. That is why the sampling rates, the atlas thresholds and the `--ablate` channel list are all in
  `plan_json`, and why a new arm of any comparison has to be added there rather than passed through
  some other way.

## Recording an experiment

1. Run it. `aporia-bench run --budgets … --seeds …` writes `results-<identity>.json` and archives
   every top-of-ladder campaign, replaying each before reporting.
2. Compare it against the previous run, field by field, and say what moved. Every "this change did
   not affect the measurement" claim in the results README is a run that was diffed, not an
   assertion. Use the committed tooling: `aporia bench verdict <file>`, `aporia compare` for archives.
3. Write the numbers into `software/benchmarks/results/README.md` **from the file that now exists**,
   never from what you expect it contains. Label historical runs as historical. Where a result is
   missing, say it is missing.
4. Record the environment: the results document writes it for you (OS, arch, pointer width,
   toolchain detected from the compiler, corpus path, and the sampling rates that cost evaluations —
   `plan` carries those because a measurement whose sampling costs are unrecorded cannot be compared).

Numbers are only documented if they were measured. No fabricated speedups, detections, benchmark
rows, citations or "successful build" claims; `TRUSTED` is never described as *proven correct*.

## Commit style

Small, coherent, dependency-ordered units; each commit leaves the tree building and its own message
says *why*. `scripts/verify-commits.sh HEAD~n..HEAD` is the check that each tree really does. Not
meaningless microcommits, and not a bundle: an interface change plus its call sites is one unit, a
documentation page plus the measurement it reports is not.

Before a push: relevant tests, `cargo check --workspace --all-targets`, `cargo fmt --check`, clippy
when the machine allows it, verify the committed tree, and `git ls-files | grep -c "^docs/"` must
print `0`. If a gate could not run, record the gap in the commit message or the results README
rather than implying it passed.
