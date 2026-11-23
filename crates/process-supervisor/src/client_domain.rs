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
use std::os::fd::{AsFd as _, AsRawFd as _, OwnedFd};
use std::os::unix::{net::UnixStream, process::CommandExt as _, process::ExitStatusExt as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

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
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    let descriptors = [init.as_fd()];
    if !ancillary.push(SendAncillaryMessage::ScmRights(&descriptors))
        || sendmsg(
            io::stdout(),
            &[IoSlice::new(b"R")],
            &mut ancillary,
            SendFlags::NOSIGNAL,
        )? != 1
    {
        return Err(io::Error::other("cannot send Client namespace descriptor"));
    }
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

fn namespace_init(channel: &UnixStream) -> io::Result<(OwnedFd, i32)> {
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

fn wait_for_exit(descriptor: &OwnedFd, channel: &io::Stdin) -> io::Result<()> {
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
    let boot_and_wait = (|| -> io::Result<()> {
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
    writeln!(
        result,
        "settled {}",
        exit.ok_or_else(|| io::Error::other("invalid Client exit state"))?
    )?;
    result.flush()
}
