//! The runtime-owned filesystem locator/authority boundary.
//!
//! Runtime subsystems that expose managed tool output consume **absolute**
//! locators. An absolute path is a locator, never authority: this module is
//! the boundary that resolves one against explicitly authorized roots and
//! enforces the per-operation mutability contract. Native
//! Read/Write/Edit/Grep/Glob intentionally do not call this module; they
//! resolve model paths against the execution cwd and accept absolute host
//! paths.
//!
//! ```text
//! runtime-authorized root   operation-specific managed-output checks
//! every other runtime path  rejected
//! ```
//!
//! Ownership stays with the owning types: [`Workspace`] owns the canonical
//! workspace root, [`ManagedToolOutput`] owns the canonical managed-output
//! root, and this module owns only the resolution/authorization decision —
//! it is not a VFS and it never stores paths.
//!
//! # Canonicalization
//!
//! Authorized roots are not one interchangeable union: a locator retains
//! its **lexical owning root**, determined before any symlink traversal.
//! The algorithm is:
//!
//! ```text
//! absolute locator
//!     |
//!     v
//! lexical normalization (`.` / `..` resolved lexically)
//!     |
//!     v
//! determine the owning root lexically: workspace, managed output,
//!     or none -> reject
//!     |
//!     v
//! canonicalize (Read: the target itself; Mutate: the deepest existing
//!     ancestor plus the remaining components)
//!     |
//!     v
//! the canonical result must remain inside the SAME owning root
//!     |
//!     v
//! apply that owner's permissions
//! ```
//!
//! A symlink can therefore never transfer authority between roots:
//! `managed-output/link -> workspace` and `workspace/link ->
//! managed-output` are both rejected even though both targets are
//! otherwise authorized roots. A locator without a lexical owner (an
//! arbitrary host path, or one that reaches an authorized root only
//! through a symlinked ancestor outside both roots) is rejected: the
//! canonical model-visible roots are the locators the runtime advertises.

use std::path::{Path, PathBuf};

use crate::tools::managed_output::ManagedToolOutput;
use crate::tools::workspace::Workspace;

/// A filesystem locator resolution/authorization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocatorError {
    /// The caller supplied a non-absolute runtime locator; this authority
    /// boundary accepts absolute paths only.
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
                "filesystem path {path:?} is not absolute; runtime locators are absolute paths"
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
    /// A read-only runtime operation: authorized against the workspace root
    /// and the managed tool-output root.
    Read,
    /// A mutating operation (Write, Edit): authorized against the workspace
    /// root only.
    Mutate,
}

/// The one owning root of a locator, determined lexically before any
/// symlink traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwningRoot {
    /// The runtime workspace root.
    Workspace,
    /// The read-only managed tool-output root (Read/Grep/Glob only).
    ManagedOutput,
}

/// Resolves one absolute runtime locator for `operation`.
///
/// The locator must be absolute; its lexical owning root is determined
/// first, the locator is canonicalized, and the canonical result must
/// remain inside that same owning root (see the module documentation).
/// For [`LocatorOperation::Read`] the target must exist and is
/// canonicalized directly; for [`LocatorOperation::Mutate`] the target may
/// not exist yet, so the deepest existing ancestor is canonicalized and
/// the remaining components are appended — a symlinked parent can never
/// escape the workspace.
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
    // The owning root is determined lexically, BEFORE any symlink
    // traversal: authority never transfers between roots through a
    // symlink, and a path that reaches a root only through a symlinked
    // ancestor outside both roots has no owner at all.
    let normalized = normalize_lexical(path);
    let owner = if normalized.starts_with(workspace.root()) {
        OwningRoot::Workspace
    } else if normalized.starts_with(tool_output.root()) {
        OwningRoot::ManagedOutput
    } else {
        return Err(LocatorError::OutsideAuthorizedRoots(locator.to_owned()));
    };
    let canonical = match operation {
        LocatorOperation::Read => std::fs::canonicalize(path),
        LocatorOperation::Mutate => canonicalize_deepest_existing(path),
    }
    .map_err(|error| LocatorError::Unresolvable(locator.to_owned(), error.to_string()))?;
    match owner {
        OwningRoot::Workspace => {
            if canonical.starts_with(workspace.root()) {
                Ok(canonical)
            } else {
                // A workspace locator escaped its owning root through a
                // symlink — including into the managed tool-output root,
                // which would be an authority transfer.
                Err(LocatorError::OutsideAuthorizedRoots(locator.to_owned()))
            }
        }
        OwningRoot::ManagedOutput => {
            if !canonical.starts_with(tool_output.root()) {
                // A managed-output locator escaped its owning root through
                // a symlink — including into the workspace, which would
                // turn the read-only region into mutation authority.
                return Err(LocatorError::OutsideAuthorizedRoots(locator.to_owned()));
            }
            match operation {
                LocatorOperation::Read => Ok(canonical),
                LocatorOperation::Mutate => {
                    Err(LocatorError::ManagedOutputReadOnly(locator.to_owned()))
                }
            }
        }
    }
}

