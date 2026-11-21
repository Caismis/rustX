//! The TCP mirror of the proven Unix `DeadlineStream`: every `Read` and
//! `Write` waits for readiness against one caller-armed absolute
//! `Instant`, `EINTR` retries without rearming, and the deadline is
//! checked again after every completed I/O.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

/// The shared armed deadline. `None` means unbounded blocking — the same
/// contract the Unix `read_record`/`write_record` call sites already use.
/// It is shared because the TLS layer owns the concrete stream; each
/// channel call re-arms it before invoking tungstenite, so a ping or
/// fragment can never outlive the caller's budget.
pub(crate) type ArmedDeadline = Arc<Mutex<Option<Instant>>>;

pub(crate) fn armed_deadline() -> ArmedDeadline {
    Arc::new(Mutex::new(None))
}

pub(crate) fn arm(deadline: &ArmedDeadline, armed: Option<Instant>) {
    *deadline.lock().unwrap_or_else(PoisonError::into_inner) = armed;
}

pub(crate) fn remaining(deadline: &ArmedDeadline) -> io::Result<Option<std::time::Duration>> {
    let armed = *deadline.lock().unwrap_or_else(PoisonError::into_inner);
    match armed {
        Some(deadline) => {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "execution record deadline expired",
                ))
            } else {
                Ok(Some(left))
            }
        }
        None => Ok(None),
    }
}

/// A nonblocking `TcpStream` whose every I/O waits for readiness under the
/// armed deadline. It never blocks an OS thread on the socket itself —
/// readiness is established with `poll`, matching `DeadlineStream`
/// byte-for-byte, so revocation that shuts the raw socket down wakes any
/// in-flight wait.
pub(crate) struct DeadlineTcp {
    stream: TcpStream,
    deadline: ArmedDeadline,
}

impl DeadlineTcp {
    pub(crate) fn new(stream: TcpStream, deadline: ArmedDeadline) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self { stream, deadline })
    }

    fn check(&self) -> io::Result<()> {
        remaining(&self.deadline).map(|_| ())
    }

    fn wait(&self, events: PollFlags) -> io::Result<()> {
        let fd = self.stream.as_fd();
        let mut fds = [PollFd::new(fd, events)];
        loop {
            // Recomputed on every retry: EINTR and spurious readiness share
            // the caller's single absolute budget instead of rearming it.
            let timeout = match remaining(&self.deadline)? {
                Some(budget) => PollTimeout::try_from(budget).unwrap_or(PollTimeout::MAX),
                None => PollTimeout::NONE,
            };
            match poll(&mut fds, timeout) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "execution record deadline expired",
                    ));
                }
                Ok(_) => {
                    if let Some(flags) = fds[0].revents()
                        && flags.intersects(events | PollFlags::POLLERR | PollFlags::POLLHUP)
                    {
                        return self.check();
                    }
                }
                Err(Errno::EINTR) => {}
                Err(errno) => return Err(io::Error::from(errno)),
            }
        }
    }
}

impl Read for DeadlineTcp {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.stream.read(buffer) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait(PollFlags::POLLIN)?;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => return result.and_then(|count| self.check().map(|()| count)),
            }
        }
    }
}

impl Write for DeadlineTcp {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.stream.write(buffer) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait(PollFlags::POLLOUT)?;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => return result.and_then(|count| self.check().map(|()| count)),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}
