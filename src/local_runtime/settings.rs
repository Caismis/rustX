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
fn io_failure(error: &std::io::Error) -> RuntimeClientError {
    report_io_failure("read_source", error);
    failure(
        "authored source I/O failed before publication; inspect document permissions and retry after reading its revision",
    )
}

/// Failure-only diagnostics contain neither source paths nor document bytes.
pub(super) fn report_io_failure(operation: &'static str, error: &std::io::Error) {
    tracing::warn!(operation, kind = ?error.kind(), errno = error.raw_os_error(), "authored source I/O failed");
}

/// Interrupted `flock` waits have not acquired the lock. Continue the same
/// operation on the same descriptor; never retry a source transaction here.
fn lock_file(file: &File, shared: bool) -> std::io::Result<()> {
    loop {
        #[cfg(test)]
        let injected = locks::attempt();
        #[cfg(not(test))]
        let injected: std::io::Result<()> = Ok(());
        let result = injected.and_then(|()| {
            if shared {
                file.lock_shared()
            } else {
                file.lock()
            }
        });
        match result {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}
// Writer artifacts beside a source document have fixed-length names, so
// every basename the filesystem accepts stays writable. The persistent writer
// locks of a directory's documents live in its one private lock directory, so
// a directory's own entries grow by at most that one however many documents
// were ever written there: bounded scans of authored resources never count
// lock history. A lock is named by a digest of the document's exact basename
// bytes: one lock per document, distinct for distinct documents. A staged
// candidate is named by a fresh UUID beside its document.
const LOCK_DIRECTORY: &str = ".rustx-locks";
const STAGE_PREFIX: &str = ".rustx-stage-";

/// The writer lock name of the document named `document`, inside its
/// directory's lock directory.
pub(super) fn lock_name(document: &OsStr) -> String {
    format!("{:x}.lock", Sha256::digest(document.as_encoded_bytes()))
}

/// The writer lock path of the document at `document`.
#[cfg(test)]
pub(super) fn lock_path_of(document: &Path) -> std::path::PathBuf {
    document.with_file_name(LOCK_DIRECTORY).join(lock_name(
        document.file_name().expect("a document is named"),
    ))
}

/// Whether `name` is the lock directory or a staged candidate, never a
/// document.
pub(super) fn is_writer_artifact(name: &OsStr) -> bool {
    name == LOCK_DIRECTORY || name.as_encoded_bytes().starts_with(STAGE_PREFIX.as_bytes())
}

/// Open the lock directory below `parent`, never following a symlink:
/// `None` when it is missing and not `create`. A creator makes it private
/// (`0700`); one that another writer created first is simply opened. It is
/// never removed, like the locks in it.
fn lock_directory(
    parent: &impl std::os::fd::AsFd,
    create: bool,
) -> std::io::Result<Option<std::os::fd::OwnedFd>> {
    let open = || nix::fcntl::openat(parent, LOCK_DIRECTORY, DIRECTORY_FLAGS, Mode::empty());
    match open() {
        Err(nix::errno::Errno::ENOENT) if create => {
            match nix::sys::stat::mkdirat(parent, LOCK_DIRECTORY, Mode::S_IRWXU) {
                Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
                Err(error) => return Err(error.into()),
            }
            Ok(Some(open()?))
        }
        Err(nix::errno::Errno::ENOENT) => Ok(None),
        opened => Ok(Some(opened?)),
    }
}

/// Open the persistent writer lock inode, creating it when missing.
///
/// The inode is created once and never unlinked, so creation is exclusive:
/// open the existing lock; when it is missing, create it with `O_EXCL`; a
/// creator that loses that race opens the inode the winner created. A
/// non-exclusive `O_CREAT` is never used: on macOS two concurrent
/// non-exclusive creators of one new name can see the loser fail with
/// `ENOENT`, which would report a lost creation race as an I/O failure.
fn open_lock(open: impl Fn(OFlag) -> nix::Result<File>) -> std::io::Result<File> {
    let flags = OFlag::O_RDWR | OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let file = match open(flags) {
        Err(nix::errno::Errno::ENOENT) => match open(flags | OFlag::O_CREAT | OFlag::O_EXCL) {
            Err(nix::errno::Errno::EEXIST) => open(flags),
            created => created,
        },
        opened => opened,
    }?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("lock is not a regular file"));
    }
    Ok(file)
}

