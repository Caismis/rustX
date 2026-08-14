//! The one traversal implementing the shared native-search policy.
//!
//! Every policy decision of the module documentation is made here, once, and
//! explicitly. `ignore::WalkBuilder` is used as a Rust-native directory
//! walker only: every filter it applies by default is switched off by name,
//! so the observable rustX file universe is defined by this function rather
//! than by the crate's defaults.

use std::path::{Path, PathBuf};

/// One file of the shared search universe.
pub(in crate::tools::native) struct SearchFile {
    /// The normalized forward-slash path relative to the search root.
    pub relative: String,
    /// The absolute path used to open the file.
    pub absolute: PathBuf,
}

/// Enumerates the shared file universe below `root`.
///
/// The returned files are regular files only, identified by their
/// normalized root-relative path, in lexical order of that path.
///
/// # Errors
///
/// Returns an explicit diagnostic when a directory below the root cannot be
/// enumerated; a partial universe is never reported as a complete one.
pub(super) fn enumerate(root: &Path) -> Result<Vec<SearchFile>, String> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        // Ignore-file semantics are never applied implicitly. Each
        // mechanism is disabled by name so a future default change in the
        // crate cannot redefine the rustX file universe.
        .standard_filters(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .parents(false)
        // Hidden files are ordinary workspace files.
        .hidden(false)
        // Symlinks are never followed: a directory symlink must not recurse
        // and a file symlink must not smuggle an out-of-workspace target
        // into the universe. Only regular files survive the filter below.
        .follow_links(false)
        // One deterministic traversal; no parallel walker whose completion
        // order would then have to be repaired.
        .threads(1)
        .sort_by_file_path(Path::cmp);
    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = entry.map_err(|error| format!("workspace traversal failed: {error}"))?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        files.push(SearchFile {
            relative: normalize_relative(relative),
            absolute: path.to_path_buf(),
        });
    }
    // The walker is already ordered, but the observable contract is lexical
    // order of the *normalized* path, which is what the tools report and
    // what the tools' callers compare against.
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(files)
}

/// Normalizes a relative path to a deterministic forward-slash string.
fn normalize_relative(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
