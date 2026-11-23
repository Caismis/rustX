//! Anchored staging roots allocated on demand by the local domain watcher.

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
use nix::unistd::geteuid;
use rustix::fs::{AtFlags, FileType, Mode, OFlags, fstat, openat, statat};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
use std::fs::File;
use std::io::{self, IoSliceMut, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd as _, AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::{
    fs::PermissionsExt as _,
    net::{UnixListener, UnixStream},
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const PREFIX: &str = ".rustx-domain-stage-";
const MAX_LEAF_BYTES: usize = 64;
const STAGING_REQUEST: u8 = b'S';
const RELEASE_REQUEST: u8 = b'D';
const MUTATION_REQUEST: u8 = b'M';
const MUTATION_RELEASE: u8 = b'F';
pub(super) const SOCKET_ENV: &str = "RUSTX_CLIENT_DOMAIN_STAGING";
static STAGING_REQUESTED: AtomicBool = AtomicBool::new(false);

pub(super) struct Staging {
    listener: UnixListener,
    python: Option<Owned>,
    mutation: Option<Owned>,
    client_pid: i32,
    closed: bool,
}

impl Staging {
    pub(super) fn listen(path: &Path, client_pid: i32) -> io::Result<Self> {
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            python: None,
            mutation: None,
            client_pid,
            closed: false,
        })
    }

    pub(super) fn descriptor(&self) -> BorrowedFd<'_> {
        self.listener.as_fd()
    }

    pub(super) fn receive(&mut self, client: &OwnedFd) -> io::Result<()> {
        let mut channel = match self.listener.accept() {
            Ok((channel, _)) => channel,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        };
        let credentials = getsockopt(&channel, PeerCredentials)?;
        if credentials.pid() != self.client_pid || credentials.uid() != geteuid().as_raw() {
            return Ok(());
        }
        // Peer credentials are captured before the pinned liveness check.
        // The numeric PID cannot name a successor while this pidfd is live.
        let mut exit = [PollFd::new(client.as_fd(), PollFlags::POLLIN)];
        if poll(&mut exit, PollTimeout::ZERO)? != 0 {
            return Ok(());
        }
        set_timeout(&channel, Duration::from_secs(2))?;
        let (owner, parent) = match receive_request(&channel)? {
            Request::Python(parent) => (&mut self.python, parent),
            Request::Mutation(parent) => (&mut self.mutation, parent),
            Request::ReleaseMutation => {
                remove_owner(&mut self.mutation)?;
                return channel.write_all(&[MUTATION_RELEASE]);
            }
            Request::Close => {
                self.closed = true;
                self.remove_all()?;
                return channel.write_all(&[RELEASE_REQUEST]);
            }
        };

        if self.closed {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Client staging admission is closed",
            ));
        }
        let owned = match owner {
            Some(owned) if owned.same_parent(&parent)? => owned,
            Some(_) => return Err(io::Error::other("Client staging parent changed")),
            None => owner.insert(Owned::create(parent)?),
        };
        owned.verify()?;
        let name = owned.name.as_bytes();
        let count = u8::try_from(name.len()).expect("bounded staging name");
        let retained = fstat(&owned.directory)?;
        channel.write_all(&[count])?;
        channel.write_all(&retained.st_dev.to_be_bytes())?;
        channel.write_all(&retained.st_ino.to_be_bytes())?;
        channel.write_all(name)
    }

    fn remove_all(&mut self) -> io::Result<()> {
        let python = remove_owner(&mut self.python);
        let mutation = remove_owner(&mut self.mutation);
        python.and(mutation)
    }

    pub(super) fn release(mut self) -> io::Result<()> {
        self.remove_all()
    }
}

fn remove_owner(owner: &mut Option<Owned>) -> io::Result<()> {
    if let Some(retained) = owner {
        retained.release()?;
        *owner = None;
    }
    Ok(())
}

struct Owned {
    parent: File,
    name: String,
    directory: OwnedFd,
}

