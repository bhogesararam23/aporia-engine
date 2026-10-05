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
use crate::store::{Loaded, StoredFinding};
use std::path::Path;

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

/// The findings of two runs, put in pairs by the region they describe.
///
/// The region is the identity APORIA can actually assert. `cell` is an index into one archive's own
/// atlas, so two runs that refined in a different order number the same stretch of space
/// differently, and pairing on it would invent a correspondence the bytes do not support. The bounds
/// and the representative are coordinates in the model's parameter space, which both runs share
/// whenever they share a parameter count, so equal geometry means the same claim about the same
/// place even when the report ranked it differently.
///
/// Within a pair every remaining field is compared, including `cell` and the report position: a
/// region that moved down the ranking, or that the atlas re-cut, is information rather than noise.
/// Regions one run did not find at all come out as `OnlyA` / `OnlyB`.
#[must_use]
pub fn findings(a: &[StoredFinding], b: &[StoredFinding]) -> Vec<Field> {
    // The count first, so a section with nothing to pair still reports: two runs that both found no
    // region have been compared and agreed, which is a different statement from a comparison that
    // could not be made at all.
    let mut out = vec![Field::scalar(
        Section::Findings,
        "count",
        a.len().to_string(),
        b.len().to_string(),
    )];
    let mut waiting: Vec<(String, usize)> =
        b.iter().enumerate().map(|(j, f)| (region(f), j)).collect();
    for found in a {
        let key = region(found);
        match waiting.iter().position(|(r, _)| *r == key) {
            Some(at) => {
                let (_, j) = waiting.remove(at);
                let paired = &b[j];
                let path = format!("region {key}");
                for (field, xa, xb) in finding_fields(found, paired) {
                    out.push(Field::scalar(
                        Section::Findings,
                        &format!("{path}.{field}"),
                        xa,
                        xb,
                    ));
                }
                // A minimised case is absent rather than empty on either side, and absence is a
                // verdict of its own: one run verified a smaller counterexample and the other did
                // not, which is not the same statement as the two claiming different cases.
                out.push(Field::optional(
                    Section::Findings,
                    &format!("{path}.case"),
                    found.case.clone(),
                    paired.case.clone(),
                ));
            }
            None => out.push(Field::optional(
                Section::Findings,
                &format!("region {key}"),
                Some(summary(found)),
                None,
            )),
        }
    }
    for (_, j) in waiting {
        let missed = &b[j];
        out.push(Field::optional(
            Section::Findings,
            &format!("region {}", region(missed)),
            None,
            Some(summary(missed)),
        ));
    }
    out
}

/// Every field of a finding except the region itself, as text from the archive's own writer. Numbers
/// go through `Json`, so a comparison cannot print a value in a form the file it read does not use.
fn finding_fields(a: &StoredFinding, b: &StoredFinding) -> Vec<(&'static str, String, String)> {
    let (ta, tb) = (a.to_json(), b.to_json());
    let value = |json: &Json, key: &str| {
        let held = json.get(key).cloned().unwrap_or(Json::Null);
        leaf(&held)
    };
    // The name a reader sees, then the key the archive actually stores it under. `report_index` is
    // spelled differently on purpose: in the file it is `index`, and here it is the position the
    // finding held in the report that wrote it.
    [
        ("report_index", "index"),
        ("cell", "cell"),
        ("label", "label"),
        ("observation", "observation"),
        ("online_risk", "online_risk"),
        ("final_risk", "final_risk"),
        ("samples", "samples"),
        ("evidence", "evidence"),
    ]
    .into_iter()
    .map(|(name, key)| (name, value(&ta, key), value(&tb, key)))
    .collect()
}

/// A finding as one value, for a region the other run did not report.
fn summary(f: &StoredFinding) -> String {
    format!(
        "{} risk {} {} samples",
        f.label,
        Json::number(f.final_risk).to_compact(),
        f.samples
    )
}

