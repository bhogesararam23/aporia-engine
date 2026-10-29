//! What the copy promises: complete regular content, a digest for every file, named refusals, and
//! no half-written workspace left behind.

use aporia_sandbox::{CopyLimits, IGNORED_DIRS, Sandbox, SandboxError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-sandbox-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
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

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, bytes).expect("write");
}

fn project() -> Temp {
    let temp = Temp::new("project");
    let root = temp.path().join("project");
    write(&root.join("README.md"), b"# a project\n");
    write(&root.join("src/lib.rs"), b"pub fn answer() -> u32 { 42 }\n");
    write(&root.join("tests/test_answer.py"), b"def test_it(): pass\n");
    // Directories that are never project content, at any depth.
    write(&root.join(".git/HEAD"), b"ref: refs/heads/main\n");
    write(&root.join("target/junk.bin"), b"junk");
    write(&root.join("node_modules/pkg/index.js"), b"module");
    write(&root.join("__pycache__/x.pyc"), b"bytecode");
    write(&root.join(".venv/pyvenv.cfg"), b"home");
    write(&root.join(".scratch/note.txt"), b"scratch");
    temp
}

#[test]
fn copies_regular_files_and_records_a_digest_for_each() {
    let temp = project();
    let source = temp.path().join("project");
    let workspace = temp.path().join("work");

    let (sandbox, report) = Sandbox::create(&source, &workspace).expect("copy");

    assert_eq!(
        sandbox.root(),
        workspace.canonicalize().expect("canonical workspace")
    );
    let paths: Vec<&str> = report.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["README.md", "src/lib.rs", "tests/test_answer.py"],
        "the report must name exactly the copied project content"
    );
    for file in &report.files {
        let bytes = std::fs::read(workspace.join(&file.path)).expect("copied file");
        assert_eq!(file.bytes, bytes.len() as u64, "{}", file.path);
        assert_eq!(
            file.sha256,
            aporia_store::digest::sha256_hex(&bytes),
            "{} has the wrong digest",
            file.path
        );
    }
    assert_eq!(
        report.files.iter().map(|f| f.bytes).sum::<u64>(),
        report.total_bytes
    );
    for ignored in [
        ".git",
        "target",
        "node_modules",
        "__pycache__",
        ".venv",
        ".scratch",
    ] {
        assert!(
            report.ignored.iter().any(|p| p == ignored),
            "{ignored} was not reported as ignored: {:?}",
            report.ignored
        );
        assert!(
            !workspace.join(ignored).exists(),
            "{ignored} was copied into the workspace"
        );
    }
    for path in &report.ignored {
        let name = path.rsplit('/').next().unwrap_or(path);
        assert!(
            IGNORED_DIRS.contains(&name),
            "{path} was reported as ignored but is not an ignored name"
        );
    }
}

#[test]
fn a_workspace_inside_the_source_is_refused_before_anything_is_created() {
    let temp = project();
    let source = temp.path().join("project");
    let workspace = source.join("inside");

    let result = Sandbox::create(&source, &workspace);

    assert!(
        matches!(result, Err(SandboxError::Overlap { .. })),
        "{result:?}"
    );
    assert!(!workspace.exists(), "a refused workspace was left on disk");
}

#[test]
fn a_workspace_containing_the_source_is_refused() {
    let temp = project();
    let source = temp.path().join("project");

    let result = Sandbox::create(&source, temp.path());

    assert!(
        matches!(result, Err(SandboxError::Overlap { .. })),
        "{result:?}"
    );
    assert!(source.join("src/lib.rs").exists(), "the source was touched");
}

#[test]
fn a_workspace_under_an_ignored_directory_is_allowed_and_not_copied_into_itself() {
    let temp = project();
    let source = temp.path().join("project");
    let workspace = source.join(".aporia/investigations/inv-1/workspace");

    let (sandbox, report) = Sandbox::create(&source, &workspace).expect("allowed");

    assert!(
        sandbox
            .root()
            .starts_with(source.canonicalize().expect("source"))
    );
    assert!(
        report
            .files
            .iter()
            .all(|file| !file.path.starts_with(".aporia")),
        "the investigation directory was copied into its own workspace: {:?}",
        report.files
    );
    assert!(workspace.is_dir());
}

#[test]
fn an_existing_workspace_is_never_overwritten() {
    let temp = project();
    let source = temp.path().join("project");
    let workspace = temp.path().join("work");
    std::fs::create_dir_all(&workspace).expect("existing");

    let result = Sandbox::create(&source, &workspace);

    assert!(
        matches!(result, Err(SandboxError::WorkspaceExists(_))),
        "{result:?}"
    );
}

#[test]
fn limits_refuse_by_name_and_remove_the_partial_workspace() {
    let temp = project();
    let source = temp.path().join("project");
    let workspace = temp.path().join("work");

    let result = Sandbox::create_with_limits(
        &source,
        &workspace,
        CopyLimits {
            max_files: 2,
            max_bytes: u64::MAX,
            max_file_bytes: u64::MAX,
        },
    );

    assert!(
        matches!(
            result,
            Err(SandboxError::TooManyFiles { found: 3, limit: 2 })
        ),
        "{result:?}"
    );
    assert!(!workspace.exists(), "a failed copy left a workspace behind");
}

#[test]
fn one_oversized_file_is_refused_not_silently_dropped() {
    let temp = project();
    let source = temp.path().join("project");
    let workspace = temp.path().join("work");

    let result = Sandbox::create_with_limits(
        &source,
        &workspace,
        CopyLimits {
            max_files: usize::MAX,
            max_bytes: u64::MAX,
            max_file_bytes: 10,
        },
    );

    assert!(
        matches!(result, Err(SandboxError::FileTooLarge { .. })),
        "{result:?}"
    );
    assert!(!workspace.exists());
}

#[test]
fn total_bytes_are_enforced() {
    let temp = project();
    let source = temp.path().join("project");
    let workspace = temp.path().join("work");

    let result = Sandbox::create_with_limits(
        &source,
        &workspace,
        CopyLimits {
            max_files: usize::MAX,
            max_bytes: 20,
            max_file_bytes: u64::MAX,
        },
    );

    assert!(
        matches!(result, Err(SandboxError::TooManyBytes { .. })),
        "{result:?}"
    );
}

#[test]
fn a_link_is_refused_rather_than_followed_out_of_the_source() {
    let temp = project();
    let source = temp.path().join("project");
    let workspace = temp.path().join("work");
    let link = source.join("escape.rs");

    #[cfg(unix)]
    let made = std::os::unix::fs::symlink("/etc/hostname", &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file("C:\\Windows\\win.ini", &link).is_ok();
    if !made {
        // Creating a Windows symlink needs a privilege this machine may not grant. The refusal
        // itself is still exercised by the reparse-point branch on any tree that contains one.
        eprintln!("skipped: this machine would not let the test create a symlink");
        return;
    }

    let result = Sandbox::create(&source, &workspace);

    assert!(
        matches!(result, Err(SandboxError::ReparsePoint(ref p)) if p == "escape.rs"),
        "{result:?}"
    );
    assert!(!workspace.exists());
}
