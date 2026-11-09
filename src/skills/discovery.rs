//! Traversal of pi-style Skill collections. A package marker ends
//! traversal before any of that package's supporting files are enumerated.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::package::SKILL_MARKDOWN_FILE;
use crate::runtime::CancellationSignal;

pub(crate) const IGNORE_FILES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];
pub(crate) struct Collection {
    pub files: Vec<PathBuf>,
    pub directories: Vec<PathBuf>,
    pub links: Vec<PathBuf>,
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

pub(crate) fn canonical_root(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    if !canonical.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "Skill collection is not a directory",
        ));
    }
    Ok(canonical)
}

pub(crate) fn collect(root: &Path, shared: bool, signal: &CancellationSignal) -> Collection {
    let mut scanner = Scanner {
        shared,
        signal,
        ancestors: BTreeSet::new(),
        rules: Vec::new(),
        found: Collection {
            files: Vec::new(),
            directories: Vec::new(),
            links: Vec::new(),
            error: None,
        },
    };
    scanner.found.error = scanner.visit(root, 0).err();
    scanner.found
}

struct Scanner<'a> {
    shared: bool,
    signal: &'a CancellationSignal,
    ancestors: BTreeSet<PathBuf>,
    rules: Vec<Gitignore>,
    found: Collection,
}

impl Scanner<'_> {
    fn visit(&mut self, path: &Path, depth: usize) -> std::io::Result<()> {
        crate::package_io::check_cancelled(self.signal)?;
        let canonical = std::fs::canonicalize(path)?;
        // Following links must not turn an ancestor link into infinite recursion.
        if !self.ancestors.insert(canonical.clone()) {
            return Ok(());
        }
        self.found.directories.push(path.to_path_buf());
        let mut builder = GitignoreBuilder::new(path);
        for name in IGNORE_FILES {
            let file = path.join(name);
            self.record_link(&file);
            crate::bounded_file::record_read(&file);
            if let Ok(target) = std::fs::canonicalize(&file)
                && target.is_file()
            {
                crate::bounded_file::record_read(&target);
                let _ = builder.add(file);
            }
        }
        self.rules
            .push(builder.build().map_err(std::io::Error::other)?);
        let result = self.children(path, depth);
        self.rules.pop();
        self.ancestors.remove(&canonical);
        result
    }

    fn record_link(&mut self, path: &Path) {
        if path.is_symlink() {
            self.found.links.push(path.to_path_buf());
        }
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
        self.record_link(&marker);
        if marker.is_file() && !self.ignored(&marker, false) {
            self.found.files.push(marker);
            return Ok(());
        }
        let mut entries = std::fs::read_dir(path)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            crate::package_io::check_cancelled(self.signal)?;
            if entry.file_name().is_some_and(hidden) {
                continue;
            }
            if entry != marker {
                self.record_link(&entry);
            }
            let Ok(metadata) = std::fs::metadata(&entry) else {
                continue;
            };
            if self.ignored(&entry, metadata.is_dir()) {
                continue;
            }
            if metadata.is_dir() {
                if let Err(error) = self.visit(&entry, depth + 1)
                    && error.kind() == std::io::ErrorKind::Interrupted
                {
                    return Err(error);
                }
            } else if metadata.is_file() && is_markdown(&entry) && self.shared == (depth > 0) {
                self.found.files.push(entry);
            }
        }
        Ok(())
    }
}
