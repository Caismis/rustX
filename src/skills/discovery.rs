//! Traversal of pi-style Skill collections. A package marker ends
//! traversal before any of that package's supporting files are enumerated.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::package::SKILL_MARKDOWN_FILE;
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

pub(crate) fn collect(root: &Path, signal: &CancellationSignal) -> Collection {
    let mut scanner = Scanner {
        signal,
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
        let canonical = std::fs::canonicalize(path)?;
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
            self.record_link(&file);
            crate::bounded_file::record_read(&file);
            if let Ok(target) = std::fs::canonicalize(&file)
                && target.is_file()
            {
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
        let bytes = crate::bounded_file::read_with_limit(target, limit).map_err(invalid_data)?;
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
            return self.offer(marker);
        }
        let limit = MAX_DIRECTORY_ENTRIES.min(self.remaining_entries);
        let mut entries = Vec::new();
        for (index, entry) in std::fs::read_dir(path)?.enumerate() {
            crate::package_io::check_cancelled(self.signal)?;
            if index == limit {
                let message = if limit == MAX_DIRECTORY_ENTRIES {
                    format!("Skill collection directory exceeds {MAX_DIRECTORY_ENTRIES} entries")
                } else {
                    format!("Skill collection exceeds {MAX_COLLECTION_ENTRIES} entries")
                };
                return Err(invalid_data(message));
            }
            self.remaining_entries -= 1;
            if let Ok(entry) = entry {
                entries.push(entry.path());
            }
        }
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
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::InvalidData
                    )
                {
                    return Err(error);
                }
            } else if metadata.is_file() && is_markdown(&entry) && depth == 0 {
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
