//! # aporia-cli
//!
//! The instrument's front door: `aporia run <model.ap>` takes a model file, compiles it with the
//! existing DSL pipeline, runs one campaign with the existing search driver, and prints what the
//! campaign concluded. Nothing here decides anything about analysis — the crate owns argument
//! handling, model *input*, and the shape of a report line, and calls `aporia-search` for the rest.
//!
//! The split is deliberate. [`run::load_model`] turns a file into a verified model, and
//! [`run::run_and_report`] takes an already-compiled `Model` plus any `aporia_runtime::Executor`. The
//! external-program adapter attaches at that second seam — `--program` already routes a model that
//! declares `output` values through `aporia_adapter::Program` and into the same campaign, with the
//! same report — so a new execution path is an argument to one function rather than a second driver.
//!
//! Benchmark commands are reachable from here as `aporia bench`, through `aporia_bench::cli` and
//! nowhere else in this crate: `bench.rs` maps a status and forwards, owning no argument parsing, no
//! corpus selection, no sweep and no results writer. That is a deliberate revision of an earlier rule
//! in this file, which kept the harness unreachable to avoid a dependency. The rule it replaced was
//! right about the danger and wrong about the remedy -- the way to avoid a second measurement tool is
//! not two binaries, it is one implementation with two names for it. A user with a model and no corpus
//! still gets an answer here that `aporia-bench` could not offer, because nothing in `run`, `replay`,
//! `report` or `compare` requires a corpus to exist.

pub mod archive;
pub mod bench;
pub mod compare;
pub mod replay;
pub mod report;
pub mod run;

pub use run::Exit;
