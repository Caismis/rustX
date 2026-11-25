//! Descriptor-owned bootstrap publication and cleanup.

use std::{
    io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

use rustx_execution_protocol::{
    BindingCredential, Bootstrap, BootstrapEndpoint, PROTOCOL_VERSION, is_normalized_absolute_path,
    transport::write_record,
};

use super::{Binding, invalid_input, server_incarnation};

/// The retained proof of one published bootstrap bundle: the open file
/// descriptor from `create_new`, the private parent directory descriptor,
/// and the file name. Teardown compares `fstatat(AT_SYMLINK_NOFOLLOW)`
/// against this proof and unlinks only when device and inode still match
/// — a replaced, symlinked, FIFO, or identical-bytes foreign file is
/// preserved, and no pathname is ever reopened.
pub struct PublishedBundle {
    file: std::fs::File,
    parent: PrivateParent,
}

impl PublishedBundle {
    /// Removes the bundle only while it is still provably this process's
    /// file. An `Err` from `stat_child` — already unlinked, or the parent
    /// renamed away — just preserves whatever is there.
    pub fn remove(&self) {
        let Ok(proof) = nix::sys::stat::fstat(&self.file) else {
            return;
        };
        self.parent.remove_if(&proof);
    }
}

/// Writes the bootstrap bundle as exactly one framed record into a
/// freshly created owner-only file inside `parent` — `O_CREAT|O_EXCL`
/// through the verified directory descriptor, so the pathname is never
/// re-resolved.
///
/// If `create_new` refuses an existing path, nothing is touched. Once the
/// file exists and is ours, any later failure removes it by the same
/// dev/inode proof teardown uses — failed publication never leaves a
/// partial bootstrap behind and never deletes a foreign replacement.
pub(super) fn publish_bootstrap(
    endpoint: BootstrapEndpoint,
    parent: &PrivateParent,
    binding: &Binding,
    credential: BindingCredential,
) -> io::Result<PublishedBundle> {
    let reservation = &binding.reservation;
    let bootstrap = Bootstrap {
        endpoint,
        version: PROTOCOL_VERSION,
        server_incarnation: server_incarnation(),
        session: reservation.session.clone(),
        environment: reservation.environment.clone(),
        instance: reservation.instance.clone(),
        generation: reservation.generation,
        workspace: reservation.workspace.clone(),
        credential,
    };
    publish_private(parent, |file| write_record(file, &bootstrap))
}

/// Publish one secret to a new file beneath the already validated parent.
pub(super) fn publish_private(
    parent: &PrivateParent,
    write: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<PublishedBundle> {
    // The parent descriptor the returned proof will own is duplicated
    // before the file exists: a dup failure here fails cleanly, while a
    // late one could strand a published credential.
    let publication = PrivateParent {
        dir: parent.dir.try_clone()?,
        name: parent.name.clone(),
    };
    let fd = nix::fcntl::openat(
        &parent.dir,
        parent.name.as_os_str(),
        nix::fcntl::OFlag::O_WRONLY
            | nix::fcntl::OFlag::O_CREAT
            | nix::fcntl::OFlag::O_EXCL
            | nix::fcntl::OFlag::O_NOFOLLOW
            | nix::fcntl::OFlag::O_CLOEXEC,
        nix::sys::stat::Mode::from_bits_truncate(0o600),
    )
    .map_err(io::Error::from)?;
    let mut file = std::fs::File::from(fd);
    let result = (|| {
        // Enforce the mode explicitly so the process umask cannot weaken
        // or strengthen it.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        write(&mut file)?;
        file.sync_all()?;
        parent.dir.sync_all()
    })();
    if let Err(error) = result {
        if let Ok(proof) = nix::sys::stat::fstat(&file) {
            publication.remove_if(&proof);
        }
        return Err(error);
    }
    Ok(PublishedBundle {
        file,
        parent: publication,
    })
}

/// The verified private parent of a publication target: the directory's
/// own descriptor and the child's file name. `create_new` publication and
/// identity-proved teardown both run through this descriptor, so the
/// pathname is never reopened and a swapped or renamed ancestor cannot
/// redirect an unlink to a foreign file.
pub(super) struct PrivateParent {
    dir: std::fs::File,
    name: std::ffi::OsString,
}

impl PrivateParent {
    /// `fstatat(AT_SYMLINK_NOFOLLOW)` on the child through the retained
    /// directory descriptor — never a pathname lookup, never an `open`,
    /// so a FIFO replacement cannot block teardown and a symlink is seen
    /// as a symlink.
    pub(super) fn stat_child(&self) -> io::Result<nix::sys::stat::FileStat> {
        nix::sys::stat::fstatat(
            &self.dir,
            self.name.as_os_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(io::Error::from)
    }

    /// Unlinks the child only while `fstatat` still proves it is the file
    /// `proof` describes — same type, device, and inode. A replaced,
    /// symlinked, or foreign file is preserved untouched.
    pub(super) fn remove_if(&self, proof: &nix::sys::stat::FileStat) {
        let Ok(current) = self.stat_child() else {
            return;
        };
        if file_is_regular(current.st_mode)
            && current.st_dev == proof.st_dev
            && current.st_ino == proof.st_ino
        {
            let _ = nix::unistd::unlinkat(
                &self.dir,
                self.name.as_os_str(),
                nix::unistd::UnlinkatFlags::NoRemoveDir,
            );
        }
    }
}

fn file_is_regular(mode: nix::sys::stat::mode_t) -> bool {
    nix::sys::stat::SFlag::from_bits_truncate(mode).contains(nix::sys::stat::SFlag::S_IFREG)
}

/// Requires `path` to be a normalized absolute direct child of an
/// existing owner-only (`0700`) directory owned by this process's euid,
/// reached through a chain containing no symlinks: the parent is opened
/// by a real no-follow walk — `openat(O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC)`
/// per component from `/` — so the returned descriptor is provably the
/// verified directory; a later ancestor rename or symlink swap cannot
/// redirect a lookup done through it.
pub(super) fn require_private_child(path: &Path) -> io::Result<PrivateParent> {
    if !is_normalized_absolute_path(path) {
        return Err(invalid_input("path must be a normalized absolute path"));
    }
    let Some(parent) = path.parent() else {
        return Err(invalid_input("path has no parent directory"));
    };
    let Some(name) = path.file_name() else {
        return Err(invalid_input("path has no file name"));
    };
    // The parent is pinned by a real no-follow walk: every ancestor is
    // `openat`'d `O_DIRECTORY|O_NOFOLLOW` from `/`, so the descriptor is
    // provably the verified directory itself — a later ancestor rename
    // or symlink swap cannot redirect a lookup done through it.
    let dir = rustx_execution_transport::open_dir_nofollow(parent)
        .map_err(|_| invalid_input("parent directory is missing or unreadable"))?;
    let metadata = dir
        .metadata()
        .map_err(|_| invalid_input("parent directory is missing or unreadable"))?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != euid()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(invalid_input(
            "parent must be an owner-only private directory",
        ));
    }
    Ok(PrivateParent {
        dir,
        name: name.to_owned(),
    })
}

fn euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}
