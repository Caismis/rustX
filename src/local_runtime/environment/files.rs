//! Descriptor-relative, bounded filesystem operations under native root grants.
use super::{Environment, Root};
use crate::package_io::PackageDirectory;
use base64::Engine as _;
use rustix::fs::{AtFlags, Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

use std::path::{Component, Path};

pub const CHUNK_BYTES: usize = 512 * 1024;
const DIRECTORY_ENTRIES: usize = 4096;
const TREE_ENTRIES: usize = 4096;
const COPY_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy)]
struct FileAuthority<'a> {
    destination: &'a Root,
    source: Option<&'a Root>,
}
impl FileAuthority<'_> {
    fn check(self) -> io::Result<()> {
        if self.destination.cancelled.is_cancelled()
            || self
                .source
                .is_some_and(|root| root.cancelled.is_cancelled())
        {
            Err(denied())
        } else {
            Ok(())
        }
    }
    fn commit<T>(self, commit: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        let publish = || {
            self.destination
                .cancelled
                .with_active(commit)
                .ok_or_else(denied)?
        };
        if let Some(source) = self.source
            && !std::ptr::eq(source, self.destination)
        {
            source.cancelled.with_active(publish).ok_or_else(denied)?
        } else {
            publish()
        }
    }
}

pub(crate) fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "environment root unavailable or unauthorized",
    )
}
pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
#[derive(Debug)]
pub(crate) struct CommitUnknown;
impl std::fmt::Display for CommitUnknown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("filesystem commit or partial outcome requires authoritative reread")
    }
}
impl std::error::Error for CommitUnknown {}
fn uncertain(_: io::Error) -> io::Error {
    io::Error::other(CommitUnknown)
}

