//! The one model-facing filesystem locator/authority boundary.
//!
//! Native filesystem tools consume **absolute** model-facing locators. An
//! absolute path is a locator, never authority: this module is the single
//! boundary that resolves a locator against the conversation's explicitly
//! authorized roots and enforces the per-operation mutability contract:
//!
//! ```text
//! workspace root            Read  Grep  Glob  Write  Edit
//! managed tool-output root  Read  Grep  Glob  --- read-only ---
//! every other host path     rejected
//! ```
//!
//! Ownership stays with the owning types: [`Workspace`] owns the canonical
//! workspace root, [`ManagedToolOutput`] owns the canonical managed-output
//! root, and this module owns only the resolution/authorization decision —
//! it is not a VFS and it never stores paths.
//!
//! # Canonicalization
//!
//! Authority is decided on canonicalized paths, never on lexical prefix
//! matching of the model-supplied string: an existing target is
//! canonicalized directly, and a not-yet-existing mutation target (Write)
//! is resolved through its deepest existing ancestor, so a symlink can
//! never turn an authorized locator into access outside its owning root.

use std::path::{Path, PathBuf};

use crate::tools::managed_output::ManagedToolOutput;
use crate::tools::workspace::Workspace;

/// A filesystem locator resolution/authorization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocatorError {
    /// The model supplied a non-absolute locator; model-facing filesystem
    /// locators are absolute paths.
    NotAbsolute(String),
    /// The locator resolves outside every authorized root.
    OutsideAuthorizedRoots(String),
    /// The locator resolves into the read-only managed tool-output root,
    /// which Write/Edit may never mutate.
    ManagedOutputReadOnly(String),
    /// The locator cannot be resolved on the local filesystem.
    Unresolvable(String, String),
}

impl core::fmt::Display for LocatorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotAbsolute(path) => write!(
                f,
                "filesystem path {path:?} is not absolute; model-facing filesystem paths are \
                 absolute locators"
            ),
            Self::OutsideAuthorizedRoots(path) => write!(
                f,
                "filesystem path {path:?} resolves outside every authorized root; only the \
                 workspace root and the read-only managed tool-output root are accessible"
            ),
            Self::ManagedOutputReadOnly(path) => write!(
                f,
                "filesystem path {path:?} is inside the managed tool-output root, which is \
                 read-only auxiliary storage; Write/Edit never mutate it"
            ),
            Self::Unresolvable(path, error) => write!(
                f,
                "filesystem path {path:?} cannot be resolved on the filesystem: {error}"
            ),
        }
    }
}

impl std::error::Error for LocatorError {}

/// The operation whose authority is being decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocatorOperation {
    /// A read-only operation (Read, Grep, Glob): authorized against the
    /// workspace root and the managed tool-output root.
    Read,
    /// A mutating operation (Write, Edit): authorized against the workspace
    /// root only.
    Mutate,
}

/// Resolves one absolute model-facing locator for `operation`.
///
/// The locator must be absolute; the resolved canonical path must be
/// contained in an authorized root for the operation (see the module
/// matrix). For [`LocatorOperation::Read`] the target must exist and is
/// canonicalized directly; for [`LocatorOperation::Mutate`] the target may
/// not exist yet, so the deepest existing ancestor is canonicalized and the
/// remaining components are appended — a symlinked parent can never escape
/// the workspace.
///
/// # Errors
///
/// Returns the specific [`LocatorError`] of the first violation.
pub fn resolve(
    workspace: &Workspace,
    tool_output: &ManagedToolOutput,
    locator: &str,
    operation: LocatorOperation,
) -> Result<PathBuf, LocatorError> {
    let path = Path::new(locator);
    if !path.is_absolute() {
        return Err(LocatorError::NotAbsolute(locator.to_owned()));
    }
    let canonical = match operation {
        LocatorOperation::Read => std::fs::canonicalize(path),
        LocatorOperation::Mutate => canonicalize_deepest_existing(path),
    }
    .map_err(|error| LocatorError::Unresolvable(locator.to_owned(), error.to_string()))?;
    if canonical.starts_with(workspace.root()) {
        return Ok(canonical);
    }
    if canonical.starts_with(tool_output.root()) {
        return match operation {
            LocatorOperation::Read => Ok(canonical),
            LocatorOperation::Mutate => {
                Err(LocatorError::ManagedOutputReadOnly(locator.to_owned()))
            }
        };
    }
    Err(LocatorError::OutsideAuthorizedRoots(locator.to_owned()))
}