impl Owned {
    fn create(parent: File) -> io::Result<Self> {
        let status = fstat(&parent)?;
        if FileType::from_raw_mode(status.st_mode) != FileType::Directory {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Client staging requires a destination directory",
            ));
        }
        let path = tempfile::Builder::new()
            .prefix(PREFIX)
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(anchored_path(&parent))?;
        let name = path
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| valid_name(name))
            .ok_or_else(|| io::Error::other("unexpected staging directory name"))?
            .to_owned();
        let directory = openat(
            &parent,
            name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let _ = path.keep();
        Ok(Self {
            parent,
            name,
            directory,
        })
    }

    fn same_parent(&self, parent: &File) -> io::Result<bool> {
        let existing = fstat(&self.parent)?;
        let requested = fstat(parent)?;
        Ok((existing.st_dev, existing.st_ino) == (requested.st_dev, requested.st_ino))
    }

    fn verify(&self) -> io::Result<()> {
        let retained = fstat(&self.directory)?;
        let named = statat(&self.parent, self.name.as_str(), AtFlags::SYMLINK_NOFOLLOW)?;
        if (named.st_dev, named.st_ino) == (retained.st_dev, retained.st_ino) {
            Ok(())
        } else {
            Err(io::Error::other(
                "Client staging directory identity changed",
            ))
        }
    }

    fn release(&self) -> io::Result<()> {
        if fstat(&self.directory)?.st_nlink == 0 {
            return self.parent.sync_all();
        }
        self.verify()?;
        std::fs::remove_dir_all(anchored_path(&self.parent).join(&self.name))?;
        self.parent.sync_all()
    }
}

fn anchored_path(parent: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()))
}

fn valid_name(name: &str) -> bool {
    name.len() <= MAX_LEAF_BYTES
        && name.strip_prefix(PREFIX).is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn set_timeout(channel: &UnixStream, timeout: Duration) -> io::Result<()> {
    if timeout.is_zero() {
        return Err(io::ErrorKind::TimedOut.into());
    }
    channel.set_read_timeout(Some(timeout))?;
    channel.set_write_timeout(Some(timeout))
}

enum Request {
    Python(File),
    Mutation(File),
    ReleaseMutation,
    Close,
}

fn receive_request(channel: &UnixStream) -> io::Result<Request> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut space);
    let mut kind = [0];
    let received = recvmsg(
        channel,
        &mut [IoSliceMut::new(&mut kind)],
        &mut ancillary,
        RecvFlags::CMSG_CLOEXEC,
    )?;
    let mut messages = ancillary.drain();
    let invalid = io::Error::other("invalid Client staging request");
    if received.bytes != 1 {
        return Err(invalid);
    }
    match (kind[0], messages.next()) {
        (RELEASE_REQUEST, None) => Ok(Request::Close),
        (MUTATION_RELEASE, None) => Ok(Request::ReleaseMutation),
        (
            kind @ (STAGING_REQUEST | MUTATION_REQUEST),
            Some(RecvAncillaryMessage::ScmRights(mut files)),
        ) => {
            let parent = files.next().ok_or(invalid)?;
            if files.next().is_some() || messages.next().is_some() {
                return Err(io::Error::other("unexpected Client staging descriptors"));
            }
            let parent = File::from(parent);
            Ok(if kind == STAGING_REQUEST {
                Request::Python(parent)
            } else {
                Request::Mutation(parent)
            })
        }
        _ => Err(invalid),
    }
}

fn connect(timeout: Duration) -> io::Result<Option<UnixStream>> {
    let Some(path) = std::env::var_os(SOCKET_ENV) else {
        return Ok(None);
    };
    let channel = UnixStream::connect(path)?;
    if getsockopt(&channel, PeerCredentials)?.uid() != geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid Client staging authority",
        ));
    }
    set_timeout(&channel, timeout)?;
    Ok(Some(channel))
}

/// Resolve the native Client's supervised staging root, if the local
/// launch provided its private socket. Remote Clients use their own Store.
///
/// # Errors
/// Returns invalid authority, timeout or allocation failures.
pub fn reserve_staging(parent: BorrowedFd<'_>) -> io::Result<Option<StagingClaim>> {
    reserve(STAGING_REQUEST, parent)
}

/// Reserve one mutation staging root under the retained destination parent.
///
/// # Errors
/// Returns unavailable or invalid staging authority and allocation failures.
pub fn reserve_mutation_staging(parent: BorrowedFd<'_>) -> io::Result<Option<StagingClaim>> {
    reserve(MUTATION_REQUEST, parent)
}