fn conflict() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "filesystem revision conflict")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileMetadata {
    pub is_directory: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub revision: String,
    pub identity: String,
    pub modified_at_ms: Option<i64>,
}
impl FileMetadata {
    fn of(file: &File) -> io::Result<Self> {
        Self::stat(&nix::sys::stat::fstat(file)?)
    }
    #[allow(clippy::useless_conversion)] // Unix libc time aliases differ by target.
    fn stat(metadata: &nix::sys::stat::FileStat) -> io::Result<Self> {
        let stamp = format!(
            "{}:{}:{}:{}:{}:{}:{}:{}",
            metadata.st_dev,
            metadata.st_ino,
            metadata.st_size,
            metadata.st_mtime,
            metadata.st_mtime_nsec,
            metadata.st_ctime,
            metadata.st_ctime_nsec,
            metadata.st_mode
        );
        let kind = metadata.st_mode & libc::S_IFMT;
        Ok(Self {
            is_directory: kind == libc::S_IFDIR,
            is_file: kind == libc::S_IFREG,
            is_symlink: kind == libc::S_IFLNK,
            size: u64::try_from(metadata.st_size).map_err(|_| invalid("negative file size"))?,
            revision: format!("{:x}", Sha256::digest(stamp.as_bytes())),
            identity: format!("{}:{}", metadata.st_dev, metadata.st_ino),
            modified_at_ms: i64::try_from(metadata.st_mtime)
                .ok()
                .and_then(|seconds| seconds.checked_mul(1000))
                .and_then(|value| {
                    i64::try_from(metadata.st_mtime_nsec)
                        .ok()
                        .and_then(|nanos| value.checked_add(nanos / 1_000_000))
                }),
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChunk {
    pub data_base64: String,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub metadata: FileMetadata,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryEntry {
    pub file_name: String,
    pub metadata: FileMetadata,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryPage {
    pub entries: Vec<DirectoryEntry>,
    pub next_offset: Option<usize>,
    pub revision: String,
}

pub(super) fn directory(root: &PackageDirectory, relative: &Path) -> io::Result<PackageDirectory> {
    let mut current = None;
    for part in relative.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(name) => {
                current = Some(current.as_ref().unwrap_or(root).child(name)?);
            }
            _ => return Err(denied()),
        }
    }
    current.map_or_else(|| root.child(std::ffi::OsStr::new(".")), Ok)
}
fn parent(root: &Root, relative: &Path) -> io::Result<(PackageDirectory, std::ffi::OsString)> {
    let leaf = relative
        .file_name()
        .ok_or_else(|| invalid("operation requires a leaf path"))?;
    let directory = directory(&root.directory, relative.parent().unwrap_or(Path::new("")))?;
    Ok((directory, leaf.to_os_string()))
}
fn open(directory: &PackageDirectory, leaf: &std::ffi::OsStr) -> io::Result<File> {
    directory.verify()?;
    Ok(File::from(rustix::fs::openat(
        directory.descriptor(),
        leaf,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
    )?))
}
pub(crate) fn metadata(root: &Root, relative: &Path) -> io::Result<FileMetadata> {
    if root.cancelled.is_cancelled() {
        return Err(denied());
    }
    if relative.as_os_str().is_empty() {
        return FileMetadata::of(&File::from(root.directory.descriptor().try_clone()?));
    }
    let (directory, leaf) = parent(root, relative)?;
    directory.verify()?;
    FileMetadata::stat(&nix::sys::stat::fstatat(
        directory.descriptor(),
        leaf.as_os_str(),
        nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
    )?)
}

/// Shared descriptor-relative range reader for native file and Skill sources.
/// It retains only the requested bytes and verifies descriptor/path identity.
pub(crate) fn read_range(
    directory: &PackageDirectory,
    relative: &Path,
    offset: u64,
    max_bytes: usize,
    expected: Option<&str>,
) -> io::Result<FileChunk> {
    if !(1..=CHUNK_BYTES).contains(&max_bytes) {
        return Err(invalid("file read exceeds chunk limit"));
    }
    let mut file = directory.file(relative)?;
    let before = FileMetadata::of(&file)?;
    if offset > before.size {
        return Err(invalid("file offset exceeds source"));
    }
    if expected.is_some_and(|expected| expected != before.revision) {
        return Err(conflict());
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    (&mut file).take(max_bytes as u64).read_to_end(&mut bytes)?;
    if before != FileMetadata::of(&file)? || before != FileMetadata::of(&directory.file(relative)?)?
    {
        return Err(conflict());
    }
    let end = offset
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| invalid("file offset overflow"))?;
    Ok(FileChunk {
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        offset,
        next_offset: (end < before.size).then_some(end),
        metadata: before,
    })
}

impl Environment {
    pub(crate) fn metadata(&self, path: &str) -> io::Result<FileMetadata> {
        self.with_paths(&[path], |paths| metadata(&paths[0].1, &paths[0].2))
    }
    pub(crate) fn read(
        &self,
        path: &str,
        offset: u64,
        max_bytes: usize,
        expected: Option<&str>,
    ) -> io::Result<FileChunk> {
        if !(1..=CHUNK_BYTES).contains(&max_bytes) {
            return Err(invalid("file read exceeds chunk limit"));
        }
        self.with_paths(&[path], |paths| {
            let (_, root, relative) = &paths[0];
            let (directory, leaf) = parent(root, relative)?;
            let chunk = read_range(&directory, Path::new(&leaf), offset, max_bytes, expected)?;
            if root.cancelled.is_cancelled() || chunk.metadata != metadata(root, relative)? {
                return Err(conflict());
            }
            Ok(chunk)
        })
    }
    pub(crate) fn write(
        &self,
        path: &str,
        bytes: &[u8],
        expected: Option<&str>,
    ) -> io::Result<FileMetadata> {
        if bytes.len() > CHUNK_BYTES {
            return Err(invalid(
                "file write exceeds chunk limit; use native upload transfer for larger files",
            ));
        }
        self.with_paths(&[path], |paths| {
            let (_, root, relative) = &paths[0];
            let (directory, leaf) = parent(root, relative)?;
            atomic_write(
                &directory,
                &leaf,
                expected,
                FileAuthority {
                    destination: root,
                    source: None,
                },
                |output| output.write_all(bytes),
            )?;
            metadata(root, relative).map_err(uncertain)
        })
    }
    pub(crate) fn list(
        &self,
        path: &str,
        offset: usize,
        limit: usize,
        expected: Option<&str>,
    ) -> io::Result<DirectoryPage> {
        if !(1..=128).contains(&limit) || offset > DIRECTORY_ENTRIES {
            return Err(invalid("invalid directory page"));
        }
        self.with_paths(&[path], |paths| {
            let (_, root, relative) = &paths[0];
            let before = metadata(root, relative)?;
            if expected.is_some_and(|expected| expected != before.revision) {
                return Err(conflict());
            }
            let directory = directory(&root.directory, relative)?;
            let names = directory.names(DIRECTORY_ENTRIES)?;
            let mut entries = Vec::new();
            for name in names.iter().skip(offset).take(limit) {
                let file_name = name
                    .to_str()
                    .ok_or_else(|| invalid("non-UTF-8 filename"))?
                    .to_owned();
                let child = nix::sys::stat::fstatat(
                    directory.descriptor(),
                    name.as_os_str(),
                    nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
                )?;
                entries.push(DirectoryEntry {
                    file_name,
                    metadata: FileMetadata::stat(&child)?,
                });
            }
            if before != metadata(root, relative)? {
                return Err(conflict());
            }
            Ok(DirectoryPage {
                next_offset: (offset + entries.len() < names.len())
                    .then_some(offset + entries.len()),
                entries,
                revision: before.revision,
            })
        })
    }
    pub(crate) fn create_directory(&self, path: &str, recursive: bool) -> io::Result<()> {
        self.with_paths(&[path], |paths| {
            let (_, root, relative) = &paths[0];
            if !recursive {
                let (parent, leaf) = parent(root, relative)?;
                FileAuthority {
                    destination: root,
                    source: None,
                }
                .commit(|| {
                    parent.verify()?;
                    rustix::fs::mkdirat(parent.descriptor(), &leaf, Mode::from_raw_mode(0o700))?;
                    Ok(())
                })?;
                return Ok(());
            }
            (|| {
                let mut current = root.directory.child(std::ffi::OsStr::new("."))?;
                for part in relative.components() {
                    let Component::Normal(name) = part else {
                        return Err(denied());
                    };
                    match (FileAuthority {
                        destination: root,
                        source: None,
                    })
                    .commit(|| {
                        current.verify()?;
                        rustix::fs::mkdirat(current.descriptor(), name, Mode::from_raw_mode(0o700))
                            .map_err(Into::into)
                    }) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    current = current.child(name)?;
                }
                Ok(())
            })()
            .map_err(uncertain)
        })
    }
    pub(crate) fn remove(&self, path: &str, recursive: bool, force: bool) -> io::Result<()> {
        self.with_paths(&[path], |paths| {
            let (_, root, relative) = &paths[0];
            let (directory, leaf) = parent(root, relative)?;
            let mut budget = TREE_ENTRIES;
            remove(&directory, &leaf, recursive, 0, &mut budget, force, root).map_err(uncertain)
        })
    }
    pub(crate) fn copy(&self, source: &str, destination: &str, recursive: bool) -> io::Result<()> {
        self.with_paths(&[source, destination], |paths| {
            let (_, source_root, source_relative) = &paths[0];
            let (_, destination_root, destination_relative) = &paths[1];
            if paths[1].0.starts_with(&paths[0].0) {
                return Err(invalid("copy destination must not be inside source"));
            }
            let (source_parent, source_leaf) = if source_relative.as_os_str().is_empty() {
                (
                    source_root.directory.child(std::ffi::OsStr::new("."))?,
                    std::ffi::OsString::from("."),
                )
            } else {
                parent(source_root, source_relative)?
            };
            let (destination_parent, destination_leaf) =
                parent(destination_root, destination_relative)?;
            copy(
                &source_parent,
                &source_leaf,
                &destination_parent,
                &destination_leaf,
                recursive,
                0,
                &mut (TREE_ENTRIES, COPY_BYTES),
                FileAuthority {
                    destination: destination_root,
                    source: Some(source_root),
                },
            )
            .map_err(uncertain)
        })
    }
}

fn atomic_write(
    directory: &PackageDirectory,
    leaf: &std::ffi::OsStr,
    expected: Option<&str>,
    authority: FileAuthority<'_>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let observed = match open(directory, leaf) {
        Ok(file) => Some(FileMetadata::of(&file)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if observed.as_ref().map(|value| value.revision.as_str()) != expected {
        return Err(conflict());
    }
    if observed.as_ref().is_some_and(|value| !value.is_file) {
        return Err(invalid("write target must be a regular file"));
    }
    let temporary = format!(".rustx-write-{}", uuid::Uuid::now_v7());
    let mut output = File::from(rustix::fs::openat(
        directory.descriptor(),
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?);
    let mut committed = false;
    let result = (|| {
        if observed.is_some() {
            output.set_permissions(open(directory, leaf)?.metadata()?.permissions())?;
        }
        write(&mut output)?;
        output.sync_all()?;
        authority.commit(|| {
            directory.verify()?;
            let current = match open(directory, leaf) {
                Ok(file) => Some(FileMetadata::of(&file)?),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            if observed != current {
                return Err(conflict());
            }
            if expected.is_none() {
                // A create never overwrites a concurrently created OS file.
                rustix::fs::linkat(
                    directory.descriptor(),
                    temporary.as_str(),
                    directory.descriptor(),
                    leaf,
                    AtFlags::empty(),
                )?;
                committed = true;
                rustix::fs::unlinkat(directory.descriptor(), temporary.as_str(), AtFlags::empty())?;
            } else {
                rustix::fs::renameat(
                    directory.descriptor(),
                    temporary.as_str(),
                    directory.descriptor(),
                    leaf,
                )?;
                committed = true;
            }
            Ok(())
        })?;
        sync_directory(directory)
    })();
    let _ = rustix::fs::unlinkat(directory.descriptor(), temporary.as_str(), AtFlags::empty());
    if committed {
        result.map_err(uncertain)
    } else {
        result
    }
}
#[cfg(test)]
thread_local! {static FAIL_DIRECTORY_SYNC:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
fn sync_directory(directory: &PackageDirectory) -> io::Result<()> {
    #[cfg(test)]
    if FAIL_DIRECTORY_SYNC.replace(false) {
        return Err(io::Error::other("injected directory sync failure"));
    }
    File::from(directory.descriptor().try_clone()?).sync_all()
}

fn take_entry(depth: usize, remaining: &mut usize) -> io::Result<()> {
    if depth > 32 || *remaining == 0 {
        return Err(invalid(
            "filesystem tree operation exceeds budget; operation may be partial",
        ));
    }
    *remaining -= 1;
    Ok(())
}
fn remove(
    parent: &PackageDirectory,
    leaf: &std::ffi::OsStr,
    recursive: bool,
    depth: usize,
    remaining: &mut usize,
    force: bool,
    authority: &Root,
) -> io::Result<()> {
    FileAuthority {
        destination: authority,
        source: None,
    }
    .check()?;
    take_entry(depth, remaining)?;
    parent.verify()?;
    let before = match nix::sys::stat::fstatat(
        parent.descriptor(),
        leaf,
        nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
    ) {
        Ok(stat) => FileMetadata::stat(&stat)?,
        Err(nix::errno::Errno::ENOENT) if force => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if before.is_directory {
        let directory = parent.child(leaf)?;
        if before.identity
            != FileMetadata::of(&File::from(directory.descriptor().try_clone()?))?.identity
        {
            return Err(conflict());
        }
        if recursive {
            for child in directory.names(DIRECTORY_ENTRIES)? {
                remove(
                    &directory,
                    child.as_os_str(),
                    true,
                    depth + 1,
                    remaining,
                    force,
                    authority,
                )?;
            }
        }
        parent.verify()?;
        let now = FileMetadata::stat(&nix::sys::stat::fstatat(
            parent.descriptor(),
            leaf,
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )?)?;
        if before.identity != now.identity {
            return Err(conflict());
        }
        FileAuthority {
            destination: authority,
            source: None,
        }
        .commit(|| {
            parent.verify()?;
            rustix::fs::unlinkat(parent.descriptor(), leaf, AtFlags::REMOVEDIR)?;
            Ok(())
        })?;
    } else if before.is_file || before.is_symlink {
        parent.verify()?;
        if before
            != FileMetadata::stat(&nix::sys::stat::fstatat(
                parent.descriptor(),
                leaf,
                nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
            )?)?
        {
            return Err(conflict());
        }
        FileAuthority {
            destination: authority,
            source: None,
        }
        .commit(|| {
            parent.verify()?;
            rustix::fs::unlinkat(parent.descriptor(), leaf, AtFlags::empty())?;
            Ok(())
        })?;
    } else {
        return Err(invalid(
            "remove requires a regular file, symlink or directory",
        ));
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Exact source and destination descriptor pairs travel together.
fn copy(
    source: &PackageDirectory,
    source_leaf: &std::ffi::OsStr,
    destination: &PackageDirectory,
    destination_leaf: &std::ffi::OsStr,
    recursive: bool,
    depth: usize,
    budget: &mut (usize, u64),
    authority: FileAuthority<'_>,
) -> io::Result<()> {
    authority.check()?;
    take_entry(depth, &mut budget.0)?;
    let mut input = open(source, source_leaf)?;
    let before = FileMetadata::of(&input)?;
    if before.is_directory {
        if !recursive {
            return Err(invalid("directory copy requires recursive"));
        }
        if destination.contains(&input)? {
            return Err(invalid("copy destination is physically inside source"));
        }
        authority.commit(|| {
            destination.verify()?;
            source.verify()?;
            rustix::fs::mkdirat(
                destination.descriptor(),
                destination_leaf,
                Mode::from_raw_mode(0o700),
            )?;
            Ok(())
        })?;
        let source = source.child(source_leaf)?;
        if before.identity
            != FileMetadata::of(&File::from(source.descriptor().try_clone()?))?.identity
        {
            return Err(conflict());
        }
        let destination = destination.child(destination_leaf)?;
        for child in source.names(DIRECTORY_ENTRIES)? {
            copy(
                &source,
                child.as_os_str(),
                &destination,
                child.as_os_str(),
                true,
                depth + 1,
                budget,
                authority,
            )?;
        }
        return Ok(());
    }
    if !before.is_file || before.size > budget.1 {
        return Err(invalid(
            "copy exceeds byte budget or unsupported entry; operation may be partial",
        ));
    }
    budget.1 -= before.size;
    atomic_write(destination, destination_leaf, None, authority, |output| {
        output.set_permissions(input.metadata()?.permissions())?;
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            authority.check()?;
            source.verify()?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            copied = copied
                .checked_add(count as u64)
                .ok_or_else(|| invalid("copy size overflow"))?;
            if copied > before.size {
                return Err(conflict());
            }
            output.write_all(&buffer[..count])?;
        }
        if copied != before.size
            || before != FileMetadata::of(&input)?
            || before != FileMetadata::of(&open(source, source_leaf)?)?
        {
            return Err(conflict());
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn authority(path: &str) -> Root {
        Root {
            directory: PackageDirectory::open(Path::new(path)).unwrap(),
            identity: (0, 0),
            cancelled: crate::runtime::CancellationSignal::new(),
        }
    }
    fn fixture() -> (tempfile::TempDir, Environment, String) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let environment = Environment::default();
        environment.configure(std::slice::from_ref(&root));
        (temporary, environment, root.display().to_string())
    }
    #[test]
    fn atomic_faults_conflicts_and_permissions_preserve_the_current_file() {
        let (_temporary, environment, root) = fixture();
        let path = format!("{root}/file");
        let metadata = environment.write(&path, b"original", None).unwrap();
        let directory = PackageDirectory::open(Path::new(&root)).unwrap();
        let revision = Some(metadata.revision.as_str());
        let root_authority = authority(&root);
        assert!(
            atomic_write(
                &directory,
                std::ffi::OsStr::new("file"),
                revision,
                FileAuthority {
                    destination: &root_authority,
                    source: None
                },
                |file| {
                    file.write_all(b"partial")?;
                    Err(io::Error::other("injected disk failure"))
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(
            atomic_write(
                &directory,
                std::ffi::OsStr::new("file"),
                revision,
                FileAuthority {
                    destination: &root_authority,
                    source: None
                },
                |file| {
                    file.write_all(b"stale")?;
                    std::fs::write(&path, b"external")
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        assert_eq!(
            directory.names(16).unwrap(),
            vec![std::path::PathBuf::from("file")]
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        let revision = environment.metadata(&path).unwrap().revision;
        environment
            .write(&path, b"replacement", Some(&revision))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }
    #[test]
    fn parent_replacement_before_commit_cannot_redirect_an_atomic_write() {
        let (temporary, environment, root) = fixture();
        environment
            .create_directory(&format!("{root}/parent"), false)
            .unwrap();
        environment
            .write(&format!("{root}/parent/file"), b"original", None)
            .unwrap();
        let directory = PackageDirectory::open(&Path::new(&root).join("parent")).unwrap();
        let revision = environment
            .metadata(&format!("{root}/parent/file"))
            .unwrap()
            .revision;
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("file"), b"private").unwrap();
        let root_authority = authority(&root);
        let result = atomic_write(
            &directory,
            std::ffi::OsStr::new("file"),
            Some(&revision),
            FileAuthority {
                destination: &root_authority,
                source: None,
            },
            |file| {
                file.write_all(b"new")?;
                std::fs::rename(
                    temporary.path().join("parent"),
                    temporary.path().join("retired"),
                )?;
                std::os::unix::fs::symlink(outside.path(), temporary.path().join("parent"))
            },
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(outside.path().join("file")).unwrap(),
            b"private"
        );
        assert_eq!(
            std::fs::read(temporary.path().join("retired/file")).unwrap(),
            b"original"
        );
    }
    #[test]
    fn root_revocation_during_staging_prevents_commit_without_waiting_on_file_io() {
        let (_temporary, environment, root) = fixture();
        let path = format!("{root}/file");
        let revision = environment
            .write(&path, b"original", None)
            .unwrap()
            .revision;
        let result = environment.with_paths(&[&path], |paths| {
            let (_, root, relative) = &paths[0];
            let (directory, leaf) = parent(root, relative)?;
            atomic_write(
                &directory,
                &leaf,
                Some(&revision),
                FileAuthority {
                    destination: root,
                    source: None,
                },
                |file| {
                    file.write_all(b"replacement")?;
                    environment.configure(&[]);
                    Ok(())
                },
            )
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
    #[test]
    fn failure_after_publication_reports_unknown_outcome_and_keeps_committed_bytes() {
        let (_temporary, environment, root) = fixture();
        let path = format!("{root}/file");
        let revision = environment
            .write(&path, b"original", None)
            .unwrap()
            .revision;
        FAIL_DIRECTORY_SYNC.set(true);
        let error = environment
            .write(&path, b"committed", Some(&revision))
            .unwrap_err();
        assert!(error.get_ref().unwrap().is::<CommitUnknown>());
        assert_eq!(std::fs::read(path).unwrap(), b"committed");
    }

    #[test]
    fn root_replacement_requires_a_new_native_grant() {
        let (temporary, environment, root) = fixture();
        environment
            .write(&format!("{root}/file"), b"original", None)
            .unwrap();
        let retired = temporary.path().with_extension("retired");
        std::fs::rename(temporary.path(), &retired).unwrap();
        std::fs::create_dir(temporary.path()).unwrap();
        std::fs::write(temporary.path().join("file"), b"replacement").unwrap();
        assert!(
            environment
                .read(&format!("{root}/file"), 0, 32, None)
                .is_err()
        );
        environment.configure(&[]);
        environment.configure(&[Path::new(&root).to_path_buf()]);
        assert!(
            environment
                .read(&format!("{root}/file"), 0, 32, None)
                .is_ok()
        );
        std::fs::remove_dir_all(retired).unwrap();
    }
    #[test]
    fn metadata_listing_and_remove_never_follow_symbolic_links() {
        let (temporary, environment, root) = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private"), b"private").unwrap();
        std::os::unix::fs::symlink(outside.path(), temporary.path().join("link")).unwrap();
        assert!(
            environment
                .metadata(&format!("{root}/link"))
                .unwrap()
                .is_symlink
        );
        assert!(
            environment.list(&root, 0, 16, None).unwrap().entries[0]
                .metadata
                .is_symlink
        );
        assert!(
            environment
                .read(&format!("{root}/link/private"), 0, 16, None)
                .is_err()
        );
        environment
            .remove(&format!("{root}/link"), true, false)
            .unwrap();
        assert_eq!(
            std::fs::read(outside.path().join("private")).unwrap(),
            b"private"
        );
        environment
            .remove(&format!("{root}/missing"), true, true)
            .unwrap();
    }
}
