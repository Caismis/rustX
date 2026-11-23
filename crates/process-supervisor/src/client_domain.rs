//! The local Linux execution domain is the namespace whose init is the
//! exact Client process. The kernel completes namespace shutdown before
//! publishing the exit of that init to its parent.

use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sched::{CloneFlags, unshare};
use nix::sys::prctl::{set_dumpable, set_pdeathsig};
use nix::sys::signal::Signal;
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, fork, getegid, geteuid, getpid};
use rustix::process::{PidfdFlags, Signal as KernelSignal, pidfd_open, pidfd_send_signal};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd as _, AsRawFd as _, OwnedFd};
use std::os::unix::{net::UnixStream, process::CommandExt as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn enter_namespace() -> io::Result<()> {
    let uid = geteuid().as_raw();
    let gid = getegid().as_raw();
    unshare(CloneFlags::CLONE_NEWUSER)?;
    std::fs::write("/proc/self/setgroups", b"deny\n")?;
    std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1\n"))?;
    std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1\n"))?;
    set_dumpable(false)?;
    unshare(CloneFlags::CLONE_NEWPID)?;
    Ok(())
}

fn prepare_client_mounts(receipt_directory: &Path) -> io::Result<()> {
    use nix::mount::{MsFlags, mount};
    unshare(CloneFlags::CLONE_NEWNS)?;
    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        None::<&str>,
    )?;
    mount(
        Some("proc"),
        "/proc",
        Some("proc"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
        None::<&str>,
    )?;
    // The original authority and proof are visible only to the trusted host.
    mount(
        Some("tmpfs"),
        receipt_directory,
        Some("tmpfs"),
        MsFlags::MS_RDONLY | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
        Some("size=4k,mode=000"),
    )?;
    Ok(())
}

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
        use rustix::fs::{Mode, OFlags, openat};
        let mut durable = File::from(openat(
            &self.directory,
            self.name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        durable.write_all(&self.token)?;
        durable.sync_all()?;
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

#[allow(unsafe_code)]
fn launch_init(gate: &mut UnixStream, command: &mut Command, receipt_directory: &Path) -> ! {
    // A lost supervisor during setup closes the private gate. After the
    // gate opens the kernel still owns the parent-death signal.
    let mut grant = [0];
    if getpid().as_raw() == 1
        && set_pdeathsig(Some(Signal::SIGKILL)).is_ok()
        && prepare_client_mounts(receipt_directory).is_ok()
        && gate.write_all(b"R").is_ok()
        && gate.read_exact(&mut grant).is_ok()
        && grant == *b"G"
    {
        let _ = command.exec();
    }
    // The process is a fork child. Skip Rust termination and retained parent IO.
    unsafe { libc::_exit(125) }
}

fn reap(init: Pid) -> io::Result<i32> {
    loop {
        match waitpid(init, None) {
            Ok(WaitStatus::Exited(observed, code)) if observed == init => return Ok(code),
            Ok(WaitStatus::Signaled(observed, signal, _)) if observed == init => {
                return Ok(128 + signal as i32);
            }
            Err(Errno::EINTR) => {}
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(io::Error::other("unexpected namespace init status")),
        }
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
            return match pidfd_send_signal(descriptor, KernelSignal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
                Err(error) => Err(error.into()),
            };
        }
    }
}

/// Runs only as a dedicated, single-threaded supervisor role. A private
/// launch grant precedes all Client execution. The final line reports the
/// reaped namespace init, after Linux has killed and reaped its descendants.
///
/// # Errors
/// Returns the first namespace, fork, supervisor transport or wait error.
#[allow(unsafe_code)]
pub fn supervise(
    receipt_path: &Path,
    program: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> io::Result<()> {
    enter_namespace()?;
    let mut input = io::stdin();
    let mut token = [0; 32];
    input.read_exact(&mut token)?;
    let receipt = Receipt::open(receipt_path, token)?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut guardian, mut launch) = UnixStream::pair()?;
    guardian.set_read_timeout(Some(Duration::from_secs(2)))?;
    let scratch = Scratch::create()?;
    command.env("TMPDIR", &scratch.logical_path);
    // This binary enters this role before creating threads. The child uses
    // only its private gate and replaces itself with the admitted Client.
    let init = match unsafe { fork() } {
        Ok(ForkResult::Parent { child }) => child,
        Ok(ForkResult::Child) => {
            drop(guardian);
            launch_init(
                &mut launch,
                &mut command,
                receipt_path.parent().expect("validated receipt directory"),
            );
        }
        Err(error) => {
            scratch.release()?;
            receipt.publish()?;
            return Err(error.into());
        }
    };
    drop(launch);
    let kernel_init = rustix::process::Pid::from_raw(init.as_raw()).expect("child PID");
    let descriptor = match pidfd_open(kernel_init, PidfdFlags::empty()) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            drop(guardian);
            reap(init)?;
            scratch.release()?;
            receipt.publish()?;
            return Err(error.into());
        }
    };
    let mut result = io::stdout().lock();
    let boot = (|| -> io::Result<()> {
        let mut ready = [0];
        guardian.read_exact(&mut ready)?;
        if ready != *b"R" {
            return Err(io::Error::other("invalid namespace setup receipt"));
        }
        writeln!(result, "ready {}", init.as_raw())?;
        result.flush()?;
        let mut grant = [0];
        input.read_exact(&mut grant)?;
        if grant != *b"G" {
            return Err(io::Error::other("Client launch grant was refused"));
        }
        guardian.write_all(&grant)
    })();
    drop(guardian);
    let wait = if boot.is_ok() {
        wait_for_exit(&descriptor, &input)
    } else {
        Ok(())
    };
    if boot.is_err() || wait.is_err() {
        let _ = pidfd_send_signal(&descriptor, KernelSignal::KILL);
    }
    let exit = reap(init)?;
    scratch.release()?;
    receipt.publish()?;
    boot?;
    wait?;
    writeln!(result, "settled {exit}")?;
    result.flush()
}