/// The map each run drew, read from the totals the run itself wrote down.
///
/// Coverage comes from `summary.json` rather than from re-counting rows of `atlas.csv` here, because
/// the fractions in the summary are what the campaign measured over the space it was given. The table
/// and the band file are compared by digest in [`artefacts`]: the partition belongs to
/// `aporia-boundary`, and this crate is not going to grow a second parser for it.
#[must_use]
pub fn atlas(a: &Loaded, b: &Loaded) -> Vec<Field> {
    let s = Section::Atlas;
    let coverage = |l: &Loaded| l.summary.get("coverage").cloned().unwrap_or(Json::Null);
    let (ca, cb) = (coverage(a), coverage(b));
    let mut out = Vec::new();
    for key in [
        "trusted_fraction",
        "suspicious_fraction",
        "unknown_fraction",
        "cells",
        "resolved_fraction",
    ] {
        out.push(nested(s, &format!("coverage.{key}"), &ca, &cb, key));
    }
    let summary_leaf = |l: &Loaded, key: &str| {
        l.summary.get(key).map(|v| match v {
            Json::Arr(items) => items.len().to_string(),
            other => leaf(other),
        })
    };
    out.push(Field::optional(
        s,
        "bands",
        summary_leaf(a, "bands"),
        summary_leaf(b, "bands"),
    ));
    out.push(Field::optional(
        s,
        "finding_bytes",
        summary_leaf(a, "finding_bytes"),
        summary_leaf(b, "finding_bytes"),
    ));
    out
}

/// One value out of two JSON objects, with `null` read as "not recorded". An archive writes `null`
/// for a field the run did not have -- a first finding, when there were none -- and printing the word
/// `null` as though it were a value would make two such archives look like they had something to
/// compare.
fn nested(section: Section, path: &str, a: &Json, b: &Json, key: &str) -> Field {
    let held = |j: &Json| match j.get(key) {
        None | Some(Json::Null) => None,
        Some(v) => Some(leaf(v)),
    };
    Field::optional(section, path, held(a), held(b))
}

/// The artefacts themselves: every file each manifest digests, by the hash that manifest recorded,
/// plus the sizes of what the two directories actually hold.
///
/// A digest difference names the file without saying what moved inside it, which is the right order
/// of trust: the sections above say what changed in fields a reader can interpret, and this says
/// whether anything that no field covers -- the record bytes, the atlas table, the decision log --
/// also disagrees.
#[must_use]
pub fn artefacts(a: &Loaded, b: &Loaded) -> Vec<Field> {
    let mut out = Vec::new();
    let names = union(
        &a.manifest
            .files
            .iter()
            .map(|(p, _)| p.clone())
            .collect::<Vec<_>>(),
        &b.manifest
            .files
            .iter()
            .map(|(p, _)| p.clone())
            .collect::<Vec<_>>(),
    );
    for name in names {
        let held = |l: &Loaded| {
            l.manifest
                .files
                .iter()
                .find(|(p, _)| *p == name)
                .map(|(_, d)| d.clone())
        };
        out.push(Field::optional(
            Section::Artefacts,
            &format!("digest[{name}]"),
            held(a),
            held(b),
        ));
    }
    out.push(Field::scalar(
        Section::Artefacts,
        "records[observations.bin]",
        a.records.items.len().to_string(),
        b.records.items.len().to_string(),
    ));
    out.push(Field::scalar(
        Section::Artefacts,
        "decisions[decisions.jsonl]",
        a.decisions.len().to_string(),
        b.decisions.len().to_string(),
    ));
    out.push(Field::scalar(
        Section::Artefacts,
        "findings[*.apx]",
        a.findings.len().to_string(),
        b.findings.len().to_string(),
    ));
    out
}

/// Two archives read against each other.
///
/// The assembly lives here rather than in whoever renders it because the section order, and the rule
/// about what may be compared at all, are part of what a comparison *means*. A command line that
/// decided those would be a second definition of the same operation.
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub fields: Vec<Field>,
    /// Sections this pair cannot be asked about, each with the reason it was skipped. A structural
    /// difference makes some comparisons meaningless rather than merely different, and saying which
    /// ones beats reporting the resulting noise as a result.
    pub skipped: Vec<String>,
}
impl Comparison {
    #[must_use]
    pub fn new(a: &Loaded, b: &Loaded) -> Self {
        let mut fields = Vec::new();
        let mut skipped = Vec::new();
        fields.extend(identity(&a.manifest, &b.manifest));
        fields.extend(configuration(&a.manifest, &b.manifest));
        fields.extend(size(&a.manifest, &b.manifest));
        fields.extend(calibration(&a.manifest, &b.manifest));
        // Regions are coordinates in a parameter space, and two archives with different numbers of
        // parameters are not describing the same space. Pairing their findings anyway would turn a
        // structural difference into a list of claims about regions one of the runs never had.
        if a.manifest.counts.params == b.manifest.counts.params {
            fields.extend(findings(&a.findings, &b.findings));
            fields.extend(atlas(a, b));
        } else {
            let because = format!(
                "{} parameters versus {}, so the two runs name coordinates in different spaces",
                a.manifest.counts.params, b.manifest.counts.params
            );
            skipped.push(format!("findings: {because}"));
            skipped.push(format!("atlas: {because}"));
        }
        fields.extend(artefacts(a, b));
        Self { fields, skipped }
    }

