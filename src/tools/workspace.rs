//! The canonical runtime-owned workspace boundary.
//!
//! One conversation runtime owns one workspace root. The root is
//! canonicalized once at construction and must be a directory. Native
//! filesystem tools and Bash operate only against this root: tool path
//! arguments are workspace-relative UTF-8 paths, absolute paths and lexical
//! `..` escapes are rejected, and the resolved path must stay inside the
//! canonical root. Symlinks may resolve only to targets still inside the
//! workspace. This is a correctness boundary, not a hostile multi-user
//! security sandbox; TOCTOU hardening is deliberately outside M5.

use std::path::{Component, Path, PathBuf};

/// A workspace path resolution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    /// The workspace root does not exist.
    RootMissing(PathBuf),
    /// The workspace root is not a directory.
    RootNotDirectory(PathBuf),
    /// The workspace root cannot be canonicalized.
    RootUnavailable(PathBuf, String),
    /// The tool supplied an empty path.
    EmptyPath,
    /// The tool supplied an absolute path; tool paths are workspace-relative.
    AbsolutePath(String),
    /// The tool supplied a path escaping the workspace.
    Escape(String),
    /// The path cannot be resolved on the local filesystem.
    Unresolvable(String, String),
}

impl core::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RootMissing(path) => {
                write!(f, "workspace root {} does not exist", path.display())
            }
            Self::RootNotDirectory(path) => {
                write!(f, "workspace root {} is not a directory", path.display())
            }
            Self::RootUnavailable(path, error) => write!(
                f,
                "workspace root {} cannot be canonicalized: {error}",
                path.display()
            ),
            Self::EmptyPath => write!(f, "tool path must not be empty"),
            Self::AbsolutePath(path) => write!(
                f,
                "tool path {path:?} is absolute; tool paths are workspace-relative"
            ),
            Self::Escape(path) => {
                write!(f, "tool path {path:?} resolves outside the workspace root")
            }
            Self::Unresolvable(path, error) => write!(
                f,
                "tool path {path:?} cannot be resolved on the filesystem: {error}"
            ),
        }
    }
}

impl std::error::Error for WorkspaceError {}

/// The canonical workspace of one conversation.
///
/// The workspace is cheaply cloneable; every native executor receives a
/// reference through its execution context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Opens the workspace rooted at `root`.
    ///
    /// The root must exist and be a directory; it is canonicalized exactly
    /// once here, so all later resolution compares against the same
    /// canonical root.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError::RootMissing`] when the root does not exist,
    /// [`WorkspaceError::RootNotDirectory`] when it is not a directory, and
    /// [`WorkspaceError::RootUnavailable`] when canonicalization fails.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(WorkspaceError::RootMissing(root.to_path_buf()));
        }
        if !root.is_dir() {
            return Err(WorkspaceError::RootNotDirectory(root.to_path_buf()));
        }
        let canonical = std::fs::canonicalize(root).map_err(|error| {
            WorkspaceError::RootUnavailable(root.to_path_buf(), error.to_string())
        })?;
        Ok(Self { root: canonical })
    }

    /// The canonical workspace root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a workspace-relative path to its canonical absolute path.
    ///
    /// Rejects absolute paths, empty paths, and lexical `..` escapes; the
    /// resolved path must remain inside the canonical root (symlinks may
    /// resolve only to targets still inside the workspace). For paths whose
    /// target does not exist yet (for example a Write target), the deepest
    /// existing ancestor is canonicalized and the remaining components are
    /// appended.
    ///
    /// # Errors
    ///
    /// Returns the specific [`WorkspaceError`] of the first violation.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, WorkspaceError> {
        let relative = Path::new(path);
        if path.is_empty() {
            return Err(WorkspaceError::EmptyPath);
        }
        if relative.is_absolute() {
            return Err(WorkspaceError::AbsolutePath(path.to_owned()));
        }
        for component in relative.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => return Err(WorkspaceError::Escape(path.to_owned())),
                Component::RootDir | Component::Prefix(_) => {
                    return Err(WorkspaceError::AbsolutePath(path.to_owned()));
                }
            }
        }
        let joined = self.root.join(relative);
        let canonical = canonicalize_deepest_existing(&joined)
            .map_err(|error| WorkspaceError::Unresolvable(path.to_owned(), error.to_string()))?;
        if !canonical.starts_with(&self.root) {
            return Err(WorkspaceError::Escape(path.to_owned()));
        }
        Ok(canonical)
    }

    /// The workspace-relative display path of a resolved canonical path.
    ///
    /// Returns `None` when the path is not inside the workspace.
    #[must_use]
    pub fn relative(&self, canonical: &Path) -> Option<String> {
        let relative = canonical.strip_prefix(&self.root).ok()?;
        Some(
            relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }
}

