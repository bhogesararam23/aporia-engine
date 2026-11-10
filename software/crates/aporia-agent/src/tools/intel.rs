//! Discovery tools: what this project is, what it depends on, how it is tested, what has been run.
//!
//! These reuse the inspection module's heuristics rather than restating them, so the initial scan
//! and the tools cannot disagree about what a manifest or a test is. Every discovery is labelled
//! heuristic where it is one: a file named `sweep.py` is a candidate experiment, not a confirmed
//! one, and the output says so.

use super::files::{SCAN_CAP, walk_files};
use super::result::{ErrorClass, ToolFailure, ToolOutput, ToolResult};
use super::{Tool, ToolContext, ToolMeta};
use crate::inspect;
use crate::scope::Risk;
use aporia_store::Json;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const OUTPUT_LIMIT: usize = 32 * 1024;
const PREFIX: u64 = 4096;
const MAX_MANIFESTS: usize = 12;
const MAX_CONFIGS: usize = 30;
const MAX_EXPERIMENT_READS: usize = 8;

/// Register the discovery tools.
pub fn register(registry: &mut super::Registry) -> Result<(), String> {
    registry.register(Box::new(InspectProject::new()))?;
    registry.register(Box::new(InspectDependencies::new()))?;
    registry.register(Box::new(InspectConfiguration::new()))?;
    registry.register(Box::new(InspectTestStructure::new()))?;
    registry.register(Box::new(InspectExperimentFiles::new()))?;
    Ok(())
}

fn workspace_files(context: &ToolContext) -> (Vec<String>, bool) {
    let root = context.sandbox().root();
    walk_files(root, root, SCAN_CAP)
}

fn read_prefix(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read as _;
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).to_string())
}

struct InspectProject {
    meta: ToolMeta,
}

impl InspectProject {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "inspect_project",
                "Re-run the deterministic project scan on the current workspace and return its numbered facts. Use after changes.",
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for InspectProject {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, _arguments: &Json) -> ToolResult {
        let inspection = inspect::inspect(context.sandbox().root()).map_err(|error| {
            ToolFailure::new(
                ErrorClass::Io,
                format!("the workspace scan failed: {error}"),
            )
        })?;
        let mut text = inspection.rendered(16 * 1024);
        let _ = writeln!(
            text,
            "[scan: {} files, truncated: {}]",
            inspection.summary.files, inspection.summary.truncated
        );
        Ok(ToolOutput {
            text,
            data: inspection.to_json(),
            artifacts: Vec::new(),
        })
    }
}

struct InspectDependencies {
    meta: ToolMeta,
}

impl InspectDependencies {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "inspect_dependencies",
                "Find dependency manifests (Cargo, Python, Node, Go, CMake, Make) and show bounded prefixes of their contents.",
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for InspectDependencies {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, _arguments: &Json) -> ToolResult {
        let (files, truncated) = workspace_files(context);
        let mut text = String::new();
        let mut entries = Vec::new();
        for relative in files.iter().filter(|path| {
            let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
            inspect::is_manifest(&name)
        }) {
            if entries.len() >= MAX_MANIFESTS {
                let _ = writeln!(text, "[stopped at {MAX_MANIFESTS} manifests]");
                break;
            }
            let name = relative
                .rsplit('/')
                .next()
                .unwrap_or(relative)
                .to_ascii_lowercase();
            let kind = inspect::manifest_kind(&name);
            let Ok(path) = context.sandbox().resolve(relative) else {
                continue;
            };
            let content = read_prefix(&path, PREFIX).unwrap_or_default();
            let _ = writeln!(text, "{relative} ({kind}):\n{content}");
            entries.push(Json::object(vec![
                ("path", Json::text(relative)),
                ("kind", Json::text(kind)),
                ("content", Json::text(content)),
            ]));
        }
        if entries.is_empty() {
            text = "no recognised dependency manifest was found\n".to_string();
        }
        if truncated {
            let _ = writeln!(text, "[the file walk was truncated; more may exist]");
        }
        Ok(ToolOutput {
            text,
            data: Json::object(vec![
                ("manifests", Json::Arr(entries)),
                ("truncated", Json::Bool(truncated)),
            ]),
            artifacts: Vec::new(),
        })
    }
}

struct InspectConfiguration {
    meta: ToolMeta,
}