/// Open the writer lock `name` in the lock directory `locks`.
fn open_lock_in(locks: &std::os::fd::OwnedFd, name: &OsStr) -> std::io::Result<File> {
    let lock = lock_name(name);
    open_lock(|flags| {
        nix::fcntl::openat(
            locks,
            OsStr::new(&lock),
            flags,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .map(File::from)
    })
}

/// Shared with create-only initialization: the same inode
/// [`SourceDirectory::lock`] locks. Never unlink the lock inode.
pub(super) fn lock_document(target: &Path) -> std::io::Result<File> {
    let name = target
        .file_name()
        .ok_or_else(|| std::io::Error::other("invalid document"))?;
    // The document's directory is named by pathname, as before; only the
    // lock directory and the lock in it are never followed.
    let directory = nix::fcntl::open(
        parent(target)?,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?;
    let locks = lock_directory(&directory, true)?
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
    let file = open_lock_in(&locks, name)?;
    lock_file(&file, false)?;
    Ok(file)
}

/// Read an authored resource document by pathname. The `open` that reads it
/// is the one that refuses a symlink or non-regular entry: the pathname is
/// resolved once, never checked and then opened again.
///
/// A `rustx.toml` source is read through its physically admitted directory
/// instead ([`read_source`], [`SourceDirectory::read`]).
pub(super) fn read_document(path: &Path) -> Result<Option<Vec<u8>>, RuntimeClientError> {
    read_opened(
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path),
    )
}

/// Read the source `document` once through its physically admitted directory
/// ([`AdmittedSource`]): no component of its pathname, ancestor or leaf, is
/// followed, a missing directory is an absent source, never created, and a
/// pathname that no longer names the admitted directory answers nothing.
pub(super) fn read_source(document: &Path) -> Result<Option<Vec<u8>>, RuntimeClientError> {
    AdmittedSource::admit(document)
        .map_err(|error| io_failure(&error))?
        .reread()
        .map_err(|failure| match failure {
            SourceReread::Unreadable(error) => error,
            SourceReread::Redirected | SourceReread::Unadmitted(_) => redirected(),
        })
}

fn redirected() -> RuntimeClientError {
    failure("authored source directory was replaced or redirected; rustX never follows it")
}

/// One `rustx.toml` bound to the physical directory it was admitted in
/// ([`SourceDirectory::open`]: no component followed, nothing created). Every
/// consumer of a source — Session resolution and its policy, source capture,
/// automatic repair's observations — reads it through this one admission:
/// its shared writer lock, its bytes and their reread are all relative to
/// that one handle, so the pathname is never resolved again to reach the
/// document, and before a read answers, the pathname must still name that
/// directory ([`AdmittedSource::reread`]).
pub(super) struct AdmittedSource {
    path: std::path::PathBuf,
    /// `None` while the directory is missing: the document is absent.
    directory: Option<SourceDirectory>,
}

/// Why an [`AdmittedSource::reread`] answers nothing.
#[derive(Debug)]
pub(super) enum SourceReread {
    /// The pathname no longer names the admitted directory.
    Redirected,
    /// A directory that was missing cannot be admitted now.
    Unadmitted(std::io::Error),
    /// The document cannot be read through its directory.
    Unreadable(RuntimeClientError),
}

impl AdmittedSource {
    /// Admit the directory of `document` as it is now; a pathname that
    /// cannot be reached without following a symlink fails.
    pub(super) fn admit(document: &Path) -> std::io::Result<Self> {
        document
            .file_name()
            .ok_or_else(|| std::io::Error::other("invalid document"))?;
        Ok(Self {
            path: document.into(),
            directory: SourceDirectory::open(document)?,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn name(&self) -> &OsStr {
        self.path
            .file_name()
            .expect("an admitted source is named in its directory")
    }

    /// The document's bytes through the admitted directory, unconfirmed:
    /// absent is `None`; a symlink, FIFO or other non-regular entry is
    /// refused without its target being opened.
    pub(super) fn read(&self) -> Result<Option<Vec<u8>>, RuntimeClientError> {
        self.directory
            .as_ref()
            .map_or(Ok(None), |directory| directory.read(self.name()))
    }

    /// See [`SourceDirectory::share_lock`]; a missing directory has no lock.
    pub(super) fn share_lock(&self) -> std::io::Result<Option<File>> {
        self.directory
            .as_ref()
            .map_or(Ok(None), |directory| directory.share_lock(self.name()))
    }

    /// Whether this document's writer lock exists now. A directory that was
    /// missing is observed again — a rustX writer creates the directory and
    /// then the lock in it — and still created by nothing.
    pub(super) fn lock_exists(&self) -> std::io::Result<bool> {
        match &self.directory {
            Some(directory) => directory.lock_exists(self.name()),
            None => match SourceDirectory::open(&self.path)? {
                Some(directory) => directory.lock_exists(self.name()),
                None => Ok(false),
            },
        }
    }

    /// Whether the pathname no longer names the admitted directory.
    pub(super) fn redirected(&self) -> bool {
        self.directory
            .as_ref()
            .is_some_and(|directory| !directory.holds())
    }

    /// The document's bytes now, read through the admitted directory, which
    /// the pathname must still name: one it no longer names is
    /// [`SourceReread::Redirected`], whatever the read returned, exactly as
    /// publication refuses it, since the reader would otherwise answer for a
    /// directory that is no longer the source. A directory that was missing
    /// is admitted again, under the same rule: if it appeared, its document
    /// is compared like any other change.
    pub(super) fn reread(&self) -> Result<Option<Vec<u8>>, SourceReread> {
        let readmitted;
        let source = if self.directory.is_some() {
            self
        } else {
            readmitted = Self::admit(&self.path).map_err(SourceReread::Unadmitted)?;
            &readmitted
        };
        let bytes = source.read();
        if source.redirected() {
            Err(SourceReread::Redirected)
        } else {
            bytes.map_err(SourceReread::Unreadable)
        }
    }
}

/// The bounded bytes of a document just opened without following its name
/// and without blocking: absent is `None`; a symlink, FIFO, directory or
/// other non-regular file is refused. Everything is checked and read on the
/// opened descriptor itself.
fn read_opened(opened: std::io::Result<File>) -> Result<Option<Vec<u8>>, RuntimeClientError> {
    let not_regular = || failure("authored source must be a regular file, not a symlink");
    let file = match opened {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Err(not_regular()),
        Err(error) => return Err(io_failure(&error)),
    };
    if !file
        .metadata()
        .map_err(|error| io_failure(&error))?
        .is_file()
    {
        return Err(not_regular());
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, crate::bounded_file::MAX_RESOURCE_BYTES + 1),
        &mut bytes,
    )
    .map_err(|error| io_failure(&error))?;
    if !crate::bounded_file::within_bound(&bytes) {
        return Err(failure("authored source exceeds 1 MiB"));
    }
    Ok(Some(bytes))
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
/// Opening a directory, and creating its missing components, claims nothing
/// about durability. Observation ([`SourceDirectory::open`],
/// [`SourceDirectory::observe`]) only opens what exists: it creates nothing
/// and syncs nothing, so reading a source, or finding it healthy, costs no
/// durability I/O. Creation ([`SourceDirectory::create`],
/// [`SourceDirectory::create_directory`], [`SourceDirectory::private_child`])
/// also makes what is missing, and syncs nothing either.
///
/// Durability is established by one operation only:
/// [`SourceDirectory::confirm`], which walks the admitted pathname from `/`
/// again and syncs the parent of every component — created by this
/// transaction, created concurrently by another writer, or merely visible —
/// and requires the walk to end at this very physical directory. Visibility
/// is not durability: an entry another process just created may not be
/// durable yet, and an entry left behind by a failed transaction never was,
/// so the publishing transaction establishes the barrier itself and trusts
/// no earlier creator, sync or cleanup. Every publication into a directory —
/// a source replacement or removal, a backup copy — is preceded by its
/// confirmation; once confirmed, every name leading to the directory is
/// durable, and a later sync of its own entries is reachable after a crash.
///
/// What the handle cannot do is stop the directory itself from being renamed.
/// [`SourceDirectory::holds`] therefore asks whether the admitted pathname
/// still names this physical directory; publication requires it immediately
/// before (as part of the confirmation) and after the atomic replacement.
pub(super) struct SourceDirectory {
    path: std::path::PathBuf,
    directory: std::os::fd::OwnedFd,
    identity: (u64, u64),
}

impl SourceDirectory {
    /// Observe the parent directory of `document`: `None` when missing. Any
    /// symlink, non-directory or noncanonical component fails: the pathname
    /// no longer names the directory it would be admitted as.
    pub(super) fn open(document: &Path) -> std::io::Result<Option<Self>> {
        Self::observe(parent(document)?)
    }

    /// Observe the directory `path` itself: `None` when missing.
    pub(super) fn observe(path: &Path) -> std::io::Result<Option<Self>> {
        Self::walked(path, Walk::Observe)
    }

    /// Open the parent directory of `document` for a write, creating missing
    /// components. Nothing is synced: publication confirms it.
    pub(super) fn create(document: &Path) -> std::io::Result<Self> {
        Self::create_directory(parent(document)?)
    }

    /// Open the directory `path` itself for a write, creating missing
    /// components. Nothing is synced: publication confirms it.
    pub(super) fn create_directory(path: &Path) -> std::io::Result<Self> {
        Self::walked(path, Walk::Create)?
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }

    /// Open the child directory `name` as this user's private (`0700`)
    /// storage, created relative to this handle when missing. Nothing is
    /// synced: publication confirms it.
    pub(super) fn private_child(&self, name: &str) -> std::io::Result<Self> {
        let directory = child(&self.directory, &self.path, OsStr::new(name), Walk::Create)?
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

    fn walked(path: &Path, mode: Walk) -> std::io::Result<Option<Self>> {
        let Some(directory) = walk(path, mode)? else {
            return Ok(None);
        };
        let identity = identity(&directory)?;
        Ok(Some(Self {
            path: path.into(),
            directory,
            identity,
        }))
    }

    /// Make every name leading to this directory durable, and prove the
    /// admitted pathname still names it: walk the pathname from `/` again
    /// without creating or following anything, syncing the parent of every
    /// component before descending, and require the walk to end at this
    /// handle's (device, inode). The handle is never replaced by what the
    /// walk opened. Fails on any failed sync, on a missing or redirected
    /// component, and on a walk that ends at another directory; a failure
    /// says nothing about which, so callers ask [`SourceDirectory::holds`].
    pub(super) fn confirm(&self) -> std::io::Result<()> {
        let reached = walk(&self.path, Walk::Confirm)?
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
        if identity(&reached)? == self.identity {
            Ok(())
        } else {
            Err(std::io::Error::other(
                "the pathname names another directory",
            ))
        }
    }

    /// Whether the admitted pathname still names this physical directory.
    pub(super) fn holds(&self) -> bool {
        walk(&self.path, Walk::Observe)
            .ok()
            .flatten()
            .is_some_and(|directory| identity(&directory).is_ok_and(|id| id == self.identity))
    }

    /// Read `name` in this directory: absent is `None`; a symlink, FIFO or
    /// other non-regular file is refused, as [`read_document`] refuses it.
    pub(super) fn read(&self, name: &OsStr) -> Result<Option<Vec<u8>>, RuntimeClientError> {
        #[cfg(test)]
        reads::opening(&self.path.join(name));
        read_opened(
            self.open_file(
                name,
                OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_NOCTTY,
                Mode::empty(),
            )
            .map_err(std::io::Error::from),
        )
    }

    /// Hold the persistent writer lock of `name` shared, for a reader: rustX
    /// writers, which hold it exclusively, are excluded while the returned
    /// file lives. Reading never creates the lock: `None` when it is absent,
    /// which means no rustX writer has ever locked the document in this
    /// directory, since lock inodes are never unlinked. The lock name is
    /// never followed.
    pub(super) fn share_lock(&self, name: &OsStr) -> std::io::Result<Option<File>> {
        let Some(locks) = lock_directory(&self.directory, false)? else {
            return Ok(None);
        };
        let file = match nix::fcntl::openat(
            &locks,
            OsStr::new(&lock_name(name)),
            OFlag::O_RDONLY
                | OFlag::O_NONBLOCK
                | OFlag::O_NOCTTY
                | OFlag::O_NOFOLLOW
                | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(nix::errno::Errno::ENOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("lock is not a regular file"));
        }
        lock_file(&file, true)?;
        Ok(Some(file))
    }

    /// Whether the writer lock of `name` exists in this directory, as any
    /// entry.
    pub(super) fn lock_exists(&self, name: &OsStr) -> std::io::Result<bool> {
        let Some(locks) = lock_directory(&self.directory, false)? else {
            return Ok(false);
        };
        match nix::sys::stat::fstatat(
            &locks,
            OsStr::new(&lock_name(name)),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => Ok(true),
            Err(nix::errno::Errno::ENOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// The persistent writer lock of `name`, the same inode
    /// [`lock_document`] locks while this directory holds. Advisory: it
    /// serializes rustX writers, never an external rename.
    pub(super) fn lock(&self, name: &OsStr) -> std::io::Result<File> {
        let locks = lock_directory(&self.directory, true)?
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
        let file = open_lock_in(&locks, name)?;
        lock_file(&file, false)?;
        Ok(file)
    }

    /// The physical identity (device, inode) of this directory: two
    /// handles with one identity are the same directory, whatever their
    /// paths.
    pub(super) fn identity(&self) -> (u64, u64) {
        self.identity
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

/// How a walk treats the components of its path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Walk {
    /// Open what exists; create and sync nothing.
    Observe,
    /// Create what is missing; sync nothing.
    Create,
    /// Open what exists, and confirm every component's entry by syncing its
    /// parent before descending, however the component came to exist.
    Confirm,
}

fn parent(document: &Path) -> std::io::Result<&Path> {
    document
        .parent()
        .ok_or_else(|| std::io::Error::other("invalid document"))
}

/// Open `path` from `/` one component at a time, never following a symlink,
/// as `mode` treats each component (see [`child`]).
fn walk(path: &Path, mode: Walk) -> std::io::Result<Option<std::os::fd::OwnedFd>> {
    use std::path::Component;
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(std::io::Error::other("source directory must be absolute"));
    }
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
/// following a symlink. A missing entry is `None` unless creating.
///
/// A creating walk makes a missing entry (`0777` before the umask; private
/// storage narrows it afterwards); an entry another writer creates first is
/// simply opened. A confirming walk syncs `parent` — always, for an entry of
/// any origin: one created a moment ago by another writer that has not synced
/// yet, or left behind by a transaction whose sync failed, is not durable
/// merely because it is visible, so the confirming walk establishes the
/// barrier itself and never relies on another writer's sync or cleanup. A
/// failed sync fails the walk and removes nothing.
fn child(
    parent: &std::os::fd::OwnedFd,
    path: &Path,
    name: &OsStr,
    mode: Walk,
) -> std::io::Result<Option<std::os::fd::OwnedFd>> {
    let next = match nix::fcntl::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(next) => next,
        Err(nix::errno::Errno::ENOENT) if mode == Walk::Create => {
            match nix::sys::stat::mkdirat(parent, name, Mode::from_bits_truncate(0o777)) {
                Ok(()) => {
                    #[cfg(test)]
                    durability::created(&path.join(name));
                }
                Err(nix::errno::Errno::EEXIST) => {}
                Err(error) => return Err(error.into()),
            }
            nix::fcntl::openat(parent, name, DIRECTORY_FLAGS, Mode::empty())?
        }
        Err(nix::errno::Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if mode == Walk::Confirm {
        sync(parent, path)?;
    }
    Ok(Some(next))
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
    durability::syncing(path);
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

/// Thread-local syscall fault injection at the real lock acquisition boundary.
#[cfg(test)]
pub(super) mod locks {
    use std::cell::RefCell;

    thread_local! {
        static NEXT: RefCell<(Option<i32>, usize)> = const { RefCell::new((None, 0)) };
    }

    pub(crate) fn fail_next(errno: i32) {
        NEXT.with_borrow_mut(|state| *state = (Some(errno), 0));
    }

    pub(crate) fn attempts() -> usize {
        NEXT.with_borrow(|state| state.1)
    }

    pub(super) fn attempt() -> std::io::Result<()> {
        NEXT.with_borrow_mut(|state| {
            state.1 += 1;
            state.0.take().map_or(Ok(()), |errno| {
                Err(std::io::Error::from_raw_os_error(errno))
            })
        })
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    /// The exact losing-creator interleaving on a real filesystem: the lock is
    /// missing when first opened, another creator wins before this one's
    /// exclusive create, and this one then opens the winner's inode. No
    /// attempt is a non-exclusive create.
    #[test]
    fn a_creator_that_loses_the_lock_creation_race_opens_the_winners_inode() {
        use std::os::unix::fs::MetadataExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("race.lock");
        let attempts = std::cell::RefCell::new(Vec::new());
        let winner = std::cell::Cell::new(None);
        let file = open_lock(|flags| {
            attempts.borrow_mut().push(flags);
            let opened = nix::fcntl::open(&path, flags, Mode::S_IRUSR | Mode::S_IWUSR);
            if attempts.borrow().len() == 1 {
                // The concurrent creator commits right after the first lookup.
                let created = File::create_new(&path).unwrap();
                winner.set(Some(created.metadata().unwrap().ino()));
            }
            opened.map(File::from)
        })
        .unwrap();
        let attempts = attempts.into_inner();
        assert_eq!(attempts.len(), 3, "open, exclusive create, open");
        assert!(!attempts[0].contains(OFlag::O_CREAT));
        assert!(attempts[1].contains(OFlag::O_CREAT | OFlag::O_EXCL));
        assert!(!attempts[2].contains(OFlag::O_CREAT));
        assert_eq!(Some(file.metadata().unwrap().ino()), winner.get());
    }

    /// Writers that race to create a document's first lock all hold the same
    /// inode, one at a time. On macOS a non-exclusive create failed the loser
    /// with ENOENT (CI run 38988313366).
    #[test]
    fn concurrent_first_writers_all_lock_the_same_inode() {
        use std::os::unix::fs::MetadataExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("rustx.toml");
        let source = SourceDirectory::create(&path).unwrap();
        for round in 0..32 {
            let document = std::ffi::OsString::from(format!("rustx-{round}.toml"));
            let barrier = std::sync::Barrier::new(8);
            let inodes: Vec<_> = std::thread::scope(|threads| {
                let workers: Vec<_> = (0..8)
                    .map(|_| {
                        threads.spawn(|| {
                            barrier.wait();
                            source
                                .lock(&document)
                                .map(|lock| lock.metadata().unwrap().ino())
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .map(|worker| worker.join().unwrap())
                    .collect()
            });
            let inodes: Result<std::collections::BTreeSet<_>, _> = inodes.into_iter().collect();
            assert_eq!(inodes.unwrap().len(), 1, "round {round}");
        }
    }

    #[test]
    fn interrupted_shared_and_initialization_locks_continue_but_other_errors_survive() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("rustx.toml");
        locks::fail_next(libc::EINTR);
        drop(lock_document(&path).unwrap());
        assert_eq!(locks::attempts(), 2);
        let source = SourceDirectory::create(&path).unwrap();
        locks::fail_next(libc::EINTR);
        drop(
            source
                .share_lock(path.file_name().unwrap())
                .unwrap()
                .unwrap(),
        );
        assert_eq!(locks::attempts(), 2);
        locks::fail_next(libc::EACCES);
        assert_eq!(
            lock_document(&path).unwrap_err().raw_os_error(),
            Some(libc::EACCES)
        );
        assert_eq!(locks::attempts(), 1);
    }
}

/// Test-only cross-thread hook right before a source entry is opened relative
/// to its admitted directory: the exact boundary that a replacement of the
/// entry, or of any component of its pathname, races with.
#[cfg(test)]
pub(super) mod reads {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    type Hook = Box<dyn FnOnce() + Send>;
    /// Keyed by absolute path; tests use distinct temporary directories.
    static OPENING_HOOKS: Mutex<Vec<(PathBuf, Hook)>> = Mutex::new(Vec::new());

    /// Run `hook` on whichever thread next opens the source entry `path`
    /// through its admitted directory, right before the `openat`.
    pub(crate) fn before_opening(path: &Path, hook: impl FnOnce() + Send + 'static) {
        OPENING_HOOKS
            .lock()
            .unwrap()
            .push((path.to_path_buf(), Box::new(hook)));
    }

    pub(super) fn opening(path: &Path) {
        let hook = {
            let mut hooks = OPENING_HOOKS.lock().unwrap();
            hooks
                .iter()
                .position(|(target, _)| target == path)
                .map(|index| hooks.remove(index).1)
        };
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// Test-only trace of this thread's durability-relevant directory steps, with
/// one injectable directory sync failure, a hook right before this thread's
/// next sync of one directory, and a cross-thread hook right after a
/// directory's creation.
#[cfg(test)]
pub(super) mod durability {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    type Hook = Box<dyn FnOnce() + Send>;
    /// Keyed by absolute path; tests use distinct temporary directories.
    static CREATED_HOOKS: Mutex<Vec<(PathBuf, Hook)>> = Mutex::new(Vec::new());

    /// Run `hook` on whichever thread next creates the directory `path`,
    /// right after its `mkdirat`.
    pub(crate) fn after_creating(path: &Path, hook: impl FnOnce() + Send + 'static) {
        CREATED_HOOKS
            .lock()
            .unwrap()
            .push((path.to_path_buf(), Box::new(hook)));
    }

    /// Run `hook` right before this thread next syncs the directory `path`.
    /// Thread-local, so a shared ancestor such as the temporary directory
    /// never runs another test's hook.
    pub(crate) fn before_syncing(path: &Path, hook: impl FnOnce() + Send + 'static) {
        TRACE.with_borrow_mut(|trace| trace.syncing = Some((path.to_path_buf(), Box::new(hook))));
    }

    pub(super) fn created(path: &Path) {
        step(Step::Created(path.to_path_buf()));
        let hook = {
            let mut hooks = CREATED_HOOKS.lock().unwrap();
            hooks
                .iter()
                .position(|(target, _)| target == path)
                .map(|index| hooks.remove(index).1)
        };
        if let Some(hook) = hook {
            hook();
        }
    }

    pub(super) fn syncing(path: &Path) {
        let hook = TRACE.with_borrow_mut(|trace| {
            if trace
                .syncing
                .as_ref()
                .is_some_and(|(target, _)| target == path)
            {
                trace.syncing.take().map(|(_, hook)| hook)
            } else {
                None
            }
        });
        if let Some(hook) = hook {
            hook();
        }
    }

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
        syncing: Option<(PathBuf, Hook)>,
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
                fail: fail.map(Path::to_path_buf),
                ..Trace::default()
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
