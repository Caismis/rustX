//! Traversal of pi-style Skill collections. A package marker ends
//! traversal before any of that package's supporting files are enumerated.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rustx_execution_protocol::EntryKind;

use super::package::SKILL_MARKDOWN_FILE;
use super::tree::{Entry, Native, SkillTree};
use crate::runtime::CancellationSignal;

pub(crate) const IGNORE_FILES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];
const MAX_DIRECTORY_ENTRIES: usize = 2048;
const MAX_COLLECTION_ENTRIES: usize = 8192;
const MAX_CANDIDATES: usize = 256;
const MAX_COLLECTION_DEPTH: usize = 128;
const MAX_IGNORE_FILE_BYTES: u64 = crate::bounded_file::MAX_RESOURCE_BYTES;
const MAX_IGNORE_COLLECTION_BYTES: u64 = 4 * MAX_IGNORE_FILE_BYTES;
const MAX_IGNORE_PATTERNS: usize = MAX_COLLECTION_ENTRIES;

pub(crate) struct Collection {
    pub files: Vec<PathBuf>,
    pub directories: Vec<PathBuf>,
    pub links: Vec<PathBuf>,
    pub error: Option<std::io::Error>,
}

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "md")
}

pub(crate) fn hidden(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b".") || name == "node_modules"
}

pub(crate) fn collect(root: &Path, signal: &CancellationSignal) -> Collection {
    collect_from(root, signal, &Native)
}

