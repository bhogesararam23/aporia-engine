//! Unified diffs of the workspace against the original project, without a dependency.
//!
//! The export is a patch a person can review and `git apply` can round-trip: `a/` and `b/`
//! prefixes, three lines of context, `/dev/null` for created and deleted files, and the
//! `\ No newline at end of file` marker when a side lacks its final newline. Binary content is
//! refused, not guessed at: a changeset that contains it names the files it cannot represent.
//!
//! The line diff is a longest-common-subsequence over the middle that remains after the common
//! prefix and suffix are trimmed. Past a bounded table size the middle becomes one replacement
//! hunk -- still a correct patch, just a less readable one, and the limit is stated here rather
//! than discovered as an out-of-memory kill.

use std::fmt::Write as _;
use std::path::Path;

/// Context lines kept around each change.
const CONTEXT: usize = 3;
/// The largest LCS table this module will build (before x after lines of the trimmed middle).
const MAX_TABLE: usize = 4_000_000;

/// Why a changeset could not be produced.
#[derive(Debug)]
pub enum DiffError {
    /// A file's bytes are not text; a unified diff cannot represent them.
    Binary(String),
    Io {
        path: String,
        reason: String,
    },
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Binary(path) => write!(f, "`{path}` is binary; a unified diff cannot carry it"),
            Self::Io { path, reason } => write!(f, "cannot read `{path}`: {reason}"),
        }
    }
}

impl std::error::Error for DiffError {}

/// One file's place in a changeset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileState {
    Created,
    Modified,
    Deleted,
}

/// A reviewable patch and what it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changeset {
    /// The unified diff text, empty when nothing differs.
    pub patch: String,
    /// The files the patch changes, in path order.
    pub files: Vec<String>,
    /// Files whose bytes differ but are binary: named, never silently dropped.
    pub binary: Vec<String>,
}

impl Changeset {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.binary.is_empty()
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// Read a file's bytes, or `None` when it does not exist.
fn read_optional(root: &Path, relative: &str) -> Result<Option<Vec<u8>>, DiffError> {
    let path = root.join(relative);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DiffError::Io {
            path: relative.to_string(),
            reason: error.to_string(),
        }),
    }
}

/// The changeset between the original project and the workspace, over the given relative paths.
/// Paths that end up identical are skipped: a change that was reverted is not a change.
pub fn changeset(
    source: &Path,
    workspace: &Path,
    paths: &[String],
) -> Result<Changeset, DiffError> {
    let mut ordered: Vec<&str> = paths.iter().map(String::as_str).collect();
    ordered.sort_unstable();
    ordered.dedup();
    let mut out = Changeset {
        patch: String::new(),
        files: Vec::new(),
        binary: Vec::new(),
    };
    for relative in ordered {
        let before = read_optional(source, relative)?;
        let after = read_optional(workspace, relative)?;
        if before == after {
            continue;
        }
        if before.as_deref().is_some_and(is_binary) || after.as_deref().is_some_and(is_binary) {
            out.binary.push(relative.to_string());
            continue;
        }
        let before_text = before
            .as_deref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        let after_text = after
            .as_deref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        out.patch.push_str(&unified_file_diff(
            relative,
            before_text.as_deref(),
            after_text.as_deref(),
        ));
        out.files.push(relative.to_string());
    }
    Ok(out)
}

/// One file's unified diff. `None` means the file does not exist on that side.
#[must_use]
pub fn unified_file_diff(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let mut out = String::new();
    let old_header = before.map_or_else(|| "/dev/null".to_string(), |_| format!("a/{path}"));
    let new_header = after.map_or_else(|| "/dev/null".to_string(), |_| format!("b/{path}"));
    let _ = write!(out, "--- {old_header}\n+++ {new_header}\n");
    let old_lines: Vec<&str> = before.map_or_else(Vec::new, split_lines);
    let new_lines: Vec<&str> = after.map_or_else(Vec::new, split_lines);
    let script = edit_script(&old_lines, &new_lines);
    for hunk in hunks(&script) {
        out.push_str(&hunk_header(&hunk));
        for op in &script[hunk.start..hunk.end] {
            match op {
                Op::Equal(line) => {
                    out.push(' ');
                    push_line(&mut out, line);
                }
                Op::Delete(line) => {
                    out.push('-');
                    push_line(&mut out, line);
                }
                Op::Insert(line) => {
                    out.push('+');
                    push_line(&mut out, line);
                }
            }
        }
    }
    out
}

