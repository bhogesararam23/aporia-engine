//! The export's patch must round-trip: `git apply` onto a pristine copy of the source must
//! reproduce the workspace byte for byte, and a conflicting target must be refused by git.

use aporia_agent::diff::{changeset, unified_file_diff};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aporia-diff-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("dir");
    for entry in fs::read_dir(from).expect("read") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

#[test]
fn a_changeset_applies_with_git_and_reproduces_the_workspace_bytes() {
    if !git_available() {
        eprintln!("skipped: this machine has no git");
        return;
    }
    let root = Temp::new("roundtrip");
    let source = root.path().join("src");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&source).expect("dir");
    fs::create_dir_all(&workspace).expect("dir");

    // A modification with context, a creation, a deletion, a missing final newline, and two
    // distant changes in one file.
    let mut original = String::new();
    for n in 1..=20 {
        let _ = writeln!(original, "line {n}");
    }
    let edited = original
        .replace("line 2\n", "LINE TWO\n")
        .replace("line 18\n", "LINE EIGHTEEN\n");
    fs::write(source.join("many.txt"), &original).expect("file");
    fs::write(workspace.join("many.txt"), &edited).expect("file");
    fs::write(source.join("modified.py"), b"def f():\n    return 1\n").expect("file");
    fs::write(workspace.join("modified.py"), b"def f():\n    return 2\n").expect("file");
    fs::write(workspace.join("created.txt"), b"new file\n").expect("file");
    fs::write(source.join("deleted.txt"), b"old file\n").expect("file");
    fs::write(source.join("tail.txt"), b"a\nb").expect("file");
    fs::write(workspace.join("tail.txt"), b"a\nc").expect("file");

    let paths: Vec<String> = [
        "many.txt",
        "modified.py",
        "created.txt",
        "deleted.txt",
        "tail.txt",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let set = changeset(&source, &workspace, &paths).expect("changeset");
    assert_eq!(set.files.len(), 5);
    assert!(set.binary.is_empty());

    let patch_path = root.path().join("changes.patch");
    fs::write(&patch_path, &set.patch).expect("patch");
    let target = root.path().join("target");
    copy_tree(&source, &target);
    // Pin conversion off: this machine's core.autocrlf would otherwise rewrite the bytes.
    let out = Command::new("git")
        .args(["-c", "core.autocrlf=false", "apply"])
        .arg(&patch_path)
        .current_dir(&target)
        .output()
        .expect("git apply runs");
    assert!(
        out.status.success(),
        "git apply refused the patch: {}\npatch:\n{}",
        String::from_utf8_lossy(&out.stderr),
        set.patch
    );
    for name in ["many.txt", "modified.py", "created.txt", "tail.txt"] {
        assert_eq!(
            fs::read(target.join(name)).expect("target file"),
            fs::read(workspace.join(name)).expect("workspace file"),
            "{name} did not round-trip"
        );
    }
    assert!(
        !target.join("deleted.txt").exists(),
        "the deletion did not apply"
    );
}

#[test]
fn a_conflicting_target_is_refused_by_git_apply() {
    if !git_available() {
        eprintln!("skipped: this machine has no git");
        return;
    }
    let root = Temp::new("conflict");
    let source = root.path().join("src");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&source).expect("dir");
    fs::create_dir_all(&workspace).expect("dir");
    fs::write(source.join("f.txt"), b"a\nb\nc\n").expect("file");
    fs::write(workspace.join("f.txt"), b"a\nB\nc\n").expect("file");
    let set = changeset(&source, &workspace, &["f.txt".to_string()]).expect("changeset");
    let patch_path = root.path().join("changes.patch");
    fs::write(&patch_path, &set.patch).expect("patch");

    let target = root.path().join("target");
    copy_tree(&source, &target);
    // The target moved on since the copy: the patch must not apply silently.
    fs::write(target.join("f.txt"), b"a\nsomeone else\nc\n").expect("conflict");
    // Pin conversion off: this machine's core.autocrlf would otherwise rewrite the bytes.
    let out = Command::new("git")
        .args(["-c", "core.autocrlf=false", "apply"])
        .arg(&patch_path)
        .current_dir(&target)
        .output()
        .expect("git apply runs");
    assert!(
        !out.status.success(),
        "git apply accepted a conflicting patch"
    );
    assert_eq!(
        fs::read(target.join("f.txt")).expect("file"),
        b"a\nsomeone else\nc\n",
        "the conflicting target was written"
    );
}

#[test]
fn an_empty_modification_produces_no_hunk_text() {
    let diff = unified_file_diff("f.txt", Some("a\n"), Some("a\n"));
    assert_eq!(diff, "--- a/f.txt\n+++ b/f.txt\n");
}
