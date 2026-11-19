//! Confined, descriptor-relative filesystem primitives.
//!
//! This crate factors out the physical operations the rustX Agent Server
//! proved in its local runtime, so the Execution Client performs the exact
//! same filesystem discipline instead of growing a parallel implementation:
//!
//! - [`PackageDirectory`] opens a root and resolves every component relative
//!   to an already opened directory descriptor with `openat(O_NOFOLLOW)`,
//!   retaining ancestor identities so a replaced directory is detected, never
//!   silently followed. No symlink or `..` can escape the opened root.
//! - [`read_range`] is the bounded, revision-checked range reader: it opens
//!   the same path twice through the caller's opener, reads at most
//!   `max_bytes`, and refuses to publish bytes if the file's revision changed
//!   across the read.
//! - [`PackageDirectory::entry_metadata`] stamps one entry without following
//!   a final symlink, and [`PackageDirectory::directory_page`] is the
//!   bounded, revision-checked directory pager.
//! - [`FileMetadata`]/[`FileChunk`]/[`DirectoryEntry`]/[`DirectoryPage`] are
//!   the shared wire shapes; their serde field names and `schemars` schema
//!   are the established contract.
//! - [`WriteTarget`]/[`StagedWrite`]/[`PublishedWrite`] are the shared
//!   physical commit kernel: retain a verified target, stage a private
//!   sibling, publish by descriptor-relative `linkat`/`renameat`, and
//!   report any post-crossing failure as [`CommitUnknown`].
//!
//! There is no Agent, cancellation, store, or process dependency here — only
//! files and directories.

mod mutation;
mod text;

pub use mutation::{CommitUnknown, Expectation, PublishedWrite, StagedWrite, WriteTarget};
pub use text::{PageEnd, PageLimits, TextPage, validate_utf8};

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use rustix::fs::{Mode, OFlags, open, openat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn conflict() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "filesystem revision conflict")
}

/// A directory held open by descriptor, resolved no-follow from a fixed root.
///
/// Every component is resolved relative to an already opened directory.
/// `ancestors` retains each parent descriptor and the name it was opened
/// under, so [`verify`](Self::verify) can prove the chain was not replaced
/// between opens.
pub struct PackageDirectory {
    fd: Arc<OwnedFd>,
    ancestors: Vec<(Arc<OwnedFd>, std::ffi::OsString)>,
}

impl PackageDirectory {
    /// The descriptor of the directory itself, for `*at` calls.
    #[must_use]
    pub fn descriptor(&self) -> &OwnedFd {
        &self.fd
    }