impl InspectConfiguration {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "inspect_configuration",
                "Heuristic: list configuration-shaped files (.toml/.yaml/.yml/.ini/.cfg/.conf/.json) with bounded prefixes of the small ones.",
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for InspectConfiguration {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, _arguments: &Json) -> ToolResult {
        let (files, truncated) = workspace_files(context);
        let configs: Vec<&String> = files
            .iter()
            .filter(|path| {
                Path::new(path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "json"
                        )
                    })
            })
            .take(MAX_CONFIGS + 1)
            .collect();
        let mut text = String::from("configuration-shaped files (heuristic by extension):\n");
        let mut entries = Vec::new();
        for (index, relative) in configs.iter().enumerate() {
            if index >= MAX_CONFIGS {
                let _ = writeln!(text, "[stopped at {MAX_CONFIGS} files]");
                break;
            }
            let Ok(path) = context.sandbox().resolve(relative) else {
                continue;
            };
            let bytes = fs::metadata(&path).map_or(0, |meta| meta.len());
            let prefix = if bytes <= 64 * 1024 && entries.len() < 10 {
                read_prefix(&path, 1024)
            } else {
                None
            };
            match &prefix {
                Some(content) => {
                    let _ = writeln!(text, "{relative} ({bytes} bytes):\n{content}");
                }
                None => {
                    let _ = writeln!(text, "{relative} ({bytes} bytes)");
                }
            }
            entries.push(Json::object(vec![
                ("path", Json::text(*relative)),
                ("bytes", Json::count(bytes)),
                ("prefix", prefix.as_ref().map_or(Json::Null, Json::text)),
            ]));
        }
        if entries.is_empty() {
            text = "no configuration-shaped file was found\n".to_string();
        }
        if truncated {
            let _ = writeln!(text, "[the file walk was truncated; more may exist]");
        }
        Ok(ToolOutput {
            text,
            data: Json::object(vec![("files", Json::Arr(entries))]),
            artifacts: Vec::new(),
        })
    }
}

struct InspectTestStructure {
    meta: ToolMeta,
}

impl InspectTestStructure {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "inspect_test_structure",
                "List test-shaped files (same heuristic as the initial scan) and the test runners the manifests suggest.",
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for InspectTestStructure {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, _arguments: &Json) -> ToolResult {
        let (files, truncated) = workspace_files(context);
        let tests: Vec<&String> = files
            .iter()
            .filter(|path| inspect::looks_like_test(Path::new(path)))
            .collect();
        let mut runners: Vec<&str> = Vec::new();
        for relative in &files {
            let name = relative
                .rsplit('/')
                .next()
                .unwrap_or(relative)
                .to_ascii_lowercase();
            match name.as_str() {
                "cargo.toml" => runners.push("cargo test (Rust manifest present)"),
                "pyproject.toml" | "pytest.ini" | "setup.cfg" | "tox.ini" => {
                    runners.push("python -m pytest or unittest (Python project files present)");
                }
                "package.json" => {
                    runners.push("npm test (Node manifest present; check its scripts)");
                }
                "go.mod" => runners.push("go test (Go module present)"),
                _ => {}
            }
        }
        runners.sort_unstable();
        runners.dedup();
        let mut text = String::from("test-shaped files (heuristic):\n");
        for test in &tests {
            let _ = writeln!(text, "{test}");
        }
        if tests.is_empty() {
            text.push_str("none found\n");
        }
        let _ = writeln!(text, "\nsuggested runners (verify before running):");
        if runners.is_empty() {
            text.push_str("none recognised; ask the user or propose a documented command\n");
        } else {
            for runner in &runners {
                let _ = writeln!(text, "- {runner}");
            }
        }
        if truncated {
            let _ = writeln!(text, "[the file walk was truncated; more may exist]");
        }
        Ok(ToolOutput {
            text,
            data: Json::object(vec![
                (
                    "tests",
                    Json::Arr(tests.iter().map(|path| Json::text(*path)).collect()),
                ),
                (
                    "runners",
                    Json::Arr(runners.iter().map(|runner| Json::text(*runner)).collect()),
                ),
            ]),
            artifacts: Vec::new(),
        })
    }
}

struct InspectExperimentFiles {
    meta: ToolMeta,
}

impl InspectExperimentFiles {
    fn new() -> Self {
        Self {
            meta: ToolMeta::new(
                "inspect_experiment_files",
                "Heuristic: find experiment-shaped files (experiment/bench/sweep/eval/baseline/results in the path) and show bounded prefixes.",
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
                Risk::ReadOnly,
                OUTPUT_LIMIT,
            ),
        }
    }
}