/// Lexically normalizes an absolute path: `.` components are dropped and
/// `..` components are resolved against the preceding lexical components
/// without touching the filesystem, so the owning-root determination
/// cannot be fooled by `/workspace/../tool-output/x`-style locators. A
/// `..` at the filesystem root stays at the root (`/..` is `/`).
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // Pop the last normal component; at the root a parent
                // directory is the root itself.
                normalized.pop();
            }
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
        }
    }
    normalized
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
        let spill = roots.tool_output.root().join("results/result_1.txt");
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

    /// Authority never transfers between authorized roots: a symlink
    /// inside the managed-output root that points INTO the workspace is
    /// rejected, and so is a workspace symlink that points INTO the
    /// managed-output root. Both targets are otherwise authorized roots;
    /// the owning root of a locator is lexical and never changes through
    /// symlink traversal.
    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_transfer_authority_between_authorized_roots() {
        use std::os::unix::fs::symlink;
        let roots = roots();

        // managed descendant symlink -> workspace file: Read rejected even
        // though the workspace target is readable.
        symlink(
            roots.workspace.root(),
            roots.tool_output.root().join("workspace-alias"),
        )
        .expect("managed -> workspace symlink");
        let locator = absolute(&roots.tool_output.root().join("workspace-alias/file.txt"));
        assert!(
            matches!(
                resolve(
                    &roots.workspace,
                    &roots.tool_output,
                    &locator,
                    LocatorOperation::Read
                ),
                Err(LocatorError::OutsideAuthorizedRoots(_))
            ),
            "managed -> workspace transfers no read authority"
        );
        // managed descendant symlink -> workspace new mutation target:
        // Mutate rejected (it would escape the read-only region AND
        // cross roots).
        let locator = absolute(&roots.tool_output.root().join("workspace-alias/new.txt"));
        assert!(
            matches!(
                resolve(
                    &roots.workspace,
                    &roots.tool_output,
                    &locator,
                    LocatorOperation::Mutate
                ),
                Err(LocatorError::OutsideAuthorizedRoots(_))
            ),
            "managed -> workspace transfers no mutation authority"
        );

        // workspace descendant symlink -> managed file: Read rejected even
        // though the managed target is readable.
        let spill = roots.tool_output.open_spill().expect("spill");
        let spill_path = spill.path().to_path_buf();
        drop(spill);
        let results_alias = roots.workspace.root().join("results-alias");
        symlink(roots.tool_output.root().join("results"), &results_alias)
            .expect("workspace -> managed symlink");
        let locator = absolute(&results_alias.join(spill_path.file_name().expect("spill name")));
        assert!(
            matches!(
                resolve(
                    &roots.workspace,
                    &roots.tool_output,
                    &locator,
                    LocatorOperation::Read
                ),
                Err(LocatorError::OutsideAuthorizedRoots(_))
            ),
            "workspace -> managed transfers no read authority"
        );
        // workspace descendant symlink -> managed mutation target:
        // rejected by the same-root invariant before the read-only rule
        // even applies.
        let locator = absolute(&results_alias.join("forged.txt"));
        assert!(
            matches!(
                resolve(
                    &roots.workspace,
                    &roots.tool_output,
                    &locator,
                    LocatorOperation::Mutate
                ),
                Err(LocatorError::OutsideAuthorizedRoots(_))
            ),
            "workspace -> managed transfers no mutation authority"
        );
    }

    /// A same-root internal symlink stays coherent: a workspace symlink
    /// whose target remains inside the workspace resolves and is readable,
    /// and a managed-root-internal symlink to a real managed file is
    /// readable but never mutable.
    #[cfg(unix)]
    #[test]
    fn same_root_internal_symlinks_stay_coherent() {
        use std::os::unix::fs::symlink;
        let roots = roots();

        symlink(
            roots.workspace.root().join("file.txt"),
            roots.workspace.root().join("internal.txt"),
        )
        .expect("workspace internal symlink");
        let locator = absolute(&roots.workspace.root().join("internal.txt"));
        let resolved = resolve(
            &roots.workspace,
            &roots.tool_output,
            &locator,
            LocatorOperation::Read,
        )
        .expect("a same-root workspace symlink resolves");
        assert!(resolved.starts_with(roots.workspace.root()));

        // Lexical `..` / `.` normalization: the owning root is determined
        // after lexical normalization, so `/workspace/sub/../file.txt`
        // keeps its workspace owner and `/workspace/../<managed>` can
        // never impersonate the workspace.
        let locator = format!("{}/sub/../file.txt", absolute(roots.workspace.root()));
        resolve(
            &roots.workspace,
            &roots.tool_output,
            &locator,
            LocatorOperation::Read,
        )
        .expect("a lexically normalized workspace locator resolves");
    }
}
