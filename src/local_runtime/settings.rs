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
/// Entry names are native [`OsStr`] bytes, never text: a document is read,
/// locked, replaced and removed under exactly the basename it is bound to.
///
/// Every directory entry a transaction creates on its way — a missing
/// ancestor, the directory itself, private backup storage — is made durable
/// by syncing its parent before the walk continues, so a later durability
/// sync of the directory's contents is reachable after a crash. See [`child`].
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
    /// storage, creating it relative to this handle when missing — durably,
    /// as [`child`] creates every entry.
    pub(super) fn private_child(&self, name: &str) -> std::io::Result<Self> {
        let directory = child(
            &self.directory,
            &self.path,
            OsStr::new(name),
            Some(Mode::S_IRWXU),
        )?
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
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
    pub(super) fn read(&self, name: &OsStr) -> Result<Option<Vec<u8>>, RuntimeClientError> {
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
    pub(super) fn lock(&self, name: &OsStr) -> std::io::Result<File> {
        let file = self.open_file(
            OsStr::new(&lock_name(name)),
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
            OsStr::new(&name),
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
    pub(super) fn replace(&self, mut staged: Staged<'_>, name: &OsStr) -> std::io::Result<()> {
        nix::fcntl::renameat(&self.directory, staged.name.as_str(), &self.directory, name)?;
        staged.published = true;
        #[cfg(test)]
        durability::step(durability::Step::Published(self.path.join(name)));
        Ok(())
    }

    /// Publish `staged` under `name` without replacing any existing entry:
    /// an existing `name` fails with [`std::io::ErrorKind::AlreadyExists`].
    /// The staged name remains and is removed when `staged` drops.
    pub(super) fn link(&self, staged: &Staged<'_>, name: &OsStr) -> std::io::Result<()> {
        nix::unistd::linkat(
            &self.directory,
            staged.name.as_str(),
            &self.directory,
            name,
            nix::fcntl::AtFlags::empty(),
        )?;
        #[cfg(test)]
        durability::step(durability::Step::Published(self.path.join(name)));
        Ok(())
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

    pub(super) fn remove(&self, name: &OsStr) -> std::io::Result<()> {
        Ok(nix::unistd::unlinkat(
            &self.directory,
            name,
            nix::unistd::UnlinkatFlags::NoRemoveDir,
        )?)
    }

    /// Make this directory's entries durable.
    pub(super) fn sync(&self) -> std::io::Result<()> {
        sync(&self.directory, &self.path)
    }

    fn open_file(&self, name: &OsStr, flags: OFlag, mode: Mode) -> nix::Result<File> {
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
            let _ = self.directory.remove(OsStr::new(&self.name));
        }
    }
}

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

/// Open `path` from `/` one component at a time, never following a symlink.
/// With `create`, a missing component is created relative to its verified
/// parent handle, durably (see [`child`]).
fn walk(path: &Path, create: bool) -> std::io::Result<Option<std::os::fd::OwnedFd>> {
    use std::path::Component;
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(std::io::Error::other("source directory must be absolute"));
    }
    let mode = create.then(|| Mode::from_bits_truncate(0o777));
    let mut directory = nix::fcntl::open("/", DIRECTORY_FLAGS, Mode::empty())?;
    let mut reached = std::path::PathBuf::from("/");
    for component in components {
        let Component::Normal(name) = component else {
            return Err(std::io::Error::other("source directory must be canonical"));
        };
        let Some(next) = child(&directory, &reached, name, mode)? else {
            return Ok(None);
        };
        directory = next;
        reached.push(name);
    }
    Ok(Some(directory))
}

/// Open the directory `name` below `parent` (whose path is `path`) without
/// following a symlink. A missing entry is `None`, unless `mode` creates it.
///
/// A created entry is durable only once its parent is synced, so the parent
/// is synced before the child is used; an entry another writer created
/// concurrently (`EEXIST`) is synced the same way, never assumed durable. A
/// failed sync fails the step and withdraws the entry this call created, so
/// no later walk mistakes it for an established one. An entry already
/// present when first looked up was made durable by the walk that created it.
fn child(
    parent: &std::os::fd::OwnedFd,
    path: &Path,
    name: &OsStr,
    mode: Option<Mode>,
) -> std::io::Result<Option<std::os::fd::OwnedFd>> {
    match nix::fcntl::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(next) => return Ok(Some(next)),
        Err(nix::errno::Errno::ENOENT) => {}
        Err(error) => return Err(error.into()),
    }
    let Some(mode) = mode else {
        return Ok(None);
    };
    let created = match nix::sys::stat::mkdirat(parent, name, mode) {
        Ok(()) => true,
        Err(nix::errno::Errno::EEXIST) => false,
        Err(error) => return Err(error.into()),
    };
    #[cfg(test)]
    if created {
        durability::step(durability::Step::Created(path.join(name)));
    }
    if let Err(error) = sync(parent, path) {
        if created {
            let _ = nix::unistd::unlinkat(parent, name, nix::unistd::UnlinkatFlags::RemoveDir);
        }
        return Err(error);
    }
    Ok(Some(nix::fcntl::openat(
        parent,
        name,
        DIRECTORY_FLAGS,
        Mode::empty(),
    )?))
}

/// Make the entries of `directory` (at `path`) durable.
#[cfg_attr(
    not(test),
    allow(
        unused_variables,
        reason = "the path names the directory in test durability traces"
    )
)]
fn sync(directory: &std::os::fd::OwnedFd, path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if durability::sync_fails(path) {
        return Err(std::io::Error::other("injected directory sync failure"));
    }
    nix::unistd::fsync(directory)?;
    #[cfg(test)]
    durability::step(durability::Step::Synced(path.into()));
    Ok(())
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

/// Test-only trace of this thread's durability-relevant directory steps, with
/// one injectable directory sync failure.
#[cfg(test)]
pub(super) mod durability {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Step {
        /// A directory entry was created at this path.
        Created(PathBuf),
        /// This directory's entries were synced.
        Synced(PathBuf),
        /// A file entry was published at this path, by link or rename.
        Published(PathBuf),
    }

    #[derive(Default)]
    struct Trace {
        recording: bool,
        steps: Vec<Step>,
        fail: Option<PathBuf>,
    }

    thread_local! {
        static TRACE: RefCell<Trace> = RefCell::default();
    }

    /// Record this thread's steps from now on; the next sync of `fail`, if
    /// given, fails.
    pub(crate) fn record(fail: Option<&Path>) {
        TRACE.with_borrow_mut(|trace| {
            *trace = Trace {
                recording: true,
                steps: Vec::new(),
                fail: fail.map(Path::to_path_buf),
            };
        });
    }

    /// Stop recording and return the recorded steps.
    pub(crate) fn take() -> Vec<Step> {
        TRACE.with_borrow_mut(std::mem::take).steps
    }

    pub(super) fn step(step: Step) {
        TRACE.with_borrow_mut(|trace| {
            if trace.recording {
                trace.steps.push(step);
            }
        });
    }

    pub(super) fn sync_fails(path: &Path) -> bool {
        TRACE.with_borrow_mut(|trace| {
            let fails = trace.fail.as_deref() == Some(path);
            if fails {
                trace.fail = None;
            }
            fails
        })
    }
}
