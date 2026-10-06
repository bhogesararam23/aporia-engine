//! The manifest: what an experiment was, written so a reader can check it.
//!
//! A manifest is the difference between a directory of numbers and an experiment. It records the
//! model and its digests, the configuration that produced the samples, the calibration the evidence
//! was scored with, the channel correlations the fusion used, and a digest for every artefact in the
//! directory. Notably it does *not* record a conclusion — the atlas and the findings are files, and
//! the manifest describes how they came to exist.
//!
//! Everything here is a plain field with a JSON name, and the JSON is written and read by this
//! crate, so a manifest from a newer APORIA is readable by an older one as long as it recognises the
//! schema string and ignores what it does not.

use crate::json::Json;

/// The schema this build writes. A reader that does not recognise it should say so rather than
/// guess.
pub const SCHEMA: &str = "aporia.experiment/1";

/// Counts that make a report interpretable without opening the record file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub evaluations: u64,
    pub instruction_steps: u64,
    pub params: u64,
    pub outputs: u64,
    pub constraints: u64,
    pub relations: u64,
    pub cells: u64,
    pub samples: u64,
    pub findings: u64,
}

impl Counts {
    pub fn json(&self) -> Json {
        Json::object(vec![
            ("evaluations", Json::count(self.evaluations)),
            ("instruction_steps", Json::count(self.instruction_steps)),
            ("params", Json::count(self.params)),
            ("outputs", Json::count(self.outputs)),
            ("constraints", Json::count(self.constraints)),
            ("relations", Json::count(self.relations)),
            ("cells", Json::count(self.cells)),
            ("samples", Json::count(self.samples)),
            ("findings", Json::count(self.findings)),
        ])
    }

    fn from_json(value: &Json) -> Self {
        let field = |k: &str| value.get(k).and_then(Json::as_u64).unwrap_or(0);
        Self {
            evaluations: field("evaluations"),
            instruction_steps: field("instruction_steps"),
            params: field("params"),
            outputs: field("outputs"),
            constraints: field("constraints"),
            relations: field("relations"),
            cells: field("cells"),
            samples: field("samples"),
            findings: field("findings"),
        }
    }
}

/// What an experiment recorded about its own environment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Environment {
    pub os: String,
    pub arch: String,
    pub pointer_width: u64,
    /// Which toolchain produced the numbers. [`Environment::current`] leaves this empty because it
    /// runs nothing; a caller writing provenance fills it with
    /// [`Environment::with_toolchain`], and [`TOOLCHAIN_NOT_DETECTED`] is what lands in the artefact
    /// when the compiler cannot be asked.
    pub rust_channel: String,
    /// Instruction-set features, filled only by a caller that measured them. Nothing in this
    /// repository detects them, because CPUID is exactly the kind of thing an instrument should not
    /// guess at; an empty list means "nobody looked", and a run that needs it says so.
    pub cpu_features: Vec<String>,
    /// Which source produced the artefact: a commit, optionally suffixed `-dirty`.
    ///
    /// The toolchain says how the numbers were compiled; this says what was compiled. A reader of a
    /// published result is entitled to ask "which version of the search, the evidence model and the
    /// atlas policy ran", and before this field the answer had to come from prose beside the file.
    /// [`Environment::current`] leaves it empty for the same reason `rust_channel` is empty there —
    /// asking is a process call — and [`Environment::with_source_version`] fills it, with
    /// [`SOURCE_NOT_DETECTED`] where there is no repository to ask.
    pub source_commit: String,
    pub notes: Vec<(String, String)>,
}

/// What is recorded when the compiler cannot be asked. An empty string would read as a toolchain that
/// reported nothing, which is not a thing a compiler does, and would let `compare` call two runs equal
/// on a field that was never filled in.
pub const TOOLCHAIN_NOT_DETECTED: &str = "not detected";

/// What is recorded when the source of a run cannot be identified: not a checkout, or a checkout whose
/// git binary is not on PATH. Same reasoning as [`TOOLCHAIN_NOT_DETECTED`] — an empty string would be a
/// value a repository could produce, and "unknown" must not read as "identical".
pub const SOURCE_NOT_DETECTED: &str = "not detected";

