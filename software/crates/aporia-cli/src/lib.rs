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
//! Benchmark internals are not re-exported and not reachable from here: no corpus, no ground truth,
//! no results files. A user with a model and no benchmark gets an answer, which is what `aporia-bench`
//! could not offer.

pub mod run;

pub use run::Exit;
