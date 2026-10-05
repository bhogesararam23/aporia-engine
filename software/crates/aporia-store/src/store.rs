//! The experiment directory: one run, written once, opened later.
//!
//! ```text
//! <root>/
//!   manifest.json        what the run was, with a digest of every artefact
//!   model.ap             the source text, byte for byte, as handed to the tool
//!   model.air            the A-IR that was actually executed
//!   observations.bin     every execution, in evaluation order (see `records`)
//!   atlas.csv            the Trust Atlas, one cell per line
//!   bands.csv            the boundary bands read out of it
//!   decisions.jsonl      one line per search decision, in order
//!   findings/NNNNNN.apx  one archive per suspicious region
//!   summary.json         the machine-readable totals a report is built from, read back by `Loaded`
//! ```
//!
//! Two rules the layout exists to enforce. A directory is never overwritten: `create` refuses a root
//! that already holds a manifest, because "the old run got replaced by the new one" is how an
//! experiment stops being evidence. And every artefact is digested into the manifest as it is
//! written, so `open` can tell whether the files still match the run that produced them.

use crate::digest::sha256_hex;
use crate::json::Json;
use crate::manifest::{Counts, Environment, Manifest, SCHEMA};
use crate::records;
use aporia_boundary::{Band, Coverage, Label};
use aporia_evidence::{Calibrator, Channel, ChannelCorrelation, Evidence};
use aporia_ir::Model;
use aporia_runtime::Records;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Anything that stops an archive being written or read the way it should be.
#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    /// The bytes were ours but did not hold together.
    Format(String),
    Truncated {
        at: usize,
        needed: usize,
    },
    Schema(String),
    /// A file in the directory no longer matches the digest the manifest recorded.
    Digest {
        path: String,
        expected: String,
        found: String,
    },
    /// Something already exists where a new experiment wanted to be written.
    Exists(PathBuf),
    Missing(PathBuf),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Format(m) => write!(f, "malformed archive: {m}"),
            Self::Truncated { at, needed } => {
                write!(f, "file ends at byte {at}, needed {needed} more")
            }
            Self::Schema(s) => write!(f, "unsupported manifest schema {s:?}"),
            Self::Digest {
                path,
                expected,
                found,
            } => write!(
                f,
                "{path} digests to {found} but the manifest recorded {expected}"
            ),
            Self::Exists(p) => write!(f, "{} already holds an experiment", p.display()),
            Self::Missing(p) => write!(f, "{} is missing", p.display()),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<crate::json::JsonError> for StoreError {
    fn from(e: crate::json::JsonError) -> Self {
        Self::Format(e.message)
    }
}

/// One suspicious region, as the archive keeps it.
///
/// A finding is stored rather than recomputed because it is the object a person reads: the region,
/// the worst evaluation inside it, the evidence that produced it, and — when a minimiser has run —
/// the reduced description of the failure.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredFinding {
    pub index: u64,
    pub cell: u32,
    pub bounds: Vec<[f64; 2]>,
    pub representative: Vec<f64>,
    pub observation: u64,
    pub online_risk: f64,
    pub final_risk: f64,
    pub samples: u64,
    /// The label the atlas gave the cell, kept as text so an older reader still parses the file.
    pub label: String,
    /// A minimised description of the failure, if one was produced.
    pub case: Option<String>,
    pub evidence: Vec<Evidence>,
    /// Evidence read back out of a `.apx` file, as JSON. A stored finding keeps its `Subject` only as
    /// the report key it was written with, so re-hydration is not possible without inventing a
    /// parser for that key; keeping the raw objects means nothing is lost on a read, and writing a
    /// loaded finding again reproduces the same file.
    pub raw_evidence: Vec<Json>,
}

