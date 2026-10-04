//! Replay: running the archive again and checking it gives the same numbers.
//!
//! This is the claim APORIA has to be able to make. A finding is only reproducible if re-executing
//! the recorded inputs on the recorded A-IR produces the *same bits*, not approximately the same
//! numbers — a comparison that would be meaningless here, since NaN does not compare equal to
//! itself and a one-ulp difference is exactly what the differential channel is built to notice. So
//! outputs, traces, raised flags and instruction-step counts are all compared as bit patterns and
//! integers.
//!
//! Replay happens at the A-IR level rather than from the model source, for two reasons: an adapter
//! that handed APORIA a program has no `.ap` file to give back, and the IR is the thing that was
//! actually executed. The manifest's digest of `model.ap` is what connects the IR back to source.

use crate::StoreError;
use crate::store::Loaded;
use aporia_ir::{Model, from_text};
use aporia_runtime::{ExecConfig, interp};
use std::path::Path;

/// One stored execution whose rerun differed.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    pub observation: u64,
    pub field: &'static str,
    pub expected: String,
    pub found: String,
}

/// The result of replaying an experiment.
#[derive(Clone, Debug)]
pub struct Replay {
    pub total: u64,
    pub matched: u64,
    pub mismatches: Vec<Mismatch>,
    /// Problems found in the archive itself — a file that no longer matches its recorded digest.
    pub integrity: Vec<String>,
    /// True only when every execution reproduced and the archive is intact.
    pub reproduced: bool,
}

impl Replay {
    /// One-line text a report or a CLI can print, worded so a partial pass cannot read as a pass.
    #[must_use]
    pub fn summary(&self) -> String {
        if !self.integrity.is_empty() {
            return format!(
                "REPLAY BLOCKED: {} integrity problem(s), first: {}",
                self.integrity.len(),
                self.integrity[0]
            );
        }
        format!(
            "replayed {}/{} executions identically{}",
            self.matched,
            self.total,
            if self.mismatches.is_empty() {
                String::new()
            } else {
                format!(", {} differed", self.mismatches.len())
            }
        )
    }
}

/// Re-execute every recorded point and compare, bit for bit.
#[must_use]
pub fn replay(model: &Model, loaded_records: &aporia_runtime::Records, cfg: ExecConfig) -> Replay {
    let mut mismatches = Vec::new();
    let mut matched = 0u64;
    for o in &loaded_records.items {
        let outcome = interp::run(model, &o.x, cfg);
        let mut local: Vec<Mismatch> = Vec::new();
        if outcome.outputs.len() == o.y.len() {
            for (i, (found, expected)) in outcome.outputs.iter().zip(&o.y).enumerate() {
                if found.to_bits() != expected.to_bits() {
                    local.push(Mismatch {
                        observation: o.id,
                        field: "output",
                        expected: format!("#{i} {}", bit_text(*expected)),
                        found: format!("#{i} {}", bit_text(*found)),
                    });
                }
            }
        } else {
            // A different output count means the archive and the IR disagree about what the model
            // computes, which is a single structural mismatch rather than one per output.
            local.push(Mismatch {
                observation: o.id,
                field: "outputs",
                expected: o.y.len().to_string(),
                found: outcome.outputs.len().to_string(),
            });
        }
        let found_flags = outcome.flags.to_bits();
        if found_flags != o.flags.to_bits() {
            local.push(Mismatch {
                observation: o.id,
                field: "flags",
                expected: format!("{:08b}", o.flags.to_bits()),
                found: format!("{found_flags:08b}"),
            });
        }
        if outcome.steps != o.steps {
            local.push(Mismatch {
                observation: o.id,
                field: "steps",
                expected: o.steps.to_string(),
                found: outcome.steps.to_string(),
            });
        }
        if outcome.traces != o.traces {
            local.push(Mismatch {
                observation: o.id,
                field: "traces",
                expected: describe_series(&o.traces),
                found: describe_series(&outcome.traces),
            });
        }
        if local.is_empty() {
            matched += 1;
        } else {
            mismatches.extend(local);
        }
    }
    let total = loaded_records.items.len() as u64;
    let reproduced = total > 0 && mismatches.is_empty();
    Replay {
        total,
        matched,
        mismatches,
        integrity: Vec::new(),
        reproduced,
    }
}

/// Open a directory and replay it: digest check, IR parse, re-execution.
pub fn replay_dir(root: &Path) -> Result<Replay, StoreError> {
    let loaded = Loaded::open(root)?;
    replay_loaded(&loaded)
}

/// Replay from something already read, so a caller can inspect the archive first.
pub fn replay_loaded(loaded: &Loaded) -> Result<Replay, StoreError> {
    let integrity = loaded.integrity_problems();
    if !integrity.is_empty() {
        return Ok(Replay {
            total: loaded.records.items.len() as u64,
            matched: 0,
            mismatches: Vec::new(),
            integrity,
            reproduced: false,
        });
    }
    let model = from_text(&loaded.air_text)
        .map_err(|e| StoreError::Format(format!("the stored A-IR does not parse: {e}")))?;
    let cfg = ExecConfig {
        fp: match loaded.manifest.exec_fp.as_str() {
            "f32" => aporia_runtime::FpMode::F32,
            _ => aporia_runtime::FpMode::F64,
        },
        max_steps: loaded.manifest.exec_max_steps,
    };
    let mut replay = replay(&model, &loaded.records, cfg);
    replay.reproduced = replay.reproduced && replay.integrity.is_empty();
    Ok(replay)
}

fn bit_text(v: f64) -> String {
    format!("{v} (bits {:016x})", v.to_bits())
}

fn describe_series(series: &[Vec<f64>]) -> String {
    series
        .iter()
        .map(|s| format!("{} points", s.len()))
        .collect::<Vec<_>>()
        .join(", ")
}
