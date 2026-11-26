//! Physical readers for the shared Skill collection scanner.

use std::io;
use std::path::{Path, PathBuf};

use rustx_execution_protocol::EntryKind;

use crate::runtime::CancellationSignal;

pub trait SkillTree {
    fn canonical(&self, path: &Path) -> io::Result<PathBuf>;
    fn entry(&self, path: &Path) -> io::Result<Option<Entry>>;
    fn children(&self, path: &Path, limit: usize) -> io::Result<Children>;
    fn read_file(&self, path: &Path, bound: u64) -> io::Result<Vec<u8>>;
    fn frontmatter(&self, path: &Path, signal: &CancellationSignal) -> io::Result<Vec<u8>>;
    fn observe(&self, path: &Path);
}

pub struct Children {
    pub paths: Vec<PathBuf>,
    pub scanned: usize,
}

pub struct Entry {
    pub kind: EntryKind,
    pub link: bool,
}

pub struct Native;

impl SkillTree for Native {
    fn canonical(&self, path: &Path) -> io::Result<PathBuf> {
        path.canonicalize()
    }

    fn entry(&self, path: &Path) -> io::Result<Option<Entry>> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let link = metadata.is_symlink();
        let target = if link {
            std::fs::metadata(path).ok()
        } else {
            Some(metadata)
        };
        let kind = match target {
            Some(target) if target.is_dir() => EntryKind::Directory,
            Some(target) if target.is_file() => EntryKind::RegularFile,
            _ => EntryKind::Other,
        };
        Ok(Some(Entry { kind, link }))
    }

    fn children(&self, path: &Path, limit: usize) -> io::Result<Children> {
        let paths: Vec<_> = std::fs::read_dir(path)?
            .take(limit)
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<_>>()?;
        Ok(Children {
            scanned: paths.len(),
            paths,
        })
    }

    fn read_file(&self, path: &Path, bound: u64) -> io::Result<Vec<u8>> {
        rustx_bounded_file::read_with_limit_io(path, bound)
    }

    fn frontmatter(&self, path: &Path, signal: &CancellationSignal) -> io::Result<Vec<u8>> {
        super::frontmatter::read(path, signal)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn observe(&self, path: &Path) {
        rustx_bounded_file::record_read(path);
    }
}
