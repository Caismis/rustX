//! Confined, fixed-buffer I/O for local package owners. This is not a cache.
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
#[cfg(test)]
use std::io::Write;
use std::io::{Read, Seek};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{Mode, OFlags, open, openat};

use crate::runtime::CancellationSignal;

pub(crate) const CHUNK_BYTES: usize = 64 * 1024;
#[cfg(test)]
thread_local! {
    pub(crate) static OPENED_FILES: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    pub(crate) static DIRECTORY_LISTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(crate) static STREAMED_FILE_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// Every component is resolved relative to an already opened directory.
pub(crate) struct PackageDirectory {
    fd: std::sync::Arc<OwnedFd>,
    ancestors: Vec<(std::sync::Arc<OwnedFd>, std::ffi::OsString)>,
}

impl PackageDirectory {
    pub(crate) fn descriptor(&self) -> &OwnedFd {
        &self.fd
    }
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        let mut directory = Self {
            fd: std::sync::Arc::new(open("/", DIRECTORY, Mode::empty())?),
            ancestors: Vec::new(),
        };
        if !path.is_absolute() {
            return Err(std::io::Error::other("package root must be absolute"));
        }
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => directory = directory.child(name)?,
                _ => return Err(std::io::Error::other("unsafe package root")),
            }
        }
        Ok(directory)
    }

    pub(crate) fn verify(&self) -> std::io::Result<()> {
        for (index, (parent, name)) in self.ancestors.iter().enumerate() {
            let child = self
                .ancestors
                .get(index + 1)
                .map_or(&self.fd, |(descriptor, _)| descriptor);
            let current =
                rustix::fs::statat(parent.as_ref(), name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
            let retained = rustix::fs::fstat(child.as_ref())?;
            if (current.st_dev, current.st_ino) != (retained.st_dev, retained.st_ino) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "authorized directory replaced",
                ));
            }
        }
        Ok(())
    }

    /// Detect a physical descendant, including case aliases on native filesystems.
    pub(crate) fn contains(&self, file: &File) -> std::io::Result<bool> {
        let source = rustix::fs::fstat(file)?;
        for descriptor in
            std::iter::once(&self.fd).chain(self.ancestors.iter().map(|(descriptor, _)| descriptor))
        {
            let current = rustix::fs::fstat(descriptor.as_ref())?;
            if (source.st_dev, source.st_ino) == (current.st_dev, current.st_ino) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn child(&self, name: &std::ffi::OsStr) -> std::io::Result<Self> {
        self.verify()?;
        let fd = openat(self.fd.as_ref(), name, DIRECTORY, Mode::empty())?;
        let mut ancestors = self.ancestors.clone();
        if name != "." {
            ancestors.push((self.fd.clone(), name.to_os_string()));
        }
        let directory = Self {
            fd: std::sync::Arc::new(fd),
            ancestors,
        };
        directory.verify()?;
        Ok(directory)
    }

    pub(crate) fn names(&self, limit: usize) -> std::io::Result<Vec<PathBuf>> {
        #[cfg(test)]
        DIRECTORY_LISTS.with(|count| count.set(count.get() + 1));
        self.verify()?;
        let mut names = Vec::new();
        for entry in rustix::fs::Dir::read_from(self.fd.as_ref())? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if names.len() == limit {
                return Err(std::io::Error::other(format!(
                    "package directory exceeds {limit} entries"
                )));
            }
            names.push(PathBuf::from(std::ffi::OsStr::from_bytes(name)));
        }
        self.verify()?;
        names.sort();
        Ok(names)
    }

    pub(crate) fn file(&self, relative: &Path) -> std::io::Result<File> {
        #[cfg(test)]
        OPENED_FILES.with(|files| files.borrow_mut().push(relative.to_path_buf()));
        self.verify()?;
        let mut parts = relative.components().peekable();
        let mut directory = None;
        while let Some(part) = parts.next() {
            let Component::Normal(name) = part else {
                return Err(std::io::Error::other("unsafe package-relative path"));
            };
            let parent = directory.as_ref().unwrap_or(self.fd.as_ref());
            if parts.peek().is_none() {
                let file = File::from(openat(
                    parent,
                    name,
                    OFlags::RDONLY
                        | OFlags::NOFOLLOW
                        | OFlags::NONBLOCK
                        | OFlags::CLOEXEC
                        | OFlags::NOCTTY,
                    Mode::empty(),
                )?);
                if !file.metadata()?.is_file() {
                    return Err(std::io::Error::other(
                        "package entry must be a regular file",
                    ));
                }
                return Ok(file);
            }
            directory = Some(openat(parent, name, DIRECTORY, Mode::empty())?);
        }
        Err(std::io::Error::other("empty package-relative path"))
    }
}

fn stamp(meta: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

pub(crate) fn check_cancelled(signal: &CancellationSignal) -> std::io::Result<()> {
    if signal.is_cancelled() {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "package I/O cancelled",
        ))
    } else {
        Ok(())
    }
}

/// Evidence over two complete matching reads. Metadata is additional mutation
/// evidence, never a substitute for content verification on a coarse clock.
#[derive(Debug)]
pub(crate) struct FileEvidence {
    pub(crate) metadata: Metadata,
    pub(crate) digest: [u8; 32],
}

