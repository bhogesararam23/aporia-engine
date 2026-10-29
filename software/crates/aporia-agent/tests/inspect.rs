//! Facts are numbered deterministically, bounded, and read without executing anything.

use aporia_agent::inspect::{InspectError, InspectLimits, inspect, inspect_with_limits};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-agent-inspect-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, contents).expect("write");
}

fn project() -> Temp {
    let temp = Temp::new("project");
    let root = temp.path();
    write(
        &root.join("Cargo.toml"),
        b"[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    );
    write(&root.join("README.md"), b"# Demo\n\nA project.\n");
    write(
        &root.join("model.ap"),
        b"model demo { input x : m in [0, 1] }\n",
    );
    write(
        &root.join("tests/test_demo.py"),
        b"def test_it():\n    pass\n",
    );
    write(&root.join("src/lib.rs"), b"pub fn one() -> u32 { 1 }\n");
    write(&root.join(".git/HEAD"), b"ref: refs/heads/main\n");
    write(
        &root.join(".git/refs/heads/main"),
        format!("{}\n", "a".repeat(40)).as_bytes(),
    );
    write(&root.join("target/junk"), b"ignored");
    temp
}

#[test]
fn facts_are_deterministic_and_carry_the_real_project_shapes() {
    let temp = project();
    let first = inspect(temp.path()).expect("inspect");
    let second = inspect(temp.path()).expect("inspect again");
    assert_eq!(first, second, "the same tree produced different facts");

    let git = first
        .facts
        .iter()
        .find(|fact| fact.kind == aporia_agent::FactKind::Git)
        .expect("git fact");
    assert_eq!(git.path, ".git/HEAD");
    assert_eq!(git.detail, "a".repeat(40));

    let manifest = first
        .facts
        .iter()
        .find(|fact| fact.kind == aporia_agent::FactKind::Manifest)
        .expect("manifest");
    assert_eq!(manifest.path, "Cargo.toml");
    assert!(
        manifest.detail.contains("Rust manifest: demo"),
        "{}",
        manifest.detail
    );

    let readme = first
        .facts
        .iter()
        .find(|fact| fact.kind == aporia_agent::FactKind::Readme)
        .expect("readme");
    assert!(readme.detail.contains("# Demo"), "{}", readme.detail);

    let model = first
        .facts
        .iter()
        .find(|fact| fact.kind == aporia_agent::FactKind::Model)
        .expect("model");
    assert_eq!(model.path, "model.ap");
    assert!(model.detail.contains("model demo"), "{}", model.detail);

    assert!(
        first
            .facts
            .iter()
            .any(|fact| fact.kind == aporia_agent::FactKind::Test
                && fact.path == "tests/test_demo.py")
    );
    assert!(
        first.summary.skipped.iter().any(|path| path == "target"),
        "{:?}",
        first.summary.skipped
    );
    assert!(first.summary.files >= 5);
    assert!(first.facts.iter().all(|fact| fact.id.starts_with('F')));
}

#[test]
fn the_scan_is_truncated_by_name_not_silently_partial() {
    let temp = Temp::new("limit");
    for index in 0..6 {
        write(&temp.path().join(format!("file{index}.rs")), b"fn x() {}\n");
    }
    let inspection = inspect_with_limits(
        temp.path(),
        InspectLimits {
            max_entries: 2,
            max_depth: 4,
            read_limit: 1024,
        },
    )
    .expect("inspect");
    assert!(inspection.summary.truncated);
    assert!(inspection.summary.entries <= 2);
}

#[test]
fn a_missing_project_is_refused() {
    let temp = Temp::new("missing");
    let error = inspect(&temp.path().join("absent")).expect_err("must fail");
    assert!(matches!(error, InspectError::NotADirectory(_)), "{error:?}");
}

#[test]
fn the_rendering_is_bounded_and_says_what_it_omitted() {
    let temp = project();
    let inspection = inspect(temp.path()).expect("inspect");
    let rendered = inspection.rendered(40);
    assert!(
        rendered.len() <= 80,
        "the prompt rendering exceeded its limit: {rendered}"
    );
    assert!(rendered.contains("omitted"), "{rendered}");
    assert!(!rendered.contains("Cargo.toml") || rendered.len() <= 40);
}

#[test]
fn the_summary_reports_what_the_readme_read() {
    let temp = Temp::new("readme");
    let long = "x".repeat(10_000);
    write(&temp.path().join("README.md"), long.as_bytes());
    let inspection = inspect(temp.path()).expect("inspect");
    let readme = inspection
        .facts
        .iter()
        .find(|fact| fact.kind == aporia_agent::FactKind::Readme)
        .expect("readme");
    assert!(readme.detail.len() <= 2048, "{}", readme.detail.len());
}
