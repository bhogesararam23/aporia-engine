//! # aporia-cli
//!
//! The instrument's front door: `aporia run <model.ap>` takes a model file, compiles it with the
//! existing DSL pipeline, runs one campaign with the existing search driver, and prints what the
//! campaign concluded. Nothing here decides anything about analysis — the crate owns argument
//! handling, model *input*, and the shape of a report line, and calls `aporia-search` for the rest.
//!
//! The split is deliberate, because the next piece of this is an adapter that runs a foreign program
//! instead of reading a `.ap` file. [`run::load_model`] is the part that changes for that;
//! [`run::analyse`] takes an already-compiled `Model` and is the whole rest of the command, so the
//! adapter attaches at the input edge rather than duplicating the loop.
//!
//! Benchmark internals are not re-exported and not reachable from here: no corpus, no ground truth,
//! no results files. A user with a model and no benchmark gets an answer, which is what `aporia-bench`
//! could not offer.

pub mod run;

pub use run::Exit;