impl StoredFinding {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("schema", Json::text("aporia.finding/1")),
            ("index", Json::count(self.index)),
            ("cell", Json::count(u64::from(self.cell))),
            ("bounds", Json::Arr(self.bounds.iter().map(edge).collect())),
            (
                "representative",
                Json::Arr(
                    self.representative
                        .iter()
                        .map(|v| Json::number(*v))
                        .collect(),
                ),
            ),
            ("observation", Json::count(self.observation)),
            ("online_risk", Json::number(self.online_risk)),
            ("final_risk", Json::number(self.final_risk)),
            ("samples", Json::count(self.samples)),
            ("label", Json::text(self.label.clone())),
            (
                "case",
                self.case
                    .as_deref()
                    .map_or(Json::Null, |c| Json::text(c.to_string())),
            ),
            (
                "evidence",
                if self.evidence.is_empty() {
                    Json::Arr(self.raw_evidence.clone())
                } else {
                    Json::Arr(self.evidence.iter().map(evidence_json).collect())
                },
            ),
        ])
    }

    pub fn from_json(value: &Json) -> Result<Self, StoreError> {
        if value.get("schema").and_then(Json::as_str) != Some("aporia.finding/1") {
            return Err(StoreError::Schema(
                value
                    .get("schema")
                    .and_then(Json::as_str)
                    .unwrap_or("<missing>")
                    .to_string(),
            ));
        }
        let bounds = value
            .get("bounds")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|axis| {
                        let lows = axis.get("lo")?.as_array()?;
                        let highs = axis.get("hi")?.as_array()?;
                        let lo: Vec<f64> = lows.iter().filter_map(Json::as_f64).collect();
                        let hi: Vec<f64> = highs.iter().filter_map(Json::as_f64).collect();
                        Some([lo.first().copied()?, hi.first().copied()?])
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            index: value.get("index").and_then(Json::as_u64).unwrap_or(0),
            cell: value.get("cell").and_then(Json::as_u64).unwrap_or(0) as u32,
            bounds,
            representative: value
                .get("representative")
                .and_then(Json::as_array)
                .map(|items| items.iter().filter_map(Json::as_f64).collect())
                .unwrap_or_default(),
            observation: value.get("observation").and_then(Json::as_u64).unwrap_or(0),
            online_risk: value
                .get("online_risk")
                .and_then(Json::as_f64)
                .unwrap_or(0.0),
            final_risk: value
                .get("final_risk")
                .and_then(Json::as_f64)
                .unwrap_or(0.0),
            samples: value.get("samples").and_then(Json::as_u64).unwrap_or(0),
            label: value
                .get("label")
                .and_then(Json::as_str)
                .unwrap_or("UNKNOWN")
                .to_string(),
            case: value.get("case").and_then(Json::as_str).map(str::to_string),
            evidence: Vec::new(),
            raw_evidence: value
                .get("evidence")
                .and_then(Json::as_array)
                .map(<[Json]>::to_vec)
                .unwrap_or_default(),
        })
    }

    /// The report block the spec asks for (§21), as text.
    ///
    /// `archive` is the directory the finding belongs to. The replay line has to name it rather than
    /// the `.apx` file because replaying one finding is not the experiment: reproduction re-executes
    /// the archived A-IR at *every* archived point and compares, and the finding's own evidence cites
    /// observations from that whole set. A command naming a single finding file would be a command
    /// that runs nothing.
    #[must_use]
    pub fn render(&self, archive: &Path) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "APORIA FINDING #{:06}   status {}   cell c{}",
            self.index, self.label, self.cell
        );
        let _ = writeln!(out, "Region: {}", self.bounds_text());
        if let Some(case) = &self.case {
            out.push_str("Minimal reproducible case\n");
            for line in case.lines() {
                let _ = writeln!(out, "  {line}");
            }
        }
        out.push_str("Evidence\n");
        // A finding read back out of a `.apx` file carries its evidence as JSON objects rather than
        // reconstructed values, because `Subject` round-trips to a report key and not back into a
        // variant. Both shapes are flattened into the same rows here so a finding reads the same
        // whether it was just produced or just loaded.
        let rows: Vec<(String, String, String, String)> = if self.evidence.is_empty() {
            self.raw_evidence
                .iter()
                .map(|e| {
                    (
                        e.get("channel")
                            .and_then(Json::as_str)
                            .unwrap_or("?")
                            .to_string(),
                        e.get("subject")
                            .and_then(Json::as_str)
                            .unwrap_or("?")
                            .to_string(),
                        e.get("strength")
                            .and_then(Json::as_f64)
                            .map_or_else(|| "?".to_string(), |v| format!("{v:.3}")),
                        e.get("detail")
                            .and_then(Json::as_str)
                            .unwrap_or("")
                            .to_string(),
                    )
                })
                .collect()
        } else {
            self.evidence
                .iter()
                .map(|e| {
                    (
                        e.channel.name().to_string(),
                        e.subject.key(),
                        e.level().name().to_string(),
                        e.detail.clone(),
                    )
                })
                .collect()
        };
        for (channel, subject, strength, detail) in rows {
            let _ = writeln!(out, "  {channel:<12} {subject:<16} {strength:>8}  {detail}");
        }
        let _ = writeln!(
            out,
            "Replay: aporia replay {}   (this finding: findings/{:06}.apx)",
            archive.display(),
            self.index
        );
        out
    }

    /// The region as text: `[lo, hi]` per axis, in axis order. `compare` uses it as the identity of a
    /// finding, because it is the part of a finding two archives can legitimately be asked to agree
    /// on -- unlike `cell`, which is an index into one archive's own atlas and so means nothing in
    /// another's.
    #[must_use]
    pub fn bounds_text(&self) -> String {
        self.bounds
            .iter()
            .map(|[lo, hi]| format!("[{lo}, {hi}]"))
            .collect::<Vec<_>>()
            .join(" x ")
    }
}