/// Canonicalizes the deepest existing ancestor of `path` and appends the
/// remaining components, so non-existent targets (Write/Edit) resolve
/// deterministically through symlinked parent directories.
fn canonicalize_deepest_existing(path: &Path) -> std::io::Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path);
    }
    let Some(parent) = path.parent() else {
        return std::fs::canonicalize(path);
    };
    let file_name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    if parent == path {
        return std::fs::canonicalize(path);
    }
    let canonical_parent = canonicalize_deepest_existing(parent)?;
    Ok(canonical_parent.join(file_name))
}

#[cfg(test)]
mod tests {
    use super::{Workspace, WorkspaceError};
    use std::fs;

    #[test]
    fn construction_requires_an_existing_directory() {
        let missing = Workspace::new("/definitely/not/a/real/path/rustx-test");
        assert!(matches!(missing, Err(WorkspaceError::RootMissing(_))));
        let file = std::env::temp_dir().join(format!("rustx-ws-file-{}", std::process::id()));
        fs::write(&file, "x").expect("write file");
        assert!(matches!(
            Workspace::new(&file),
            Err(WorkspaceError::RootNotDirectory(_))
        ));
        fs::remove_file(&file).expect("remove file");
    }

    #[test]
    fn canonical_root_is_determined_at_construction() {
        let dir = std::env::temp_dir().join(format!(
            "rustx-ws-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        fs::create_dir_all(dir.join("sub")).expect("create");
        let workspace = Workspace::new(&dir).expect("workspace");
        let resolved = workspace.resolve("sub").expect("resolve");
        assert_eq!(resolved, dir.canonicalize().expect("canonical").join("sub"));
        fs::remove_dir_all(&dir).expect("remove");
    }

    #[test]
    fn rejects_absolute_empty_and_parent_escape_paths() {
        let dir = std::env::temp_dir().join(format!(
            "rustx-ws-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        fs::create_dir_all(&dir).expect("create");
        let workspace = Workspace::new(&dir).expect("workspace");
        assert!(matches!(
            workspace.resolve(""),
            Err(WorkspaceError::EmptyPath)
        ));
        assert!(matches!(
            workspace.resolve("/etc/passwd"),
            Err(WorkspaceError::AbsolutePath(_))
        ));
        assert!(matches!(
            workspace.resolve("../escape"),
            Err(WorkspaceError::Escape(_))
        ));
        assert!(matches!(
            workspace.resolve("a/../../escape"),
            Err(WorkspaceError::Escape(_))
        ));
        fs::remove_dir_all(&dir).expect("remove");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_may_not_escape_the_workspace() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!(
            "rustx-ws-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        let outside = std::env::temp_dir().join(format!(
            "rustx-outside-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        fs::create_dir_all(&dir).expect("create");
        fs::create_dir_all(&outside).expect("create outside");
        fs::write(outside.join("secret.txt"), "secret").expect("write outside");
        symlink(&outside, dir.join("linked")).expect("symlink");
        let workspace = Workspace::new(&dir).expect("workspace");
        assert!(matches!(
            workspace.resolve("linked/secret.txt"),
            Err(WorkspaceError::Escape(_))
        ));
        fs::remove_dir_all(&dir).expect("remove");
        fs::remove_dir_all(&outside).expect("remove outside");
    }

    #[test]
    fn relative_display_paths_stay_workspace_relative() {
        let dir = std::env::temp_dir().join(format!(
            "rustx-ws-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t")
        ));
        fs::create_dir_all(dir.join("a/b")).expect("create");
        fs::write(dir.join("a/b/f.txt"), "x").expect("write");
        let workspace = Workspace::new(&dir).expect("workspace");
        let resolved = workspace.resolve("a/b/f.txt").expect("resolve");
        assert_eq!(workspace.relative(&resolved).as_deref(), Some("a/b/f.txt"));
        fs::remove_dir_all(&dir).expect("remove");
    }
}