    /// Opens `path` component by component from `/`, refusing every
    /// non-normal component and following no symlink.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] when the path is not absolute, contains an
    /// unsafe component, or a component cannot be opened as a directory.
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut directory = Self {
            fd: Arc::new(open("/", DIRECTORY, Mode::empty())?),
            ancestors: Vec::new(),
        };
        if !path.is_absolute() {
            return Err(io::Error::other("package root must be absolute"));
        }
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => directory = directory.child(name)?,
                _ => return Err(io::Error::other("unsafe package root")),
            }
        }
        Ok(directory)
    }

    /// Re-verifies every retained ancestor identity: the descriptor chain
    /// must still name the same directories it opened.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::PermissionDenied`] when an ancestor was
    /// replaced, or the underlying `stat` error.
    pub fn verify(&self) -> io::Result<()> {
        for (index, (parent, name)) in self.ancestors.iter().enumerate() {
            let child = self
                .ancestors
                .get(index + 1)
                .map_or(&self.fd, |(descriptor, _)| descriptor);
            let current =
                rustix::fs::statat(parent.as_ref(), name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
            let retained = rustix::fs::fstat(child.as_ref())?;
            if (current.st_dev, current.st_ino) != (retained.st_dev, retained.st_ino) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "authorized directory replaced",
                ));
            }
        }
        Ok(())
    }

    /// Detects a physical descendant, including case aliases on native
    /// filesystems.
    ///
    /// # Errors
    ///
    /// Returns the underlying `stat` error.
    pub fn contains(&self, file: &File) -> io::Result<bool> {
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

    /// Opens the child directory `name`, which must be exactly one normal
    /// component — or `.` for the current directory — never a path with
    /// separators, `..`, or an empty name.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for an unsafe name,
    /// [`io::ErrorKind::PermissionDenied`] if the retained chain was
    /// replaced, or the underlying `openat` error.
    pub fn child(&self, name: &OsStr) -> io::Result<Self> {
        let bytes = name.as_bytes();
        if bytes != b"." && (bytes.is_empty() || bytes == b".." || bytes.contains(&b'/')) {
            return Err(invalid("unsafe directory name"));
        }
        self.verify()?;
        let fd = openat(self.fd.as_ref(), name, DIRECTORY, Mode::empty())?;
        let mut ancestors = self.ancestors.clone();
        if name != "." {
            ancestors.push((self.fd.clone(), name.to_os_string()));
        }
        let directory = Self {
            fd: Arc::new(fd),
            ancestors,
        };
        directory.verify()?;
        Ok(directory)
    }

    /// Lists entry names, failing closed at `limit` entries.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] when the chain was replaced, the directory
    /// exceeds `limit` entries, or reading it fails.
    pub fn names(&self, limit: usize) -> io::Result<Vec<PathBuf>> {
        self.verify()?;
        let mut names = Vec::new();
        for entry in rustix::fs::Dir::read_from(self.fd.as_ref())? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if names.len() == limit {
                return Err(io::Error::other(format!(
                    "package directory exceeds {limit} entries"
                )));
            }
            names.push(PathBuf::from(OsStr::from_bytes(name)));
        }
        self.verify()?;
        names.sort();
        Ok(names)
    }

    /// Opens `relative` as a regular file: every component must be a normal
    /// name resolved no-follow under this directory.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] for an unsafe path, a non-regular entry, or
    /// the underlying `openat` error.
    pub fn file(&self, relative: &Path) -> io::Result<File> {
        self.verify()?;
        let mut parts = relative.components().peekable();
        let mut directory = None;
        while let Some(part) = parts.next() {
            let Component::Normal(name) = part else {
                return Err(io::Error::other("unsafe package-relative path"));
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
                    return Err(io::Error::other("package entry must be a regular file"));
                }
                return Ok(file);
            }
            directory = Some(openat(parent, name, DIRECTORY, Mode::empty())?);
        }
        Err(io::Error::other("empty package-relative path"))
    }

    /// Resolves `relative` component by component to a directory handle
    /// inside this root: every component opens no-follow as a directory,
    /// `.` components are skipped, and anything else is denied. An empty
    /// path re-opens this directory itself (`.`).
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::PermissionDenied`] for a `..`, root, or
    /// prefix component, or the underlying `openat`/`verify` error — an
    /// intermediate symlink or non-directory component fails the
    /// no-follow directory open.
    pub fn descend(&self, relative: &Path) -> io::Result<Self> {
        let mut current = None;
        for part in relative.components() {
            match part {
                Component::CurDir => {}
                Component::Normal(name) => {
                    current = Some(current.as_ref().unwrap_or(self).child(name)?);
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "unsafe package-relative path",
                    ));
                }
            }
        }
        current.map_or_else(|| self.child(OsStr::new(".")), Ok)
    }

    /// Resolves `relative` like [`descend`](Self::descend) but stops at
    /// the deepest existing ancestor: the returned names are the
    /// still-missing components in order. Every name that did open was a
    /// no-follow directory — a symlink or non-directory component fails
    /// the walk, never enters `missing`.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`descend`](Self::descend) except that
    /// a missing component collects the remainder instead of failing.
    pub fn descend_deepest(&self, relative: &Path) -> io::Result<(Self, Vec<std::ffi::OsString>)> {
        let mut current: Option<Self> = None;
        let mut missing = Vec::new();
        for part in relative.components() {
            match part {
                Component::CurDir => {}
                Component::Normal(name) => {
                    if !missing.is_empty() {
                        missing.push(name.to_os_string());
                        continue;
                    }
                    match current.as_ref().unwrap_or(self).child(name) {
                        Ok(next) => current = Some(next),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            missing.push(name.to_os_string());
                        }
                        Err(error) => return Err(error),
                    }
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "unsafe package-relative path",
                    ));
                }
            }
        }
        let deepest = current.map_or_else(|| self.child(OsStr::new(".")), Ok)?;
        Ok((deepest, missing))
    }

    /// Splits `relative` into its containing directory and leaf name —
    /// the shape `fstatat`/`renameat`-style calls need.
    fn parent_entry(&self, relative: &Path) -> io::Result<(Self, std::ffi::OsString)> {
        let leaf = relative
            .file_name()
            .ok_or_else(|| invalid("operation requires a leaf path"))?;
        let parent = self.descend(relative.parent().unwrap_or_else(|| Path::new("")))?;
        Ok((parent, leaf.to_os_string()))
    }

    /// Stamps the metadata of `relative` inside this directory. Every
    /// intermediate component resolves no-follow as a directory and the
    /// leaf is `fstatat`'ed with `AT_SYMLINK_NOFOLLOW` — a symlinked leaf
    /// reports `is_symlink` and is never followed. An empty `relative`
    /// stamps this directory itself. The retained ancestor chain is
    /// re-verified immediately before and after the stat.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::PermissionDenied`] for a `..` component
    /// or a replaced ancestor, [`io::ErrorKind::InvalidInput`] when the
    /// path has no leaf, or the underlying `openat`/`fstatat` error.
    pub fn entry_metadata(&self, relative: &Path) -> io::Result<FileMetadata> {
        if relative.as_os_str().is_empty() {
            self.verify()?;
            let metadata = FileMetadata::of(&File::from(self.fd.try_clone()?))?;
            self.verify()?;
            return Ok(metadata);
        }
        let (parent, leaf) = self.parent_entry(relative)?;
        parent.verify()?;
        let metadata = FileMetadata::stat(&nix::sys::stat::fstatat(
            parent.fd.as_ref(),
            leaf.as_os_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )?)?;
        parent.verify()?;
        Ok(metadata)
    }

    /// One bounded, revision-checked page of the directory `relative`
    /// names — an empty path lists this directory itself.
    ///
    /// `limit` must be within `1..=page_cap` and `offset` must not exceed
    /// `name_cap`; the directory is enumerated fail-closed at `name_cap`
    /// entries. A supplied `expected` revision must match the directory's
    /// current revision. Entry metadata is stat'ed without following —
    /// a symlinked member reports `is_symlink`. The directory is stat'ed
    /// before and after the listing; a change in between is
    /// [`io::ErrorKind::AlreadyExists`], never published.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for out-of-range paging
    /// arguments or a non-UTF-8 member name,
    /// [`io::ErrorKind::AlreadyExists`] for a stale `expected` revision
    /// or a directory changed across the listing, and the underlying
    /// traversal/stat error otherwise.
    pub fn directory_page(
        &self,
        relative: &Path,
        offset: usize,
        limit: usize,
        expected: Option<&str>,
        name_cap: usize,
        page_cap: usize,
    ) -> io::Result<DirectoryPage> {
        if !(1..=page_cap).contains(&limit) || offset > name_cap {
            return Err(invalid("invalid directory page"));
        }
        // `before` describes the directory this page is read from — the
        // opened descriptor itself, never a pathname whose target could
        // be swapped between stamping and descent.
        let directory = self.descend(relative)?;
        let before = FileMetadata::of(&File::from(directory.descriptor().try_clone()?))?;
        if expected.is_some_and(|expected| expected != before.revision) {
            return Err(conflict());
        }
        let names = directory.names(name_cap)?;
        let mut entries = Vec::new();
        for name in names.iter().skip(offset).take(limit) {
            let file_name = name
                .to_str()
                .ok_or_else(|| invalid("non-UTF-8 filename"))?
                .to_owned();
            let member = nix::sys::stat::fstatat(
                directory.fd.as_ref(),
                name.as_os_str(),
                nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
            )?;
            entries.push(DirectoryEntry {
                file_name,
                metadata: FileMetadata::stat(&member)?,
            });
        }
        // Before release the retained chain must still resolve, the
        // opened directory itself must be unchanged on the same
        // descriptor, and the pathname must still resolve to that same
        // directory — otherwise the delivered entries and the stamped
        // revision could describe different things (ABA swap).
        directory.verify()?;
        if before != FileMetadata::of(&File::from(directory.descriptor().try_clone()?))?
            || before != self.entry_metadata(relative)?
        {
            return Err(conflict());
        }
        Ok(DirectoryPage {
            next_offset: (offset + entries.len() < names.len()).then_some(offset + entries.len()),
            entries,
            revision: before.revision,
        })
    }
}

