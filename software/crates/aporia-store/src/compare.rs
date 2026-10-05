//! Comparing two stored runs, from their bytes only.
//!
//! `aporia report` answers "what does this archive say"; this answers "what is different between
//! these two". Both are read-only: nothing here re-executes a model, re-fits a calibrator or
//! re-derives a risk. That restriction is what makes the answer trustworthy and also what limits it —
//! two archives can be compared only on the facts they each recorded, and a difference here is a
//! difference in what was *written down*, which is not always the same as a difference in what
//! happened.
//!
//! Three properties the shape of [`Field`] exists to keep:
//!
//! - **Ordered.** Sections come out in [`Section::ORDER`] and fields within a section in the order the
//!   archive holds them, so two runs of the comparison on the same pair produce the same text. A
//!   machine reading a diff needs that; so does a test.
//! - **Four-valued.** Every field is the same, changed, present only in the first archive, or present
//!   only in the second. "Different" and "missing" are different claims and are not collapsed.
//! - **Value-visible.** Both sides' values travel with the verdict, because "config.budget changed"
//!   is not actionable; "config.budget 40 -> 640" is.

use crate::json::Json;
use crate::manifest::Manifest;

/// Which part of the archive a compared field belongs to, in reporting order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// What the run was: schema, tool, model identity.
    Identity,
    /// The instructions the run was given: campaign configuration and execution mode.
    Run,
    /// What the run cost and produced, as counts.
    Size,
    /// The scales and channel correlations the scores were produced with.
    Calibration,
    /// The suspicious regions.
    Findings,
    /// The map: cell labels and boundary bands.
    Atlas,
    /// The artefacts themselves, by recorded digest.
    Artefacts,
}

impl Section {
    /// The order a comparison is reported in: identity first, because it decides whether anything
    /// after it is about the same experiment.
    pub const ORDER: [Section; 7] = [
        Section::Identity,
        Section::Run,
        Section::Size,
        Section::Calibration,
        Section::Findings,
        Section::Atlas,
        Section::Artefacts,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Run => "run",
            Self::Size => "size",
            Self::Calibration => "calibration",
            Self::Findings => "findings",
            Self::Atlas => "atlas",
            Self::Artefacts => "artefacts",
        }
    }
}

/// How one field of two archives relates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Same,
    Changed,
    /// Recorded by the first archive and not the second.
    OnlyA,
    /// Recorded by the second archive and not the first.
    OnlyB,
}

impl Change {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Changed => "changed",
            Self::OnlyA => "only in A",
            Self::OnlyB => "only in B",
        }
    }
}

/// One compared fact, with both sides' values.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub section: Section,
    /// Dotted address inside the archive: `config.budget`, `calibration.physical`.
    pub path: String,
    /// Text as the archive holds it, or `None` when that archive does not hold the field at all.
    pub a: Option<String>,
    pub b: Option<String>,
    pub change: Change,
}

impl Field {
    /// A field both sides were asked about, with its verdict derived from the two values.
    #[must_use]
    pub fn scalar(section: Section, path: &str, a: String, b: String) -> Self {
        Self {
            section,
            path: path.to_string(),
            change: if a == b {
                Change::Same
            } else {
                Change::Changed
            },
            a: Some(a),
            b: Some(b),
        }
    }

    /// A field one side may not have recorded. Absence is a verdict of its own, never `Changed`.
    #[must_use]
    pub fn optional(section: Section, path: &str, a: Option<String>, b: Option<String>) -> Self {
        let change = match (&a, &b) {
            (Some(x), Some(y)) if x == y => Change::Same,
            (Some(_), Some(_)) => Change::Changed,
            (Some(_), None) => Change::OnlyA,
            (None, Some(_)) => Change::OnlyB,
            (None, None) => Change::Same,
        };
        Self {
            section,
            path: path.to_string(),
            a,
            b,
            change,
        }
    }
}

/// What a comparison of two manifests says. Manifests are compared field by field rather than by
/// digest because a reader wants the name of what moved, not the fact that something did.
#[must_use]
pub fn identity(a: &Manifest, b: &Manifest) -> Vec<Field> {
    let s = Section::Identity;
    vec![
        Field::scalar(s, "schema", a.schema.clone(), b.schema.clone()),
        Field::scalar(
            s,
            "tool_version",
            a.tool_version.clone(),
            b.tool_version.clone(),
        ),
        Field::scalar(s, "model", a.model_name.clone(), b.model_name.clone()),
        Field::scalar(
            s,
            "model_sha256",
            a.model_sha256.clone(),
            b.model_sha256.clone(),
        ),
        Field::scalar(s, "air_sha256", a.air_sha256.clone(), b.air_sha256.clone()),
    ]
}

/// The instructions the two runs were given: the campaign configuration, and how the model was
/// executed. Configuration is compared key by key because it is stored as a JSON object whose keys
/// belong to whoever ran the campaign.
#[must_use]
pub fn configuration(a: &Manifest, b: &Manifest) -> Vec<Field> {
    let mut out = leaves(Section::Run, "config", &a.config, &b.config);
    let s = Section::Run;
    out.push(Field::scalar(
        s,
        "exec.fp",
        a.exec_fp.clone(),
        b.exec_fp.clone(),
    ));
    out.push(Field::scalar(
        s,
        "exec.max_steps",
        a.exec_max_steps.to_string(),
        b.exec_max_steps.to_string(),
    ));
    out.extend(environment(a, b));
    out
}