pub(super) fn collect_from(
    root: &Path,
    signal: &CancellationSignal,
    tree: &dyn SkillTree,
) -> Collection {
    let mut scanner = Scanner {
        signal,
        tree,
        ancestors: BTreeSet::new(),
        rules: Vec::new(),
        remaining_entries: MAX_COLLECTION_ENTRIES,
        remaining_ignore_bytes: MAX_IGNORE_COLLECTION_BYTES,
        remaining_patterns: MAX_IGNORE_PATTERNS,
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
    signal: &'a CancellationSignal,
    tree: &'a dyn SkillTree,
    ancestors: BTreeSet<PathBuf>,
    rules: Vec<Gitignore>,
    remaining_entries: usize,
    remaining_ignore_bytes: u64,
    remaining_patterns: usize,
    found: Collection,
}

impl Scanner<'_> {
    fn visit(&mut self, path: &Path, depth: usize) -> std::io::Result<()> {
        crate::package_io::check_cancelled(self.signal)?;
        let canonical = self.tree.canonical(path)?;
        // Following links must not turn an ancestor link into infinite recursion.
        if !self.ancestors.insert(canonical.clone()) {
            return Ok(());
        }
        if depth > MAX_COLLECTION_DEPTH {
            return Err(invalid_data(format!(
                "Skill collection exceeds {MAX_COLLECTION_DEPTH} directory levels"
            )));
        }
        self.found.directories.push(path.to_path_buf());
        let mut builder = GitignoreBuilder::new(path);
        for name in IGNORE_FILES {
            let file = path.join(name);
            self.tree.observe(&file);
            if self
                .entry(&file)?
                .is_some_and(|entry| entry.kind == EntryKind::RegularFile)
            {
                let target = self.tree.canonical(&file)?;
                self.add_ignore(&mut builder, &file, &target)?;
            }
        }
        self.rules
            .push(builder.build().map_err(std::io::Error::other)?);
        let result = self.children(path, depth);
        self.rules.pop();
        self.ancestors.remove(&canonical);
        result
    }

    fn add_ignore(
        &mut self,
        builder: &mut GitignoreBuilder,
        file: &Path,
        target: &Path,
    ) -> std::io::Result<()> {
        let limit = self.remaining_ignore_bytes.min(MAX_IGNORE_FILE_BYTES);
        let bytes = self
            .tree
            .read_file(target, limit)
            .map_err(|error| invalid_data(error.to_string()))?;
        self.remaining_ignore_bytes -= bytes.len() as u64;
        for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            crate::package_io::check_cancelled(self.signal)?;
            let Ok(line) = std::str::from_utf8(line) else {
                break;
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            let line = if index == 0 {
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            if line.starts_with('#') || line.trim_end().is_empty() {
                continue;
            }
            if self.remaining_patterns == 0 {
                return Err(invalid_data(format!(
                    "Skill collection exceeds {MAX_IGNORE_PATTERNS} ignore patterns"
                )));
            }
            self.remaining_patterns -= 1;
            let _ = builder.add_line(Some(file.to_path_buf()), line);
        }
        Ok(())
    }

    fn entry(&mut self, path: &Path) -> std::io::Result<Option<Entry>> {
        let entry = self.tree.entry(path)?;
        if entry.as_ref().is_some_and(|entry| entry.link) {
            self.found.links.push(path.to_path_buf());
        }
        Ok(entry)
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
        let marker_kind = self.entry(&marker)?.map(|entry| entry.kind);
        if marker_kind == Some(EntryKind::RegularFile) && !self.ignored(&marker, false) {
            return self.offer(marker);
        }
        let limit = MAX_DIRECTORY_ENTRIES.min(self.remaining_entries);
        let mut entries = self.tree.children(path, limit + 1)?;
        if entries.len() > limit {
            let message = if limit == MAX_DIRECTORY_ENTRIES {
                format!("Skill collection directory exceeds {MAX_DIRECTORY_ENTRIES} entries")
            } else {
                format!("Skill collection exceeds {MAX_COLLECTION_ENTRIES} entries")
            };
            return Err(invalid_data(message));
        }
        self.remaining_entries -= entries.len();
        entries.sort();
        for entry in entries {
            crate::package_io::check_cancelled(self.signal)?;
            if entry.file_name().is_some_and(hidden) {
                continue;
            }
            let kind = if entry == marker {
                marker_kind
            } else {
                self.entry(&entry)?.map(|entry| entry.kind)
            };
            let Some(kind) = kind else {
                continue;
            };
            if self.ignored(&entry, kind == EntryKind::Directory) {
                continue;
            }
            if kind == EntryKind::Directory {
                self.visit(&entry, depth + 1)?;
            } else if kind == EntryKind::RegularFile && is_markdown(&entry) && depth == 0 {
                self.offer(entry)?;
            }
        }
        Ok(())
    }

    fn offer(&mut self, path: PathBuf) -> std::io::Result<()> {
        if self.found.files.len() == MAX_CANDIDATES {
            return Err(invalid_data(format!(
                "Skill collection exceeds {MAX_CANDIDATES} candidate files"
            )));
        }
        self.found.files.push(path);
        Ok(())
    }
}

fn invalid_data(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(root: &Path, message: &str) {
        let collection = collect(root, &CancellationSignal::new());
        let error = collection
            .error
            .expect("collection budget must stop discovery");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains(message), "{error}");
    }

    #[test]
    fn collection_budgets_apply_inside_nested_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("skills");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        for index in 0..=MAX_DIRECTORY_ENTRIES {
            std::fs::write(nested.join(index.to_string()), []).unwrap();
        }
        rejected(&root, "directory exceeds");
    }

    #[test]
    fn collection_budget_bounds_repeated_symlink_traversal() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("skills");
        std::fs::create_dir(&root).unwrap();
        let levels: Vec<_> = (0..15)
            .map(|level| temporary.path().join(level.to_string()))
            .collect();
        for level in &levels {
            std::fs::create_dir(level).unwrap();
        }
        for pair in levels.windows(2) {
            for name in ["first", "second"] {
                std::os::unix::fs::symlink(&pair[1], pair[0].join(name)).unwrap();
            }
        }
        std::os::unix::fs::symlink(&levels[0], root.join("linked")).unwrap();
        rejected(&root, "collection exceeds");
    }

    #[test]
    fn candidate_and_depth_budgets_apply_before_parsing() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("skills");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        for index in 0..=MAX_CANDIDATES {
            let package = nested.join(index.to_string());
            std::fs::create_dir(&package).unwrap();
            std::fs::write(package.join(SKILL_MARKDOWN_FILE), []).unwrap();
        }
        rejected(&root, "candidate files");

        std::fs::remove_dir_all(nested).unwrap();
        let mut deep = root.clone();
        for _ in 0..=MAX_COLLECTION_DEPTH {
            deep.push("nested");
        }
        std::fs::create_dir_all(deep).unwrap();
        rejected(&root, "directory levels");
    }

    #[test]
    fn ignore_reads_are_bounded_even_when_linked() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("skills");
        std::fs::create_dir(&root).unwrap();
        let target = temporary.path().join("external-ignore");
        let file = std::fs::File::create(&target).unwrap();
        file.set_len(MAX_IGNORE_FILE_BYTES + 1).unwrap();
        std::os::unix::fs::symlink(&target, root.join(".ignore")).unwrap();
        rejected(&root, "1048576");
        std::fs::write(target, "file\n".repeat(MAX_IGNORE_PATTERNS + 1)).unwrap();
        rejected(&root, "ignore patterns");
    }
}