impl Tool for InspectExperimentFiles {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn run(&self, context: &mut ToolContext, _arguments: &Json) -> ToolResult {
        const NEEDLES: [&str; 6] = [
            "experiment",
            "bench",
            "sweep",
            "eval",
            "baseline",
            "results",
        ];
        let (files, truncated) = workspace_files(context);
        let candidates: Vec<&String> = files
            .iter()
            .filter(|path| {
                let lower = path.to_ascii_lowercase();
                NEEDLES.iter().any(|needle| lower.contains(needle))
            })
            .collect();
        let mut text =
            String::from("experiment-shaped files (heuristic by name; confirm by reading):\n");
        let mut entries = Vec::new();
        let mut reads = 0;
        for relative in &candidates {
            let Ok(path) = context.sandbox().resolve(relative) else {
                continue;
            };
            let bytes = fs::metadata(&path).map_or(0, |meta| meta.len());
            let prefix = if reads < MAX_EXPERIMENT_READS && bytes <= 64 * 1024 {
                reads += 1;
                read_prefix(&path, 1024)
            } else {
                None
            };
            match &prefix {
                Some(content) => {
                    let _ = writeln!(text, "{relative} ({bytes} bytes):\n{content}");
                }
                None => {
                    let _ = writeln!(text, "{relative} ({bytes} bytes)");
                }
            }
            entries.push(Json::object(vec![
                ("path", Json::text(*relative)),
                ("bytes", Json::count(bytes)),
                ("prefix", prefix.as_ref().map_or(Json::Null, Json::text)),
            ]));
        }
        if entries.is_empty() {
            text =
                "no experiment-shaped file was found; this is a heuristic, not proof of absence\n"
                    .to_string();
        }
        if truncated {
            let _ = writeln!(text, "[the file walk was truncated; more may exist]");
        }
        Ok(ToolOutput {
            text,
            data: Json::object(vec![("files", Json::Arr(entries))]),
            artifacts: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolCall;
    use crate::tools::{Registry, dispatch};
    use aporia_sandbox::Sandbox;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "aporia-intel-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("temp");
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (Temp, Temp, Sandbox, PathBuf) {
        let source = Temp::new("src");
        fs::create_dir_all(source.0.join("tests")).expect("dir");
        fs::create_dir_all(source.0.join("experiments")).expect("dir");
        fs::create_dir_all(source.0.join("results")).expect("dir");
        fs::write(
            source.0.join("pyproject.toml"),
            b"[project]\nname = \"demo\"\n",
        )
        .expect("file");
        fs::write(source.0.join("config.yaml"), b"n: 100\n").expect("file");
        fs::write(
            source.0.join("tests/test_rule.py"),
            b"def test_x():\n    assert True\n",
        )
        .expect("file");
        fs::write(source.0.join("experiments/sweep.py"), b"print('sweep')\n").expect("file");
        fs::write(source.0.join("results/run1.json"), b"{\"error\": 0.1}\n").expect("file");
        let workspace = Temp::new("ws");
        let (sandbox, _) = Sandbox::create(&source.0, &workspace.0.join("copy")).expect("sandbox");
        let instrument = std::env::current_exe().expect("exe");
        (source, workspace, sandbox, instrument)
    }

    fn run_tool(sandbox: &Sandbox, source: &Path, instrument: &Path, name: &str) -> ToolResult {
        let mut registry = Registry::new();
        register(&mut registry).expect("register");
        let cancel = AtomicBool::new(false);
        let mut context = ToolContext::new(sandbox, source, instrument, None, &cancel);
        let call = ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments: "{}".to_string(),
        };
        dispatch(&registry, &mut context, &call)
    }

    #[test]
    fn the_project_scan_returns_numbered_facts() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output = run_tool(&sandbox, &source.0, &instrument, "inspect_project").expect("ok");
        assert!(output.text.contains("F1"));
        assert!(output.text.contains("pyproject.toml"));
    }

    #[test]
    fn dependencies_are_found_with_their_contents() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output =
            run_tool(&sandbox, &source.0, &instrument, "inspect_dependencies").expect("ok");
        assert!(output.text.contains("pyproject.toml (Python manifest)"));
        assert!(output.text.contains("name = \"demo\""));
    }

    #[test]
    fn configuration_files_are_listed_as_heuristic() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output =
            run_tool(&sandbox, &source.0, &instrument, "inspect_configuration").expect("ok");
        assert!(output.text.contains("config.yaml"));
        assert!(output.text.contains("heuristic"));
    }

    #[test]
    fn test_structure_names_files_and_runners() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output =
            run_tool(&sandbox, &source.0, &instrument, "inspect_test_structure").expect("ok");
        assert!(output.text.contains("tests/test_rule.py"));
        assert!(output.text.contains("pytest"));
    }

    #[test]
    fn experiment_files_include_results_and_sweeps() {
        let (source, _ws, sandbox, instrument) = fixture();
        let output =
            run_tool(&sandbox, &source.0, &instrument, "inspect_experiment_files").expect("ok");
        assert!(output.text.contains("experiments/sweep.py"));
        assert!(output.text.contains("results/run1.json"));
        assert!(output.text.contains("heuristic"));
    }
}