/// Split into lines, keeping each line's ending so a missing final newline survives.
fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    // split_inclusive on an empty string yields nothing; on a trailing \n it yields no empty
    // tail, which is what a diff wants.
    if text.is_empty() {
        lines.clear();
    }
    lines
}

/// Append a diff line; a line without its newline gets one, then the marker.
fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    if !line.ends_with('\n') {
        out.push_str("\n\\ No newline at end of file\n");
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Op<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

/// The edit script: common prefix, an LCS middle, common suffix.
fn edit_script<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Op<'a>> {
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];

    let mut script: Vec<Op<'a>> = Vec::new();
    for line in &old[..prefix] {
        script.push(Op::Equal(line));
    }
    if old_mid.len() * new_mid.len() > MAX_TABLE {
        // One replacement hunk: correct, bounded, and honest about its size.
        for line in old_mid {
            script.push(Op::Delete(line));
        }
        for line in new_mid {
            script.push(Op::Insert(line));
        }
    } else {
        lcs_middle(old_mid, new_mid, &mut script);
    }
    for line in &old[old.len() - suffix..] {
        script.push(Op::Equal(line));
    }
    script
}

/// The LCS backtrack over the trimmed middle, appending deletes and inserts in order.
fn lcs_middle<'a>(old: &[&'a str], new: &[&'a str], script: &mut Vec<Op<'a>>) {
    let rows = old.len() + 1;
    let cols = new.len() + 1;
    let mut table = vec![0u32; rows * cols];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * cols + j] = if old[i] == new[j] {
                table[(i + 1) * cols + j + 1] + 1
            } else {
                table[(i + 1) * cols + j].max(table[i * cols + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            script.push(Op::Equal(old[i]));
            i += 1;
            j += 1;
        } else if table[(i + 1) * cols + j] >= table[i * cols + j + 1] {
            script.push(Op::Delete(old[i]));
            i += 1;
        } else {
            script.push(Op::Insert(new[j]));
            j += 1;
        }
    }
    while i < old.len() {
        script.push(Op::Delete(old[i]));
        i += 1;
    }
    while j < new.len() {
        script.push(Op::Insert(new[j]));
        j += 1;
    }
}

/// One hunk's span in the edit script, with its line numbers on both sides.
struct Hunk {
    start: usize,
    end: usize,
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
}

/// Group the script's changes into hunks with `CONTEXT` lines around them.
fn hunks(script: &[Op<'_>]) -> Vec<Hunk> {
    // Positions of the changed ops.
    let changes: Vec<usize> = script
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Equal(_)))
        .map(|(index, _)| index)
        .collect();
    if changes.is_empty() {
        return Vec::new();
    }
    // Merge change regions whose context windows touch.
    let mut regions: Vec<(usize, usize)> = Vec::new();
    for &index in &changes {
        let start = index.saturating_sub(CONTEXT);
        let end = (index + CONTEXT + 1).min(script.len());
        if let Some(last) = regions.last_mut()
            && start <= last.1
        {
            last.1 = end;
            continue;
        }
        regions.push((start, end));
    }
    // Line numbers: walk the script once, recording the counts at each region boundary.
    let mut out = Vec::new();
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut position = 0usize;
    for (start, end) in regions {
        while position < start {
            match script[position] {
                Op::Equal(_) => {
                    old_line += 1;
                    new_line += 1;
                }
                Op::Delete(_) => old_line += 1,
                Op::Insert(_) => new_line += 1,
            }
            position += 1;
        }
        let old_start = old_line;
        let new_start = new_line;
        let mut old_count = 0;
        let mut new_count = 0;
        while position < end {
            match script[position] {
                Op::Equal(_) => {
                    old_count += 1;
                    new_count += 1;
                }
                Op::Delete(_) => old_count += 1,
                Op::Insert(_) => new_count += 1,
            }
            position += 1;
        }
        old_line += old_count;
        new_line += new_count;
        out.push(Hunk {
            start,
            end,
            old_start,
            old_count,
            new_start,
            new_count,
        });
    }
    out
}

