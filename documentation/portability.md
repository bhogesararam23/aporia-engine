# Supported platforms, and what is actually known about each

This page states what APORIA has been *run on*, what it is expected to work on, and what nobody has
checked. It is deliberately not a compatibility badge: the tiers below are separated by whether a
measurement exists, and the absence of one is written as the absence of one.

## Prerequisites

Required:

- **Rust**, edition 2024. The workspace declares `rust-version = "1.88"` in `software/Cargo.toml`;
  everything published here was built with **1.99.0** (`rustc -vV`, 2026-09-28). There is no
  `rust-toolchain.toml` on purpose — pinning would force a download on someone who does not want one,
  and the MSRV declaration is the checkable part of the policy.
- **A link toolchain for the target**: MSVC 14.44 + Windows SDK on `*-pc-windows-msvc`, `cc`/`ar` on
  Linux, the Xcode command line tools on macOS. Rust installs none of these.
- **git**, for three tests that ask it what a checkout would write
  (`aporia-cli/tests/byte_contract.rs`). Without git those print `SKIP` on stderr.

Optional, each for exactly one thing:

- **Python 3** — `software/examples/beam.py`, the language-independent adapter fixture. Seven tests
  in `aporia-adapter/tests/python_program.rs` run it; without an interpreter they print `SKIP`.
  Python is not a dependency of APORIA: the core never starts it, and nothing is installed for it.
- **A POSIX shell with `awk`, `grep`, `tar`, `mktemp`, `sleep`, `tr`, `cut`, `ls`** — the helper
  scripts in `software/scripts/`. Every one of them is a convenience wrapper around a `cargo` command.
  Nothing in the Rust code calls a shell, and no correctness property lives only in a script except
  where this page says it does.

`curl` appears only in `bootstrap-windows.sh`, which downloads a toolchain on a machine that has
none. It is a first-run convenience, not a project dependency.

## Tiers

| tier | platform | status |
|---|---|---|
| 1 — measured | Windows 11, x86_64, MSVC, Rust 1.99.0 | Everything in this repository was built, tested, measured and archived here. Every published number, every replay, every gate run. |
| 2 — expected | Linux x86_64, macOS x86_64 and aarch64 | `cargo build --release && cargo test --release` is the whole portability surface: the Rust code has **zero** `cfg(windows)` / `cfg(unix)` / `target_os` branches (measured: `grep` over `software/crates` returns nothing), spawns one kind of child process through `std::process::Command` with no shell, writes no path separator by hand into any digested artefact, and does its own float formatting. Nobody has run the suite there, so this tier says "nothing in the code is platform-specific and no one has been contradicted". It does not say "works". |
| 3 — not claimed | 32-bit targets, other architectures, non-MSVC Windows toolchains, anything without a C toolchain | Unexamined. `pointer_width` is recorded in every archive, so a future run there is at least distinguishable from a run here. |

Promoting tier 2 to tier 1 is a small, well-defined job, and it is the first thing a new machine
should do:

```sh
cargo build --release && cargo test --release && cargo fmt --all --check && cargo clippy --workspace --all-targets
cargo run --release -p aporia-bench -- run --budgets 40,80,160,320,640 --seeds 1,2,3
```

The last command writes a results document whose `environment` block records the OS, architecture,
pointer width and toolchain of the machine that ran it, and whose measurement identity is the same
12 hex digits the Windows run produced — because the identity deliberately excludes the environment
(the same experiment, asked elsewhere). If the measured fields match, tier 2 becomes tier 1 with
evidence instead of expectation. If they do not, that difference is itself the finding, and it goes
in `software/benchmarks/results/README.md` rather than being explained away.

## Reproducibility, precisely

- **Numbers on the wire.** Every float that crosses a file or a pipe is written by Rust's own
  `Display` for `f64` — shortest round-tripping, locale-independent by language rule, no `LC_NUMERIC`
  anywhere in the path. Binary records are explicit little-endian (`to_le_bytes`, never `to_ne_bytes`),
  so a big-endian host reads the same values. SHA-256 is implemented here over bytes.
- **Line endings are a byte contract.** `.gitattributes` declares a rule for every tracked file, and
  `cargo test` checks both that the rule exists and that a checkout of a committed archive still
  matches its own manifest digests. A `core.autocrlf` clone of this repository gets the same bytes.
- **Archive ordering is not filesystem-dependent.** Findings are read in sorted filename order, and a
  manifest digests its artefacts in write order, so a filesystem that returns entries in another order
  produces the same archive.
- **The honest gap: transcendental functions.** Basic arithmetic is IEEE-754 and bit-reproducible
  across the platforms in question. `exp`, `ln`, `log10` and `powf`/`powi` are not specified bit-for-bit
  by IEEE-754; they come from the target's library. APORIA uses them in evidence strength (the excess
  function), in calibration and in significant-digit rounding. Replay compares **bits**. So an archive
  written on Windows is *not guaranteed* to reproduce byte-identically on Linux, and a reproduction
  failure (status `6`) on a different platform from the one that wrote the archive would be that
  difference, not a moved algorithm. Nobody has measured how large it is, because nobody has run the
  ladder twice on two platforms yet. Replay's semantics are therefore stated as: **bit-exact on the
  platform that produced the archive**; cross-platform equality is an open question, and the answer will
  be a measurement, not a relaxation of the check.
- **Process teardown.** A timeout stops the process APORIA started. It does not stop a process that
  process spawned — no job object on Windows, no process-group signal on Unix. A solver that forks
  workers leaves them running. Documented rather than silently relied on.
- **Programs get to finish.** `stop()` closes stdin and waits up to 250 ms for the child to exit at
  end-of-input before killing it, so a program that checkpoints or flushes a log on EOF can do it.

## Windows notes that are not about APORIA

Two machine policies have shaped this repository's tooling and neither is a project defect:

- **Smart App Control** refuses freshly linked executables by hash (`os error 4551`) until Microsoft's
  cloud verdict arrives, and it refuses `cargo-clippy.exe` outright. `scripts/test.sh` retries with a
  forced re-link and prints why it is waiting; `scripts/clippy.sh` performs the substitution
  `cargo clippy` would have done — setting `RUSTC_WORKSPACE_WRAPPER` to the allowed `clippy-driver` and
  re-entering `cargo check` — so the lint gate is clippy's own, not a weaker stand-in.
- **A plain Git Bash has no MSVC environment.** `scripts/dev-env.sh` discovers the toolset and SDK and
  exports `INCLUDE`/`LIB`. On a platform that does not link through MSVC it reports that there is
  nothing to set and succeeds, so the verification scripts run everywhere; it fails loudly only where
  the toolset is genuinely missing.

## What is *not* here

No CI configuration exists. This is a deliberate absence rather than an oversight: there is nothing to
run on a hosted runner that `cargo test --release` does not run locally, and a workflow file nobody has
executed is documentation pretending to be a gate. The gates are `scripts/verify-commits.sh` (every
commit's own tree builds), `scripts/clippy.sh`, `cargo fmt --check`, and the suite itself.
