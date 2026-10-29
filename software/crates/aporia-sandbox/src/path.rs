//! Resolving a name a tool gave into a path inside the workspace.
//!
//! The rule is lexical first and canonical second: `..`, a root, a prefix and an absolute path are
//! refused before the filesystem is asked anything, and then the resolved path is canonicalised
//! and checked again. The second check is what catches a symlink that looks local and points out.

use std::path::{Component, Path, PathBuf};

/// Why a name does not name a place inside the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathError {
    Empty,
    Absolute,
    /// A parent component or a canonicalised result left the workspace.
    Escapes(String),
    /// The path could not be resolved at all (for example, no ancestor exists).
    Io(String),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "the path is empty"),
            Self::Absolute => write!(f, "an absolute path is not a workspace path"),
            Self::Escapes(path) => write!(f, "`{path}` leaves the workspace"),
            Self::Io(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for PathError {}

/// Canonicalise the part of a path that exists and append the rest.
///
/// `canonicalize` alone is useless for a file a tool is about to create; a parent can be
/// canonicalised and the leaf kept, which is enough to decide containment.
pub(crate) fn canonical_ish(path: &Path) -> Result<PathBuf, PathError> {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(mut base) = current.canonicalize() {
            for part in tail.iter().rev() {
                base.push(part);
            }
            return Ok(base);
        }
        let Some(name) = current.file_name() else {
            return Err(PathError::Io(format!(
                "cannot resolve {}: no existing ancestor",
                path.display()
            )));
        };
        tail.push(name.to_os_string());
        let Some(parent) = current.parent() else {
            return Err(PathError::Io(format!(
                "cannot resolve {}: no parent",
                path.display()
            )));
        };
        current = parent.to_path_buf();
    }
}

/// Resolve `relative` under an already-canonical workspace root.
pub(crate) fn resolve(canonical_root: &Path, relative: &str) -> Result<PathBuf, PathError> {
    if relative.trim().is_empty() {
        return Err(PathError::Empty);
    }
    let mut local = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(name) => local.push(name),
            Component::CurDir => {}
            Component::ParentDir => return Err(PathError::Escapes(relative.to_string())),
            Component::RootDir | Component::Prefix(_) => return Err(PathError::Absolute),
        }
    }
    let candidate = canonical_root.join(&local);
    let resolved = canonical_ish(&candidate)?;
    if !resolved.starts_with(canonical_root) {
        return Err(PathError::Escapes(relative.to_string()));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Root(PathBuf);

    impl Root {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aporia-sandbox-path-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("root");
            Self(path.canonicalize().expect("canonical"))
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_normal_relative_path_resolves_inside() {
        let root = Root::new();
        std::fs::write(root.0.join("model.ap"), b"model x { }").expect("write");
        let resolved = resolve(&root.0, "model.ap").expect("resolve");
        assert!(resolved.starts_with(&root.0));
        assert_eq!(resolved, root.0.join("model.ap"));
    }

    #[test]
    fn a_leaf_that_does_not_exist_yet_still_resolves() {
        let root = Root::new();
        std::fs::create_dir_all(root.0.join("output")).expect("dir");
        let resolved = resolve(&root.0, "output/new.txt").expect("resolve");
        assert_eq!(resolved, root.0.join("output/new.txt"));
    }

    #[test]
    fn parent_components_are_refused() {
        let root = Root::new();
        for name in ["../outside", "a/../../outside", ".."] {
            assert!(
                matches!(resolve(&root.0, name), Err(PathError::Escapes(_))),
                "{name} was not refused"
            );
        }
    }

    #[test]
    fn absolute_and_prefixed_paths_are_refused() {
        let root = Root::new();
        assert!(matches!(
            resolve(&root.0, "/etc/hostname"),
            Err(PathError::Absolute)
        ));
        #[cfg(windows)]
        assert!(matches!(
            resolve(&root.0, "C:\\Windows\\win.ini"),
            Err(PathError::Absolute)
        ));
    }

    #[test]
    fn an_empty_or_current_directory_name_is_handled_explicitly() {
        let root = Root::new();
        assert!(matches!(resolve(&root.0, "   "), Err(PathError::Empty)));
        assert_eq!(resolve(&root.0, ".").expect("root"), root.0);
    }

    #[test]
    fn a_link_that_points_out_is_refused() {
        let root = Root::new();
        let outside = Root::new();
        std::fs::write(outside.0.join("secret.txt"), b"secret").expect("write");
        let link = root.0.join("link.txt");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(outside.0.join("secret.txt"), &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(outside.0.join("secret.txt"), &link).is_ok();
        if !made {
            eprintln!("skipped: this machine would not let the test create a symlink");
            return;
        }
        assert!(
            matches!(resolve(&root.0, "link.txt"), Err(PathError::Escapes(_))),
            "a link out of the workspace resolved"
        );
    }
}