fn edge(axis: &[f64; 2]) -> Json {
    Json::object(vec![
        ("lo", Json::Arr(vec![Json::number(axis[0])])),
        ("hi", Json::Arr(vec![Json::number(axis[1])])),
    ])
}

/// Evidence as data. Kept verbose on purpose: a stored finding that cannot show its magnitudes and
/// the observations it consumed is a conclusion without an argument.
fn evidence_json(e: &Evidence) -> Json {
    Json::object(vec![
        ("channel", Json::text(e.channel.name())),
        ("subject", Json::text(e.subject.key())),
        ("magnitude", Json::number(e.magnitude)),
        ("strength", Json::number(e.strength)),
        ("fixed", Json::Bool(e.is_fixed())),
        (
            "observations",
            Json::Arr(e.observations.iter().map(|o| Json::count(*o)).collect()),
        ),
        ("detail", Json::text(e.detail.clone())),
    ])
}

#[derive(Debug)]
/// Everything needed to write one experiment. The caller assembles it from a campaign so this crate
/// stays out of the search's business.
pub struct Run<'a> {
    pub model: &'a Model,
    pub model_text: &'a str,
    pub air_text: &'a str,
    pub records: &'a Records,
    pub config: Json,
    pub coverage: Coverage,
    /// The atlas table, produced by `aporia_boundary::Atlas::to_csv`. The store writes it and never
    /// interprets it, so the partition logic stays in one crate.
    pub atlas_csv: &'a str,
    pub bands: &'a [Band],
    pub findings: &'a [StoredFinding],
    /// One JSON object per search decision, already shaped by whoever made it.
    pub decisions: &'a [Json],
    pub calibrator: &'a Calibrator,
    pub correlation: &'a ChannelCorrelation,
    pub exec: aporia_runtime::ExecConfig,
    pub evaluations: u64,
    pub instruction_steps: u64,
    pub environment: Environment,
    pub created_unix_ms: u64,
}

/// What was written, and how big each file ended up.
#[derive(Clone, Debug)]
pub struct Receipt {
    pub root: PathBuf,
    pub files: Vec<(String, u64)>,
}

impl Receipt {
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|(_, b)| b).sum()
    }
}

/// A directory being written.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    written: Vec<(String, String, u64)>,
}

