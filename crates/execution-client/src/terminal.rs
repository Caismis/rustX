//! The Client-owned physical terminal for PTY process operations.
//!
//! Ported from the Server's retired local command runner: both endpoints
//! are allocated close-on-exec atomically, reads and writes are
//! readiness-driven and nonblocking so a full peer pipe never stalls the
//! drain, resize is a real `TIOCSWINSZ`, and `eof` composes the canonical
//! EOT sequence under the live termios policy.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::OwnedFd;
use std::path::PathBuf;

use rustx_execution_protocol::process::PtySpec;

/// One allocated pseudoterminal: the nonblocking master the worker
/// selects on, the slave descriptor handed to the supervisor's
/// invocation, and the slave path the invocation advertises.
pub(crate) struct Terminal {
    pub(crate) master: tokio::io::unix::AsyncFd<OwnedFd>,
    pub(crate) slave: Option<OwnedFd>,
    pub(crate) path: PathBuf,
}

impl Terminal {
    /// Allocate a terminal pair sized to `size`.
    ///
    /// # Errors
    /// Any `ptmx`/`grantpt`/`unlockpt`/slave-open failure before a
    /// business spawn — never a fabricated terminal.
    pub(crate) fn open(size: PtySpec) -> std::io::Result<Self> {
        use rustix::fs::{Mode, OFlags, open};
        // Allocate both endpoints close-on-exec atomically. openpty followed by
        // fcntl leaves a window for another command's concurrent spawn to inherit
        // the master/slave and keep an unrelated terminal alive.
        let master = open(
            "/dev/ptmx",
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?;
        rustix::pty::grantpt(&master)?;
        rustix::pty::unlockpt(&master)?;
        let name = rustix::pty::ptsname(&master, Vec::new())?;
        let path = PathBuf::from(OsStr::from_bytes(name.to_bytes()));
        let slave = open(
            &path,
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let terminal = Self {
            master: tokio::io::unix::AsyncFd::new(master)?,
            slave: Some(slave),
            path,
        };
        terminal.resize(size)?;
        Ok(terminal)
    }

    /// One readiness-driven nonblocking read; `EIO` on a PTY master is the
    /// physical end-of-output, not an error.
    pub(crate) async fn read(&self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let mut ready = self.master.readable().await?;
            match ready
                .try_io(|fd| rustix::io::read(fd.get_ref(), &mut *buffer).map_err(Into::into))
            {
                Ok(Err(error)) if error.raw_os_error() == Some(libc::EIO) => return Ok(0),
                Ok(result) => return result,
                Err(_) => {}
            }
        }
    }

    /// One readiness-driven nonblocking write: returns the byte count the
    /// kernel actually accepted; never blocks the worker on a full peer.
    pub(crate) async fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            let mut ready = self.master.writable().await?;
            if let Ok(result) =
                ready.try_io(|fd| rustix::io::write(fd.get_ref(), bytes).map_err(Into::into))
            {
                let count = result?;
                if count == 0 {
                    return Err(std::io::ErrorKind::WriteZero.into());
                }
                return Ok(count);
            }
        }
    }

    /// Apply the terminal's new geometry.
    ///
    /// # Errors
    /// The `TIOCSWINSZ` ioctl failed.
    pub(crate) fn resize(&self, size: PtySpec) -> std::io::Result<()> {
        rustix::termios::tcsetwinsize(
            self.master.get_ref(),
            rustix::termios::Winsize {
                ws_row: size.rows,
                ws_col: size.cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )?;
        Ok(())
    }

    /// The canonical end-of-input bytes for this terminal under its live
    /// termios policy — `Err` when the policy makes a byte-delivered EOF
    /// ambiguous rather than composing a wrong sequence.
    ///
    /// # Errors
    /// The termios policy cannot express an unambiguous EOT.
    pub(crate) fn eof(&self) -> std::io::Result<[u8; 3]> {
        use nix::sys::termios::{InputFlags, LocalFlags, SpecialCharacterIndices as C, tcgetattr};
        let attributes = tcgetattr(self.master.get_ref())?;
        let eof = attributes.control_chars[C::VEOF as usize];
        let conflicts = |characters: &[C]| {
            characters
                .iter()
                .any(|character| attributes.control_chars[*character as usize] == eof)
        };
        #[cfg(target_os = "macos")]
        let platform_conflict = attributes
            .local_flags
            .intersects(LocalFlags::ISIG | LocalFlags::IEXTEN)
            && conflicts(&[C::VDSUSP, C::VSTATUS]);
        #[cfg(target_os = "linux")]
        let platform_conflict =
            eof.is_ascii_uppercase() && attributes.input_flags.contains(InputFlags::IUCLC);
        if !attributes.local_flags.contains(LocalFlags::ICANON)
            || attributes.local_flags.contains(LocalFlags::EXTPROC)
            || platform_conflict
            || eof == libc::_POSIX_VDISABLE
            || eof == b'\n'
            || eof == b'\r'
                && attributes
                    .input_flags
                    .intersects(InputFlags::ICRNL | InputFlags::IGNCR)
            || eof >= 128 && attributes.input_flags.contains(InputFlags::ISTRIP)
            || conflicts(&[C::VERASE, C::VKILL, C::VEOL, C::VEOL2])
            || attributes.local_flags.contains(LocalFlags::ISIG)
                && conflicts(&[C::VINTR, C::VQUIT, C::VSUSP])
            || attributes.local_flags.contains(LocalFlags::IEXTEN)
                && conflicts(&[C::VLNEXT, C::VWERASE, C::VREPRINT, C::VDISCARD])
            || attributes.input_flags.contains(InputFlags::IXON)
                && conflicts(&[C::VSTART, C::VSTOP])
        {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        // Pending VLNEXT may quote the first byte. A further VEOF releases a
        // pending canonical line, then the final VEOF ends the following read.
        Ok([eof; 3])
    }
}