/// Canonicalizes the deepest existing ancestor of `path` and appends the
/// remaining components, so non-existent targets (Write) resolve
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
    use super::{LocatorError, LocatorOperation, resolve};
    use crate::runtime::identity::ConversationId;
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::workspace::Workspace;
    use std::fs;

    struct Roots {
        _dir: tempfile::TempDir,
        workspace: Workspace,
        tool_output: ManagedToolOutput,
    }

    fn roots() -> Roots {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("workspace/sub")).expect("workspace");
        fs::write(dir.path().join("workspace/file.txt"), "x").expect("file");
        let workspace = Workspace::new(dir.path().join("workspace")).expect("workspace");
        let tool_output = ManagedToolOutput::new(
            ConversationId::new("conv-1"),
            dir.path().join("tool-output"),
        )
        .expect("managed output");
        let spill = tool_output.open_spill().expect("spill");
        drop(spill);
        Roots {
            _dir: dir,
            workspace,
            tool_output,
        }
    }

    fn absolute(path: &std::path::Path) -> String {
        path.to_str().expect("utf8 path").to_owned()
    }

    #[test]
    fn relative_locators_are_rejected_for_every_operation() {
        let roots = roots();
        for operation in [LocatorOperation::Read, LocatorOperation::Mutate] {
            for locator in ["file.txt", "sub/file.txt", "../escape", "./x", ""] {
                assert!(
                    matches!(
                        resolve(&roots.workspace, &roots.tool_output, locator, operation),
                        Err(LocatorError::NotAbsolute(_))
                    ),
                    "{locator:?} must be rejected for {operation:?}"
                );
            }
        }
    }

    #[test]
    fn workspace_locators_are_authorized_for_read_and_mutate() {
        let roots = roots();
        let existing = roots.workspace.root().join("file.txt");
        let resolved = resolve(
            &roots.workspace,
            &roots.tool_output,
            &absolute(&existing),
            LocatorOperation::Read,
        )
        .expect("read inside the workspace");
        assert_eq!(resolved, existing.canonicalize().expect("canonical"));
        let missing = roots.workspace.root().join("new/deep/file.txt");
        let resolved = resolve(
            &roots.workspace,
            &roots.tool_output,
            &absolute(&missing),
            LocatorOperation::Mutate,
        )
        .expect("write to a not-yet-existing workspace path");
        assert!(resolved.starts_with(roots.workspace.root()));
    }

    #[test]
    fn managed_output_is_read_only() {
        let roots = roots();
        let spill = roots.tool_output.root().join("output_1.log");
        let locator = absolute(&spill);
        resolve(
            &roots.workspace,
            &roots.tool_output,
            &locator,
            LocatorOperation::Read,
        )
        .expect("read of managed output");
        assert!(
            matches!(
                resolve(
                    &roots.workspace,
                    &roots.tool_output,
                    &locator,
                    LocatorOperation::Mutate
                ),
                Err(LocatorError::ManagedOutputReadOnly(_))
            ),
            "mutation of the managed tool-output root is rejected by the authority contract"
        );
    }

    #[test]
    fn absolute_host_paths_outside_every_root_are_rejected() {
        let roots = roots();
        // A real absolute file outside every authorized root (a fresh
        // tempdir), so the assertion does not depend on a platform-specific
        // host path existing.
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("outside.txt"), "outside").expect("outside file");
        let outside_locator = absolute(&outside.path().join("outside.txt"));
        for operation in [LocatorOperation::Read, LocatorOperation::Mutate] {
            assert!(
                matches!(
                    resolve(
                        &roots.workspace,
                        &roots.tool_output,
                        &outside_locator,
                        operation
                    ),
                    Err(LocatorError::OutsideAuthorizedRoots(_))
                ),
                "absolute syntax is not authorization ({operation:?})"
            );
        }
        // The enclosing runtime-private directory of the managed-output
        // root is not implicitly opened.
        let enclosing = roots
            .tool_output
            .root()
            .parent()
            .expect("the managed root has a parent")
            .join("conversation.sqlite");
        fs::write(&enclosing, "private").expect("private file");
        assert!(
            matches!(
                resolve(
                    &roots.workspace,
                    &roots.tool_output,
                    &absolute(&enclosing),
                    LocatorOperation::Read
                ),
                Err(LocatorError::OutsideAuthorizedRoots(_))
            ),
            "the runtime-private sibling of the managed root stays closed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_their_owning_root() {
        use std::os::unix::fs::symlink;
        let roots = roots();
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("secret.txt"), "secret").expect("secret");

        // A symlink inside the workspace pointing outside it: rejected for
        // both reads and not-yet-existing mutation targets below it.
        symlink(outside.path(), roots.workspace.root().join("linked")).expect("workspace symlink");
        let read_locator = absolute(&roots.workspace.root().join("linked/secret.txt"));
        assert!(matches!(
            resolve(
                &roots.workspace,
                &roots.tool_output,
                &read_locator,
                LocatorOperation::Read
            ),
            Err(LocatorError::OutsideAuthorizedRoots(_))
        ));
        let write_locator = absolute(&roots.workspace.root().join("linked/new.txt"));
        assert!(matches!(
            resolve(
                &roots.workspace,
                &roots.tool_output,
                &write_locator,
                LocatorOperation::Mutate
            ),
            Err(LocatorError::OutsideAuthorizedRoots(_))
        ));

        // A symlink inside the managed-output root pointing outside it:
        // rejected for reads as well.
        symlink(outside.path(), roots.tool_output.root().join("linked")).expect("managed symlink");
        let managed_locator = absolute(&roots.tool_output.root().join("linked/secret.txt"));
        assert!(matches!(
            resolve(
                &roots.workspace,
                &roots.tool_output,
                &managed_locator,
                LocatorOperation::Read
            ),
            Err(LocatorError::OutsideAuthorizedRoots(_))
        ));
    }
}
