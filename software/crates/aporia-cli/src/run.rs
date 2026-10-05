//! The one command that exists so far: `aporia run <model.ap>`.
//!
//! Two steps, kept apart on purpose. [`load_model`] turns a file into a verified A-IR model, and
//! [`analyse`] spends an evaluation budget on that model with the campaign driver from
//! `aporia-search` and writes a report. An adapter that runs a foreign program replaces the first
//! step and calls the second unchanged — which is why nothing in `analyse` knows the word `.ap`,
//! a path, or the DSL.
//!
//! Nothing here analyses anything. The sampling, the five channels, the calibration, the fusion and
//! the atlas all live in the crates this crate calls; duplicating any of it would produce a second
//! answer to compare against the first.

use aporia_ir::Model;
use aporia_search::{Campaign, Config, run_with};
use aporia_store::label_text;
use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

/// What the process exits with, and what the number is claiming.
///
/// `Clean` and `Suspicious` are both *successful* runs: the analysis happened and the report is on
/// stdout. They are separated because a CI job wants to fail on the second one. `Input` means APORIA
/// never got to look at a model — the file could not be read, or it did not compile, or the A-IR it
/// lowered to failed verification — and `Usage` means no analysis was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Clean,
    Suspicious,
    Usage,
    Input,
    /// The model's values come from a program, and the program stopped answering. The report was
    /// still written, because where a run broke is information, but the map is not a result: after
    /// this point every unanswered point is a non-answer, and the atlas is describing a broken pipe.
    Program,
}

impl Exit {
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            Self::Clean => 0,
            Self::Suspicious => 1,
            Self::Usage => 2,
            Self::Input => 3,
            Self::Program => 4,
        }
    }
}

/// A model that compiled and verified, with everything the input step learned on the way.
#[derive(Debug)]
pub struct Loaded {
    pub model: Model,
    /// The model's own name, as the author wrote it.
    pub name: String,
    /// Warnings from the front end and the verifier. Kept separate from errors because they did not
    /// stop the model from existing, and a run that hides them is a run whose numbers a reader cannot
    /// audit.
    pub notices: Vec<String>,
}

