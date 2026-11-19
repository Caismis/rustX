//! The shared physical commit kernel for file writes: retain a verified
//! target, stage bytes into a private sibling, publish by `linkat` or
//! `renameat`, then finish with a durable directory sync.
//!
//! Every check runs descriptor-relative against the retained
//! [`PackageDirectory`]: the leaf comparison and the staging-name proof are
//! `fstatat`/`fstat` device+inode checks, never a pathname lookup that could
//! be redirected mid-commit. A proof and the `*at` call that follows it are
//! not atomic — an external writer can still rename or replace an entry in
//! between; the checks narrow that window and turn a detected change into a
//! conflict, they do not close it. The publish syscall is the commit
//! crossing — once it succeeds the bytes are visible, so a later failure
//! reports [`CommitUnknown`] and is never re-executed. The revision proofs
//! are honest pre-commit checks a racing change fails as a conflict; there
//! is no compare-and-swap syscall and none is claimed.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::unix::ffi::OsStrExt;

use rustix::fs::{Mode, OFlags, openat};

use crate::{FileMetadata, PackageDirectory, conflict, invalid};

/// The commit crossing succeeded but a finishing step — the staging unlink
/// or the directory sync — failed, so the durable outcome is unknown.
/// Callers downcast the `io::Error` to this marker to distinguish an
/// unknown-outcome commit from a clean pre-commit failure; it carries no
/// locator.
#[derive(Debug)]
pub struct CommitUnknown;

impl std::fmt::Display for CommitUnknown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("filesystem commit or partial outcome requires authoritative reread")
    }
}

impl std::error::Error for CommitUnknown {}

fn unknown(_: io::Error) -> io::Error {
    io::Error::other(CommitUnknown)
}