// The metadata stamp and content revision of one filesystem entry. The wire
// shape and schema are the established rustX environment contract — the
// schema must not drift, so neither the type nor its fields carry doc
// comments: `revision` is a content-stable stamp consumers compare
// literally, and `identity` is the inode identity.
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
    /// Stamps the metadata of an opened file.
    ///
    /// # Errors
    ///
    /// Returns the underlying `fstat` error or an invalid negative size.
    pub fn of(file: &File) -> io::Result<Self> {
        Self::stat(&nix::sys::stat::fstat(file)?)
    }

    /// Stamps the metadata of one `fstat`/`fstatat` result.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for a negative file size.
    #[allow(clippy::useless_conversion)] // Unix libc time aliases differ by target.
    #[allow(clippy::similar_names)] // The stamp is the proven wire format.
    pub fn stat(metadata: &nix::sys::stat::FileStat) -> io::Result<Self> {
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
        let kind = metadata.st_mode & nix::libc::S_IFMT;
        Ok(Self {
            is_directory: kind == nix::libc::S_IFDIR,
            is_file: kind == nix::libc::S_IFREG,
            is_symlink: kind == nix::libc::S_IFLNK,
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

// One bounded range of file bytes with the revision it was read under —
// field comments stay non-doc so the generated schema does not drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChunk {
    pub data_base64: String,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub metadata: FileMetadata,
}

// One named directory entry with its no-follow metadata stamp — wire
// shape and schema are the established contract, so fields carry no doc
// comments. `file_name` is verbatim name data: one component, never
// re-joined into a path by the consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryEntry {
    pub file_name: String,
    pub metadata: FileMetadata,
}

// One bounded, revision-checked page of directory entries — wire shape
// and schema are the established contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryPage {
    pub entries: Vec<DirectoryEntry>,
    pub next_offset: Option<usize>,
    pub revision: String,
}