impl Store {
    /// Start a new experiment directory. Refuses a root that already holds a manifest, so a rerun
    /// cannot silently replace an experiment someone is reading.
    pub fn create(root: &Path) -> Result<Self, StoreError> {
        if root.join("manifest.json").exists() {
            return Err(StoreError::Exists(root.to_path_buf()));
        }
        fs::create_dir_all(root.join("findings"))?;
        Ok(Self {
            root: root.to_path_buf(),
            written: Vec::new(),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Write the whole run. Every artefact is digested as it goes, and the manifest is written last
    /// so a directory with a manifest is a complete directory.
    pub fn write(&mut self, run: &Run<'_>) -> Result<Receipt, StoreError> {
        self.text("model.ap", run.model_text)?;
        self.text("model.air", run.air_text)?;

        let mut buffer = Vec::new();
        records::write(&mut buffer, run.records)?;
        self.bytes("observations.bin", &buffer)?;

        self.text("atlas.csv", run.atlas_csv)?;

        self.text("bands.csv", &bands_csv(run.bands, run.model))?;

        let mut lines = String::new();
        for decision in run.decisions {
            lines.push_str(&decision.to_compact());
            lines.push('\n');
        }
        self.text("decisions.jsonl", &lines)?;

        let mut finding_bytes = 0u64;
        for finding in run.findings {
            let name = format!("{:06}.apx", finding.index);
            let text = finding.to_json().to_pretty();
            finding_bytes += self.text(&format!("findings/{name}"), &text)?;
        }

        let counts = Counts {
            evaluations: run.evaluations,
            instruction_steps: run.instruction_steps,
            params: run.model.params.len() as u64,
            outputs: run.model.outputs.len() as u64,
            constraints: run.model.constraints.len() as u64,
            relations: run.model.relations.len() as u64,
            cells: run.coverage.cells as u64,
            samples: run.coverage.samples as u64,
            findings: run.findings.len() as u64,
        };

        let summary = summary_json(run, &counts, finding_bytes);
        self.text("summary.json", &summary.to_pretty())?;

        let manifest = Manifest {
            schema: SCHEMA.to_string(),
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
            created_unix_ms: run.created_unix_ms,
            model_name: run.model.name.clone(),
            model_sha256: sha256_hex(run.model_text.as_bytes()),
            air_sha256: sha256_hex(run.air_text.as_bytes()),
            config: run.config.clone(),
            exec_fp: match run.exec.fp {
                aporia_runtime::FpMode::F64 => "f64".to_string(),
                aporia_runtime::FpMode::F32 => "f32".to_string(),
            },
            exec_max_steps: run.exec.max_steps,
            counts,
            environment: run.environment.clone(),
            calibration: Channel::ALL
                .iter()
                .map(|c| (c.name().to_string(), run.calibrator.scale_of(*c)))
                .collect(),
            correlation: correlation_pairs(run.correlation),
            correlation_samples: run.correlation.samples() as u64,
            files: self
                .written
                .iter()
                .map(|(p, d, _)| (p.clone(), d.clone()))
                .collect(),
        };
        self.text("manifest.json", &manifest.to_json().to_pretty())?;
        Ok(Receipt {
            root: self.root.clone(),
            files: self
                .written
                .iter()
                .map(|(p, _, n)| (p.clone(), *n))
                .collect(),
        })
    }

    /// Write text into the directory and digest it. Returns the byte count.
    pub fn text(&mut self, relative: &str, contents: &str) -> Result<u64, StoreError> {
        self.bytes(relative, contents.as_bytes())
    }

    pub fn bytes(&mut self, relative: &str, contents: &[u8]) -> Result<u64, StoreError> {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create(&path)?;
        file.write_all(contents)?;
        file.flush()?;
        self.written.push((
            relative.to_string(),
            sha256_hex(contents),
            contents.len() as u64,
        ));
        Ok(contents.len() as u64)
    }
}

/// A directory that has been read back.
#[derive(Debug)]
pub struct Loaded {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub model_text: String,
    pub air_text: String,
    pub records: Records,
    pub atlas_csv: String,
    pub bands_csv: String,
    /// `summary.json` as written. The archive's own totals -- coverage fractions, the band list, the
    /// size of the findings -- are here rather than recomputable from the table, because the run that
    /// measured them is the only thing entitled to state them.
    pub summary: Json,
    pub decisions: Vec<Json>,
    pub findings: Vec<StoredFinding>,
    /// The bytes of each file as read, kept so digest verification does not re-read from disk.
    digests: Vec<(String, String)>,
}

impl Loaded {
    /// Read an experiment directory. A missing manifest is `Missing`, an unrecognised schema is an
    /// error rather than a guess.
    pub fn open(root: &Path) -> Result<Self, StoreError> {
        let manifest_path = root.join("manifest.json");
        if !manifest_path.exists() {
            return Err(StoreError::Missing(manifest_path));
        }
        let manifest_text = fs::read_to_string(&manifest_path)?;
        let manifest = Manifest::from_json(&Json::parse(&manifest_text)?)?;
        let model_text = fs::read_to_string(root.join("model.ap"))?;
        let air_text = fs::read_to_string(root.join("model.air"))?;
        let observation_bytes = fs::read(root.join("observations.bin"))?;
        let records = records::read(&observation_bytes)?;
        let atlas_csv = fs::read_to_string(root.join("atlas.csv"))?;
        let bands_csv = fs::read_to_string(root.join("bands.csv"))?;
        let summary_text = fs::read_to_string(root.join("summary.json"))?;
        let summary = Json::parse(&summary_text)?;
        let decisions = fs::read_to_string(root.join("decisions.jsonl"))?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(Json::parse_line)
            .collect::<Result<Vec<_>, _>>()?;
        let mut findings = Vec::new();
        let mut names = fs::read_dir(root.join("findings"))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "apx"))
            .collect::<Vec<_>>();
        names.sort();
        for path in names {
            let text = fs::read_to_string(&path)?;
            findings.push(StoredFinding::from_json(&Json::parse(&text)?)?);
        }
        let mut digests = Vec::new();
        for (path, _) in &manifest.files {
            let bytes = fs::read(root.join(path))?;
            digests.push((path.clone(), sha256_hex(&bytes)));
        }
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
            model_text,
            air_text,
            records,
            atlas_csv,
            bands_csv,
            summary,
            decisions,
            findings,
            digests,
        })
    }

    /// Every artefact whose recorded digest no longer matches, as human-readable problems.
    ///
    /// The model source is checked against the manifest too, because the most common way an
    /// experiment stops being reproducible is someone editing the model after the run.
    #[must_use]
    pub fn integrity_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (path, found) in &self.digests {
            if let Some((_, expected)) = self.manifest.files.iter().find(|(p, _)| p == path)
                && expected != found
            {
                out.push(format!(
                    "{path} digests to {found} but the manifest recorded {expected}"
                ));
            }
        }
        let model_found = sha256_hex(self.model_text.as_bytes());
        if model_found != self.manifest.model_sha256 {
            out.push(format!(
                "model.ap digests to {model_found} but the manifest recorded {}",
                self.manifest.model_sha256
            ));
        }
        let air_found = sha256_hex(self.air_text.as_bytes());
        if air_found != self.manifest.air_sha256 {
            out.push(format!(
                "model.air digests to {air_found} but the manifest recorded {}",
                self.manifest.air_sha256
            ));
        }
        out
    }

    #[must_use]
    pub fn samples(&self) -> usize {
        self.records.items.len()
    }
}

