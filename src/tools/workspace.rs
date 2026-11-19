//! The canonical runtime-owned workspace boundary.
//!
//! One conversation runtime owns one workspace root. The root is
//! canonicalized once at construction and must be a directory. The
//! workspace owns the authoritative execution cwd used by native file tools
//! and Bash. Native Read/Write/Edit/Grep/Glob resolve relative model paths
//! against this root and accept absolute host paths; runtime-owned managed
//! output remains owned by [`crate::tools::managed_output`] and runtime read
//! locators remain in [`crate::tools::locator`]. This is a correctness
//! boundary, not a hostile
//! multi-user security sandbox; TOCTOU hardening is deliberately outside M5.

use std::path::{Path, PathBuf};

use crate::local_runtime::session_runtime_manager::SessionExecutionScope;

/// A workspace path resolution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    /// The workspace root does not exist.
    RootMissing(PathBuf),
    /// The workspace root is not a directory.
    RootNotDirectory(PathBuf),
    /// The workspace root cannot be canonicalized.
    RootUnavailable(PathBuf, String),
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
    /// The Session's non-owning execution route, bound at composition —
    /// `None` for a workspace nobody composed with execution, which is an
    /// honest dispatch refusal, never a local filesystem fallback.
    execution_scope: Option<SessionExecutionScope>,
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
        Ok(Self {
            root: canonical,
            // A bare workspace is always unbound: composition binds the
            // route through `bind_execution_scope` before the runtime —
            // and therefore this workspace — is published.
            execution_scope: None,
        })
    }

    /// The canonical workspace root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Binds the Session's execution route — construction-time only.
    ///
    /// Called by the conversation runtime composition before the runtime
    /// is published; there is no post-construction mutation path and no
    /// caller outside the crate can attach a route.
    pub(crate) fn bind_execution_scope(&mut self, scope: Option<SessionExecutionScope>) {
        self.execution_scope = scope;
    }

    /// The Session's execution route, when composition bound one.
    ///
    /// `None` is an honest unbound route — consumers must refuse, never
    /// fall back to a Server-local path.
    #[must_use]
    pub fn execution_scope(&self) -> Option<&SessionExecutionScope> {
        self.execution_scope.as_ref()
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

#[cfg(test)]
mod tests {
    use super::{Workspace, WorkspaceError};
    use std::fs;
    use std::path::Path;

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
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("sub")).expect("create");
        let workspace = Workspace::new(dir.path()).expect("workspace");
        assert_eq!(
            workspace.root(),
            dir.path().canonicalize().expect("canonical").as_path()
        );
    }

    #[test]
    fn relative_display_paths_stay_workspace_relative() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("a/b")).expect("create");
        fs::write(dir.path().join("a/b/f.txt"), "x").expect("write");
        let workspace = Workspace::new(dir.path()).expect("workspace");
        let canonical = dir
            .path()
            .canonicalize()
            .expect("canonical")
            .join("a/b/f.txt");
        assert_eq!(workspace.relative(&canonical).as_deref(), Some("a/b/f.txt"));
        // The root itself resolves to the empty relative path. Compare
        // against the canonical root (on macOS /var is a /private/var link).
        let canonical_root = dir.path().canonicalize().expect("canonical root");
        assert_eq!(workspace.relative(&canonical_root).as_deref(), Some(""));
        assert_eq!(workspace.relative(Path::new("/etc")), None);
    }
}
