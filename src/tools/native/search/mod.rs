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
//!   [`Workspace`] boundary, so it is always inside the workspace root.
//!   Enumeration never leaves that root.
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
//!   absolute path.
//! - **Deterministic enumeration.** Files are returned in lexical order of
//!   that normalized relative path, so physical filesystem enumeration order
//!   can never become observable result order.
//!
//! This module is not a tool. It is not registered with the tool registry,
//! it is not a search-provider trait, and it is not an extension point: it
//! exists because Glob and Grep share one concrete responsibility.
//!
//! [`Workspace`]: crate::tools::workspace::Workspace

mod traversal;

use std::path::PathBuf;

use crate::tools::workspace::Workspace;

pub(super) use traversal::SearchFile;

/// The workspace-relative search root used when a tool's `path` is omitted.
///
/// Both Glob and Grep default to the whole workspace, so an omitted `path`
/// means the same thing for both tools.
pub(super) const DEFAULT_SEARCH_ROOT: &str = ".";

/// One resolved search root: the canonical directory whose file universe
/// Glob and Grep observe identically.
pub(super) struct SearchRoot {
    /// The canonical absolute directory the traversal starts from.
    absolute: PathBuf,
}

impl SearchRoot {
    /// Resolves the model-supplied search root through the workspace
    /// boundary; an omitted `path` means [`DEFAULT_SEARCH_ROOT`].
    ///
    /// # Errors
    ///
    /// Returns the deterministic workspace rejection message when the path
    /// escapes the workspace or cannot be resolved, and an explicit
    /// diagnostic when it does not name a directory.
    pub(super) fn resolve(workspace: &Workspace, path: Option<&str>) -> Result<Self, String> {
        let requested = path.unwrap_or(DEFAULT_SEARCH_ROOT);
        let absolute = workspace
            .resolve(requested)
            .map_err(|error| error.to_string())?;
        if !absolute.is_dir() {
            let display = workspace.relative(&absolute).unwrap_or_default();
            return Err(format!("{display} is not a directory"));
        }
        Ok(Self { absolute })
    }

    /// The shared file universe of this search root, in deterministic
    /// lexical order of the normalized root-relative path.
    ///
    /// # Errors
    ///
    /// Returns an explicit traversal diagnostic when a directory below the
    /// root cannot be enumerated.
    pub(super) fn files(&self) -> Result<Vec<SearchFile>, String> {
        traversal::enumerate(&self.absolute)
    }
}