    /// True only when every compared field agrees and nothing had to be skipped: a pair of archives
    /// that were not compared completely is not a pair that was found to be the same.
    #[must_use]
    pub fn identical(&self) -> bool {
        self.skipped.is_empty() && self.fields.iter().all(|f| f.change == Change::Same)
    }

    /// How many fields changed, how many only the first archive holds, how many only the second.
    #[must_use]
    pub fn tally(&self) -> (usize, usize, usize) {
        (
            self.fields
                .iter()
                .filter(|f| f.change == Change::Changed)
                .count(),
            self.fields
                .iter()
                .filter(|f| f.change == Change::OnlyA)
                .count(),
            self.fields
                .iter()
                .filter(|f| f.change == Change::OnlyB)
                .count(),
        )
    }

    /// The fields that are not `Same`, in the order they were produced.
    #[must_use]
    pub fn differences(&self) -> Vec<&Field> {
        self.fields
            .iter()
            .filter(|f| f.change != Change::Same)
            .collect()
    }

    /// What the pair turned out to be, decided once here so no caller has to re-derive it from the
    /// field list and get it subtly wrong.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        if self.identical() {
            Verdict::Identical
        } else if self.differences().is_empty() {
            // Nothing disagreed, but something could not be asked: an incomplete comparison is not a
            // match.
            Verdict::NotFullyComparable
        } else {
            Verdict::Differing
        }
    }

    /// The comparison as text: one line per section, and beneath each section only the fields there
    /// is something to say about.
    ///
    /// Deterministic by construction -- sections in [`Section::ORDER`], fields in the order they were
    /// produced, no timestamps and no path canonicalisation -- because this text is what a test
    /// asserts on and what a reader pastes into a report.
    #[must_use]
    pub fn describe(&self, a: &Path, b: &Path) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "A  {}", a.display());
        let _ = writeln!(out, "B  {}", b.display());
        for section in Section::ORDER {
            let mine: Vec<&Field> = self
                .fields
                .iter()
                .filter(|f| f.section == section && f.change != Change::Same)
                .collect();
            let total = self.fields.iter().filter(|f| f.section == section).count();
            if total == 0 {
                let _ = writeln!(out, "{}  not compared", section.name());
                continue;
            }
            if mine.is_empty() {
                let _ = writeln!(out, "{}  same ({total} field(s))", section.name());
                continue;
            }
            let _ = writeln!(
                out,
                "{}  {} difference(s) of {total}",
                section.name(),
                mine.len()
            );
            for f in mine {
                let _ = match (f.a.as_deref(), f.b.as_deref()) {
                    (Some(x), Some(y)) => writeln!(out, "  {}  {x} -> {y}", f.path),
                    (Some(x), None) => writeln!(out, "  {}  {x}  (only in A)", f.path),
                    (None, Some(y)) => writeln!(out, "  {}  (only in B) {y}", f.path),
                    (None, None) => writeln!(out, "  {}  absent on both sides", f.path),
                };
            }
        }
        for reason in &self.skipped {
            let _ = writeln!(out, "skipped  {reason}");
        }
        let (changed, only_a, only_b) = self.tally();
        let _ = match self.verdict() {
            Verdict::Identical => writeln!(
                out,
                "verdict  IDENTICAL: {} field(s) agree",
                self.fields.len()
            ),
            Verdict::Differing => writeln!(
                out,
                "verdict  DIFFERENT: {changed} field(s) changed, {only_a} only in A, {only_b} only \
                 in B"
            ),
            Verdict::NotFullyComparable => writeln!(
                out,
                "verdict  NOT FULLY COMPARABLE: nothing disagreed, but {} section part(s) could not \
                 be asked",
                self.skipped.len()
            ),
        };
        let _ = writeln!(
            out,
            "source  every value above was read from the two archives; neither model was executed"
        );
        out
    }
}

/// How a pair of archives ended up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every compared field agreed, and every section could be compared.
    Identical,
    /// At least one field disagreed.
    Differing,
    /// No field disagreed, but a section could not be asked -- which is not the same as agreement.
    NotFullyComparable,
}

impl Verdict {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Identical => "IDENTICAL",
            Self::Differing => "DIFFERENT",
            Self::NotFullyComparable => "NOT FULLY COMPARABLE",
        }
    }
}

/// The region a finding is about, in the form both archives can produce from their own bytes.
fn region(f: &StoredFinding) -> String {
    f.bounds_text()
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
