//! Bounded traversal of pi-style Skill collections. A package marker ends
//! traversal before any of that package's supporting files are enumerated.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::package::{MAX_SKILL_ROOT_ENTRIES, SKILL_MARKDOWN_FILE};
use crate::runtime::CancellationSignal;

pub(crate) const IGNORE_FILES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];
const MAX_DEPTH: usize = 64;
const MAX_ENTRIES: usize = MAX_SKILL_ROOT_ENTRIES * 16;

pub(crate) struct Collection {
    pub files: Vec<PathBuf>,
    pub directories: Vec<PathBuf>,
    pub error: Option<std::io::Error>,
}

pub(crate) fn is_shared(root: &Path) -> bool {
    root.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == ".agents")
}

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "md")
}

pub(crate) fn hidden(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b".") || name == "node_modules"
}

pub(crate) fn canonical_root(path: &Path, workspace: Option<&Path>) -> std::io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    if workspace.is_some_and(|workspace| !canonical.starts_with(workspace)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Skill collection resolves outside its Workspace authority",
        ));
    }
    if !canonical.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "Skill collection is not a directory",
        ));
    }
    Ok(canonical)
}

pub(crate) fn collect(
    root: &Path,
    shared: bool,
    boundary: Option<&Path>,
    signal: &CancellationSignal,
) -> Collection {
    let mut scanner = Scanner {
        shared,
        boundary,
        signal,
        remaining: MAX_ENTRIES,
        ancestors: BTreeSet::new(),
        rules: Vec::new(),
        found: Collection {
            files: Vec::new(),
            directories: Vec::new(),
            error: None,
        },
    };
    scanner.found.error = scanner.visit(root, 0).err();
    scanner.found
}

struct Scanner<'a> {
    shared: bool,
    boundary: Option<&'a Path>,
    signal: &'a CancellationSignal,
    remaining: usize,
    ancestors: BTreeSet<PathBuf>,
    rules: Vec<Gitignore>,
    found: Collection,
}

impl Scanner<'_> {
    fn visit(&mut self, path: &Path, depth: usize) -> std::io::Result<()> {
        crate::package_io::check_cancelled(self.signal)?;
        let canonical = std::fs::canonicalize(path)?;
        if self
            .boundary
            .is_some_and(|boundary| !canonical.starts_with(boundary))
            || !self.ancestors.insert(canonical.clone())
        {
            return Ok(());
        }
        if depth > MAX_DEPTH {
            return Err(std::io::Error::other(
                "Skill collection exceeds its discovery depth",
            ));
        }
        self.found.directories.push(path.to_path_buf());
        let mut builder = GitignoreBuilder::new(path);
        for name in IGNORE_FILES {
            let file = path.join(name);
            if file.is_file()
                && self.boundary.is_none_or(|boundary| {
                    std::fs::canonicalize(&file).is_ok_and(|file| file.starts_with(boundary))
                })
            {
                // Bound ignore input too. Keep a usable subset of rules if an individual line is invalid.
                if let Ok(bytes) = crate::bounded_file::read_with_limit(&file, 1024 * 1024)
                    && let Ok(content) = std::str::from_utf8(&bytes)
                {
                    for line in content.lines() {
                        let _ = builder.add_line(Some(file.clone()), line);
                    }
                }
            }
        }
        self.rules
            .push(builder.build().map_err(std::io::Error::other)?);
        let result = self.children(path, depth);
        self.rules.pop();
        self.ancestors.remove(&canonical);
        result
    }

    fn ignored(&self, path: &Path, directory: bool) -> bool {
        self.rules
            .iter()
            .rev()
            .map(|rules| rules.matched(path, directory))
            .find(|result| !result.is_none())
            .is_some_and(|result| result.is_ignore())
    }

    fn children(&mut self, path: &Path, depth: usize) -> std::io::Result<()> {
        let marker = path.join(SKILL_MARKDOWN_FILE);
        if marker.is_file() && !self.ignored(&marker, false) {
            self.found.files.push(marker);
            return Ok(());
        }
        let entries = std::fs::read_dir(path)?
            .take(MAX_SKILL_ROOT_ENTRIES.min(self.remaining) + 1)
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        if entries.len() > MAX_SKILL_ROOT_ENTRIES {
            return Err(std::io::Error::other(format!(
                "Skill directory exceeds {MAX_SKILL_ROOT_ENTRIES} entries",
            )));
        }
        if entries.len() > self.remaining {
            return Err(std::io::Error::other(
                "Skill collection exceeds its discovery entry budget",
            ));
        }
        self.remaining -= entries.len();
        let mut entries = entries;
        entries.sort();
        for entry in entries {
            crate::package_io::check_cancelled(self.signal)?;
            if entry.file_name().is_some_and(hidden) {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&entry) else {
                continue;
            };
            if self.ignored(&entry, metadata.is_dir()) {
                continue;
            }
            if metadata.is_dir() {
                // A broken/unreadable child is local to that branch. Budget
                // and cancellation failures still stop the entire collection.
                match self.visit(&entry, depth + 1) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                        ) => {}
                    Err(error) => return Err(error),
                }
            } else if metadata.is_file() && is_markdown(&entry) && self.shared == (depth > 0) {
                self.found.files.push(entry);
            }
        }
        Ok(())
    }
}
