//! Writing an archive of what just ran.
//!
//! The format is `aporia-store`'s and this module adds no rules to it: it fills in a `Run` from a
//! finished campaign and hands it to `Store`, which digests every artefact and writes the manifest
//! last. Two decisions here are worth stating because they are the ones a reader of an archive will
//! care about.
//!
//! `case` is always empty. The `.apx` finding block has a field for a minimised counterexample, and
//! minimisation is a separate claim with its own oracle question (decisions 0012 and 0020). A command
//! that ran a campaign does not get to fill it: `Campaign::stored_findings` asks every caller for a
//! case per finding and this one answers `None`, so the archive says plainly what the campaign
//! established and nothing more.
//!
//! `created_unix_ms` is 0, the same choice the benchmark harness makes, so that writing the same run
//! twice produces two archives with identical digests. When a file was written is the filesystem's
//! answer; the manifest's answer is what the run contained.

use aporia_ir::Model;
use aporia_runtime::value::{ExecConfig, FpMode};
use aporia_search::Campaign;
use aporia_store::{Environment, Json, Receipt, Run, Store};
use std::path::Path;

/// Archive a finished campaign into `dir`.
///
/// `execution` records who computed the values — the scalar interpreter, or a named program — because
/// an archive whose replay recipe is ambiguous is an archive that will be mis-checked later.
pub fn write(
    model: &Model,
    model_text: &str,
    campaign: &Campaign,
    execution: &str,
    dir: &Path,
) -> Result<Receipt, String> {
    let air = aporia_ir::to_text(model);
    let findings = campaign.stored_findings(|_| None);
    let decisions: Vec<Json> = campaign
        .decisions
        .iter()
        .map(|d| {
            Json::object(vec![
                ("evaluation", Json::count(u64::from(d.evaluation as u32))),
                ("family", Json::text(format!("{:?}", d.family))),
                ("cell", Json::count(u64::from(d.cell))),
                ("risk", Json::number(d.risk)),
                ("gain", Json::number(d.gain)),
                (
                    "x",
                    Json::Arr(d.x.iter().map(|v| Json::number(*v)).collect()),
                ),
            ])
        })
        .collect();
    let mut environment = Environment::current().with_toolchain();
    environment
        .notes
        .push(("execution".to_string(), execution.to_string()));
    let config = campaign.config.json();
    let bands = campaign.atlas.bands();
    let run = Run {
        model,
        model_text,
        air_text: &air,
        records: &campaign.records,
        config,
        coverage: campaign.atlas.coverage(),
        atlas_csv: &campaign.atlas.to_csv(),
        bands: &bands,
        findings: &findings,
        decisions: &decisions,
        calibrator: &campaign.calibrator,
        correlation: &campaign.correlation,
        exec: ExecConfig {
            fp: FpMode::F64,
            max_steps: campaign.config.max_steps_per_evaluation,
        },
        evaluations: campaign.evaluations,
        instruction_steps: campaign.instruction_steps,
        environment,
        created_unix_ms: 0,
    };
    let mut store = Store::create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    store
        .write(&run)
        .map_err(|e| format!("{}: {e}", dir.display()))
}