/// The bands, one per line, with the parameter name spelled out.
#[must_use]
pub fn bands_csv(bands: &[Band], model: &Model) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "axis,axis_name,lo,hi,facing_lo,facing_hi,transitions,width"
    );
    for b in bands {
        let name = model
            .params
            .get(b.axis as usize)
            .map_or_else(|| format!("p{}", b.axis), |p| p.name.clone());
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{}",
            b.axis,
            csv_escape(&name),
            b.lo,
            b.hi,
            b.facing[0],
            b.facing[1],
            b.transitions,
            b.hi - b.lo
        );
    }
    out
}

/// A field containing a comma or a quote needs quoting, or the CSV is unreadable.
fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn correlation_pairs(correlation: &ChannelCorrelation) -> Vec<(String, String, f64)> {
    let mut out = Vec::new();
    for (i, a) in Channel::ALL.iter().enumerate() {
        for b in Channel::ALL.iter().skip(i + 1) {
            let rho = correlation.between(*a, *b);
            if rho != 0.0 {
                out.push((a.name().to_string(), b.name().to_string(), rho));
            }
        }
    }
    out
}

fn summary_json(run: &Run<'_>, counts: &Counts, finding_bytes: u64) -> Json {
    Json::object(vec![
        ("schema", Json::text("aporia.summary/1")),
        ("model", Json::text(run.model.name.clone())),
        ("counts", counts.json()),
        (
            "coverage",
            Json::object(vec![
                ("trusted_fraction", Json::number(run.coverage.trusted)),
                ("suspicious_fraction", Json::number(run.coverage.suspicious)),
                ("unknown_fraction", Json::number(run.coverage.unknown)),
                ("cells", Json::count(run.coverage.cells as u64)),
                ("resolved_fraction", Json::number(run.coverage.resolved())),
            ]),
        ),
        (
            "bands",
            Json::Arr(
                run.bands
                    .iter()
                    .map(|b| {
                        Json::object(vec![
                            ("axis", Json::count(b.axis as u64)),
                            ("lo", Json::number(b.lo)),
                            ("hi", Json::number(b.hi)),
                            ("width", Json::number(b.hi - b.lo)),
                            ("transitions", Json::count(b.transitions as u64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("findings", Json::count(run.findings.len() as u64)),
        ("finding_bytes", Json::count(finding_bytes)),
        (
            "first_finding",
            run.findings
                .iter()
                .map(|f| {
                    Json::object(vec![
                        ("observation", Json::count(f.observation)),
                        ("evaluation", Json::count(f.index)),
                    ])
                })
                .next()
                .unwrap_or(Json::Null),
        ),
    ])
}

/// The label as text, so a reader that only knows strings can still group findings.
#[must_use]
pub fn label_text(label: Label) -> &'static str {
    match label {
        Label::Trusted => "TRUSTED",
        Label::Suspicious => "SUSPICIOUS",
        Label::Unknown => "UNKNOWN",
    }
}