/// The bounded, revision-checked range read, parametrized by the caller's
/// no-follow file opener.
///
/// `open` is invoked twice on `relative`: once for the read and once for a
/// fresh identity check, so the published bytes are proven against the same
/// physical file both descriptor-wise and pathname-wise. `max_bytes` must be
/// within `1..=limit`; `offset` must not exceed the file size; a supplied
/// `expected` revision must match the file's current revision. A changed
/// file — by descriptor metadata or by reopened-path metadata — fails with
/// [`io::ErrorKind::AlreadyExists`], never publishing unverified bytes.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] for an out-of-range `max_bytes`,
/// `offset`, or arithmetic overflow; [`io::ErrorKind::AlreadyExists`] for a
/// stale `expected` revision or a file changed during the read; and the
/// opener's own error otherwise.
pub fn read_range(
    relative: &Path,
    offset: u64,
    max_bytes: usize,
    limit: usize,
    expected: Option<&str>,
    open: impl Fn(&Path) -> io::Result<File>,
) -> io::Result<FileChunk> {
    if !(1..=limit).contains(&max_bytes) {
        return Err(invalid("file read exceeds chunk limit"));
    }
    let mut file = open(relative)?;
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
    if before != FileMetadata::of(&file)? || before != FileMetadata::of(&open(relative)?)? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn root() -> (tempfile::TempDir, PackageDirectory) {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().canonicalize().unwrap();
        let directory = PackageDirectory::open(&path).unwrap();
        (temporary, directory)
    }

    #[test]
    fn child_accepts_only_one_normal_component_or_dot() {
        let (_temporary, directory) = root();
        directory.child(OsStr::new(".")).unwrap();
        for name in ["", "..", "a/b", "/a", "./a"] {
            assert!(directory.child(OsStr::new(name)).is_err(), "{name}");
        }
    }

    #[test]
    fn no_symlink_fifo_or_parent_component_can_escape_an_opened_root() {
        let (temporary, directory) = root();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::write(root.join("plain"), b"private").unwrap();
        std::os::unix::fs::symlink("plain", root.join("link")).unwrap();
        nix::unistd::mkfifo(&root.join("pipe"), nix::sys::stat::Mode::S_IRWXU).unwrap();
        for relative in ["link", "pipe", "../plain", "/plain", "", "./plain"] {
            assert!(directory.file(Path::new(relative)).is_err(), "{relative}");
        }
    }

    #[test]
    fn read_range_reads_a_sparse_file_without_materializing_it() {
        let (temporary, directory) = root();
        // A sparse file whose only extent is a marker ending exactly at the
        // 8 MiB end: the read seeks and takes the bound, never loads the
        // file, and reports `next_offset` `None` at the end of file.
        let path = temporary.path().join("blob");
        let marker = b"end-marker";
        let marker_at = 8 * 1024 * 1024 - marker.len() as u64;
        let mut file = std::fs::File::create(&path).unwrap();
        file.seek(SeekFrom::Start(marker_at)).unwrap();
        file.write_all(marker).unwrap();
        drop(file);

        let open = |relative: &Path| directory.file(relative);
        let chunk = read_range(Path::new("blob"), marker_at, 64, 64, None, open).unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap(),
            marker
        );
        assert_eq!(chunk.next_offset, None);
        assert_eq!(chunk.offset, marker_at);
    }

    #[test]
    fn read_range_returns_verified_bytes_revision_and_next_offset() {
        let (temporary, directory) = root();
        let path = temporary.path().join("asset");
        std::fs::write(&path, b"0123456789ab").unwrap();
        let open = |relative: &Path| directory.file(relative);
        let chunk = read_range(Path::new("asset"), 2, 4, 8, None, open).unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .unwrap(),
            b"2345"
        );
        assert_eq!(chunk.next_offset, Some(6));
        let tail = read_range(
            Path::new("asset"),
            chunk.next_offset.unwrap(),
            4,
            8,
            Some(&chunk.metadata.revision),
            open,
        )
        .unwrap();
        assert_eq!(tail.next_offset, Some(10));
        let end = read_range(
            Path::new("asset"),
            tail.next_offset.unwrap(),
            4,
            8,
            Some(&tail.metadata.revision),
            open,
        )
        .unwrap();
        assert_eq!(end.next_offset, None);
    }

    #[test]
    fn read_range_rejects_stale_revision_and_invalid_ranges() {
        let (temporary, directory) = root();
        let path = temporary.path().join("asset");
        std::fs::write(&path, b"0123456789").unwrap();
        let open = |relative: &Path| directory.file(relative);
        let chunk = read_range(Path::new("asset"), 0, 4, 8, None, open).unwrap();

        // A stale expectation is a conflict, never a silent newer read.
        let mut changed = File::options().append(true).open(&path).unwrap();
        changed.write_all(b"!").unwrap();
        assert_eq!(
            read_range(
                Path::new("asset"),
                0,
                4,
                8,
                Some(&chunk.metadata.revision),
                open
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::AlreadyExists
        );

        // Bound and limit are enforced before the file is opened; the offset
        // is enforced against the opened file's size.
        for (offset, max_bytes) in [(u64::MAX, 1), (0, 0), (0, 9)] {
            assert_eq!(
                read_range(Path::new("asset"), offset, max_bytes, 8, None, open)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput,
                "{offset} {max_bytes}"
            );
        }
        assert_eq!(
            read_range(Path::new("asset"), 100, 1, 8, None, open)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn read_range_conflicts_when_the_path_is_replaced_between_opens() {
        let (temporary, directory) = root();
        let path = temporary.path().join("asset");
        std::fs::write(&path, b"0123456789").unwrap();
        let opens = std::cell::Cell::new(0_u32);
        let open = |relative: &Path| {
            let call = opens.get();
            opens.set(call + 1);
            if call == 1 {
                // The second invocation is the post-read identity recheck:
                // replace the pathname so it resolves to a different file.
                std::fs::rename(&path, path.with_extension("retired")).unwrap();
                std::fs::write(&path, b"swapped!").unwrap();
            }
            directory.file(relative)
        };
        assert_eq!(
            read_range(Path::new("asset"), 0, 4, 8, None, open)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(opens.get(), 2);
    }
}
