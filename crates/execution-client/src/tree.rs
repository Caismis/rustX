//! Bounded, descriptor-relative file tree mutations in the Client workspace.
use crate::mutation::Arbitration;
use rustix::fs::{AtFlags, Mode, OFlags};
use rustx_confined_fs::{Expectation, FileMetadata, PackageDirectory, WriteTarget};
use rustx_execution_protocol::{ClientReply, TreeMutation};
use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path};

const TREE_DEPTH: usize = 32;
const TREE_ENTRIES: usize = 4096;
const COPY_BYTES: u64 = 256 * 1024 * 1024;

struct Authority<'a> {
    arbitration: &'a Arbitration,
    changed: Cell<bool>,
}

impl Authority<'_> {
    fn check(&self) -> io::Result<()> {
        self.arbitration.check_grant()
    }

    fn commit(&self, crossing: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
        self.arbitration.with_grant(|| {
            crossing()?;
            self.changed.set(true);
            Ok(())
        })
    }
}

pub(super) fn mutate(
    root: &PackageDirectory,
    arbitration: &Arbitration,
    request: &TreeMutation,
) -> ClientReply {
    let authority = Authority {
        arbitration,
        changed: Cell::new(false),
    };
    match authority
        .check()
        .and_then(|()| execute(root, &authority, request))
    {
        Ok(()) => ClientReply::TreeMutated,
        Err(_) if authority.changed.get() => ClientReply::TreeOutcomeUnknown,
        Err(error) => ClientReply::Error {
            kind: super::read_error(&error),
        },
    }
}

fn execute(
    root: &PackageDirectory,
    authority: &Authority<'_>,
    request: &TreeMutation,
) -> io::Result<()> {
    match request {
        TreeMutation::CreateDirectory {
            path,
            recursive: true,
        } => {
            leaf_path(path)?;
            let mut current = root.child(OsStr::new("."))?;
            for part in path.components() {
                let Component::Normal(name) = part else {
                    return Err(invalid("directory path requires normal components"));
                };
                match mkdir(&current, name, authority) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                current = current.child(name)?;
            }
            Ok(())
        }
        TreeMutation::CreateDirectory {
            path,
            recursive: false,
        } => {
            let (directory, leaf) = parent(root, path)?;
            mkdir(&directory, &leaf, authority)
        }
        TreeMutation::Remove {
            path,
            recursive,
            force,
        } => {
            let (directory, leaf) = match parent(root, path) {
                Err(error) if *force && error.kind() == io::ErrorKind::NotFound => return Ok(()),
                result => result?,
            };
            let mut remaining = TREE_ENTRIES;
            remove(
                &directory,
                &leaf,
                *recursive,
                0,
                &mut remaining,
                *force,
                authority,
            )
        }
        TreeMutation::Copy {
            source,
            destination,
            recursive,
        } => {
            if destination.starts_with(source) {
                return Err(invalid("copy destination must not be inside source"));
            }
            let (source, source_leaf) = parent(root, source)?;
            let (destination, destination_leaf) = parent(root, destination)?;
            copy(
                &source,
                &source_leaf,
                &destination,
                &destination_leaf,
                *recursive,
                0,
                &mut (TREE_ENTRIES, COPY_BYTES),
                authority,
            )
        }
    }
}

fn leaf_path(relative: &Path) -> io::Result<&OsStr> {
    super::relative_entry(relative)
        .map_err(|_| invalid("operation requires a workspace-relative leaf"))?;
    if relative.components().count() > TREE_DEPTH {
        return Err(invalid("filesystem tree path exceeds depth budget"));
    }
    relative
        .file_name()
        .ok_or_else(|| invalid("operation requires a leaf path"))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn conflict() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "filesystem revision conflict")
}

fn parent(root: &PackageDirectory, relative: &Path) -> io::Result<(PackageDirectory, OsString)> {
    let leaf = leaf_path(relative)?;
    let directory = root.descend(relative.parent().unwrap_or(Path::new("")))?;
    Ok((directory, leaf.to_os_string()))
}

fn open(directory: &PackageDirectory, leaf: &OsStr) -> io::Result<File> {
    directory.verify()?;
    Ok(File::from(rustix::fs::openat(
        directory.descriptor(),
        leaf,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
    )?))
}

fn mkdir(directory: &PackageDirectory, name: &OsStr, authority: &Authority<'_>) -> io::Result<()> {
    authority.commit(|| {
        directory.verify()?;
        rustix::fs::mkdirat(directory.descriptor(), name, Mode::from_raw_mode(0o700))
            .map_err(Into::into)
    })
}