fn reserve(kind: u8, parent: BorrowedFd<'_>) -> io::Result<Option<StagingClaim>> {
    let Some(mut channel) = connect(Duration::from_secs(2))? else {
        return Ok(None);
    };
    // A missing response still leaves allocation responsibility with the
    // requested owner, so terminal shutdown must reconcile it.
    STAGING_REQUESTED.store(true, Ordering::Release);
    super::send_descriptor(&channel, kind, parent)?;
    let mut count = [0_u8];
    channel.read_exact(&mut count)?;
    let length = usize::from(count[0]);
    if length > MAX_LEAF_BYTES {
        return Err(io::Error::other("invalid Client staging name size"));
    }
    let mut device = [0_u8; 8];
    let mut inode = [0_u8; 8];
    channel.read_exact(&mut device)?;
    channel.read_exact(&mut inode)?;
    let mut name = [0; MAX_LEAF_BYTES];
    channel.read_exact(&mut name[..length])?;
    let name = std::str::from_utf8(&name[..length]).map_err(io::Error::other)?;
    if !valid_name(name) {
        return Err(io::Error::other("invalid Client staging name"));
    }
    Ok(Some(StagingClaim {
        name: name.to_owned(),
        device: u64::from_be_bytes(device),
        inode: u64::from_be_bytes(inode),
    }))
}

/// The identity acknowledged by the retained domain watcher.
pub struct StagingClaim {
    /// The single-component path beneath the requested parent.
    pub name: String,
    device: u64,
    inode: u64,
}

impl StagingClaim {
    /// Verify the caller opened the exact acknowledged directory.
    ///
    /// # Errors
    /// Returns an identity mismatch or an unreadable descriptor.
    pub fn verify(&self, directory: &impl std::os::fd::AsFd) -> io::Result<()> {
        let status = fstat(directory)?;
        if (status.st_dev, status.st_ino) == (self.device, self.inode) {
            Ok(())
        } else {
            Err(io::Error::other(
                "Client staging identity differs from its acknowledged owner",
            ))
        }
    }
}

/// Close staging admission and confirm removal during authenticated Revoke.
///
/// # Errors
/// Returns an invalid or missing acknowledgement within the caller's cut.
pub fn release_staging(deadline: Instant) -> io::Result<()> {
    if !STAGING_REQUESTED.load(Ordering::Acquire) {
        return Ok(());
    }
    send_release(RELEASE_REQUEST, deadline)
}

/// Release the stage after one mutation has settled.
///
/// # Errors
/// Returns unconfirmed removal or loss of the acknowledged owner.
pub fn release_mutation_staging(deadline: Instant) -> io::Result<()> {
    send_release(MUTATION_RELEASE, deadline)
}

fn send_release(request: u8, deadline: Instant) -> io::Result<()> {
    let remaining = || {
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(2))
    };
    let Some(mut channel) = connect(remaining())? else {
        return Ok(());
    };
    channel.write_all(&[request])?;
    set_timeout(&channel, remaining())?;
    let mut reply = [0_u8; 1];
    channel.read_exact(&mut reply)?;
    if reply == [request] {
        Ok(())
    } else {
        Err(io::Error::other(
            "invalid Client staging release acknowledgement",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_reap_preserves_a_replacement_and_reports_unknown() {
        let root = tempfile::tempdir().unwrap();
        let owned = Owned::create(File::open(root.path()).unwrap()).unwrap();
        let selected = root.path().join(&owned.name);
        let moved = root.path().join("moved");
        std::fs::write(selected.join("stage"), b"private stage").unwrap();
        std::fs::rename(&selected, &moved).unwrap();
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("replacement"), b"keep").unwrap();
        assert!(owned.release().is_err());
        assert_eq!(
            std::fs::read(moved.join("stage")).unwrap(),
            b"private stage"
        );
        assert_eq!(
            std::fs::read(selected.join("replacement")).unwrap(),
            b"keep"
        );

        let replacement = root.path().join("replacement");
        std::fs::rename(&selected, &replacement).unwrap();
        std::fs::rename(&moved, &selected).unwrap();
        owned.release().unwrap();
        assert!(!selected.exists());
        assert_eq!(
            std::fs::read(replacement.join("replacement")).unwrap(),
            b"keep"
        );
    }
}
