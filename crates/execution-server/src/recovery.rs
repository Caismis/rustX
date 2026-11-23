//! Durable admission fence for a physical binding which might outlive Server
//! memory. Only exact, confirmed binding settlement may remove this record.
use std::fs::File;
use std::io::{self, Write as _};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{Mode, fstat, fstatat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use sha2::{Digest, Sha256};

use super::binding::ConnectionOwner;

#[cfg(target_os = "linux")]
mod domain;
#[cfg(target_os = "linux")]
pub use domain::LocalDomainGrant;

const DIRECTORY: &str = "execution-pending";

fn pending_directory(root: &File) -> io::Result<File> {
    let directory = File::from(openat(
        root,
        DIRECTORY,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    let metadata = directory.metadata()?;
    if metadata.uid() != nix::unistd::geteuid().as_raw() || metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe execution recovery directory",
        ));
    }
    Ok(directory)
}

fn marker_name(session: &rustx_execution_protocol::SessionId) -> std::ffi::OsString {
    format!("{:x}.json", Sha256::digest(session.as_str())).into()
}

/// An exclusive durable reservation for one Session's possible physical work.
/// Dropping its descriptors retains the record.
pub struct RecoveryFence {
    owner: std::sync::Weak<ConnectionOwner>,
    file: std::fs::File,
    name: std::ffi::OsString,
    directory: std::fs::File,
    #[cfg(target_os = "linux")]
    domain: domain::DomainKey,
}

impl RecoveryFence {
    pub(super) fn create(
        product_root: &Path,
        admission: &rustx_execution_protocol::Admission,
        owner: std::sync::Weak<ConnectionOwner>,
    ) -> io::Result<Self> {
        Self::require_session_settled(product_root, &admission.session).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(io::ErrorKind::AlreadyExists, error)
            } else {
                error
            }
        })?;
        let root = rustx_execution_transport::open_dir_nofollow(product_root)?;
        match mkdirat(&root, DIRECTORY, Mode::from_bits_truncate(0o700)) {
            Ok(()) | Err(nix::errno::Errno::EEXIST) => (),
            Err(error) => return Err(error.into()),
        }
        let directory = pending_directory(&root)?;
        root.sync_all()?;
        let name = marker_name(&admission.session);
        let mut file = std::fs::File::from(
            openat(
                &directory,
                name.as_os_str(),
                OFlag::O_WRONLY
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|error| {
                if error == nix::errno::Errno::EEXIST {
                    io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "previous Session execution domain requires physical settlement",
                    )
                } else {
                    error.into()
                }
            })?,
        );
        // Preserve an unknown marker on I/O failure. A fresh Server must
        // fail closed even if the record could not be fully encoded.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        let bytes = serde_json::to_vec(admission)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        directory.sync_all()?;
        Ok(RecoveryFence {
            owner,
            file,
            name,
            directory,
            #[cfg(target_os = "linux")]
            domain: domain::DomainKey::new(product_root.join(DIRECTORY), &bytes),
        })
    }

    /// Checks the durable fence before destructive cleanup of a deleted Session.
    /// The caller must already have durably closed Session admission.
    ///
    /// # Errors
    /// Fails while a possible physical owner remains or the trusted directory
    /// cannot be checked. Even an unreadable marker retains the fence.
    pub fn require_session_settled(
        product_root: &Path,
        session: &rustx_execution_protocol::SessionId,
    ) -> io::Result<()> {
        let root = rustx_execution_transport::open_dir_nofollow(product_root)?;
        let directory = match pending_directory(&root) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        match fstatat(
            &directory,
            marker_name(session).as_os_str(),
            AtFlags::AT_SYMLINK_NOFOLLOW,
        ) {
            Err(nix::errno::Errno::ENOENT) => directory.sync_all(),
            Err(error) => Err(error.into()),
            Ok(_) => {
                #[cfg(target_os = "linux")]
                if domain::reconcile(&directory, session)? {
                    return Ok(());
                }
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "previous Session execution domain requires physical settlement",
                ))
            }
        }
    }
}

impl RecoveryFence {
    /// Arms an independent trusted Linux host before granting Client START.
    /// The durable store retains the verifier. Send the token only to the
    /// helper through its private inherited pipe.
    /// # Errors
    /// Refuses an unsafe, already armed or no longer owned marker.
    #[cfg(target_os = "linux")]
    pub fn arm_local_domain(&self) -> io::Result<LocalDomainGrant> {
        self.verify_marker()?;
        self.domain.arm(&self.directory)
    }

    pub(super) fn belongs_to(&self, owner: &std::sync::Arc<ConnectionOwner>) -> bool {
        self.owner.ptr_eq(&std::sync::Arc::downgrade(owner))
    }

    fn verify_marker(&self) -> io::Result<()> {
        let retained = fstat(&self.file)?;
        let current = fstatat(
            &self.directory,
            self.name.as_os_str(),
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )?;
        if current.st_dev != retained.st_dev || current.st_ino != retained.st_ino {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "execution recovery marker identity changed",
            ));
        }
        Ok(())
    }

    pub(super) fn remove(&self) -> io::Result<()> {
        self.verify_marker()?;
        unlinkat(
            &self.directory,
            self.name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        )?;
        self.directory.sync_all()?;
        #[cfg(target_os = "linux")]
        self.domain.cleanup(&self.directory)?;
        Ok(())
    }
}
