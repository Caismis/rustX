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
    domain: domain::DomainKey,
}

impl RecoveryFence {
    /// Observes a previous execution claim without consuming settlement or
    /// modifying the trusted product root.
    ///
    /// # Errors
    /// Fails when the trusted record directory cannot be inspected.
    pub fn session_pending(
        product_root: &Path,
        session: &rustx_execution_protocol::SessionId,
    ) -> io::Result<bool> {
        let root = rustx_execution_transport::open_dir_nofollow(product_root)?;
        let directory = match pending_directory(&root) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        match fstatat(
            &directory,
            marker_name(session).as_os_str(),
            AtFlags::AT_SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => Ok(true),
            Err(nix::errno::Errno::ENOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

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
    /// Create one private grant for a separately trusted deployment host.
    /// The grant proves only the exact reservation and is withheld from the
    /// execution transport and its Client bootstrap.
    ///
    /// # Errors
    /// Rejects unsafe destinations, conflicting files or a lost reservation.
    pub fn publish_external_grant(&self, path: &Path) -> io::Result<()> {
        use base64::Engine as _;

        #[derive(serde::Serialize)]
        struct Grant<'a> {
            receipt_path: &'a Path,
            proof_base64: String,
        }

        let parent = super::bootstrap::require_private_child(path)?;
        self.file.lock()?;
        let result = (|| {
            self.verify_marker()?;
            let grant = self.domain.arm(&self.directory)?;
            let publication = Grant {
                receipt_path: grant.receipt(),
                proof_base64: base64::engine::general_purpose::STANDARD.encode(grant.token()),
            };
            super::bootstrap::publish_private(&parent, |file| {
                serde_json::to_writer(file, &publication).map_err(io::Error::other)
            })?;
            Ok(())
        })();
        let unlock = self.file.unlock();
        result.and(unlock)
    }

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

    #[cfg(target_os = "linux")]
    pub(super) fn release_independent_domain(&self) -> io::Result<()> {
        self.file.lock()?;
        let result = (|| {
            self.verify_marker()?;
            if !self.domain.proved(&self.directory)? {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "previous Client namespace settlement is unconfirmed",
                ));
            }
            self.remove_locked()
        })();
        let unlock = self.file.unlock();
        result.and(unlock)
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
        self.file.lock()?;
        let result = self.remove_locked();
        let unlock = self.file.unlock();
        result.and(unlock)
    }

    fn remove_locked(&self) -> io::Result<()> {
        self.verify_marker()?;
        unlinkat(
            &self.directory,
            self.name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        )?;
        self.directory.sync_all()?;
        self.domain.cleanup(&self.directory)?;
        Ok(())
    }
}
