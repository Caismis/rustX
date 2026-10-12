//! Revision and persistent writer-lock primitives shared by CFG3 source authoring.
use crate::runtime_client::types::RuntimeClientError;
use nix::fcntl::OFlag;
use nix::sys::stat::Mode;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

fn failure(message: &str) -> RuntimeClientError {
    RuntimeClientError::InvalidRequest {
        message: message.into(),
    }
}
fn io_failure(_: impl std::fmt::Debug) -> RuntimeClientError {
    failure(
        "authored source I/O failed before publication; inspect document permissions and retry after reading its revision",
    )
}
// Writer artifacts beside a source document have fixed-length names, so
// every basename the filesystem accepts stays writable. The persistent writer
// lock is named by a digest of the document's exact basename bytes: one lock
// per document, distinct for distinct documents. A staged candidate is named
// by a fresh UUID.
const LOCK_PREFIX: &str = ".rustx-lock-";
const STAGE_PREFIX: &str = ".rustx-stage-";

/// The writer lock name of the document named `document` in its directory.
pub(super) fn lock_name(document: &OsStr) -> String {
    format!(
        "{LOCK_PREFIX}{:x}.lock",
        Sha256::digest(document.as_encoded_bytes())
    )
}

/// Whether `name` is a writer lock or staged candidate, never a document.
pub(super) fn is_writer_artifact(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    name.starts_with(LOCK_PREFIX.as_bytes()) || name.starts_with(STAGE_PREFIX.as_bytes())
}

/// Shared with create-only initialization. Never unlink the lock inode.
pub(super) fn lock_document(target: &Path) -> std::io::Result<File> {
    let name = target
        .file_name()
        .ok_or_else(|| std::io::Error::other("invalid document"))?;
    let lock = target.with_file_name(lock_name(name));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(lock)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("lock is not a regular file"));
    }
    file.lock()?;
    Ok(file)
}

pub(super) fn read_document(path: &Path) -> Result<Option<Vec<u8>>, RuntimeClientError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => Err(failure(
            "authored source must be a regular file, not a symlink",
        )),
        Ok(_) => crate::bounded_file::read_bounded(path)
            .map(Some)
            .map_err(io_failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_failure(error)),
    }
}
pub(super) fn revision(bytes: Option<&[u8]>) -> String {
    bytes.map_or_else(
        || "missing".into(),
        |bytes| format!("sha256:{:x}", Sha256::digest(bytes)),
    )
}

/// The physical authority of one directory — a source document's parent, or
/// private backup storage — opened once when its transaction is admitted, and
/// the (device, inode) identity of that directory.
///
/// Source paths are canonical (a Workspace is validated canonical; the User
/// binding is normalized when bound), so the directory is reached from `/`
/// without following any symlink. Every later step of the transaction — read,
/// writer lock, staging, atomic replacement and durability sync — is relative
/// to this one handle, so replacing or redirecting any component of the
/// pathname afterwards can never move the transaction to another directory.
///
/// What the handle cannot do is stop the directory itself from being renamed.
/// [`SourceDirectory::holds`] therefore asks whether the admitted pathname
/// still names this physical directory; publication requires it immediately
/// before and after the atomic replacement.
pub(super) struct SourceDirectory {
    path: std::path::PathBuf,
    directory: std::os::fd::OwnedFd,
    identity: (u64, u64),
}

impl SourceDirectory {
    /// Admit the parent directory of `document`. A missing directory is
    /// `None` unless `create`, which creates the missing components the same
    /// no-follow way. Any symlink, non-directory or noncanonical component
    /// fails: the pathname no longer names the directory it was admitted as.
    pub(super) fn open(document: &Path, create: bool) -> std::io::Result<Option<Self>> {
        let path = document
            .parent()
            .ok_or_else(|| std::io::Error::other("invalid document"))?;
        Self::admit(path, create)
    }

    /// Admit the directory `path` itself, creating missing components the
    /// same no-follow way.
    pub(super) fn create(path: &Path) -> std::io::Result<Self> {
        Self::admit(path, true)?.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }

    /// Admit the child directory `name` as this user's private (`0700`)
    /// storage, creating it relative to this handle when missing.
    pub(super) fn private_child(&self, name: &str) -> std::io::Result<Self> {
        match nix::sys::stat::mkdirat(&self.directory, name, Mode::S_IRWXU) {
            Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
            Err(error) => return Err(error.into()),
        }
        let directory = nix::fcntl::openat(
            &self.directory,
            name,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?;
        let status = nix::sys::stat::fstat(&directory)?;
        if status.st_uid != nix::unistd::geteuid().as_raw() {
            return Err(std::io::Error::other("directory is not owned by this user"));
        }
        nix::sys::stat::fchmod(&directory, Mode::S_IRWXU)?;
        Ok(Self {
            path: self.path.join(name),
            identity: identity(&directory)?,
            directory,
        })
    }

    fn admit(path: &Path, create: bool) -> std::io::Result<Option<Self>> {
        let Some(directory) = walk(path, create)? else {
            return Ok(None);
        };
        let identity = identity(&directory)?;
        Ok(Some(Self {
            path: path.into(),
            directory,
            identity,
        }))
    }

    /// Whether the admitted pathname still names this physical directory.
    pub(super) fn holds(&self) -> bool {
        walk(&self.path, false)
            .ok()
            .flatten()
            .is_some_and(|directory| identity(&directory).is_ok_and(|id| id == self.identity))
    }

    /// Read `name` in this directory: absent is `None`; a symlink, FIFO or
    /// other non-regular file is refused, as [`read_document`] refuses it.
    pub(super) fn read(&self, name: &str) -> Result<Option<Vec<u8>>, RuntimeClientError> {
        match self.open_file(name, OFlag::O_RDONLY | OFlag::O_NONBLOCK, Mode::empty()) {
            Ok(file) => crate::bounded_file::read_bounded_file(file)
                .map(Some)
                .map_err(io_failure),
            Err(nix::errno::Errno::ENOENT) => Ok(None),
            Err(nix::errno::Errno::ELOOP) => Err(failure(
                "authored source must be a regular file, not a symlink",
            )),
            Err(error) => Err(io_failure(error)),
        }
    }

    /// The persistent writer lock of `name`, the same inode
    /// [`lock_document`] locks while this directory holds. Advisory: it
    /// serializes rustX writers, never an external rename.
    pub(super) fn lock(&self, name: &str) -> std::io::Result<File> {
        let file = self.open_file(
            &lock_name(OsStr::new(name)),
            OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NONBLOCK,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("lock is not a regular file"));
        }
        file.lock()?;
        Ok(file)
    }

