//! Descriptor-relative, bounded filesystem operations under native root grants.
use super::{Environment, Root};
use crate::package_io::PackageDirectory;
use rustix::fs::{AtFlags, Mode, OFlags};
use std::fs::File;
use std::io::{self, Read, Write};
#[cfg(test)]
use std::os::unix::fs::MetadataExt;

use std::path::{Component, Path};

// The physical metadata/chunk contract and its stamping live in
// `rustx_confined_fs`, shared with the Execution Client; they are re-exported
// so there is exactly one business type on each side of the workspace.
pub use rustx_confined_fs::{DirectoryEntry, DirectoryPage, FileChunk, FileMetadata};

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

pub(super) fn directory(root: &PackageDirectory, relative: &Path) -> io::Result<PackageDirectory> {
    root.descend(relative)
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
/// The physical stat lives in `rustx_confined_fs` — no-follow descent,
/// leaf `fstatat(AT_SYMLINK_NOFOLLOW)`, chain verify before and after.
/// The root grant's cancellation is the only product-level check here.
pub(crate) fn metadata(root: &Root, relative: &Path) -> io::Result<FileMetadata> {
    if root.cancelled.is_cancelled() {
        return Err(denied());
    }
    root.directory.entry_metadata(relative)
}

/// Retain the exact native watch source instead of resolving its pathname again
/// inside an OS watcher. Both files and directories use no-follow traversal.
pub(crate) fn watch_source(root: &Root, relative: &Path) -> io::Result<(File, FileMetadata)> {
    let before = metadata(root, relative)?;
    if !before.is_file && !before.is_directory {
        return Err(denied());
    }
    let file = if relative.as_os_str().is_empty() {
        File::from(root.directory.descriptor().try_clone()?)
    } else {
        let (directory, leaf) = parent(root, relative)?;
        let file = open(&directory, &leaf)?;
        directory.verify()?;
        file
    };
    let retained = FileMetadata::of(&file)?;
    if (!retained.is_file && !retained.is_directory)
        || before.identity != retained.identity
        || metadata(root, relative)?.identity != retained.identity
    {
        return Err(denied());
    }
    Ok((file, retained))
}

/// Shared descriptor-relative range reader for native file and Skill sources.
/// It retains only the requested bytes and verifies descriptor/path identity.
/// The physical algorithm lives in `rustx_confined_fs`; the opener is the
/// root's `PackageDirectory::file` so both underlying opens stay instrumented.
pub(crate) fn read_range(
    directory: &PackageDirectory,
    relative: &Path,
    offset: u64,
    max_bytes: usize,
    expected: Option<&str>,
) -> io::Result<FileChunk> {
    rustx_confined_fs::read_range(
        relative,
        offset,
        max_bytes,
        CHUNK_BYTES,
        expected,
        |relative| directory.file(relative),
    )
}

impl Environment {
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
        #[cfg(test)]
        committed_write(directory, leaf);
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
type WriteCommitHook = Box<dyn FnOnce() + Send>;
#[cfg(test)]
type WriteCommitKey = (u64, u64, std::ffi::OsString);
#[cfg(test)]
static WRITE_COMMITS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<WriteCommitKey, WriteCommitHook>>,
> = std::sync::LazyLock::new(std::sync::Mutex::default);
#[cfg(test)]
pub(crate) fn write_commit_hook(path: &Path, hook: WriteCommitHook) {
    let metadata = std::fs::metadata(path.parent().unwrap()).unwrap();
    WRITE_COMMITS.lock().unwrap().insert(
        (
            metadata.dev(),
            metadata.ino(),
            path.file_name().unwrap().to_owned(),
        ),
        hook,
    );
}
#[cfg(test)]
fn committed_write(directory: &PackageDirectory, leaf: &std::ffi::OsStr) {
    let metadata = File::from(directory.descriptor().try_clone().unwrap())
        .metadata()
        .unwrap();
    let hook = WRITE_COMMITS
        .lock()
        .unwrap()
        .remove(&(metadata.dev(), metadata.ino(), leaf.into()));
    if let Some(hook) = hook {
        hook();
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
        let written = environment.write(&path, b"original", None).unwrap();
        let directory = PackageDirectory::open(Path::new(&root)).unwrap();
        let revision = Some(written.revision.as_str());
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
        let revision = environment
            .with_paths(&[&path], |paths| metadata(&paths[0].1, &paths[0].2))
            .unwrap()
            .revision;
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
            .with_paths(&[format!("{root}/parent/file").as_str()], |paths| {
                metadata(&paths[0].1, &paths[0].2)
            })
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
                .with_paths(&[format!("{root}/file").as_str()], |paths| metadata(
                    &paths[0].1,
                    &paths[0].2
                ))
                .is_err()
        );
        environment.configure(&[]);
        environment.configure(&[Path::new(&root).to_path_buf()]);
        assert!(
            environment
                .with_paths(&[format!("{root}/file").as_str()], |paths| metadata(
                    &paths[0].1,
                    &paths[0].2
                ))
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
                .with_paths(&[format!("{root}/link").as_str()], |paths| metadata(
                    &paths[0].1,
                    &paths[0].2
                ))
                .unwrap()
                .is_symlink
        );
        assert!(
            environment
                .with_paths(&[root.as_str()], |paths| paths[0]
                    .1
                    .directory
                    .directory_page(&paths[0].2, 0, 16, None, DIRECTORY_ENTRIES, 128))
                .unwrap()
                .entries[0]
                .metadata
                .is_symlink
        );
        assert!(
            environment
                .with_paths(
                    &[format!("{root}/link/private").as_str()],
                    |paths| metadata(&paths[0].1, &paths[0].2)
                )
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
