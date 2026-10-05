//! Writing an archive of what just ran.
//!
//! The format is `aporia-store`'s and this module adds no rules to it: it fills in a `Run` from a
//! finished campaign and hands it to `Store`, which digests every artefact and writes the manifest
//! last. Two decisions here are worth stating because they are the ones a reader of an archive will
//! care about.
//!
//! `case` is always empty. The `.apx` finding block has a field for a minimised counterexample, and
//! minimisation is a separate claim with its own oracle question (decisions 0012 and 0020). A command
//! that ran a campaign does not get to fill it, so the archive says plainly what the campaign
//! established and nothing more.
//!
//! `created_unix_ms` is 0, the same choice the benchmark harness makes, so that writing the same run
//! twice produces two archives with identical digests. When a file was written is the filesystem's
//! answer; the manifest's answer is what the run contained.

use aporia_ir::Model;
use aporia_runtime::value::{ExecConfig, FpMode};
use aporia_search::{Campaign, Finding};
use aporia_store::{Environment, Json, Receipt, Run, Store, StoredFinding, label_text};
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
    let findings: Vec<StoredFinding> = campaign
        .findings
        .iter()
        .enumerate()
        .map(|(i, f)| stored(i as u64, f, campaign))
        .collect();
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
    let mut environment = Environment::current();
    environment
        .notes
        .push(("execution".to_string(), execution.to_string()));
    let config = Json::parse(&campaign.config.to_json()).unwrap_or(Json::Null);
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

/// One finding, in the shape the archive stores. Shared with nothing on purpose: the bench harness
/// builds its own because it also minimises each finding and records ground-truth notes, and a third
/// caller should make that choice explicitly rather than inherit one.
fn stored(index: u64, f: &Finding, campaign: &Campaign) -> StoredFinding {
    StoredFinding {
        index,
        cell: f.cell,
        bounds: f.bounds.clone(),
        representative: f.representative.clone(),
        observation: f.observation,
        online_risk: f.online_risk,
        final_risk: f.final_risk,
        samples: f.samples as u64,
        label: campaign
            .atlas
            .cell(f.cell)
            .map_or("UNKNOWN", |c| label_text(c.label))
            .to_string(),
        case: None,
        evidence: f.evidence.clone(),
        raw_evidence: Vec::new(),
    }
}
