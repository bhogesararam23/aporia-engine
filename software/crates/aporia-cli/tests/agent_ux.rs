//! The workbench's answers about its own state: the changes list must say what changed, and an
//! approval must show the plan it is asking about.

use aporia_agent::investigation::{ChangeKind, FileChange, Investigation};
use aporia_cli::agent::render_changes;

#[test]
fn an_empty_workspace_says_so() {
    let investigation = Investigation::default();
    assert_eq!(
        render_changes(&investigation),
        "no changes have been made to the workspace yet\n"
    );
}

#[test]
fn the_changes_list_names_each_change_and_its_identities() {
    let mut investigation = Investigation::default();
    investigation.changes.push(FileChange {
        path: "rule.py".to_string(),
        kind: ChangeKind::Modified,
        before: Some("a".repeat(64)),
        after: Some("b".repeat(64)),
    });
    investigation.changes.push(FileChange {
        path: "extra.txt".to_string(),
        kind: ChangeKind::Created,
        before: None,
        after: Some("c".repeat(64)),
    });
    let text = render_changes(&investigation);
    assert!(text.contains("rule.py (modified)"), "{text}");
    assert!(text.contains("extra.txt (created)"), "{text}");
    assert!(text.contains("aaaaaaaaaaaa -> bbbbbbbbbbbb"), "{text}");
    assert!(text.contains("absent -> cccccccccccc"), "{text}");
    assert!(
        !text.contains("no changes"),
        "a workspace with changes was reported as unchanged: {text}"
    );
}
