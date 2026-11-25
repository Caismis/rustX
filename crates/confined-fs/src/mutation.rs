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
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;

use rustix::fs::{AtFlags, RenameFlags, mkdirat, renameat_with, unlinkat};
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

/// One proven-owned creation inside the private staging directory:
/// the retained containing descriptor, the entry's name, and the
/// device:inode proven from the descriptor opened at creation.
/// Cleanup only ever removes a name that still resolves to this exact
/// inode — a substituted entry is never unlinked.
struct OwnedEntry {
    /// The open descriptor of the directory containing `name`.
    parent: File,
    name: OsString,
    device: nix::libc::dev_t,
    inode: nix::libc::ino_t,
    directory: bool,
}

/// The `.rustx-mkdir-*` staging directory and the owned subtree built
/// inside it. The directory itself *is* the first missing component —
/// publish crosses it into the retained parent with one `NOREPLACE`
/// rename, so no staged interior pathname is ever re-resolved by name.
/// `owned` holds every successfully created interior entry in creation
/// order so a pre-crossing failure can remove exactly what this stage
/// made — in reverse, each re-proven by device:inode against its
/// retained containing descriptor — and nothing else.
struct SubtreeStage {
    root: File,
    owned: Vec<OwnedEntry>,
}

/// The `fstatat` stamp of `name` under `containing`, no-follow.
fn named_stat(containing: &File, name: &OsStr) -> io::Result<nix::sys::stat::FileStat> {
    Ok(nix::sys::stat::fstatat(
        containing,
        name,
        nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
    )?)
}

impl SubtreeStage {
    /// Creates `missing_tail` interior directories and the staged leaf
    /// file, registering every successful creation for proven cleanup
    /// before the next fallible step, then runs `writer` and fsyncs.
    /// On any error the caller removes every registered entry — only
    /// a creation that could not be proven at all can be left behind.
    fn create_interior(
        &mut self,
        missing_tail: &[OsString],
        leaf: &OsStr,
        writer: impl FnOnce(&mut File) -> io::Result<()>,
        mut sync: impl FnMut(&File) -> io::Result<()>,
    ) -> io::Result<File> {
        let mut level = self.root.try_clone()?;
        for component in missing_tail {
            mkdirat(&level, component.as_os_str(), Mode::from_raw_mode(0o700))?;
            // Open the created directory for both the descent and its
            // identity proof: the descriptor is the strongest available
            // evidence the name still resolves to what was created.
            let child = File::from(openat(
                &level,
                component.as_os_str(),
                OFlags::RDONLY
                    .union(OFlags::DIRECTORY)
                    .union(OFlags::NOFOLLOW)
                    .union(OFlags::CLOEXEC),
                Mode::empty(),
            )?);
            let stat = nix::sys::stat::fstat(&child)?;
            self.owned.push(OwnedEntry {
                parent: level,
                name: component.clone(),
                device: stat.st_dev,
                inode: stat.st_ino,
                directory: true,
            });
            level = child;
        }
        let mut output = File::from(openat(
            &level,
            leaf,
            OFlags::WRONLY
                .union(OFlags::CREATE)
                .union(OFlags::EXCL)
                .union(OFlags::NOFOLLOW)
                .union(OFlags::CLOEXEC),
            Mode::from_raw_mode(0o600),
        )?);
        let stat = nix::sys::stat::fstat(&output)?;
        self.owned.push(OwnedEntry {
            parent: level.try_clone()?,
            name: leaf.to_os_string(),
            device: stat.st_dev,
            inode: stat.st_ino,
            directory: false,
        });
        writer(&mut output)?;
        output.sync_all()?;
        // The leaf entry, then every directory entry above it, must be
        // durable before the staging root becomes visible.
        for entry in self.owned.iter().rev() {
            sync(&entry.parent)?;
        }
        Ok(output)
    }