/// The leaf's current no-follow stamp — `None` when it is absent.
fn leaf_metadata(parent: &PackageDirectory, leaf: &OsStr) -> io::Result<Option<FileMetadata>> {
    match nix::sys::stat::fstatat(
        parent.descriptor(),
        leaf,
        nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
    ) {
        Ok(stat) => Ok(Some(FileMetadata::stat(&stat)?)),
        Err(nix::errno::Errno::ENOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The leaf's current stamp must still equal the prepared original.
fn recheck(
    parent: &PackageDirectory,
    leaf: &OsStr,
    original: Option<&FileMetadata>,
) -> io::Result<()> {
    if leaf_metadata(parent, leaf)?.as_ref() != original {
        return Err(conflict());
    }
    Ok(())
}

/// A retained mutation target: the verified parent directory, the leaf
/// name, and — for a replacement — the target's own open descriptor and
/// stamp. [`prepare`](Self::prepare) creates no temporary file;
/// [`stage`](Self::stage) owns the private sibling.
pub struct WriteTarget {
    parent: PackageDirectory,
    leaf: OsString,
    original: Option<(File, FileMetadata)>,
}

impl WriteTarget {
    /// Retains `leaf` under the verified `parent` for a staged write.
    ///
    /// `leaf` must be exactly one normal component — no separator, `.`,
    /// `..`, or NUL. `expected` is the caller's contract: `Some` requires
    /// the leaf's current revision to match exactly; `None` is create-only
    /// and requires the leaf to be absent. An existing target opens
    /// through the retained parent with `O_NOFOLLOW|NONBLOCK` and only a
    /// regular file may be replaced. The retained ancestor chain is
    /// verified before and after the retain.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for an unsafe leaf or a
    /// non-regular target, [`io::ErrorKind::AlreadyExists`] when the
    /// current revision does not equal `expected`,
    /// [`io::ErrorKind::PermissionDenied`] for a replaced ancestor, and
    /// the underlying `openat`/`fstat` error otherwise.
    pub fn prepare(
        parent: PackageDirectory,
        leaf: &OsStr,
        expected: Option<&str>,
    ) -> io::Result<Self> {
        let bytes = leaf.as_bytes();
        if bytes.is_empty()
            || bytes == b"."
            || bytes == b".."
            || bytes.contains(&b'/')
            || bytes.contains(&0)
        {
            return Err(invalid("unsafe file name"));
        }
        parent.verify()?;
        let original = match openat(
            parent.descriptor(),
            leaf,
            OFlags::RDONLY
                .union(OFlags::NOFOLLOW)
                .union(OFlags::NONBLOCK)
                .union(OFlags::CLOEXEC)
                .union(OFlags::NOCTTY),
            Mode::empty(),
        )
        .map_err(io::Error::from)
        {
            Ok(descriptor) => {
                let file = File::from(descriptor);
                let metadata = FileMetadata::of(&file)?;
                if !metadata.is_file {
                    return Err(invalid("write target must be a regular file"));
                }
                Some((file, metadata))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if original
            .as_ref()
            .map(|(_, metadata)| metadata.revision.as_str())
            != expected
        {
            return Err(conflict());
        }
        parent.verify()?;
        Ok(Self {
            parent,
            leaf: leaf.to_os_string(),
            original,
        })
    }

    /// The stamp the retained target was prepared under — `None` for a
    /// create-only target.
    #[must_use]
    pub fn original(&self) -> Option<&FileMetadata> {
        self.original.as_ref().map(|(_, metadata)| metadata)
    }

    /// Stages `writer`'s bytes into a private `.rustx-write-{uuid}` sibling
    /// under the retained parent.
    ///
    /// The retained chain and the leaf's revision are re-verified before
    /// the sibling is `O_EXCL`/`O_NOFOLLOW`-created; an existing target's
    /// mode is copied from the retained original descriptor — the target
    /// is never re-opened for it. The sibling is `sync_all`'d before the
    /// [`StagedWrite`] is released, and a [`StagedWrite`] dropped without
    /// publishing removes its own staging name.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::AlreadyExists`] when the leaf changed
    /// since [`prepare`](Self::prepare), and the underlying open, write,
    /// or sync error otherwise.
    pub fn stage(
        self,
        writer: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<StagedWrite> {
        self.parent.verify()?;
        recheck(&self.parent, &self.leaf, self.original())?;
        let name = OsString::from(format!(".rustx-write-{}", uuid::Uuid::now_v7()));
        let output = File::from(openat(
            self.parent.descriptor(),
            name.as_os_str(),
            OFlags::WRONLY
                .union(OFlags::CREATE)
                .union(OFlags::EXCL)
                .union(OFlags::NOFOLLOW)
                .union(OFlags::CLOEXEC),
            Mode::from_raw_mode(0o600),
        )?);
        let mut staged = StagedWrite {
            parent: self.parent,
            leaf: self.leaf,
            original: self.original,
            output,
            name,
            published: false,
        };
        let result = (|| {
            if let Some((original, _)) = &staged.original {
                staged
                    .output
                    .set_permissions(original.metadata()?.permissions())?;
            }
            writer(&mut staged.output)?;
            staged.output.sync_all()
        })();
        result.map(|()| staged)
    }
}

/// A staged write: the private sibling holds the new bytes, fsync'd,
/// awaiting [`publish`](Self::publish). Dropping without publishing
/// removes only the staging name that still names this exact file —
/// never a foreign replacement that took the name.
pub struct StagedWrite {
    parent: PackageDirectory,
    leaf: OsString,
    original: Option<(File, FileMetadata)>,
    output: File,
    name: OsString,
    published: bool,
}

impl StagedWrite {
    /// Publishes the staged bytes at the leaf.
    ///
    /// Callers run this inside their physical cancellation gate. The
    /// retained chain, the leaf's no-follow revision, and the staging
    /// name's device:inode are re-verified first; a create-only target
    /// then publishes by `linkat` (never overwriting a concurrent file)
    /// and a replacement by `renameat`. The syscall's success is the
    /// commit crossing — it is recorded immediately, any failure after it
    /// reports [`CommitUnknown`], and the crossing is never re-executed.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::AlreadyExists`] when the leaf changed
    /// since preparation, [`io::ErrorKind::PermissionDenied`] when the
    /// staging name was replaced, a downcastable [`CommitUnknown`] when a
    /// post-crossing step fails, and the underlying `*at` error otherwise.
    pub fn publish(mut self) -> io::Result<PublishedWrite> {
        self.parent.verify()?;
        recheck(
            &self.parent,
            &self.leaf,
            self.original.as_ref().map(|(_, metadata)| metadata),
        )?;
        let staging = nix::sys::stat::fstatat(
            self.parent.descriptor(),
            self.name.as_os_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )?;
        let own = nix::sys::stat::fstat(&self.output)?;
        if (staging.st_dev, staging.st_ino) != (own.st_dev, own.st_ino) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "staging file replaced",
            ));
        }
        if self.original.is_none() {
            rustix::fs::linkat(
                self.parent.descriptor(),
                self.name.as_os_str(),
                self.parent.descriptor(),
                self.leaf.as_os_str(),
                rustix::fs::AtFlags::empty(),
            )?;
            self.published = true;
            rustix::fs::unlinkat(
                self.parent.descriptor(),
                self.name.as_os_str(),
                rustix::fs::AtFlags::empty(),
            )
            .map_err(|error| unknown(error.into()))?;
        } else {
            rustix::fs::renameat(
                self.parent.descriptor(),
                self.name.as_os_str(),
                self.parent.descriptor(),
                self.leaf.as_os_str(),
            )?;
            self.published = true;
        }
        Ok(PublishedWrite {
            parent: self.parent.child(OsStr::new(".")).map_err(unknown)?,
            leaf: self.leaf.clone(),
            file: self.output.try_clone().map_err(unknown)?,
        })
    }
}

impl Drop for StagedWrite {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        // Unlink only while the staging name still names this exact file —
        // device+inode proven rather than a foreign file that took the
        // name. The proof and the `unlinkat` are not atomic: a concurrent
        // rename can still interleave between them; the check narrows the
        // window, it does not close it.
        let own = nix::sys::stat::fstat(&self.output);
        let named = nix::sys::stat::fstatat(
            self.parent.descriptor(),
            self.name.as_os_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        );
        if let (Ok(own), Ok(named)) = (own, named)
            && (own.st_dev, own.st_ino) == (named.st_dev, named.st_ino)
        {
            let _ = rustix::fs::unlinkat(
                self.parent.descriptor(),
                self.name.as_os_str(),
                rustix::fs::AtFlags::empty(),
            );
        }
    }
}

/// A published write: the commit crossing already happened, so the only
/// remaining work is durable finish — the parent directory sync and the
/// name/descriptor identity proof.
pub struct PublishedWrite {
    parent: PackageDirectory,
    leaf: OsString,
    file: File,
}

impl PublishedWrite {
    /// Syncs the parent directory, proves the named leaf still resolves to
    /// this descriptor, and returns the descriptor's own stamp.
    ///
    /// # Errors
    ///
    /// Returns a downcastable [`CommitUnknown`] for any failure — the bytes
    /// are already published, so the outcome is never a clean pre-commit
    /// error.
    pub fn finish(self) -> io::Result<FileMetadata> {
        self.finish_with(File::sync_all)
    }

    /// [`finish`](Self::finish) with the caller's directory sync — the
    /// existing post-commit failure hook. The cancellation gate never
    /// covers this fsync: it runs after publication, so a failure is
    /// [`CommitUnknown`].
    ///
    /// # Errors
    ///
    /// Returns a downcastable [`CommitUnknown`] for the sync's failure or
    /// a broken post-publication identity proof.
    pub fn finish_with(
        self,
        sync: impl FnOnce(&File) -> io::Result<()>,
    ) -> io::Result<FileMetadata> {
        (|| {
            let directory = File::from(self.parent.descriptor().try_clone()?);
            sync(&directory)?;
            self.parent.verify()?;
            let named = nix::sys::stat::fstatat(
                self.parent.descriptor(),
                self.leaf.as_os_str(),
                nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
            )?;
            let published = nix::sys::stat::fstat(&self.file)?;
            if (named.st_dev, named.st_ino) != (published.st_dev, published.st_ino) {
                return Err(io::Error::other(
                    "published name no longer names the committed file",
                ));
            }
            let metadata = FileMetadata::of(&self.file)?;
            self.parent.verify()?;
            Ok(metadata)
        })()
        .map_err(unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn root() -> (tempfile::TempDir, PackageDirectory) {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().canonicalize().unwrap();
        (temporary, PackageDirectory::open(&path).unwrap())
    }

    fn staging_names(path: &Path) -> Vec<OsString> {
        let mut names: Vec<OsString> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.as_bytes().starts_with(b".rustx-write-"))
            .collect();
        names.sort();
        names
    }

    fn revision(directory: &PackageDirectory, leaf: &str) -> String {
        directory.entry_metadata(Path::new(leaf)).unwrap().revision
    }

    #[test]
    fn a_leaf_swap_before_staging_is_rejected_and_everything_is_untouched() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        std::fs::write(&file, b"original").unwrap();
        let current = revision(&directory, "file");
        let target = WriteTarget::prepare(directory, OsStr::new("file"), Some(&current)).unwrap();
        let moved = temporary.path().join("moved");
        std::fs::rename(&file, &moved).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), &file).unwrap();
        let error = target
            .stage(|output| output.write_all(b"new"))
            .map(|_| ())
            .expect_err("a swapped leaf must reject");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(file.is_symlink());
        assert_eq!(std::fs::read(&moved).unwrap(), b"original");
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
    }

    #[test]
    fn a_leaf_swap_before_publishing_is_rejected_and_the_staging_name_is_cleaned() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        std::fs::write(&file, b"original").unwrap();
        let current = directory
            .entry_metadata(Path::new("file"))
            .unwrap()
            .revision;
        let staged = WriteTarget::prepare(directory, OsStr::new("file"), Some(&current))
            .unwrap()
            .stage(|output| output.write_all(b"new"))
            .unwrap();
        let moved = temporary.path().join("moved");
        std::fs::rename(&file, &moved).unwrap();
        std::fs::create_dir(&file).unwrap();
        let error = staged
            .publish()
            .map(|_| ())
            .expect_err("a swapped leaf must reject");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(file.is_dir());
        assert_eq!(std::fs::read(&moved).unwrap(), b"original");
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
    }

    #[test]
    fn a_revision_change_between_prepare_and_commit_is_a_conflict() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        std::fs::write(&file, b"original").unwrap();
        let current = revision(&directory, "file");
        // Changed before staging.
        let target = WriteTarget::prepare(
            directory.child(OsStr::new(".")).unwrap(),
            OsStr::new("file"),
            Some(&current),
        )
        .unwrap();
        std::fs::write(&file, b"external").unwrap();
        assert_eq!(
            target
                .stage(|output| output.write_all(b"new"))
                .map(|_| ())
                .expect_err("a changed leaf must reject")
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        // Changed between staging and publishing.
        let staged = WriteTarget::prepare(directory, OsStr::new("file"), Some(&revision_of(&file)))
            .unwrap()
            .stage(|output| output.write_all(b"new"))
            .unwrap();
        std::fs::write(&file, b"external2").unwrap();
        assert_eq!(
            staged
                .publish()
                .map(|_| ())
                .expect_err("a changed leaf must reject")
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"external2");
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
    }

    fn revision_of(path: &Path) -> String {
        FileMetadata::of(&File::open(path).unwrap())
            .unwrap()
            .revision
    }

    #[test]
    fn a_dropped_staging_write_cleans_only_its_own_temporary() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        std::fs::write(&file, b"original").unwrap();
        let current = revision(&directory, "file");
        let staged = WriteTarget::prepare(directory, OsStr::new("file"), Some(&current))
            .unwrap()
            .stage(|output| output.write_all(b"new"))
            .unwrap();
        assert_eq!(staging_names(temporary.path()).len(), 1);
        drop(staged);
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
    }

    #[test]
    fn a_create_only_publish_links_and_a_replacement_publishes_with_its_mode() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        // Create: `None` publishes by link, then the staging name is gone.
        let created = WriteTarget::prepare(
            directory.child(OsStr::new(".")).unwrap(),
            OsStr::new("file"),
            None,
        )
        .unwrap()
        .stage(|output| output.write_all(b"created"))
        .unwrap()
        .publish()
        .unwrap()
        .finish()
        .unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"created");
        assert_eq!(
            created,
            directory.entry_metadata(Path::new("file")).unwrap()
        );
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
        // Replace: the retained original's mode survives publication. The
        // chmod itself moves the revision, so prepare against a fresh one.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o750)).unwrap();
        let current = revision(&directory, "file");
        let replaced = WriteTarget::prepare(directory, OsStr::new("file"), Some(&current))
            .unwrap()
            .stage(|output| output.write_all(b"replaced"))
            .unwrap()
            .publish()
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"replaced");
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            replaced,
            PackageDirectory::open(temporary.path())
                .unwrap()
                .entry_metadata(Path::new("file"))
                .unwrap()
        );
    }

    #[test]
    fn a_post_publication_failure_is_commit_unknown_and_keeps_the_bytes() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        std::fs::write(&file, b"original").unwrap();
        let current = revision(&directory, "file");
        let visible = WriteTarget::prepare(directory, OsStr::new("file"), Some(&current))
            .unwrap()
            .stage(|output| output.write_all(b"committed"))
            .unwrap()
            .publish()
            .unwrap();
        let error = visible
            .finish_with(|_| Err(io::Error::other("injected directory sync failure")))
            .expect_err("an injected sync failure must error");
        assert!(error.get_ref().unwrap().is::<CommitUnknown>());
        assert_eq!(std::fs::read(&file).unwrap(), b"committed");
    }

    #[test]
    fn finish_after_parent_replacement_is_commit_unknown() {
        let (temporary, root) = root();
        let parent_path = temporary.path().join("parent");
        std::fs::create_dir(&parent_path).unwrap();
        let parent = root.child(OsStr::new("parent")).unwrap();
        let file = parent_path.join("file");
        std::fs::write(&file, b"original").unwrap();
        let current = revision(&parent, "file");
        let visible = WriteTarget::prepare(parent, OsStr::new("file"), Some(&current))
            .unwrap()
            .stage(|output| output.write_all(b"committed"))
            .unwrap()
            .publish()
            .unwrap();
        // Move the retained parent away and plant a different directory
        // at its path: the published bytes are unreachable by name, so
        // finish must report the unknown outcome, never success.
        let moved = temporary.path().join("retired");
        std::fs::rename(&parent_path, &moved).unwrap();
        std::fs::create_dir(&parent_path).unwrap();
        let error = visible
            .finish()
            .expect_err("a replaced parent must be commit-unknown");
        assert!(error.get_ref().unwrap().is::<CommitUnknown>());
        assert!(!parent_path.join("file").exists());
        assert_eq!(std::fs::read(moved.join("file")).unwrap(), b"committed");
    }
}