/// Read a `.ap` file and lower it, refusing anything that does not survive both the checker and the
/// A-IR verifier.
pub fn load_model(path: &Path) -> Result<Loaded, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let named = path
        .file_name()
        .map_or_else(|| "model.ap".to_string(), |n| n.display().to_string());
    let source = aporia_dsl::span::Source::new(&named, text);
    let compiled = aporia_dsl::lower::compile(&named, source.text());
    if compiled.diagnostics.has_errors() {
        return Err(format!(
            "{} did not compile:\n{}",
            path.display(),
            compiled.diagnostics.render_all(&source)
        ));
    }
    // Anything short of an error still gets shown: a warning is the front end saying it had to make a
    // judgement, and a report whose reader cannot see that judgement is not inspectable.
    let mut notices: Vec<String> = Vec::new();
    if !compiled.diagnostics.is_empty() {
        notices.push(compiled.diagnostics.render_all(&source));
    }
    let model = compiled
        .into_model()
        .ok_or_else(|| format!("{} produced no model", path.display()))?;
    let report = aporia_ir::verify(&model);
    if !report.is_ok() {
        return Err(format!(
            "{} lowered to A-IR that does not verify:\n{}",
            path.display(),
            report
                .errors
                .iter()
                .map(|e| format!("  {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    for w in &report.warnings {
        notices.push(format!("warning: {w}"));
    }
    Ok(Loaded {
        name: model.name.clone(),
        model,
        notices,
    })
}

/// Spend a budget on a model with the scalar interpreter and write what the campaign concluded.
pub fn analyse(loaded: &Loaded, config: Config, out: &mut impl Write) -> Exit {
    run_and_report(
        loaded,
        config,
        &mut aporia_runtime::Interp,
        "scalar interpreter (the model's own A-IR instructions)",
        out,
    )
}

/// Spend a budget on a model with an arbitrary execution path and write the same report.
///
/// The `execution` string is printed in the report because provenance is not decoration: a reader
/// looking at a Trust Atlas has to be able to tell whether APORIA did the arithmetic or asked
/// something else to do it. The campaign itself is the same code either way — that is the entire
/// point of the boundary, and the reason this function takes an `Executor` rather than a flag saying
/// "external".
pub fn run_and_report(
    loaded: &Loaded,
    config: Config,
    engine: &mut dyn aporia_runtime::Executor,
    execution: &str,
    out: &mut impl Write,
) -> Exit {
    let campaign = run_with(&loaded.model, config, engine);
    for notice in &loaded.notices {
        let _ = writeln!(out, "{notice}");
    }
    let _ = write!(out, "{}", report(&loaded.model, &campaign, execution));
    if campaign.findings.is_empty() {
        Exit::Clean
    } else {
        Exit::Suspicious
    }
}

/// Does this model declare values only a program can supply?
#[must_use]
pub fn needs_adapter(model: &Model) -> bool {
    aporia_runtime::needs_adapter(model)
}

/// The report: what ran, how the map came out, and one block per finding up to three.
///
/// Deliberately not the `.apx` finding block from `aporia-store`, whose renderer ends with
/// `Replay: aporia replay findings/000000.apx`. That sentence is true of an archived run and a lie
/// about this one, which writes no archive — so the command prints the same facts in fewer lines and
/// says nothing it cannot stand behind.
fn report(model: &Model, campaign: &Campaign, execution: &str) -> String {
    let mut out = String::new();
    let coverage = campaign.atlas.coverage();
    let _ = writeln!(
        out,
        "model {}  strategy {:?}  seed {}  budget {}",
        campaign.model_name, campaign.config.strategy, campaign.config.seed, campaign.config.budget
    );
    let _ = writeln!(out, "execution  {execution}");
    let _ = writeln!(
        out,
        "campaign {} evaluations  {} instruction steps  {} records  {} decisions",
        campaign.evaluations,
        campaign.instruction_steps,
        campaign.records.len(),
        campaign.decisions.len()
    );
    let _ = writeln!(
        out,
        "atlas {} cells ({} leaves)  trusted {:.4}  suspicious {:.4}  unknown {:.4}  resolved {:.4}",
        coverage.cells,
        campaign.atlas.leaf_ids().len(),
        coverage.trusted,
        coverage.suspicious,
        coverage.unknown,
        coverage.resolved()
    );
    let _ = writeln!(out, "calibration {}", campaign.calibrator.describe());
    let _ = writeln!(out, "findings {}", campaign.findings.len());
    for (i, f) in campaign.findings.iter().take(3).enumerate() {
        let label = campaign
            .atlas
            .cell(f.cell)
            .map_or("UNKNOWN", |c| label_text(c.label));
        let _ = writeln!(
            out,
            "  #{i} cell c{}  {label}  risk {:.3} (online {:.3})  {} samples",
            f.cell, f.final_risk, f.online_risk, f.samples
        );
        let _ = writeln!(out, "     region   {}", region_text(model, &f.bounds));
        // The loudest item, quoted in full: "why is this suspicious" is the question a reader has,
        // and the channel names are the report's own, not a paraphrase of them.
        match f
            .evidence
            .iter()
            .max_by(|a, b| a.strength.total_cmp(&b.strength))
        {
            Some(e) => {
                let _ = writeln!(
                    out,
                    "     loudest  {:<11} {:<18} strength {:.3}  {}",
                    e.channel.name(),
                    e.subject.key(),
                    e.strength,
                    e.detail
                );
            }
            None => {
                let _ = writeln!(out, "     loudest  (no evidence attached)");
            }
        }
        let _ = writeln!(
            out,
            "     evidence {} items, {} distinct claims",
            f.evidence.len(),
            {
                let mut keys: Vec<String> = f.evidence.iter().map(|e| e.subject.key()).collect();
                keys.sort();
                keys.dedup();
                keys.len()
            }
        );
    }
    if campaign.findings.len() > 3 {
        let _ = writeln!(
            out,
            "  … and {} further finding(s) not printed",
            campaign.findings.len() - 3
        );
    }
    let _ = writeln!(
        out,
        "verdict {}",
        if campaign.findings.is_empty() {
            "nothing suspicious under the evidence model that ran"
        } else {
            "SUSPICIOUS regions reported"
        }
    );
    // The sentence this project refuses to leave out: the atlas reports evidence, not proof.
    out.push_str(
        "note TRUSTED means no current evidence of a problem under the tested assumptions and\n     evidence model; it does not mean proven correct.\n",
    );
    out
}

/// A cell's bounds with the model's own parameter names, so a reader can put the coordinates back
/// into the file they wrote.
fn region_text(model: &Model, bounds: &[[f64; 2]]) -> String {
    bounds
        .iter()
        .enumerate()
        .map(|(i, [lo, hi])| {
            let name = model
                .params
                .get(i)
                .map_or_else(|| format!("p{i}"), |p| p.name.clone());
            format!("{name} in [{lo}, {hi}]")
        })
        .collect::<Vec<_>>()
        .join("  ")
}