fn hunk_header(hunk: &Hunk) -> String {
    // A side with zero lines starts at 0; otherwise lines are 1-based.
    let old_start = if hunk.old_count == 0 {
        0
    } else {
        hunk.old_start + 1
    };
    let new_start = if hunk.new_count == 0 {
        0
    } else {
        hunk.new_start + 1
    };
    format!(
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n",
        old_count = hunk.old_count,
        new_count = hunk.new_count
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modification_keeps_context_and_counts() {
        let before = "a\nb\nc\nd\ne\n";
        let after = "a\nb\nX\nd\ne\n";
        let diff = unified_file_diff("f.txt", Some(before), Some(after));
        assert!(diff.contains("--- a/f.txt\n+++ b/f.txt\n"), "{diff}");
        assert!(diff.contains("@@ -1,5 +1,5 @@\n"), "{diff}");
        assert!(diff.contains("-c\n+X\n"), "{diff}");
        assert!(diff.contains(" a\n"), "{diff}");
    }

    #[test]
    fn a_creation_reads_from_dev_null() {
        let diff = unified_file_diff("new.txt", None, Some("one\ntwo\n"));
        assert!(diff.contains("--- /dev/null\n+++ b/new.txt\n"), "{diff}");
        assert!(diff.contains("@@ -0,0 +1,2 @@\n"), "{diff}");
        assert!(diff.contains("+one\n+two\n"), "{diff}");
    }

    #[test]
    fn a_deletion_writes_to_dev_null() {
        let diff = unified_file_diff("gone.txt", Some("one\ntwo\n"), None);
        assert!(diff.contains("--- a/gone.txt\n+++ /dev/null\n"), "{diff}");
        assert!(diff.contains("@@ -1,2 +0,0 @@\n"), "{diff}");
        assert!(diff.contains("-one\n-two\n"), "{diff}");
    }

    #[test]
    fn a_missing_final_newline_is_marked_on_both_sides() {
        let diff = unified_file_diff("f.txt", Some("a\nb"), Some("a\nc"));
        assert!(
            diff.contains("-b\n\\ No newline at end of file\n"),
            "{diff}"
        );
        assert!(
            diff.contains("+c\n\\ No newline at end of file\n"),
            "{diff}"
        );
    }

    #[test]
    fn distant_changes_become_separate_hunks() {
        let mut before = String::new();
        for n in 1..=20 {
            let _ = writeln!(before, "line {n}");
        }
        let after = before
            .replace("line 2\n", "LINE 2\n")
            .replace("line 18\n", "LINE 18\n");
        let diff = unified_file_diff("f.txt", Some(&before), Some(&after));
        assert_eq!(diff.matches("@@").count() / 2, 2, "{diff}");
    }

    #[test]
    fn identical_content_has_no_hunks() {
        let diff = unified_file_diff("f.txt", Some("a\n"), Some("a\n"));
        assert!(!diff.contains("@@"), "{diff}");
    }

    #[test]
    fn binary_content_is_named_not_diffed() {
        let dir = std::env::temp_dir().join(format!("aporia-diff-bin-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).expect("dir");
        std::fs::create_dir_all(dir.join("ws")).expect("dir");
        std::fs::write(dir.join("src/data.bin"), b"a\0b").expect("file");
        std::fs::write(dir.join("ws/data.bin"), b"a\0c").expect("file");
        let set = changeset(&dir.join("src"), &dir.join("ws"), &["data.bin".to_string()])
            .expect("changeset");
        assert!(set.files.is_empty());
        assert_eq!(set.binary, vec!["data.bin".to_string()]);
        assert!(set.patch.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reverted_path_is_not_a_change() {
        let dir = std::env::temp_dir().join(format!("aporia-diff-rev-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).expect("dir");
        std::fs::create_dir_all(dir.join("ws")).expect("dir");
        std::fs::write(dir.join("src/f.txt"), b"same\n").expect("file");
        std::fs::write(dir.join("ws/f.txt"), b"same\n").expect("file");
        let set =
            changeset(&dir.join("src"), &dir.join("ws"), &["f.txt".to_string()]).expect("set");
        assert!(set.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
