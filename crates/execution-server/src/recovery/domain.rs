//! Independent execution-domain settlement from a trusted physical owner.
//! The durable scope is the exact Admission, including its original incarnation.

use super::marker_name;
use constant_time_eq::constant_time_eq_32;
use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{Mode, fstat, fstatat};
use nix::unistd::{UnlinkatFlags, geteuid, unlinkat};
use rustx_execution_protocol::{Admission, SessionId};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

/// The local-only bootstrap of a single independent physical domain. The
/// bytes are private until the trusted helper proves quiescence and cleanup.
pub struct LocalDomainGrant {
    receipt: PathBuf,
    token: [u8; 32],
}

impl LocalDomainGrant {
    /// The exact receipt path in the protected physical ownership directory.
    #[must_use]
    pub fn receipt(&self) -> &Path {
        &self.receipt
    }

    /// The private capability sent through the helper's inherited stdin.
    #[must_use]
    pub fn token(&self) -> &[u8; 32] {
        &self.token
    }
}

pub(super) struct DomainKey {
    parent: PathBuf,
    stem: String,
}

impl DomainKey {
    pub(super) fn new(parent: PathBuf, admission_bytes: &[u8]) -> Self {
        Self {
            parent,
            stem: format!("linux-{:x}", Sha256::digest(admission_bytes)),
        }
    }

    fn verifier_name(&self) -> String {
        format!("{}.authority", self.stem)
    }

    fn receipt_name(&self) -> String {
        format!("{}.proof", self.stem)
    }

    pub(super) fn arm(&self, directory: &File) -> io::Result<LocalDomainGrant> {
        let mut token = [0; 32];
        getrandom::fill(&mut token).map_err(io::Error::other)?;
        let mut authority = File::from(openat(
            directory,
            self.verifier_name().as_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?);
        authority.write_all(&Sha256::digest(token))?;
        authority.sync_all()?;
        directory.sync_all()?;
        Ok(LocalDomainGrant {
            receipt: self.parent.join(self.receipt_name()),
            token,
        })
    }

    pub(super) fn proved(&self, directory: &File) -> io::Result<bool> {
        let (Some(verifier), Some(receipt)) = (
            private_file(directory, self.verifier_name().as_str())?,
            private_file(directory, self.receipt_name().as_str())?,
        ) else {
            return Ok(false);
        };
        let Some(expected) = exact_token(verifier)? else {
            return Ok(false);
        };
        let Some(actual) = exact_token(receipt)? else {
            return Ok(false);
        };
        Ok(constant_time_eq_32(
            &expected,
            &Sha256::digest(actual).into(),
        ))
    }

    pub(super) fn cleanup(&self, directory: &File) -> io::Result<()> {
        let authority = private_file(directory, self.verifier_name().as_str())?;
        if let Some(authority) = &authority {
            authority.lock()?;
        }
        for name in [
            self.verifier_name(),
            self.receipt_name(),
            format!("{}.staged-proof", self.stem),
        ] {
            match unlinkat(directory, name.as_str(), UnlinkatFlags::NoRemoveDir) {
                Ok(()) | Err(nix::errno::Errno::ENOENT) => {}
                Err(error) => return Err(error.into()),
            }
        }
        directory.sync_all()
    }
}

fn exact_token(file: File) -> io::Result<Option<[u8; 32]>> {
    let mut bytes = Vec::new();
    file.take(33).read_to_end(&mut bytes)?;
    Ok(bytes.try_into().ok())
}

fn private_file(directory: &File, name: &(impl nix::NixPath + ?Sized)) -> io::Result<Option<File>> {
    let flags = OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK;
    let file = match openat(directory, name, flags, Mode::empty()) {
        Ok(file) => File::from(file),
        Err(nix::errno::Errno::ENOENT | nix::errno::Errno::ELOOP) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let identity = file.metadata()?;
    Ok((identity.is_file()
        && identity.uid() == geteuid().as_raw()
        && identity.mode() & 0o777 == 0o600)
        .then_some(file))
}

pub(super) fn reconcile(directory: &File, session: &SessionId) -> io::Result<bool> {
    let name = marker_name(session);
    let Some(mut marker) = private_file(directory, name.as_os_str())? else {
        return Ok(false);
    };
    marker.lock()?;
    let mut bytes = Vec::new();
    (&mut marker).take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192
        || !serde_json::from_slice::<Admission>(&bytes).is_ok_and(|owner| owner.session == *session)
    {
        return Ok(false);
    }
    let domain = DomainKey::new(PathBuf::new(), &bytes);
    if !domain.proved(directory)? {
        return Ok(false);
    }
    let retained = fstat(&marker)?;
    let current = match fstatat(directory, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(identity) => identity,
        Err(nix::errno::Errno::ENOENT) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if (current.st_dev, current.st_ino) != (retained.st_dev, retained.st_ino) {
        return Ok(false);
    }
    unlinkat(directory, name.as_os_str(), UnlinkatFlags::NoRemoveDir)?;
    directory.sync_all()?;
    domain.cleanup(directory)?;
    Ok(true)
}
