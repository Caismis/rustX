//! The local Linux execution domain is the namespace whose init is the
//! exact Client process. A retained pidfd becomes readable only after the
//! kernel finishes namespace shutdown.

use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::unistd::{geteuid, getpid};
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, sendmsg, sockopt::set_socket_passcred,
};
use rustix::process::{PidfdFlags, Signal as KernelSignal, pidfd_open, pidfd_send_signal};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd as _, BorrowedFd, OwnedFd};
use std::os::unix::{net::UnixStream, process::CommandExt as _, process::ExitStatusExt as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

mod staging;
pub use staging::{
    StagingClaim, release_mutation_staging, release_staging, reserve_mutation_staging,
    reserve_staging,
};

struct Receipt<'a> {
    directory: File,
    name: &'a OsStr,
    token: [u8; 32],
}

impl<'a> Receipt<'a> {
    fn open(path: &'a Path, token: [u8; 32]) -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags, open, openat};
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid domain receipt path");
        let name = path.file_name().ok_or_else(invalid)?;
        let parent = path
            .parent()
            .filter(|path| path.is_absolute())
            .ok_or_else(invalid)?;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut directory = File::from(open("/", flags, Mode::empty())?);
        for component in parent.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    directory = File::from(openat(&directory, name, flags, Mode::empty())?)
                }
                _ => return Err(invalid()),
            }
        }
        let identity = rustix::fs::fstat(&directory)?;
        if identity.st_uid != geteuid().as_raw() || identity.st_mode & 0o777 != 0o700 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "domain receipt directory must be private",
            ));
        }
        Ok(Self {
            directory,
            name,
            token,
        })
    }

    fn publish(&self) -> io::Result<()> {
        use rustix::fs::{
            AtFlags, Mode, OFlags, RenameFlags, fstat, openat, renameat_with, statat,
        };
        let authority = Path::new(self.name).with_extension("authority");
        let authority_fd = match openat(
            &self.directory,
            &authority,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => File::from(descriptor),
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        authority_fd.lock()?;
        let retained = fstat(&authority_fd)?;
        let named = match statat(&self.directory, &authority, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(identity) => identity,
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if (named.st_dev, named.st_ino) != (retained.st_dev, retained.st_ino) {
            return Err(io::Error::other("Client domain authority identity changed"));
        }
        let staged = Path::new(self.name).with_extension("staged-proof");
        let mut durable = File::from(openat(
            &self.directory,
            &staged,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        durable.write_all(&self.token)?;
        durable.sync_all()?;
        renameat_with(
            &self.directory,
            &staged,
            &self.directory,
            self.name,
            RenameFlags::NOREPLACE,
        )?;
        self.directory.sync_all()
    }
}

struct Scratch {
    parent: File,
    logical_path: PathBuf,
    retained: OwnedFd,
}

impl Scratch {
    fn create() -> io::Result<Self> {
        use rustix::fs::{Mode, OFlags, open, openat};
        use std::os::unix::fs::PermissionsExt as _;
        let root = std::env::temp_dir().canonicalize()?;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let parent = File::from(open(&root, flags, Mode::empty())?);
        let logical_path = tempfile::Builder::new()
            .prefix("rustx-client-domain-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .disable_cleanup(true)
            .tempdir_in(&root)?
            .keep();
        let name = logical_path.file_name().expect("private scratch name");
        let retained = openat(&parent, name, flags, Mode::empty())?;
        Ok(Self {
            parent,
            logical_path,
            retained,
        })
    }

    fn release(&self) -> io::Result<()> {
        use rustix::fs::{AtFlags, fstat, statat};
        let name = self.logical_path.file_name().expect("private scratch name");
        let exact = fstat(&self.retained)?;
        let named = statat(&self.parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
        if (named.st_dev, named.st_ino) != (exact.st_dev, exact.st_ino) {
            return Err(io::Error::other(
                "Client scratch directory identity changed",
            ));
        }
        let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", self.parent.as_raw_fd()));
        std::fs::remove_dir_all(descriptor_path.join(name))?;
        self.parent.sync_all()
    }
}

/// Runs inside the private PID namespace before the Server grants launch.
/// # Errors
/// Fails if setup is incomplete or the private launch pipe loses its owner.
pub fn launch_init(
    program: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> io::Result<()> {
    if getpid().as_raw() != 1 {
        return Err(io::Error::other(
            "Client launch requires a private PID namespace",
        ));
    }
    let init = pidfd_open(rustix::process::getpid(), PidfdFlags::empty())?;
    send_descriptor(io::stdout(), b'R', init.as_fd())?;
    drop(init);
    let mut grant = [0];
    io::stdin().read_exact(&mut grant)?;
    if grant != *b"G" {
        return Err(io::Error::other("Client launch grant was refused"));
    }
    Err(Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .exec())
}

fn send_descriptor(channel: impl AsFd, kind: u8, descriptor: BorrowedFd<'_>) -> io::Result<()> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    let descriptors = [descriptor];
    if !ancillary.push(SendAncillaryMessage::ScmRights(&descriptors))
        || sendmsg(
            channel,
            &[IoSlice::new(&[kind])],
            &mut ancillary,
            SendFlags::NOSIGNAL,
        )? != 1
    {
        return Err(io::Error::other("cannot send Client domain descriptor"));
    }
    Ok(())
}

fn namespace_init(channel: impl AsFd) -> io::Result<(OwnedFd, i32)> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1), ScmCredentials(1))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut space);
    let mut ready = [0];
    let received = recvmsg(
        channel,
        &mut [IoSliceMut::new(&mut ready)],
        &mut ancillary,
        RecvFlags::CMSG_CLOEXEC,
    )?;
    let mut descriptor = None;
    let mut sender = None;
    for message in ancillary.drain() {
        match message {
            RecvAncillaryMessage::ScmRights(mut descriptors) if descriptor.is_none() => {
                descriptor = descriptors.next();
                if descriptors.next().is_some() {
                    return Err(io::Error::other("unexpected namespace descriptors"));
                }
            }
            RecvAncillaryMessage::ScmCredentials(credentials)
                if credentials.uid.as_raw() == geteuid().as_raw() =>
            {
                sender = Some(credentials.pid.as_raw_pid());
            }
            _ => return Err(io::Error::other("unexpected namespace setup message")),
        }
    }
    match (descriptor, sender) {
        (Some(descriptor), Some(sender)) if received.bytes == 1 && ready == *b"R" => {
            Ok((descriptor, sender))
        }
        _ => Err(io::Error::other("missing Client namespace identity")),
    }
}

fn wait_for_exit(descriptor: &OwnedFd, channel: &impl AsFd) -> io::Result<()> {
    let mut sources = [
        PollFd::new(descriptor.as_fd(), PollFlags::POLLIN),
        PollFd::new(channel.as_fd(), PollFlags::POLLIN | PollFlags::POLLHUP),
    ];
    loop {
        match poll(&mut sources, PollTimeout::NONE) {
            Err(Errno::EINTR) => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        if sources[0]
            .revents()
            .is_some_and(|events| events.contains(PollFlags::POLLIN))
        {
            return Ok(());
        }
        if sources[1].any() == Some(true) {
            return request_exit(descriptor);
        }
    }
}

fn request_exit(descriptor: &OwnedFd) -> io::Result<()> {
    match pidfd_send_signal(descriptor, KernelSignal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn verify_exit(descriptor: &OwnedFd) -> io::Result<()> {
    let mut sources = [PollFd::new(descriptor.as_fd(), PollFlags::POLLIN)];
    loop {
        match poll(&mut sources, PollTimeout::NONE) {
            Ok(_)
                if sources[0]
                    .revents()
                    .is_some_and(|events| events.contains(PollFlags::POLLIN)) =>
            {
                return Ok(());
            }
            Err(Errno::EINTR) => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(io::Error::other("Client namespace exit was not confirmed")),
        }
    }
}

/// Ends the retained Linux namespace if the supervisor's private socket
/// closes. Only the independent supervisor may write settlement evidence.
///
/// # Errors
/// Returns an invalid handoff or an unconfirmed kernel exit.
pub fn watch_lease(socket: &Path, client_pid: i32) -> io::Result<()> {
    let mut channel = UnixStream::from(io::stdin().as_fd().try_clone_to_owned()?);
    let (descriptor, _) = namespace_init(&channel)?;
    let staging = staging::Staging::listen(socket, client_pid)?;
    let staging_pid = descriptor.try_clone()?;
    let stages = std::thread::Builder::new()
        .name("client-domain-staging".into())
        .spawn(move || staging.serve(&staging_pid))?;
    channel.write_all(b"W")?;
    loop {
        let mut sources = [
            PollFd::new(descriptor.as_fd(), PollFlags::POLLIN),
            PollFd::new(channel.as_fd(), PollFlags::POLLIN | PollFlags::POLLHUP),
        ];
        match poll(&mut sources, PollTimeout::NONE) {
            Err(Errno::EINTR) => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        if sources[0].any() == Some(true) {
            break;
        }
        if sources[1].any() == Some(true) {
            request_exit(&descriptor)?;
            break;
        }
    }
    verify_exit(&descriptor)?;
    stages
        .join()
        .map_err(|_| io::Error::other("Client staging settlement panicked"))?
}

fn arm_lease(
    descriptor: &OwnedFd,
    socket: &Path,
    client_pid: i32,
) -> io::Result<(UnixStream, std::process::Child)> {
    let (mut supervisor, watcher) = UnixStream::pair()?;
    supervisor.set_read_timeout(Some(Duration::from_secs(2)))?;
    set_socket_passcred(&watcher, true)?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("client-domain-lease")
        .arg(socket)
        .arg(client_pid.to_string())
        .stdin(OwnedFd::from(watcher))
        .stdout(Stdio::null())
        .spawn()?;
    let armed = send_descriptor(&supervisor, b'R', descriptor.as_fd()).and_then(|()| {
        let mut ready = [0_u8; 1];
        supervisor.read_exact(&mut ready)?;
        if ready == *b"W" {
            Ok(())
        } else {
            Err(io::Error::other(
                "invalid Client namespace lease acknowledgement",
            ))
        }
    });
    if let Err(error) = armed {
        drop(supervisor);
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok((supervisor, child))
}

fn run_in_namespace(
    receipt_path: &Path,
    scratch: &Scratch,
    program: &Path,
    arguments: impl IntoIterator<Item = OsString>,
    input: &mut io::Stdin,
) -> io::Result<ExitStatus> {
    let (mut guardian, launch) = UnixStream::pair()?;
    guardian.set_read_timeout(Some(Duration::from_secs(2)))?;
    set_socket_passcred(&guardian, true)?;
    let uid = geteuid().as_raw().to_string();
    let receipt_directory = receipt_path.parent().expect("validated receipt directory");
    let launcher_input = OwnedFd::from(launch.try_clone()?);
    let launcher_output = OwnedFd::from(launch);
    let staging_socket = scratch.logical_path.join("staging.sock");
    let mut launcher = Command::new("/usr/bin/bwrap")
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--as-pid-1",
            "--die-with-parent",
            "--uid",
        ])
        .arg(uid)
        .args(["--dev-bind", "/", "/", "--proc", "/proc", "--tmpfs"])
        .arg(receipt_directory)
        .arg("--remount-ro")
        .arg(receipt_directory)
        .arg("--")
        .arg(std::env::current_exe()?)
        .arg("client-domain-init")
        .arg(program)
        .args(arguments)
        .env("TMPDIR", &scratch.logical_path)
        .env(staging::SOCKET_ENV, &staging_socket)
        .stdin(launcher_input)
        .stdout(launcher_output)
        .spawn()?;
    let (descriptor, sender) = match namespace_init(&guardian) {
        Ok(ready) => ready,
        Err(error) => {
            // No PID descriptor means the private launch grant was never sent.
            drop(guardian);
            let status = launcher.wait()?;
            eprintln!("Client namespace setup failed: {error}");
            return Ok(status);
        }
    };
    let mut result = io::stdout().lock();
    let mut lease = None;
    let boot_and_wait = (|| -> io::Result<()> {
        // Bubblewrap's parent-death signal is useful during setup. Its
        // launcher and child run independently, so require a separate
        // acknowledged pidfd owner before admitting any Client work.
        lease = Some(arm_lease(&descriptor, &staging_socket, sender)?);
        writeln!(result, "ready {sender}")?;
        result.flush()?;
        let mut grant = [0];
        input.read_exact(&mut grant)?;
        if grant != *b"G" {
            return Err(io::Error::other("Client launch grant was refused"));
        }
        guardian.write_all(&grant)?;
        wait_for_exit(&descriptor, input)
    })();
    drop(guardian);
    if boot_and_wait.is_err() {
        request_exit(&descriptor)?;
    }
    let status = launcher.wait()?;
    // Bubblewrap owns the launcher wait. Only the exact PID1 descriptor
    // proves that all descendants and their writers have left this domain.
    verify_exit(&descriptor)?;
    if let Some((lease, mut watcher)) = lease {
        drop(lease);
        if !watcher.wait()?.success() {
            return Err(io::Error::other(
                "Client staging settlement could not be confirmed",
            ));
        }
    }
    if let Err(error) = boot_and_wait {
        eprintln!("Client namespace lease closed: {error}");
    }
    Ok(status)
}

/// Keeps independent ownership of the local Client's exact Linux PID
/// namespace. The private launch grant precedes all Client execution.
///
/// # Errors
/// Returns the first setup, supervision or durable cleanup error.
pub fn supervise(
    receipt_path: &Path,
    program: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> io::Result<()> {
    nix::sys::prctl::set_dumpable(false)?;
    let mut input = io::stdin();
    let mut token = [0; 32];
    input.read_exact(&mut token)?;
    let receipt = Receipt::open(receipt_path, token)?;
    let scratch = Scratch::create()?;
    let status = run_in_namespace(receipt_path, &scratch, program, arguments, &mut input)?;
    scratch.release()?;
    receipt.publish()?;
    let exit = status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal));
    let mut result = io::stdout().lock();
    let exit = exit.ok_or_else(|| io::Error::other("invalid Client exit state"))?;
    match writeln!(result, "settled {exit}").and_then(|()| result.flush()) {
        Ok(()) => Ok(()),
        // The durable observation remains authoritative after the Server
        // and its reporting pipe have gone away.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error),
    }
}