/// Read the opened regular file, then verify the complete content again through
/// that descriptor. Callers consume chunks once; growth/truncation/replacement
/// and changed content fail closed. This does not claim an atomic snapshot of a
/// mutable directory. Publication verifies the actual copied package separately.
pub(crate) fn stream(
    directory: &PackageDirectory,
    relative: &Path,
    signal: &CancellationSignal,
    chunk: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<FileEvidence> {
    check_cancelled(signal)?;
    let mut file = directory.file(relative)?;
    let before = file.metadata()?;
    let mut buffer = vec![0; CHUNK_BYTES];
    let digest = read_pass(&mut file, before.len(), signal, &mut buffer, chunk)?;
    if stamp(&before) != stamp(&file.metadata()?)
        || stamp(&before) != stamp(&directory.file(relative)?.metadata()?)
    {
        return Err(std::io::Error::other(
            "package file changed during inspection",
        ));
    }
    file.rewind()?;
    let verified = read_pass(&mut file, before.len(), signal, &mut buffer, |_| Ok(()))?;
    if digest != verified
        || stamp(&before) != stamp(&file.metadata()?)
        || stamp(&before) != stamp(&directory.file(relative)?.metadata()?)
    {
        return Err(std::io::Error::other(
            "package file content changed during verification",
        ));
    }
    check_cancelled(signal)?;
    Ok(FileEvidence {
        metadata: before,
        digest,
    })
}

fn read_pass(
    file: &mut File,
    length: u64,
    signal: &CancellationSignal,
    buffer: &mut [u8],
    mut chunk: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<[u8; 32]> {
    let mut consumed = 0_u64;
    let mut hash = Sha256::new();
    loop {
        check_cancelled(signal)?;
        let read = file.read(buffer)?;
        if read == 0 {
            break;
        }
        #[cfg(test)]
        STREAMED_FILE_BYTES.with(|count| count.set(count.get() + read as u64));
        consumed = consumed
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::other("package file length overflow"))?;
        if consumed > length {
            return Err(std::io::Error::other("package file grew during inspection"));
        }
        hash.update(&buffer[..read]);
        chunk(&buffer[..read])?;
    }
    if consumed != length {
        return Err(std::io::Error::other(
            "package file truncated during inspection",
        ));
    }
    Ok(hash.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opened_files_reject_growth_truncation_replacement_and_same_size_edits() {
        for mutation in ["grow", "truncate", "replace", "overwrite"] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let path = root.join("asset");
            std::fs::write(&path, vec![b'x'; CHUNK_BYTES * 2]).unwrap();
            let directory = PackageDirectory::open(&root).unwrap();
            let mut changed = false;
            let result = stream(
                &directory,
                Path::new("asset"),
                &CancellationSignal::new(),
                |_| {
                    if !changed {
                        changed = true;
                        match mutation {
                            "grow" => std::fs::OpenOptions::new()
                                .append(true)
                                .open(&path)?
                                .write_all(b"x")?,
                            "truncate" => File::options().write(true).open(&path)?.set_len(1)?,
                            "replace" => {
                                std::fs::rename(&path, root.join("old"))?;
                                std::fs::write(&path, vec![b'y'; CHUNK_BYTES * 2])?;
                            }
                            _ => {
                                File::options().write(true).open(&path)?.write_all(b"y")?;
                            }
                        }
                    }
                    Ok(())
                },
            );
            assert!(
                result.is_err(),
                "{mutation} cannot certify an unchanged read"
            );
        }
    }

    #[test]
    fn cancellation_settles_between_chunks_and_io_failures_propagate() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::write(root.join("asset"), vec![0; CHUNK_BYTES * 3]).unwrap();
        let directory = PackageDirectory::open(&root).unwrap();
        let cancellation = CancellationSignal::new();
        let mut read = 0;
        assert!(
            stream(&directory, Path::new("asset"), &cancellation, |bytes| {
                read += bytes.len();
                cancellation.cancel();
                Ok(())
            })
            .unwrap_err()
            .to_string()
            .contains("cancelled")
        );
        assert_eq!(read, CHUNK_BYTES);
        assert!(
            stream(
                &directory,
                Path::new("asset"),
                &CancellationSignal::new(),
                |_| { Err(std::io::Error::other("injected write failure")) }
            )
            .unwrap_err()
            .to_string()
            .contains("injected write failure")
        );
        #[cfg(target_os = "linux")]
        {
            let mut full = File::options().write(true).open("/dev/full").unwrap();
            let error = stream(
                &directory,
                Path::new("asset"),
                &CancellationSignal::new(),
                |bytes| full.write_all(bytes),
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(nix::libc::ENOSPC));
        }
    }

    #[test]
    fn no_symlink_fifo_or_parent_component_can_escape_an_opened_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::write(root.join("plain"), b"private").unwrap();
        std::os::unix::fs::symlink("plain", root.join("link")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("directory-link")).unwrap();
        nix::unistd::mkfifo(&root.join("fifo"), nix::sys::stat::Mode::S_IRUSR).unwrap();
        let directory = PackageDirectory::open(&root).unwrap();
        for relative in [
            "link",
            "fifo",
            "directory-link/plain",
            "../plain",
            "/plain",
            "",
        ] {
            assert!(directory.file(Path::new(relative)).is_err(), "{relative}");
        }
    }
}
