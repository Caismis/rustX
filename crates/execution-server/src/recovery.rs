//! Durable admission fence for a physical binding which might outlive Server
//! memory. Only exact, confirmed binding settlement may remove this record.
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use nix::fcntl::{OFlag, openat};
use nix::sys::stat::{Mode, fstat, mkdirat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use sha2::{Digest, Sha256};

use super::binding::ConnectionOwner;

const DIRECTORY: &str = "execution-pending";

/// An exclusive durable reservation for one Session's possible physical work.
/// Dropping its descriptors retains the record.
pub struct RecoveryFence {
    owner: std::sync::Weak<ConnectionOwner>,
    file: std::fs::File,
    name: std::ffi::OsString,
    directory: std::fs::File,
}

impl RecoveryFence {
    pub(super) fn create(
        product_root: &Path,
        admission: &rustx_execution_protocol::Admission,
        owner: std::sync::Weak<ConnectionOwner>,
    ) -> io::Result<Self> {
        let root = rustx_execution_transport::open_dir_nofollow(product_root)?;
        match mkdirat(&root, DIRECTORY, Mode::from_bits_truncate(0o700)) {
            Ok(()) | Err(nix::errno::Errno::EEXIST) => (),
            Err(error) => return Err(error.into()),
        }
        let directory = std::fs::File::from(openat(
            &root,
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
        root.sync_all()?;
        let name = std::ffi::OsString::from(format!(
            "{:x}.json",
            Sha256::digest(admission.session.as_str())
        ));
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
        serde_json::to_writer(&mut file, admission)?;
        file.sync_all()?;
        directory.sync_all()?;
        Ok(RecoveryFence {
            owner,
            file,
            name,
            directory,
        })
    }
}

impl RecoveryFence {
    pub(super) fn belongs_to(&self, owner: &std::sync::Arc<ConnectionOwner>) -> bool {
        self.owner.ptr_eq(&std::sync::Arc::downgrade(owner))
    }

    pub(super) fn remove(&self) -> io::Result<()> {
        let retained = fstat(&self.file)?;
        let current = nix::sys::stat::fstatat(
            &self.directory,
            self.name.as_os_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )?;
        if current.st_dev != retained.st_dev || current.st_ino != retained.st_ino {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "execution recovery marker identity changed",
            ));
        }
        unlinkat(
            &self.directory,
            self.name.as_os_str(),
            UnlinkatFlags::NoRemoveDir,
        )?;
        self.directory.sync_all()
    }
}