    /// Write and sync `bytes` to a new private file in this directory. The
    /// staged file is removed again unless it is published.
    pub(super) fn stage(&self, bytes: &[u8]) -> std::io::Result<Staged<'_>> {
        let name = format!("{STAGE_PREFIX}{}.tmp", uuid::Uuid::now_v7().simple());
        let mut file = self.open_file(
            &name,
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?;
        // Created exclusively above, so only this entry is ever removed.
        let staged = Staged {
            directory: self,
            name,
            published: false,
        };
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(staged)
    }

    /// Atomically replace `name` with `staged`, inside this directory.
    pub(super) fn replace(&self, mut staged: Staged<'_>, name: &str) -> std::io::Result<()> {
        nix::fcntl::renameat(&self.directory, staged.name.as_str(), &self.directory, name)?;
        staged.published = true;
        Ok(())
    }

    /// Publish `staged` under `name` without replacing any existing entry:
    /// an existing `name` fails with [`std::io::ErrorKind::AlreadyExists`].
    /// The staged name remains and is removed when `staged` drops.
    pub(super) fn link(&self, staged: &Staged<'_>, name: &str) -> std::io::Result<()> {
        Ok(nix::unistd::linkat(
            &self.directory,
            staged.name.as_str(),
            &self.directory,
            name,
            nix::fcntl::AtFlags::empty(),
        )?)
    }

    /// Whether this physical directory is `ancestor` or lies below it,
    /// following `..` from the handle, never a pathname.
    pub(super) fn within(&self, ancestor: &SourceDirectory) -> std::io::Result<bool> {
        let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        let mut current = self.identity;
        let mut parent: Option<std::os::fd::OwnedFd> = None;
        loop {
            if current == ancestor.identity {
                return Ok(true);
            }
            let next = nix::fcntl::openat(
                parent.as_ref().unwrap_or(&self.directory),
                "..",
                flags,
                Mode::empty(),
            )?;
            let id = identity(&next)?;
            if id == current {
                return Ok(false);
            }
            current = id;
            parent = Some(next);
        }
    }

    pub(super) fn remove(&self, name: &str) -> std::io::Result<()> {
        Ok(nix::unistd::unlinkat(
            &self.directory,
            name,
            nix::unistd::UnlinkatFlags::NoRemoveDir,
        )?)
    }

    /// Make this directory's entries durable.
    pub(super) fn sync(&self) -> std::io::Result<()> {
        Ok(nix::unistd::fsync(&self.directory)?)
    }

    fn open_file(&self, name: &str, flags: OFlag, mode: Mode) -> nix::Result<File> {
        nix::fcntl::openat(
            &self.directory,
            name,
            flags | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            mode,
        )
        .map(File::from)
    }
}

/// A synced candidate beside its document, removed unless published.
pub(super) struct Staged<'a> {
    directory: &'a SourceDirectory,
    name: String,
    published: bool,
}

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = self.directory.remove(&self.name);
        }
    }
}

/// Open `path` from `/` one component at a time, never following a symlink.
/// With `create`, a missing component is created relative to its verified
/// parent handle.
fn walk(path: &Path, create: bool) -> std::io::Result<Option<std::os::fd::OwnedFd>> {
    use std::path::Component;
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(std::io::Error::other("source directory must be absolute"));
    }
    let mut directory = nix::fcntl::open("/", flags, Mode::empty())?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(std::io::Error::other("source directory must be canonical"));
        };
        directory = match nix::fcntl::openat(&directory, name, flags, Mode::empty()) {
            Ok(next) => next,
            Err(nix::errno::Errno::ENOENT) if create => {
                match nix::sys::stat::mkdirat(&directory, name, Mode::from_bits_truncate(0o777)) {
                    Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
                    Err(error) => return Err(error.into()),
                }
                nix::fcntl::openat(&directory, name, flags, Mode::empty())?
            }
            Err(nix::errno::Errno::ENOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
    }
    Ok(Some(directory))
}

fn identity(directory: &std::os::fd::OwnedFd) -> std::io::Result<(u64, u64)> {
    let status = nix::sys::stat::fstat(directory)?;
    #[allow(
        clippy::cast_sign_loss,
        clippy::unnecessary_cast,
        reason = "dev_t and ino_t are signed or narrower on some platforms"
    )]
    Ok((status.st_dev as u64, status.st_ino as u64))
}
