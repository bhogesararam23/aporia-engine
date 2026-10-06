//! Reporting a stored run, without executing anything.
//!
//! `aporia run` prints what a campaign concluded while it is happening; this reads an archive and
//! prints what it says. The separation is the point: a report that can only be produced by re-running
//! the analysis is not a report of the archived experiment, it is a new experiment that happens to
//! share a name. Everything here comes from bytes on disk — the manifest's config and counts, the
//! calibration the run fitted, its channel correlations, its bands, its decisions, its finding blocks —
//! and nothing re-derives a number the archive already holds.
//!
//! Integrity is still checked first, because presenting a corrupted archive as a result would be worse
//! than not reading it at all. Unlike `replay`, a report exits 0 when the archive is intact and says
//! nothing about suspicion: whether a region is flagged is a question about the model, and it was
//! answered when the run wrote the archive.

use aporia_store::{Json, Loaded};

use crate::run::Exit;

/// `aporia report <archive-dir>`.
pub fn command(dir: &std::path::Path, out: &mut dyn std::io::Write) -> Exit {
    let loaded = match Loaded::open(dir) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!(
                "aporia: cannot read the archive at `{}`: {e}",
                dir.display()
            );
            return Exit::Input;
        }
    };
    let integrity = loaded.integrity_problems();
    let _ = writeln!(
        out,
        "run  {}  model {}  tool {}",
        dir.display(),
        loaded.manifest.model_name,
        loaded.manifest.tool_version
    );
    let _ = writeln!(out, "config  {}", flatten(&loaded.manifest.config));
    let counts = &loaded.manifest.counts;
    let _ = writeln!(
        out,
        "size  {} evaluations  {} instruction steps  {} samples in {} cells  {} findings",
        counts.evaluations, counts.instruction_steps, counts.samples, counts.cells, counts.findings
    );
    let _ = writeln!(
        out,
        "contents  {} parameters, {} outputs, {} rules, {} relations",
        counts.params, counts.outputs, counts.constraints, counts.relations
    );
    let _ = writeln!(
        out,
        "exec  fp={}  max_steps_per_evaluation={}",
        loaded.manifest.exec_fp, loaded.manifest.exec_max_steps
    );
    // Who computed this, and from which source. The archive has always held the notes — a
    // program-executed run records `execution program \`…\`` there — and printed none of it, so the
    // one command a reader runs to ask "what does this archive say" answered the question by opening
    // manifest.json by hand. The toolchain and source commit belong on the same line for the same
    // reason: a measurement is attributable to a compiler and a tree, or it is not attributable.
    let _ = writeln!(out, "{}", build_line(&loaded.manifest.environment));
    let _ = writeln!(
        out,
        "calibration  {}",
        loaded
            .manifest
            .calibration
            .iter()
            .map(|(channel, scale)| format!("{channel}={scale}"))
            .collect::<Vec<_>>()
            .join("  ")
    );
    let _ = writeln!(
        out,
        "correlation  {} channel pairs on {} shared samples",
        loaded.manifest.correlation.len(),
        loaded.manifest.correlation_samples
    );
    let bands = loaded.bands_csv.lines().count().saturating_sub(1);
    let _ = writeln!(out, "bands  {bands}");
    let _ = writeln!(out, "decisions  {}", loaded.decisions.len());
    if !integrity.is_empty() {
        for problem in &integrity {
            let _ = writeln!(out, "  {problem}");
        }
        let _ = writeln!(
            out,
            "integrity  {} problem(s): this archive is not the one that was written",
            integrity.len()
        );
        return Exit::Integrity;
    }
    let _ = writeln!(
        out,
        "integrity  {} file(s) digested against the manifest: all match",
        loaded.manifest.files.len()
    );
    for finding in loaded.findings.iter().take(5) {
        let _ = write!(out, "{}", finding.render(&loaded.root));
    }
    if loaded.findings.len() > 5 {
        let _ = writeln!(
            out,
            "… and {} further finding(s) in the archive",
            loaded.findings.len() - 5
        );
    }
    let _ = writeln!(
        out,
        "source  every number above was read from the archive, not recomputed"
    );
    Exit::Clean
}

/// `key=value` pairs from the archived config, in the order the run wrote them. Nested values would
/// need a second shape, and the archived configs are flat, so a non-object just prints as itself.
fn flatten(config: &Json) -> String {
    match config {
        Json::Obj(fields) => fields
            .iter()
            .map(|(k, v)| format!("{k}={}", leaf(v)))
            .collect::<Vec<_>>()
            .join("  "),
        other => other.to_compact(),
    }
}

fn leaf(value: &Json) -> String {
    match value {
        Json::Str(s) => s.clone(),
        other => other.to_compact(),
    }
}

/// The provenance line: what environment, which compiler, which source, and whatever else the run
/// recorded about itself.
///
/// Fields the archive does not carry are omitted rather than guessed. An artefact written before the
/// toolchain and source fields existed says nothing about them, and "not recorded" is a different
/// statement from a blank or a default — it is the same distinction `compare` draws between a field
/// that differs and a field only one side has.
fn build_line(environment: &aporia_store::Environment) -> String {
    let mut parts = vec![format!(
        "build  os={} arch={} width={}",
        environment.os, environment.arch, environment.pointer_width
    )];
    if !environment.rust_channel.is_empty() {
        parts.push(format!("toolchain={}", environment.rust_channel));
    }
    if !environment.source_commit.is_empty() {
        parts.push(format!("source={}", environment.source_commit));
    }
    for (name, value) in &environment.notes {
        parts.push(format!("{name}={value}"));
    }
    parts.join("  ")
}
