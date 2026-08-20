//! The private native-search substrate shared by Glob and Grep.
//!
//! Glob and Grep answer two different questions about the same workspace:
//! Glob asks *which paths match a pattern*, Grep asks *which lines match an
//! expression*. Both questions are asked over one filesystem universe, and
//! that universe is the responsibility of this module — not of either tool,
//! and not of whatever defaults the underlying traversal crate happens to
//! ship with.
//!
//! ```text
//! Glob contract/executor      Grep contract/executor
//!             \                        /
//!              \                      /
//!        shared native-search traversal  <- this module
//!                      |
//!             ignore::WalkBuilder (explicitly configured)
//! ```
//!
//! # The one filesystem-universe policy
//!
//! Given the same workspace and the same search root, Glob and Grep observe
//! exactly the same set of files. A caller-supplied filter (Glob's pattern,
//! Grep's optional `glob`) only ever *narrows* that shared set; it can never
//! widen it, and neither tool can reach a file the other cannot see.
//!
//! The policy is stated here, once, and is deliberately explicit rather than
//! inherited:
//!
//! - **Containment.** The search root is resolved through the canonical
//!   locator authority ([`crate::tools::locator`]): an omitted `path` means
//!   the workspace root; a supplied path is an absolute locator contained
//!   in the workspace root or the read-only managed tool-output root.
//!   Enumeration never leaves the resolved root.
//! - **Ignore files are not applied.** `.gitignore`, `.ignore`, git's global
//!   excludes, and `.git/info/exclude` have no effect. A workspace file is
//!   part of the search universe because it exists, not because a version
//!   control system happens to track it. Every ignore mechanism of the
//!   underlying crate is switched off explicitly, so a future default change
//!   in that crate cannot silently redefine rustX semantics.
//! - **Hidden files are visible.** A leading dot has no meaning here.
//! - **Symlinks are never followed.** Neither a directory symlink (which
//!   would recurse, possibly outside the workspace or into a cycle) nor a
//!   file symlink (whose target may live outside the workspace) is part of
//!   the universe. Only regular files are enumerated.
//! - **Normalized relative paths.** Every enumerated file is identified by
//!   its forward-slash path relative to the *search root*, never by an
//!   absolute path. A single-file root (one managed spill file searched
//!   directly) is identified by its file name.
//! - **Deterministic enumeration.** Files are returned in lexical order of
//!   that normalized relative path, so physical filesystem enumeration order
//!   can never become observable result order.
//!
//! This module is not a tool. It is not registered with the tool registry,
//! it is not a search-provider trait, and it is not an extension point: it
//! exists because Glob and Grep share one concrete responsibility.
//!
//! [`Workspace`]: crate::tools::workspace::Workspace
//! [`ManagedToolOutput`]: crate::tools::managed_output::ManagedToolOutput

mod traversal;

use std::path::PathBuf;

use crate::tools::locator::{LocatorOperation, resolve};
use crate::tools::managed_output::ManagedToolOutput;
use crate::tools::workspace::Workspace;

pub(super) use traversal::SearchFile;

/// One resolved search root: the canonical directory (or single file) whose
/// file universe Glob and Grep observe identically.
pub(super) enum SearchRoot {
    /// A canonical directory the traversal starts from.
    Directory(PathBuf),
    /// One canonical file (for example a single managed spill file): the
    /// universe is exactly that file.
    File(PathBuf),
}

impl SearchRoot {
    /// Resolves the model-supplied search root through the one locator
    /// authority; an omitted `path` means the workspace root.
    ///
    /// A supplied path must be an absolute locator contained in the
    /// workspace root or the read-only managed tool-output root. It may
    /// name a directory (searched recursively) or — for Grep — a single
    /// file, so a model can search one managed spill file directly.
    ///
    /// # Errors
    ///
    /// Returns the deterministic locator rejection message when the path is
    /// not absolute, escapes every authorized root, or cannot be resolved.
    pub(super) fn resolve(
        workspace: &Workspace,
        tool_output: &ManagedToolOutput,
        path: Option<&str>,
    ) -> Result<Self, String> {
        let Some(requested) = path else {
            return Ok(Self::Directory(workspace.root().to_path_buf()));
        };
        let absolute = resolve(workspace, tool_output, requested, LocatorOperation::Read)
            .map_err(|error| error.to_string())?;
        if absolute.is_dir() {
            return Ok(Self::Directory(absolute));
        }
        if absolute.is_file() {
            return Ok(Self::File(absolute));
        }
        Err(format!(
            "{} is not a directory or a file",
            absolute.display()
        ))
    }

    /// The shared file universe of this search root, in deterministic
    /// lexical order of the normalized root-relative path.
    ///
    /// # Errors
    ///
    /// Returns an explicit traversal diagnostic when a directory below the
    /// root cannot be enumerated.
    pub(super) fn files(&self) -> Result<Vec<SearchFile>, String> {
        match self {
            Self::Directory(root) => traversal::enumerate(root),
            Self::File(path) => Ok(vec![SearchFile {
                relative: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                absolute: path.clone(),
            }]),
        }
    }

    /// Whether this root is a single file. Glob searches a directory
    /// universe only; a single-file root is a Grep-only contract.
    pub(super) fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }
}