    /// Proves the staging name and every registered owned entry still
    /// resolve to the exact inodes this stage created — a substituted
    /// staging directory, interior directory, or leaf is a denial,
    /// never a publish or cleanup target.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::PermissionDenied`] for a substituted
    /// name and the underlying stat error otherwise.
    fn verify(&self, parent: &PackageDirectory, name: &OsStr) -> io::Result<()> {
        let named = nix::sys::stat::fstatat(
            parent.descriptor(),
            name,
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )?;
        let own = nix::sys::stat::fstat(&self.root)?;
        if (named.st_dev, named.st_ino) != (own.st_dev, own.st_ino) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "staging directory replaced",
            ));
        }
        for entry in &self.owned {
            match named_stat(&entry.parent, &entry.name) {
                Ok(named) if (named.st_dev, named.st_ino) == (entry.device, entry.inode) => {}
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "staged subtree entry replaced",
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Removes only the proven-owned subtree: every registered entry in
    /// reverse creation order — the leaf file first, directories deepest
    /// first — each re-proven by device:inode against its retained
    /// containing descriptor so a substituted name is never unlinked,
    /// then the staging directory itself, proven against the root
    /// descriptor. No recursion, no rewalking of untrusted components.
    /// Best-effort: every step is attempted even if an earlier fails.
    fn remove(&self, parent: &PackageDirectory, name: &OsStr) {
        for entry in self.owned.iter().rev() {
            if let Ok(named) = named_stat(&entry.parent, &entry.name)
                && (named.st_dev, named.st_ino) == (entry.device, entry.inode)
            {
                let flags = if entry.directory {
                    AtFlags::REMOVEDIR
                } else {
                    AtFlags::empty()
                };
                let _ = unlinkat(&entry.parent, &entry.name, flags);
            }
        }
        let named = nix::sys::stat::fstatat(
            parent.descriptor(),
            name,
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        );
        let own = nix::sys::stat::fstat(&self.root);
        if matches!((own, named), (Ok(own), Ok(named)) if (own.st_dev, own.st_ino) == (named.st_dev, named.st_ino))
        {
            let _ = unlinkat(parent.descriptor(), name, AtFlags::REMOVEDIR);
        }
    }
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

/// What a ticketed preparation expects the named leaf to be.
#[derive(Debug, Clone, Copy)]
pub enum Expectation<'a> {
    /// Retain whatever the leaf currently is — present or absent. This is
    /// the pre-authorization object; it never implies publish permission.
    Current,
    /// Create-only: the leaf must be absent at retain and at publish.
    Absent,
    /// The leaf must be an existing file with exactly this revision.
    Revision(&'a str),
}

/// A retained mutation target: the verified deepest existing ancestor
/// directory, the still-missing interior names, the leaf name, and — for
/// a replacement — the target's own open descriptor and stamp.
/// [`prepare`](Self::prepare) creates no temporary file and no missing
/// directory; [`stage`](Self::stage) owns the private sibling.
pub struct WriteTarget {
    /// The deepest existing ancestor — the leaf's parent when `missing`
    /// is empty.
    parent: PackageDirectory,
    /// Directory names still missing between `parent` and the leaf's
    /// containing directory — empty for an existing path.
    missing: Vec<OsString>,
    leaf: OsString,
    original: Option<(File, FileMetadata)>,
}

impl WriteTarget {
    /// Retains `leaf` under the verified `parent` for a staged write.
    ///
    /// `leaf` must be exactly one normal component — no separator, `.`,
    /// `..`, or NUL — and so must every name in `missing`, the directories
    /// still absent between `parent` and the leaf's containing directory.
    /// `missing` is created only inside the private staging subtree at
    /// [`stage`](Self::stage) and only crosses into `parent` as one
    /// `NOREPLACE` rename at publish — nothing about the target or its
    /// missing parents changes before that crossing.
    ///
    /// `expected` is the caller's contract: [`Expectation::Revision`]
    /// requires the leaf's current revision to match exactly,
    /// [`Expectation::Absent`] requires the leaf to be absent (including
    /// the first missing directory staying absent), and
    /// [`Expectation::Current`] retains whatever is present — an absent
    /// leaf yields `original() == None`. A leaf inside a missing subtree
    /// can never satisfy a `Revision`. An existing target opens through
    /// the retained parent with `O_NOFOLLOW|NONBLOCK` and only a regular
    /// file may be replaced. The retained ancestor chain is verified
    /// before and after the retain.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] for an unsafe leaf or
    /// missing name or a non-regular target,
    /// [`io::ErrorKind::AlreadyExists`] when the current state
    /// contradicts `expected`, [`io::ErrorKind::PermissionDenied`] for a
    /// replaced ancestor, and the underlying `openat`/`fstat` error
    /// otherwise.
    pub fn prepare(
        parent: PackageDirectory,
        leaf: &OsStr,
        missing: Vec<OsString>,
        expected: Expectation<'_>,
    ) -> io::Result<Self> {
        for name in std::iter::once(leaf).chain(missing.iter().map(OsString::as_os_str)) {
            let bytes = name.as_bytes();
            if bytes.is_empty()
                || bytes == b"."
                || bytes == b".."
                || bytes.contains(&b'/')
                || bytes.contains(&0)
            {
                return Err(invalid("unsafe file name"));
            }
        }
        parent.verify()?;
        // A leaf below a missing subtree cannot exist: only an existing
        // containing directory — `missing` empty — can name an original.
        let original = if missing.is_empty() {
            match openat(
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
            }
        } else {
            None
        };
        let holds = match expected {
            Expectation::Current => true,
            Expectation::Absent => original.is_none(),
            Expectation::Revision(revision) => original
                .as_ref()
                .is_some_and(|(_, metadata)| metadata.revision == revision),
        };
        if !holds {
            return Err(conflict());
        }
        parent.verify()?;
        Ok(Self {
            parent,
            missing,
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

    /// Stages `writer`'s bytes into a `O_EXCL` `.rustx-write-{uuid}` sibling
    /// under the retained parent.
    ///
    /// The retained chain and the leaf's revision are re-verified before
    /// the sibling is `O_EXCL`/`O_NOFOLLOW`-created; the target is never
    /// re-opened for it. The sibling's name
    /// is unique, not protected: a hostile writer could still replace it
    /// before [`publish`](StagedWrite::publish), whose device:inode proof
    /// refuses that case before the crossing. The sibling is `sync_all`'d
    /// before the [`StagedWrite`] is released, and a [`StagedWrite`]
    /// dropped without publishing removes its own staging name.
    ///
    /// The stage stays `0600` while it is staged regardless of the
    /// original's mode — a private payload is never readable by whoever
    /// could read the source. [`publish`](StagedWrite::publish) restores
    /// the original's ordinary permission bits on the published
    /// descriptor after the rename crossing, so a replacement keeps its
    /// mode semantics without the stage ever being exposed at them.
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
        self.stage_at(None, writer)
    }

    /// The retained destination parent. A local supervisor can use this
    /// descriptor to allocate a stage on the same filesystem before writing.
    #[must_use]
    pub fn destination_parent(&self) -> &PackageDirectory {
        &self.parent
    }

    /// Stage the bytes inside a separately owned directory on the destination
    /// filesystem. The same physical publication gate applies.
    ///
    /// # Errors
    /// Returns the stage's confinement, identity, write or sync error.
    pub fn stage_at(
        self,
        area: Option<PackageDirectory>,
        writer: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<StagedWrite> {
        self.parent.verify()?;
        if let Some(area) = &area {
            area.verify()?;
        }
        if self.missing.is_empty() {
            recheck(&self.parent, &self.leaf, self.original())?;
            return self.stage_file(area, writer);
        }
        self.stage_subtree(area, writer)
    }

    /// Stages the sibling file under the retained parent — the existing
    /// path when the leaf's containing directory already exists.
    fn stage_file(
        self,
        area: Option<PackageDirectory>,
        writer: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<StagedWrite> {
        let name = OsString::from(format!(".rustx-write-{}", uuid::Uuid::now_v7()));
        let stage_parent = area.as_ref().unwrap_or(&self.parent);
        let output = File::from(openat(
            stage_parent.descriptor(),
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
            missing: self.missing,
            leaf: self.leaf,
            original: self.original,
            output,
            name,
            area,
            staging: None,
            published: false,
        };
        // The stage stays at the create-time 0600 regardless of the
        // original's mode: a staged payload is private until the publish
        // crossing, never readable by whoever could read the source.
        // `publish` restores the original's mode on the published
        // descriptor after the rename so a replacement keeps its
        // semantics without ever exposing the stage at that mode.
        let result = (|| {
            writer(&mut staged.output)?;
            staged.output.sync_all()
        })();
        result.map(|()| staged)
    }

    /// Stages the whole missing subtree: `mkdirat` a private
    /// `.rustx-mkdir-*` sibling — owner-only `0700` — which itself
    /// *becomes* the first missing component at publish. Only
    /// `missing[1..]` and the leaf are created inside it, each proven
    /// from its own descriptor and registered for owned cleanup before
    /// the next fallible step. Nothing crosses into the retained parent
    /// before `publish`'s single `NOREPLACE` rename of the staging name
    /// to the first missing name, and that name must still be absent in
    /// the parent or staging is refused before anything is created.
    fn stage_subtree(
        self,
        area: Option<PackageDirectory>,
        writer: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<StagedWrite> {
        // The first missing name must still be absent: anything that
        // claimed it since prepare contradicts the retained expectation.
        if leaf_metadata(&self.parent, &self.missing[0])?.is_some() {
            return Err(conflict());
        }
        let stage_parent = area.as_ref().unwrap_or(&self.parent);
        let name = OsString::from(format!(".rustx-mkdir-{}", uuid::Uuid::now_v7()));
        mkdirat(
            stage_parent.descriptor(),
            name.as_os_str(),
            Mode::from_raw_mode(0o700),
        )?;
        let mut stage = match openat(
            stage_parent.descriptor(),
            name.as_os_str(),
            OFlags::RDONLY
                .union(OFlags::DIRECTORY)
                .union(OFlags::NOFOLLOW)
                .union(OFlags::CLOEXEC),
            Mode::empty(),
        ) {
            Ok(root) => SubtreeStage {
                root: File::from(root),
                owned: Vec::new(),
            },
            Err(error) => {
                // The staging directory exists but cannot be proven —
                // remove only while its name still names it.
                let _ = unlinkat(
                    stage_parent.descriptor(),
                    name.as_os_str(),
                    AtFlags::REMOVEDIR,
                );
                return Err(error.into());
            }
        };
        match stage.create_interior(&self.missing[1..], &self.leaf, writer, File::sync_all) {
            Ok(output) => Ok(StagedWrite {
                parent: self.parent,
                missing: self.missing,
                leaf: self.leaf,
                original: None,
                output,
                name,
                area,
                staging: Some(stage),
                published: false,
            }),
            Err(error) => {
                stage.remove(stage_parent, &name);
                Err(error)
            }
        }
    }

    /// Reads the whole retained original descriptor, bounded at `max`
    /// bytes and at the stamped `before.size` — never the leaf reopened
    /// by name. The retained parent and the named leaf's identity are
    /// re-verified after the read, so a replaced ancestor, a swapped
    /// leaf, or a source whose observed bytes contradict its stamp is a
    /// conflict, not content.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::FileTooLarge`] when the stamped source
    /// exceeds `max`, [`io::ErrorKind::InvalidInput`] when no original
    /// was retained, [`io::ErrorKind::AlreadyExists`] when the source or
    /// its name binding changed mid-read, and the underlying I/O errors
    /// otherwise.
    pub fn read_original_bounded(&self, max: u64) -> io::Result<Vec<u8>> {
        let Some((file, before)) = &self.original else {
            return Err(invalid(
                "read_original_bounded requires a retained original",
            ));
        };
        if before.size > max {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!(
                    "the source is {} bytes, above the {max}-byte cap",
                    before.size
                ),
            ));
        }
        self.parent.verify()?;
        io::Seek::seek(&mut &*file, io::SeekFrom::Start(0))?;
        let mut bytes = Vec::with_capacity(usize::try_from(before.size).unwrap_or(0));
        let observed = io::Read::take(file, before.size).read_to_end(&mut bytes)?;
        if u64::try_from(observed).unwrap_or(u64::MAX) != before.size {
            return Err(conflict());
        }
        // The retained descriptor must still be the stamped source and
        // the leaf must still name it — the read may have raced a swap.
        if FileMetadata::of(file)? != *before {
            return Err(conflict());
        }
        recheck(&self.parent, &self.leaf, self.original())?;
        Ok(bytes)
    }
}

/// A staged write: the private sibling holds the new bytes, fsync'd,
/// awaiting [`publish`](Self::publish). Dropping without publishing
/// removes only the staging name that still names this exact file —
/// never a foreign replacement that took the name.
pub struct StagedWrite {
    parent: PackageDirectory,
    missing: Vec<OsString>,
    leaf: OsString,
    original: Option<(File, FileMetadata)>,
    output: File,
    /// The private sibling name — `.rustx-write-*` for a file stage,
    /// `.rustx-mkdir-*` for a missing-subtree stage.
    name: OsString,
    /// The independently retained path that contains the staging name.
    area: Option<PackageDirectory>,
    /// The private staging directory and its registered owned subtree
    /// when `missing` is non-empty: the staging name itself crosses
    /// whole under the first missing name at publish.
    staging: Option<SubtreeStage>,
    published: bool,
}

impl StagedWrite {
    fn staging_parent(&self) -> &PackageDirectory {
        self.area.as_ref().unwrap_or(&self.parent)
    }

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
    pub fn publish(self) -> io::Result<PublishedWrite> {
        self.publish_with_gate(|cross| cross())
    }

    /// Publishes like [`Self::publish`], except `gate` owns the crossing:
    /// every pre-crossing verification runs first, then `gate` receives
    /// the one closure that performs the `linkat`/`renameat` (or the
    /// subtree `NOREPLACE` rename) itself. A caller runs that closure
    /// inside its arbitration — under the per-ticket lock — so a won
    /// cancellation refuses before the syscall runs and `Crossed` is
    /// recorded iff the crossing actually succeeded. `self.published` is
    /// set the moment the crossing succeeds, before `gate`'s own verdict
    /// is trusted, so a refusal or a failed syscall unwinds the private
    /// stage exactly like a pre-crossing error.
    ///
    /// # Errors
    ///
    /// Identical to [`Self::publish`]; a `gate` refusal surfaces `gate`'s
    /// own error and the staged write cleans itself on drop.
    pub fn publish_with_gate<G>(mut self, gate: G) -> io::Result<PublishedWrite>
    where
        G: FnOnce(&mut dyn FnMut() -> io::Result<()>) -> io::Result<()>,
    {
        self.parent.verify()?;
        self.staging_parent().verify()?;
        if self.staging.is_some() {
            return self.publish_subtree_with_gate(gate);
        }
        recheck(
            &self.parent,
            &self.leaf,
            self.original.as_ref().map(|(_, metadata)| metadata),
        )?;
        let staging = nix::sys::stat::fstatat(
            self.staging_parent().descriptor(),
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
        // The crossing alone runs inside the caller's gate: `crossed`
        // records the physical truth before `gate`'s verdict is read, so
        // a refused gate never hides a crossing that already happened.
        let mut crossed = false;
        let verdict = gate(&mut || -> io::Result<()> {
            if self.original.is_none() {
                rustix::fs::linkat(
                    self.staging_parent().descriptor(),
                    self.name.as_os_str(),
                    self.parent.descriptor(),
                    self.leaf.as_os_str(),
                    rustix::fs::AtFlags::empty(),
                )?;
            } else {
                rustix::fs::renameat(
                    self.staging_parent().descriptor(),
                    self.name.as_os_str(),
                    self.parent.descriptor(),
                    self.leaf.as_os_str(),
                )?;
            }
            crossed = true;
            Ok(())
        });
        self.published |= crossed;
        verdict?;
        if self.original.is_none() {
            rustix::fs::unlinkat(
                self.staging_parent().descriptor(),
                self.name.as_os_str(),
                rustix::fs::AtFlags::empty(),
            )
            .map_err(|error| unknown(error.into()))?;
        } else {
            // The private stage was 0600; the published replacement now
            // takes the verified original's mode on the published
            // descriptor. Post-crossing, so a failure is an unknown
            // outcome, never a clean denial.
            if let Some((original, _)) = &self.original {
                use std::os::unix::fs::PermissionsExt;
                // Only the ordinary bits cross: a staged publish is a
                // newly-owned, newly-written inode — set-ID and sticky
                // bits are never propagated onto it.
                let mode = original.metadata().map_err(unknown)?.permissions().mode();
                self.output
                    .set_permissions(std::fs::Permissions::from_mode(mode & 0o777))
                    .map_err(unknown)?;
            }
        }
        Ok(PublishedWrite {
            parent: self.parent.child(OsStr::new(".")).map_err(unknown)?,
            missing: Vec::new(),
            leaf: self.leaf.clone(),
            file: self.output.try_clone().map_err(unknown)?,
        })
    }

    /// Publishes a staged missing subtree: the first missing name must
    /// still be absent, the `.rustx-mkdir-*` staging directory and every
    /// registered owned entry are re-proven by device:inode, and the
    /// staging directory itself crosses into the retained parent under
    /// the first missing name by one `NOREPLACE` `renameat` — the commit
    /// crossing. The staging guard stays armed until the syscall
    /// succeeds, so any pre-crossing failure unwinds only proven-owned
    /// entries.
    ///
    /// The device:inode proofs and the `renameat` are not atomic: a
    /// same-euid writer that replaced the staging name between the
    /// verify and the syscall could redirect the rename onto a foreign
    /// directory — the checks narrow that window, they cannot close it
    /// (documented same-euid limitation, never claimed as a sandbox).
    /// A `NOREPLACE` failure, a claimed first name, or a substituted
    /// staged entry is a clean pre-commit conflict. After the crossing
    /// the published name is re-proven against the staging descriptor —
    /// a locator that no longer resolves to it reports [`CommitUnknown`],
    /// never certainty either way.
    fn publish_subtree_with_gate<G>(mut self, gate: G) -> io::Result<PublishedWrite>
    where
        G: FnOnce(&mut dyn FnMut() -> io::Result<()>) -> io::Result<()>,
    {
        let first = self.missing[0].clone();
        // The staged name must still be absent in the retained parent —
        // anything that claimed it since staging is a conflict, not a
        // target for replacement.
        if leaf_metadata(&self.parent, &first)?.is_some() {
            return Err(conflict());
        }
        self.staging
            .as_ref()
            .expect("subtree publish requires its staging directory")
            .verify(self.staging_parent(), &self.name)?;
        // One atomic crossing, inside the caller's gate: the staging
        // directory itself becomes `parent/first` — `NOREPLACE` so a
        // concurrent claim can never be overwritten. The staged
        // interior is never re-resolved by name.
        let mut crossed = false;
        let verdict = gate(&mut || -> io::Result<()> {
            renameat_with(
                self.staging_parent().descriptor(),
                self.name.as_os_str(),
                self.parent.descriptor(),
                first.as_os_str(),
                RenameFlags::NOREPLACE,
            )?;
            crossed = true;
            Ok(())
        });
        self.published |= crossed;
        verdict?;
        // Receipt revalidation: the published name must now resolve to
        // the exact staging inode. A same-euid swap across the crossing
        // leaves the outcome unknowable — never claim certainty.
        let stage = self
            .staging
            .as_ref()
            .expect("staging retained until the crossing");
        let published = nix::sys::stat::fstatat(
            self.parent.descriptor(),
            first.as_os_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(|error| unknown(error.into()))?;
        let own = nix::sys::stat::fstat(&stage.root).map_err(|error| unknown(error.into()))?;
        if (published.st_dev, published.st_ino) != (own.st_dev, own.st_ino) {
            return Err(unknown(io::Error::other(
                "published subtree name no longer names the staged directory",
            )));
        }
        Ok(PublishedWrite {
            parent: self.parent.child(OsStr::new(".")).map_err(unknown)?,
            missing: self.missing.clone(),
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
        if let Some(staging) = self.staging.take() {
            staging.remove(self.staging_parent(), &self.name);
            return;
        }
        let own = nix::sys::stat::fstat(&self.output);
        let named = nix::sys::stat::fstatat(
            self.staging_parent().descriptor(),
            self.name.as_os_str(),
            nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
        );
        if let (Ok(own), Ok(named)) = (own, named)
            && (own.st_dev, own.st_ino) == (named.st_dev, named.st_ino)
        {
            let _ = rustix::fs::unlinkat(
                self.staging_parent().descriptor(),
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
    /// The subtree names the publish crossed whole — the path from
    /// `parent` to the leaf's containing directory.
    missing: Vec<OsString>,
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
            // Descend through the published subtree to the leaf's own
            // containing directory — for `missing` empty that is the
            // retained parent itself.
            let mut containing = self.parent.child(OsStr::new("."))?;
            for component in &self.missing {
                containing = containing.child(component)?;
            }
            let named = nix::sys::stat::fstatat(
                containing.descriptor(),
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
            .filter(|name| {
                name.as_bytes().starts_with(b".rustx-write-")
                    || name.as_bytes().starts_with(b".rustx-mkdir-")
            })
            .collect();
        names.sort();
        names
    }

    fn revision(directory: &PackageDirectory, leaf: &str) -> String {
        directory.entry_metadata(Path::new(leaf)).unwrap().revision
    }

    #[test]
    fn an_owned_staging_directory_keeps_bytes_outside_the_destination_until_publication() {
        let (temporary, directory) = root();
        let inbox = temporary.path().join("stage");
        std::fs::create_dir(&inbox).unwrap();
        let staging = directory.child(OsStr::new("stage")).unwrap();
        let target = WriteTarget::prepare(
            directory,
            OsStr::new("file"),
            vec![OsString::from("subtree"), OsString::from("nested")],
            Expectation::Absent,
        )
        .unwrap();
        let staged = target
            .stage_at(Some(staging), |output| output.write_all(b"commit"))
            .unwrap();
        assert_eq!(staging_names(&inbox).len(), 1);
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
        assert!(!temporary.path().join("subtree").exists());
        staged.publish().unwrap().finish().unwrap();
        assert_eq!(
            std::fs::read(temporary.path().join("subtree/nested/file")).unwrap(),
            b"commit"
        );
        assert_eq!(std::fs::read_dir(inbox).unwrap().count(), 0);
    }

    #[test]
    fn a_leaf_swap_before_staging_is_rejected_and_everything_is_untouched() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        std::fs::write(&file, b"original").unwrap();
        let current = revision(&directory, "file");
        let target = WriteTarget::prepare(
            directory,
            OsStr::new("file"),
            Vec::new(),
            Expectation::Revision(&current),
        )
        .unwrap();
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
        let staged = WriteTarget::prepare(
            directory,
            OsStr::new("file"),
            Vec::new(),
            Expectation::Revision(&current),
        )
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
            Vec::new(),
            Expectation::Revision(&current),
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
        let staged = WriteTarget::prepare(
            directory,
            OsStr::new("file"),
            Vec::new(),
            Expectation::Revision(&revision_of(&file)),
        )
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
        let staged = WriteTarget::prepare(
            directory,
            OsStr::new("file"),
            Vec::new(),
            Expectation::Revision(&current),
        )
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
            Vec::new(),
            Expectation::Absent,
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
        let replaced = WriteTarget::prepare(
            directory,
            OsStr::new("file"),
            Vec::new(),
            Expectation::Revision(&current),
        )
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
    fn a_stage_is_private_and_only_ordinary_bits_reach_the_published_file() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        std::fs::write(&file, b"original").unwrap();
        // A world-readable, set-ID/sticky-marked original: none of the
        // exposure may reach the staged bytes, and only the ordinary
        // bits may reach the published replacement.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o4764)).unwrap();
        let current = revision(&directory, "file");
        let staged = WriteTarget::prepare(
            directory,
            OsStr::new("file"),
            Vec::new(),
            Expectation::Revision(&current),
        )
        .unwrap()
        .stage(|output| output.write_all(b"new"))
        .unwrap();
        let staging = staging_names(temporary.path());
        assert_eq!(staging.len(), 1);
        assert_eq!(
            std::fs::metadata(temporary.path().join(&staging[0]))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o600,
            "the staged payload never takes the original's mode"
        );
        staged.publish().unwrap().finish().unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777,
            0o764,
            "the published replacement keeps the ordinary bits, never set-ID/sticky"
        );
    }

    #[test]
    fn a_post_publication_failure_is_commit_unknown_and_keeps_the_bytes() {
        let (temporary, directory) = root();
        let file = temporary.path().join("file");
        std::fs::write(&file, b"original").unwrap();
        let current = revision(&directory, "file");
        let visible = WriteTarget::prepare(
            directory,
            OsStr::new("file"),
            Vec::new(),
            Expectation::Revision(&current),
        )
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
    fn a_missing_subtree_publishes_whole_as_one_rename() {
        let (temporary, directory) = root();
        let staged = WriteTarget::prepare(
            directory,
            OsStr::new("leaf.txt"),
            vec![OsString::from("deep"), OsString::from("deeper")],
            Expectation::Absent,
        )
        .unwrap()
        .stage(|output| output.write_all(b"subtree"))
        .unwrap();
        // Exactly one private staging directory, interior hidden.
        assert_eq!(staging_names(temporary.path()).len(), 1);
        assert!(!temporary.path().join("deep").exists());
        let published = staged.publish().unwrap().finish().unwrap();
        assert_eq!(
            std::fs::read(temporary.path().join("deep/deeper/leaf.txt")).unwrap(),
            b"subtree"
        );
        assert_eq!(published.size, 7);
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
    }

    #[test]
    fn a_missing_subtree_ignores_a_same_named_leaf_in_its_ancestor() {
        let (temporary, directory) = root();
        let root_leaf = temporary.path().join("leaf.txt");
        std::fs::write(&root_leaf, b"ancestor").unwrap();
        WriteTarget::prepare(
            directory,
            OsStr::new("leaf.txt"),
            vec!["deep".into(), "deeper".into()],
            Expectation::Absent,
        )
        .unwrap()
        .stage(|output| output.write_all(b"nested"))
        .unwrap()
        .publish()
        .unwrap()
        .finish()
        .unwrap();
        assert_eq!(std::fs::read(root_leaf).unwrap(), b"ancestor");
        assert_eq!(
            std::fs::read(temporary.path().join("deep/deeper/leaf.txt")).unwrap(),
            b"nested"
        );
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
    }

    #[test]
    fn a_nested_stage_syncs_every_parent_bottom_up() {
        let temporary = tempfile::tempdir().unwrap();
        let mut stage = SubtreeStage {
            root: File::open(temporary.path()).unwrap(),
            owned: Vec::new(),
        };
        let mut synced = Vec::new();
        stage
            .create_interior(
                &["a".into(), "b".into()],
                OsStr::new("leaf"),
                |output| output.write_all(b"x"),
                |directory| {
                    synced.push(nix::sys::stat::fstat(directory)?.st_ino);
                    Ok(())
                },
            )
            .unwrap();
        let inode = |relative| {
            nix::sys::stat::fstat(File::open(temporary.path().join(relative)).unwrap())
                .unwrap()
                .st_ino
        };
        assert_eq!(synced, [inode("a/b"), inode("a"), inode("")]);
    }

    #[test]
    fn a_writer_failure_mid_subtree_leaves_zero_residue() {
        let (temporary, directory) = root();
        let error = WriteTarget::prepare(
            directory,
            OsStr::new("leaf.txt"),
            vec![OsString::from("deep"), OsString::from("deeper")],
            Expectation::Absent,
        )
        .unwrap()
        .stage(|output| {
            output.write_all(b"partial").unwrap();
            Err(io::Error::other("injected writer failure"))
        })
        .err()
        .expect("the writer failure must surface");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        // Every created interior dir, the staged leaf, and the staging
        // directory itself are removed — nothing claimed `deep`.
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
        assert!(!temporary.path().join("deep").exists());
    }

    #[test]
    fn a_dropped_subtree_stage_removes_only_what_it_created() {
        let (temporary, directory) = root();
        let staged = WriteTarget::prepare(
            directory,
            OsStr::new("leaf.txt"),
            vec![OsString::from("deep"), OsString::from("deeper")],
            Expectation::Absent,
        )
        .unwrap()
        .stage(|output| output.write_all(b"subtree"))
        .unwrap();
        assert_eq!(staging_names(temporary.path()).len(), 1);
        drop(staged);
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
        assert!(!temporary.path().join("deep").exists());
    }

    #[test]
    fn a_competing_first_missing_name_is_never_modified() {
        let (temporary, directory) = root();
        let staged = WriteTarget::prepare(
            directory,
            OsStr::new("leaf.txt"),
            vec![OsString::from("deep"), OsString::from("deeper")],
            Expectation::Absent,
        )
        .unwrap()
        .stage(|output| output.write_all(b"subtree"))
        .unwrap();
        // A competitor claims the first missing name with its own data.
        std::fs::create_dir(temporary.path().join("deep")).unwrap();
        std::fs::write(temporary.path().join("deep/competitor.txt"), b"mine").unwrap();
        let error = staged
            .publish()
            .err()
            .expect("a claimed first name must conflict");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(temporary.path().join("deep/competitor.txt")).unwrap(),
            b"mine"
        );
        assert_eq!(staging_names(temporary.path()), Vec::<OsString>::new());
    }

    #[test]
    fn a_substituted_staged_leaf_is_denied_and_never_deleted() {
        let (temporary, directory) = root();
        let staged = WriteTarget::prepare(
            directory,
            OsStr::new("leaf.txt"),
            vec![OsString::from("deep")],
            Expectation::Absent,
        )
        .unwrap()
        .stage(|output| output.write_all(b"subtree"))
        .unwrap();
        // Replace the staged leaf inside the private staging directory
        // with a foreign file — a different inode under the same name.
        let staging = temporary.path().join(&staging_names(temporary.path())[0]);
        let foreign = staging.join("leaf.txt");
        std::fs::remove_file(&foreign).unwrap();
        std::fs::write(&foreign, b"foreign").unwrap();
        let error = staged
            .publish()
            .err()
            .expect("a substituted staged entry must be denied");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        // Cleanup re-proves every name: the foreign leaf is never
        // unlinked and its containing directories stay with it.
        assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign");
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
        let visible = WriteTarget::prepare(
            parent,
            OsStr::new("file"),
            Vec::new(),
            Expectation::Revision(&current),
        )
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