impl Environment {
    /// What `std` knows without running anything. A caller may add the compiler channel and the CPU
    /// feature list it detected, because those change what a benchmark result means.
    #[must_use]
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            pointer_width: usize::BITS as u64,
            rust_channel: String::new(),
            cpu_features: Vec::new(),
            source_commit: String::new(),
            notes: Vec::new(),
        }
    }

    /// Ask the compiler which toolchain this is, and record it.
    ///
    /// `APORIA_TOOLCHAIN` wins, because the `rustc` on PATH is not necessarily the one that built the
    /// binary now running — a copied `target/`, a cross-compiled check, a CI image. Otherwise this
    /// reads `rustc -vV`, whose `release` and `commit-hash` together name the exact build, and records
    /// [`TOOLCHAIN_NOT_DETECTED`] when there is no compiler to ask. A measurement taken on one
    /// code generator is not the same measurement taken on another, which is why the field is
    /// compared, not just stored.
    #[must_use]
    pub fn with_toolchain(mut self) -> Self {
        if let Some(declared) = std::env::var_os("APORIA_TOOLCHAIN") {
            self.rust_channel = declared.to_string_lossy().into_owned();
            return self;
        }
        self.rust_channel = std::process::Command::new("rustc")
            .arg("-vV")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| parse_toolchain(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_else(|| TOOLCHAIN_NOT_DETECTED.to_string());
        self
    }

    /// Ask the checkout which source this run came from, and record it.
    ///
    /// `APORIA_SOURCE_COMMIT` wins, because a build system that exports the revision knows better than
    /// a `git` call made from whatever the process's working directory happens to be — a packaged run,
    /// a container, a copy of `target/` without a `.git`. Otherwise this reads `git rev-parse HEAD` and
    /// appends `-dirty` when tracked files differ from that commit: **a measurement taken on uncommitted
    /// source is not reproducible from the commit alone**, and a field that said `a49e20d` while the
    /// tree held 200 uncommitted lines would be worse than a field that says it cannot be identified.
    ///
    /// Absent or unparseable git output records [`SOURCE_NOT_DETECTED`], never an empty string: two
    /// runs whose provenance was never asked about must not compare equal on a field neither filled.
    #[must_use]
    pub fn with_source_version(mut self) -> Self {
        if let Some(declared) = std::env::var_os("APORIA_SOURCE_COMMIT") {
            self.source_commit = declared.to_string_lossy().into_owned();
            return self;
        }
        let head = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(commit) = head else {
            self.source_commit = SOURCE_NOT_DETECTED.to_string();
            return self;
        };
        let dirty = std::process::Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=no"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .is_some_and(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty());
        self.source_commit = if dirty {
            format!("{commit}-dirty")
        } else {
            commit
        };
        self
    }

    fn to_json(&self) -> Json {
        Json::object(vec![
            ("os", Json::text(self.os.clone())),
            ("arch", Json::text(self.arch.clone())),
            ("pointer_width", Json::count(self.pointer_width)),
            ("rust_channel", Json::text(self.rust_channel.clone())),
            ("source_commit", Json::text(self.source_commit.clone())),
            (
                "cpu_features",
                Json::Arr(
                    self.cpu_features
                        .iter()
                        .map(|f| Json::text(f.clone()))
                        .collect(),
                ),
            ),
            (
                "notes",
                Json::object(
                    self.notes
                        .iter()
                        .map(|(k, v)| (k.as_str(), Json::text(v.clone())))
                        .collect(),
                ),
            ),
        ])
    }

    fn from_json(value: &Json) -> Self {
        Self {
            os: value
                .get("os")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            arch: value
                .get("arch")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            pointer_width: value
                .get("pointer_width")
                .and_then(Json::as_u64)
                .unwrap_or(0),
            rust_channel: value
                .get("rust_channel")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            // An empty string here means "this artefact was written before the field existed", which is
            // a different statement from `SOURCE_NOT_DETECTED` ("a writer that has the field could not
            // fill it"). Unlike a finding's coordinates, where a default attaches evidence to the wrong
            // execution, a default here only records that nothing was recorded, and the committed
            // archives predate both fields.
            source_commit: value
                .get("source_commit")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            cpu_features: value
                .get("cpu_features")
                .and_then(Json::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Json::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            notes: value
                .get("notes")
                .and_then(|n| match n {
                    Json::Obj(fields) => Some(
                        fields
                            .iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default(),
        }
    }
}

/// The toolchain line from `rustc -vV`, as `release` plus the compiler's own commit.
///
/// Only these two fields are taken. `host` duplicates `arch`, `LLVM version` is a fact about a
/// dependency rather than about the build anyone could reproduce, and the date is already implied by
/// the hash.
fn parse_toolchain(text: &str) -> Option<String> {
    let mut release = None;
    let mut commit = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("release: ") {
            release = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("commit-hash: ") {
            // A nightly or locally-built compiler reports an empty hash. That is no commit, not a
            // commit whose name is a pair of parentheses.
            let short: String = v.trim().chars().take(8).collect();
            if !short.is_empty() {
                commit = Some(short);
            }
        }
    }
    let release = release.filter(|v| !v.is_empty())?;
    Some(commit.map_or(release.clone(), |c| format!("{release} ({c})")))
}

/// Everything a manifest says about one experiment.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub schema: String,
    pub tool_version: String,
    /// Milliseconds since the Unix epoch. Recorded, never used in a digest, so a replay of the same
    /// inputs at a different time still produces byte-identical artefacts.
    pub created_unix_ms: u64,
    pub model_name: String,
    pub model_sha256: String,
    pub air_sha256: String,
    /// The campaign configuration, as JSON from whoever ran it. Kept as a value rather than a
    /// struct so this crate does not have to know what a search strategy's fields are.
    pub config: Json,
    pub counts: Counts,
    /// How the model was executed, recorded because a replay has to use the same configuration and
    /// an f32 run is a different experiment from an f64 one.
    pub exec_fp: String,
    pub exec_max_steps: u64,
    pub environment: Environment,
    /// Fitted scale per channel, including the ones that were never fitted. Without this, a risk
    /// number from one experiment cannot be compared with a number from another.
    pub calibration: Vec<(String, f64)>,
    /// Estimated correlation per channel pair, upper triangle only.
    pub correlation: Vec<(String, String, f64)>,
    pub correlation_samples: u64,
    /// Relative path to digest, in the order the files were written.
    pub files: Vec<(String, String)>,
}

impl Manifest {
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object(vec![
            ("schema", Json::text(self.schema.clone())),
            ("tool_version", Json::text(self.tool_version.clone())),
            ("created_unix_ms", Json::count(self.created_unix_ms)),
            ("model_name", Json::text(self.model_name.clone())),
            ("model_sha256", Json::text(self.model_sha256.clone())),
            ("air_sha256", Json::text(self.air_sha256.clone())),
            ("config", self.config.clone()),
            ("counts", self.counts.json()),
            (
                "exec",
                Json::object(vec![
                    ("fp", Json::text(self.exec_fp.clone())),
                    ("max_steps", Json::count(self.exec_max_steps)),
                ]),
            ),
            ("environment", self.environment.to_json()),
            (
                "calibration",
                Json::object(
                    self.calibration
                        .iter()
                        .map(|(c, v)| (c.as_str(), Json::number(*v)))
                        .collect(),
                ),
            ),
            (
                "channel_correlation",
                Json::Arr(
                    self.correlation
                        .iter()
                        .map(|(a, b, v)| {
                            Json::object(vec![
                                ("a", Json::text(a.clone())),
                                ("b", Json::text(b.clone())),
                                ("rho", Json::number(*v)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("correlation_samples", Json::count(self.correlation_samples)),
            (
                "files",
                Json::object(
                    self.files
                        .iter()
                        .map(|(p, d)| (p.as_str(), Json::text(d.clone())))
                        .collect(),
                ),
            ),
        ])
    }

    /// Read a manifest. Unknown schema is returned as an error rather than parsed optimistically.
    pub fn from_json(value: &Json) -> Result<Self, crate::StoreError> {
        if value.get("schema").and_then(Json::as_str) != Some(SCHEMA) {
            return Err(crate::StoreError::Schema(
                value
                    .get("schema")
                    .and_then(Json::as_str)
                    .unwrap_or("<missing>")
                    .to_string(),
            ));
        }
        let text = |k: &str| {
            value
                .get(k)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Ok(Self {
            schema: text("schema"),
            tool_version: text("tool_version"),
            created_unix_ms: value
                .get("created_unix_ms")
                .and_then(Json::as_u64)
                .unwrap_or(0),
            model_name: text("model_name"),
            model_sha256: text("model_sha256"),
            air_sha256: text("air_sha256"),
            config: value.get("config").cloned().unwrap_or(Json::Null),
            counts: Counts::from_json(value.get("counts").unwrap_or(&Json::Null)),
            exec_fp: value
                .get("exec")
                .and_then(|e| e.get("fp"))
                .and_then(Json::as_str)
                .unwrap_or("f64")
                .to_string(),
            exec_max_steps: value
                .get("exec")
                .and_then(|e| e.get("max_steps"))
                .and_then(Json::as_u64)
                .unwrap_or(0),
            environment: Environment::from_json(value.get("environment").unwrap_or(&Json::Null)),
            calibration: value
                .get("calibration")
                .and_then(|c| match c {
                    Json::Obj(fields) => Some(
                        fields
                            .iter()
                            .map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0)))
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default(),
            correlation: value
                .get("channel_correlation")
                .and_then(Json::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let a = item.get("a")?.as_str()?.to_string();
                            let b = item.get("b")?.as_str()?.to_string();
                            let rho = item.get("rho")?.as_f64()?;
                            Some((a, b, rho))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            correlation_samples: value
                .get("correlation_samples")
                .and_then(Json::as_u64)
                .unwrap_or(0),
            files: value
                .get("files")
                .and_then(|f| match f {
                    Json::Obj(fields) => Some(
                        fields
                            .iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                            .collect(),
                    ),
                    _ => None,
                })
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toolchain_is_its_version_and_its_compiler_commit() {
        let text = "rustc 1.99.0 (b940084d7 2026-09-28)\n\
                    binary: rustc\n\
                    commit-hash: b940084d7eb6a299eb4bfeb8e34901bc051e7ac4\n\
                    commit-date: 2026-09-28\n\
                    host: x86_64-pc-windows-msvc\n\
                    release: 1.99.0\n\
                    LLVM version: 23.1.1\n";
        assert_eq!(parse_toolchain(text).as_deref(), Some("1.99.0 (b940084d)"));
        // A nightly or dev build reports an empty hash, and that is not a reason to lose the version.
        assert_eq!(
            parse_toolchain("release: 1.99.0\ncommit-hash: \n").as_deref(),
            Some("1.99.0")
        );
    }

    #[test]
    fn an_unparsable_compiler_answer_is_not_a_toolchain() {
        for text in [
            "",
            "rustc 1.99.0",
            "release: \ncommit-hash: abc\n",
            "not a rustc banner at all",
        ] {
            assert_eq!(parse_toolchain(text), None, "{text:?}");
        }
    }

    #[test]
    fn asking_for_the_toolchain_always_leaves_something_written() {
        // The reason the field exists: `compare` reports a difference in it, and an empty string makes
        // two runs agree about nothing. Whether a compiler was found is environment-dependent, so what
        // is asserted here is that no path through the detection writes a blank.
        let detected = Environment::current().with_toolchain();
        assert!(!detected.rust_channel.is_empty());
        assert!(
            detected.rust_channel.chars().any(|c| c.is_ascii_digit())
                || detected.rust_channel == TOOLCHAIN_NOT_DETECTED,
            "{}",
            detected.rust_channel
        );
    }

    #[test]
    fn asking_for_the_source_always_leaves_something_written() {
        // Same rule as the toolchain: a provenance field must never end up blank, because blank is
        // what an artefact written before the field existed looks like, and `compare` would call the
        // two equal. Whether git answers is environment-dependent, so what is asserted is that no path
        // through the detection writes nothing.
        let detected = Environment::current().with_source_version();
        assert!(!detected.source_commit.is_empty());
        assert!(
            detected
                .source_commit
                .chars()
                .any(|c| c.is_ascii_hexdigit())
                || detected.source_commit == SOURCE_NOT_DETECTED,
            "{}",
            detected.source_commit
        );
        // Inside this repository, on a checkout, the answer is a commit — and `-dirty` when the tree
        // carries uncommitted tracked changes, which is the case a bare hash would misrepresent.
        let in_repo = std::path::Path::new(".git").exists()
            || std::path::Path::new("../.git").exists()
            || std::env::var_os("APORIA_SOURCE_COMMIT").is_some();
        if in_repo && detected.source_commit != SOURCE_NOT_DETECTED {
            let hash = detected.source_commit.trim_end_matches("-dirty");
            assert!(
                hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit()),
                "{}",
                detected.source_commit
            );
        }
    }

    #[test]
    fn an_artefact_written_before_the_source_field_existed_still_reads() {
        // The committed archives have no `source_commit`. Reading them must yield "nothing was
        // recorded", which is an empty string, and must not invent a commit or a refusal.
        let text = r#"{"os":"linux","arch":"x86_64","pointer_width":64,"notes":{}}"#;
        let loaded = Environment::from_json(&Json::parse(text).unwrap());
        assert_eq!(loaded.source_commit, "");
        assert_eq!(loaded.rust_channel, "");
        let written = Environment {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            pointer_width: 64,
            ..Environment::current()
        };
        let Json::Obj(fields) = written.to_json() else {
            panic!("an environment is an object");
        };
        assert!(
            fields.iter().any(|(k, _)| k == "source_commit"),
            "a fresh artefact always writes the field, empty or not: {fields:?}"
        );
    }

    fn manifest() -> Manifest {
        Manifest {
            schema: SCHEMA.to_string(),
            tool_version: "0.1.0".to_string(),
            created_unix_ms: 1_760_000_000_000,
            model_name: "projectile".to_string(),
            model_sha256: "aa".repeat(32),
            air_sha256: "bb".repeat(32),
            config: Json::object(vec![
                ("budget", Json::count(400)),
                ("strategy", Json::text("adaptive")),
            ]),
            exec_fp: "f64".to_string(),
            exec_max_steps: 50_000_000,
            counts: Counts {
                evaluations: 400,
                instruction_steps: 12_874_501,
                params: 3,
                outputs: 1,
                constraints: 2,
                relations: 1,
                cells: 37,
                samples: 400,
                findings: 4,
            },
            environment: Environment {
                os: "windows".to_string(),
                arch: "x86_64".to_string(),
                pointer_width: 64,
                rust_channel: "1.99.0".to_string(),
                cpu_features: vec!["avx2".to_string(), "fma".to_string()],
                source_commit: "a49e20d".to_string(),
                notes: vec![("gpu".to_string(), "none present".to_string())],
            },
            calibration: vec![
                ("behavioral".to_string(), 1.98),
                ("physical".to_string(), 1.0),
                ("numerical".to_string(), 0.02),
                ("differential".to_string(), 1.0),
                ("sensitivity".to_string(), 2.4),
            ],
            correlation: vec![("behavioral".to_string(), "sensitivity".to_string(), 0.72)],
            correlation_samples: 400,
            files: vec![
                ("model.ap".to_string(), "cc".repeat(32)),
                ("observations.bin".to_string(), "dd".repeat(32)),
            ],
        }
    }

    #[test]
    fn a_manifest_round_trips_through_its_own_json() {
        let m = manifest();
        let text = m.to_json().to_pretty();
        let back = Manifest::from_json(&Json::parse(&text).unwrap()).unwrap();
        assert_eq!(m, back, "a written manifest must be readable without loss");
    }

    #[test]
    fn the_schema_is_checked_rather_than_assumed() {
        let mut m = manifest();
        m.schema = "aporia.experiment/99".to_string();
        let e = Manifest::from_json(&m.to_json()).unwrap_err();
        match e {
            crate::StoreError::Schema(s) => assert_eq!(s, "aporia.experiment/99"),
            other => panic!("expected a schema refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_field_reads_as_zero_not_as_a_panic() {
        // An older or hand-edited manifest should degrade loudly in the counts, not crash the tool
        // that is trying to report on it.
        let value = Json::parse(r#"{"schema":"aporia.experiment/1"}"#).unwrap();
        let m = Manifest::from_json(&value).unwrap();
        assert_eq!(m.counts, Counts::default());
        assert!(m.files.is_empty());
    }

    #[test]
    fn file_order_is_preserved_so_two_manifests_can_be_diffed() {
        let text = manifest().to_json().to_pretty();
        let at = text.find(r#""files""#).unwrap();
        let model = text.find("model.ap").unwrap();
        let observations = text.find("observations.bin").unwrap();
        assert!(at < model && model < observations);
    }
}