fn atomic_write(
    directory: &PackageDirectory,
    leaf: &std::ffi::OsStr,
    expected: Option<&str>,
    authority: &Authority<'_>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<FileMetadata> {
    let target = WriteTarget::prepare(
        directory.child(OsStr::new("."))?,
        leaf,
        Vec::new(),
        expected.map_or(Expectation::Absent, Expectation::Revision),
    )?;
    let staged = target.stage(write)?;
    let visible = staged.publish_with_gate(|cross| authority.commit(cross))?;
    visible.finish_with(|directory| {
        #[cfg(test)]
        if FAIL_DIRECTORY_SYNC.replace(false) {
            return Err(io::Error::other("injected directory sync failure"));
        }
        directory.sync_all()
    })
}
#[cfg(test)]
thread_local! {static FAIL_DIRECTORY_SYNC:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}

fn take_entry(depth: usize, remaining: &mut usize) -> io::Result<()> {
    if depth > TREE_DEPTH || *remaining == 0 {
        return Err(invalid(
            "filesystem tree operation exceeds budget; operation may be partial",
        ));
    }
    *remaining -= 1;
    Ok(())
}
fn remove(
    parent: &PackageDirectory,
    leaf: &std::ffi::OsStr,
    recursive: bool,
    depth: usize,
    remaining: &mut usize,
    force: bool,
    authority: &Authority<'_>,
) -> io::Result<()> {
    authority.check()?;
    take_entry(depth, remaining)?;
    parent.verify()?;
    let before = match nix::sys::stat::fstatat(
        parent.descriptor(),
        leaf,
        nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
    ) {
        Ok(stat) => FileMetadata::stat(&stat)?,
        Err(nix::errno::Errno::ENOENT) if force => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if before.is_directory {
        let directory = parent.child(leaf)?;
        if before.identity
            != FileMetadata::of(&File::from(directory.descriptor().try_clone()?))?.identity
        {
            return Err(conflict());
        }
        if recursive {
            for child in directory.names(TREE_ENTRIES)? {
                remove(
                    &directory,
                    child.as_os_str(),
                    true,
                    depth + 1,
                    remaining,
                    force,
                    authority,
                )?;
            }
        }
        parent.verify()?;
        let now = FileMetadata::stat(&nix::sys::stat::fstatat(
            parent.descriptor(),
            leaf,
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )?)?;
        if before.identity != now.identity {
            return Err(conflict());
        }
        authority.commit(|| {
            parent.verify()?;
            rustix::fs::unlinkat(parent.descriptor(), leaf, AtFlags::REMOVEDIR)?;
            Ok(())
        })?;
    } else if before.is_file || before.is_symlink {
        parent.verify()?;
        if before
            != FileMetadata::stat(&nix::sys::stat::fstatat(
                parent.descriptor(),
                leaf,
                nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
            )?)?
        {
            return Err(conflict());
        }
        authority.commit(|| {
            parent.verify()?;
            rustix::fs::unlinkat(parent.descriptor(), leaf, AtFlags::empty())?;
            Ok(())
        })?;
    } else {
        return Err(invalid(
            "remove requires a regular file, symlink or directory",
        ));
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Exact source and destination descriptor pairs travel together.
fn copy(
    source: &PackageDirectory,
    source_leaf: &std::ffi::OsStr,
    destination: &PackageDirectory,
    destination_leaf: &std::ffi::OsStr,
    recursive: bool,
    depth: usize,
    budget: &mut (usize, u64),
    authority: &Authority<'_>,
) -> io::Result<()> {
    authority.check()?;
    take_entry(depth, &mut budget.0)?;
    let mut input = open(source, source_leaf)?;
    let before = FileMetadata::of(&input)?;
    if before.is_directory {
        if !recursive {
            return Err(invalid("directory copy requires recursive"));
        }
        if destination.contains(&input)? {
            return Err(invalid("copy destination is physically inside source"));
        }
        authority.commit(|| {
            destination.verify()?;
            source.verify()?;
            rustix::fs::mkdirat(
                destination.descriptor(),
                destination_leaf,
                Mode::from_raw_mode(0o700),
            )?;
            Ok(())
        })?;
        let source = source.child(source_leaf)?;
        if before.identity
            != FileMetadata::of(&File::from(source.descriptor().try_clone()?))?.identity
        {
            return Err(conflict());
        }
        let destination = destination.child(destination_leaf)?;
        for child in source.names(TREE_ENTRIES)? {
            copy(
                &source,
                child.as_os_str(),
                &destination,
                child.as_os_str(),
                true,
                depth + 1,
                budget,
                authority,
            )?;
        }
        return Ok(());
    }
    if !before.is_file || before.size > budget.1 {
        return Err(invalid(
            "copy exceeds byte budget or unsupported entry; operation may be partial",
        ));
    }
    budget.1 -= before.size;
    atomic_write(destination, destination_leaf, None, authority, |output| {
        output.set_permissions(input.metadata()?.permissions())?;
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            authority.check()?;
            source.verify()?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            copied = copied
                .checked_add(count as u64)
                .ok_or_else(|| invalid("copy size overflow"))?;
            if copied > before.size {
                return Err(conflict());
            }
            output.write_all(&buffer[..count])?;
        }
        if copied != before.size
            || before != FileMetadata::of(&input)?
            || before != FileMetadata::of(&open(source, source_leaf)?)?
        {
            return Err(conflict());
        }
        Ok(())
    })
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, String) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .display()
            .to_string();
        (temporary, root)
    }

    fn authority(arbitration: &Arbitration) -> Authority<'_> {
        Authority {
            arbitration,
            changed: Cell::new(false),
        }
    }

    #[test]
    fn atomic_faults_conflicts_and_permissions_preserve_the_current_file() {
        let (_temporary, root) = fixture();
        let path = format!("{root}/file");
        std::fs::write(&path, b"original").unwrap();
        let directory = PackageDirectory::open(Path::new(&root)).unwrap();
        let revision = Some(
            directory
                .entry_metadata(Path::new("file"))
                .unwrap()
                .revision,
        );
        let revision = revision.as_deref();
        let arbitration = Arbitration::new();
        let authority = authority(&arbitration);
        assert!(
            atomic_write(
                &directory,
                std::ffi::OsStr::new("file"),
                revision,
                &authority,
                |file| {
                    file.write_all(b"partial")?;
                    Err(io::Error::other("injected disk failure"))
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(
            atomic_write(
                &directory,
                std::ffi::OsStr::new("file"),
                revision,
                &authority,
                |file| {
                    file.write_all(b"stale")?;
                    std::fs::write(&path, b"external")
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        assert_eq!(
            directory.names(16).unwrap(),
            vec![std::path::PathBuf::from("file")]
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        let revision = directory
            .entry_metadata(Path::new("file"))
            .unwrap()
            .revision;
        atomic_write(
            &directory,
            std::ffi::OsStr::new("file"),
            Some(&revision),
            &authority,
            |file| file.write_all(b"replacement"),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }
    #[test]
    fn parent_replacement_before_commit_cannot_redirect_an_atomic_write() {
        let (temporary, root) = fixture();
        std::fs::create_dir(temporary.path().join("parent")).unwrap();
        std::fs::write(temporary.path().join("parent/file"), b"original").unwrap();
        let directory = PackageDirectory::open(&Path::new(&root).join("parent")).unwrap();
        let revision = directory
            .entry_metadata(Path::new("file"))
            .unwrap()
            .revision;
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("file"), b"private").unwrap();
        let arbitration = Arbitration::new();
        let authority = authority(&arbitration);
        let result = atomic_write(
            &directory,
            std::ffi::OsStr::new("file"),
            Some(&revision),
            &authority,
            |file| {
                file.write_all(b"new")?;
                std::fs::rename(
                    temporary.path().join("parent"),
                    temporary.path().join("retired"),
                )?;
                std::os::unix::fs::symlink(outside.path(), temporary.path().join("parent"))
            },
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(outside.path().join("file")).unwrap(),
            b"private"
        );
        assert_eq!(
            std::fs::read(temporary.path().join("retired/file")).unwrap(),
            b"original"
        );
    }
    #[test]
    fn root_revocation_during_staging_prevents_commit_without_waiting_on_file_io() {
        let (_temporary, root) = fixture();
        let directory = PackageDirectory::open(Path::new(&root)).unwrap();
        let path = format!("{root}/file");
        std::fs::write(&path, b"original").unwrap();
        let revision = directory
            .entry_metadata(Path::new("file"))
            .unwrap()
            .revision;
        let arbitration = Arbitration::new();
        let authority = authority(&arbitration);
        let result = atomic_write(
            &directory,
            OsStr::new("file"),
            Some(&revision),
            &authority,
            |file| {
                file.write_all(b"replacement")?;
                arbitration.revoke();
                Ok(())
            },
        );
        assert!(!authority.changed.get());
        assert!(result.is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
    #[test]
    fn failure_after_publication_reports_unknown_outcome_and_keeps_committed_bytes() {
        let (_temporary, root) = fixture();
        let path = format!("{root}/file");
        let arbitration = Arbitration::new();
        let authority = authority(&arbitration);
        let directory = PackageDirectory::open(Path::new(&root)).unwrap();
        let leaf = std::ffi::OsStr::new("file");
        let revision = atomic_write(&directory, leaf, None, &authority, |file| {
            file.write_all(b"original")
        })
        .unwrap()
        .revision;
        FAIL_DIRECTORY_SYNC.set(true);
        let error = atomic_write(&directory, leaf, Some(&revision), &authority, |file| {
            file.write_all(b"committed")
        })
        .unwrap_err();
        assert!(
            error
                .get_ref()
                .unwrap()
                .is::<rustx_confined_fs::CommitUnknown>()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"committed");
    }
}