/// How each run described the machine it happened on. Notes are the part a reader compares: an
/// archived benchmark names its entry, strategy and seed there, and the command line names who did
/// the arithmetic there.
fn environment(a: &Manifest, b: &Manifest) -> Vec<Field> {
    let mut out = vec![
        Field::scalar(
            Section::Run,
            "os",
            a.environment.os.clone(),
            b.environment.os.clone(),
        ),
        Field::scalar(
            Section::Run,
            "arch",
            a.environment.arch.clone(),
            b.environment.arch.clone(),
        ),
        Field::scalar(
            Section::Run,
            "rust_channel",
            a.environment.rust_channel.clone(),
            b.environment.rust_channel.clone(),
        ),
    ];
    let keys = union(
        &a.environment
            .notes
            .iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>(),
        &b.environment
            .notes
            .iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>(),
    );
    for key in keys {
        out.push(Field::optional(
            Section::Run,
            &format!("note.{key}"),
            a.environment
                .notes
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone()),
            b.environment
                .notes
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone()),
        ));
    }
    out
}

/// The counts, which make a difference in cost or coverage visible without opening the record file.
#[must_use]
pub fn size(a: &Manifest, b: &Manifest) -> Vec<Field> {
    let s = Section::Size;
    let num = |x: u64| x.to_string();
    [
        ("evaluations", a.counts.evaluations, b.counts.evaluations),
        (
            "instruction_steps",
            a.counts.instruction_steps,
            b.counts.instruction_steps,
        ),
        ("params", a.counts.params, b.counts.params),
        ("outputs", a.counts.outputs, b.counts.outputs),
        ("constraints", a.counts.constraints, b.counts.constraints),
        ("relations", a.counts.relations, b.counts.relations),
        ("cells", a.counts.cells, b.counts.cells),
        ("samples", a.counts.samples, b.counts.samples),
        ("findings", a.counts.findings, b.counts.findings),
    ]
    .iter()
    .map(|(path, x, y)| Field::scalar(s, path, num(*x), num(*y)))
    .collect()
}

/// The fitted scale per channel, and the channel correlations the fusion used. Two risk numbers
/// from two different calibrations are not the same quantity, so a comparison that skipped this
/// section would be reporting differences that may be entirely explained by it.
#[must_use]
pub fn calibration(a: &Manifest, b: &Manifest) -> Vec<Field> {
    let mut out = Vec::new();
    let channels = union(
        &a.calibration
            .iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>(),
        &b.calibration
            .iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>(),
    );
    for channel in channels {
        let of = |m: &Manifest| {
            m.calibration
                .iter()
                .find(|(k, _)| *k == channel)
                .map(|(_, v)| Json::number(*v).to_compact())
        };
        out.push(Field::optional(
            Section::Calibration,
            &format!("scale.{channel}"),
            of(a),
            of(b),
        ));
    }
    let pairs = union(
        &a.correlation
            .iter()
            .map(|(x, y, _)| format!("{x}/{y}"))
            .collect::<Vec<_>>(),
        &b.correlation
            .iter()
            .map(|(x, y, _)| format!("{x}/{y}"))
            .collect::<Vec<_>>(),
    );
    for pair in pairs {
        let of = |m: &Manifest| {
            m.correlation
                .iter()
                .find(|(x, y, _)| format!("{x}/{y}") == pair)
                .map(|(_, _, rho)| Json::number(*rho).to_compact())
        };
        out.push(Field::optional(
            Section::Calibration,
            &format!("correlation.{pair}"),
            of(a),
            of(b),
        ));
    }
    out.push(Field::scalar(
        Section::Calibration,
        "correlation_samples",
        a.correlation_samples.to_string(),
        b.correlation_samples.to_string(),
    ));
    out
}

/// Leaf values of two JSON objects, addressed by dotted path. The archived configs APORIA writes are
/// flat, so a value that is itself an object or array is compared as one compact JSON string rather
/// than descended into: guessing a nested shape here would be inventing a field the format does not
/// have.
fn leaves(section: Section, prefix: &str, a: &Json, b: &Json) -> Vec<Field> {
    if let (Json::Obj(x), Json::Obj(y)) = (a, b) {
        let keys = union(
            &x.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
            &y.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        );
        return keys
            .iter()
            .map(|key| {
                Field::optional(
                    section,
                    &format!("{prefix}.{key}"),
                    a.get(key).map(leaf),
                    b.get(key).map(leaf),
                )
            })
            .collect();
    }
    vec![Field::optional(
        section,
        prefix,
        Some(leaf(a)),
        Some(leaf(b)),
    )]
}

/// A value as a reader would say it: text unwrapped from its quotes, anything else as compact JSON.
fn leaf(value: &Json) -> String {
    match value {
        Json::Str(s) => s.clone(),
        other => other.to_compact(),
    }
}

/// The keys of both sides, first the ones from `a` in order, then the ones only `b` has. Stable
/// without sorting, so the report order follows the order the archives wrote their fields in.
fn union(a: &[String], b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for key in a.iter().chain(b.iter()) {
        if !out.contains(key) {
            out.push(key.clone());
        }
    }
    out
}
