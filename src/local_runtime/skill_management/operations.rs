use super::super::configuration::{
    UserConfigManager,
    observation::digest,
    settings::{
        ResourceTransfer, SettingsError, SourceMutation, SourceTarget, TransferOperation,
        TransferRejection,
    },
};
use super::super::settings::{SourceDirectory, is_writer_artifact};
use crate::package_io::{self, PackageDirectory};
use crate::runtime::{CancellationSignal, resources::SourceScope};
use crate::skills::{
    AutomaticSkillRoot, SKILLS_ROOT, SkillDiscovery, SkillDiscoveryConfig, SkillDiscoveryOutcome,
    SkillPackage, SkillSource,
    package::{SKILL_MARKDOWN_FILE, validate_skill_name},
};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, RenameFlags, renameat_with};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, Metadata, Permissions};
use std::io::{self, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const OWNED_DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn managed_root<'a>(package: &'a SkillPackage, collection: &Path) -> Option<&'a Path> {
    let marker = Path::new(package.location());
    let root = marker.parent()?;
    (marker.file_name()? == SKILL_MARKDOWN_FILE
        && marker == package.instruction_source()
        && root != collection
        && root.starts_with(collection))
    .then_some(root)
}

fn directory_metadata(directory: &PackageDirectory) -> io::Result<Metadata> {
    File::from(directory.descriptor().try_clone()?).metadata()
}

fn stamp(meta: &Metadata) -> (u64, u64, u64, u32, i64, i64, i64, i64) {
    (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mode(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

/// Listing fingerprints the admitted directory and instruction file without
/// opening any supporting files. Transfers validate the full tree at commit.
pub(super) fn operation_revision(package: &SkillPackage, collection: &Path) -> Option<String> {
    let root = managed_root(package, collection)?;
    let directory = PackageDirectory::open(root).ok()?;
    let root_metadata = directory_metadata(&directory).ok()?;
    let marker = directory
        .file(Path::new(SKILL_MARKDOWN_FILE))
        .ok()?
        .metadata()
        .ok()?;
    digest(&(
        root,
        root_metadata.dev(),
        root_metadata.ino(),
        stamp(&marker),
        package.version_id(),
    ))
    .ok()
}

#[derive(PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    directory: bool,
    stamp: (u64, u64, u64, u32, i64, i64, i64, i64),
}

struct Tree {
    device: u64,
    /// Package-root mode captured with the scan. It is not an entry.
    root_mode: u32,
    entries: Vec<Entry>,
    bytes_left: u64,
    entries_left: usize,
    copying: bool,
}

impl Tree {
    fn scan(
        directory: &PackageDirectory,
        signal: &CancellationSignal,
        copying: bool,
    ) -> io::Result<Self> {
        let root = directory_metadata(directory)?;
        let mut tree = Self {
            device: root.dev(),
            root_mode: root.mode(),
            entries: Vec::new(),
            bytes_left: MAX_BYTES,
            entries_left: MAX_ENTRIES,
            copying,
        };
        tree.walk(directory, Path::new(""), 0, signal)?;
        directory.verify()?;
        Ok(tree)
    }

    fn walk(
        &mut self,
        directory: &PackageDirectory,
        prefix: &Path,
        depth: usize,
        signal: &CancellationSignal,
    ) -> io::Result<()> {
        package_io::check_cancelled(signal)?;
        if depth > 64 {
            return Err(io::Error::other(
                "Skill package exceeds 64 directory levels",
            ));
        }
        for name in directory.names(MAX_ENTRIES)? {
            if is_writer_artifact(name.as_os_str()) {
                continue;
            }
            self.entries_left = self
                .entries_left
                .checked_sub(1)
                .ok_or_else(|| io::Error::other("Skill package exceeds 4096 entries"))?;
            let path = prefix.join(&name);
            let child = directory.child(name.as_os_str()).ok();
            let meta = if let Some(child) = &child {
                directory_metadata(child)?
            } else if self.copying {
                directory.file(&name)?.metadata()?
            } else {
                // Retirement unlinks these entries without following them.
                continue;
            };
            if meta.dev() != self.device {
                return Err(io::Error::other(
                    "Skill package crosses a filesystem boundary",
                ));
            }
            self.entries.push(Entry {
                path: path.clone(),
                directory: child.is_some(),
                stamp: stamp(&meta),
            });
            if let Some(child) = child {
                self.walk(&child, &path, depth + 1, signal)?;
            } else {
                self.bytes_left = self
                    .bytes_left
                    .checked_sub(meta.len())
                    .ok_or_else(|| io::Error::other("Skill package exceeds 256 MiB"))?;
            }
        }
        Ok(())
    }

    fn copy_to(
        &self,
        source: &PackageDirectory,
        destination: &Path,
        signal: &CancellationSignal,
        mut on_content: impl FnMut(),
    ) -> io::Result<()> {
        // The destination is the already-created stage directory. Creating it
        // again would nest the package and force a cross-parent publish rename.
        let mut signaled = false;
        for entry in &self.entries {
            let path = destination.join(&entry.path);
            if entry.directory {
                fs::create_dir(path)?;
            } else {
                if !signaled {
                    signaled = true;
                    on_content();
                }
                let mut output = File::options().write(true).create_new(true).open(&path)?;
                let mut remaining = entry.stamp.2;
                let observed = package_io::stream(source, &entry.path, signal, |chunk| {
                    remaining = remaining
                        .checked_sub(chunk.len() as u64)
                        .ok_or_else(|| io::Error::other("Skill package changed during transfer"))?;
                    output.write_all(chunk)
                })?;
                if stamp(&observed.metadata) != entry.stamp {
                    return Err(io::Error::other("Skill package changed during transfer"));
                }
                output.set_permissions(Permissions::from_mode(observed.metadata.mode() & 0o777))?;
                output.sync_all()?;
            }
        }
        if !signaled {
            on_content();
        }
        Ok(())
    }

    /// Restore recorded subdirectory modes only after every child has been
    /// written. Setting a private parent first would block the copy that
    /// still needs it. The stage root stays writable (`0700`): renaming a
    /// non-writable directory fails with EACCES on XNU (seen on the macOS-15
    /// runners), so `root_mode` is applied to the published directory only
    /// after the same-parent publish rename.
    fn restore_directory_modes(&self, destination: &Path) -> io::Result<()> {
        for entry in self.entries.iter().rev().filter(|entry| entry.directory) {
            fs::set_permissions(
                destination.join(&entry.path),
                Permissions::from_mode(entry.stamp.3 & 0o777),
            )?;
        }
        fs::set_permissions(destination, Permissions::from_mode(0o700))
    }

    fn sync_directories(&self, destination: &Path) -> io::Result<()> {
        for entry in self.entries.iter().rev().filter(|entry| entry.directory) {
            File::open(destination.join(&entry.path))?.sync_all()?;
        }
        File::open(destination)?.sync_all()
    }
}

fn stage_in(parent: &Path) -> io::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(".rustx-skill-")
        .tempdir_in(parent)
}

fn lock_skill_collections<'a>(
    source_target: &SourceTarget,
    source: &'a SourceDirectory,
    destination: &'a SourceDirectory,
) -> Result<(File, File), SettingsError> {
    let (first, second) = if source_target.scope() == SourceScope::User {
        (source, destination)
    } else {
        (destination, source)
    };
    let first = first
        .lock(OsStr::new(SKILLS_ROOT))
        .map_err(SettingsError::Io)?;
    let second = second
        .lock(OsStr::new(SKILLS_ROOT))
        .map_err(SettingsError::Io)?;
    Ok((first, second))
}

/// An unpublished stage directory in its destination collection.
///
/// Publication is a same-parent rename, so dropping this removes only the
/// original stage name. It never follows a replacement or changes a published
/// package's mode.
struct OwnedStage {
    path: PathBuf,
    name: OsString,
    identity: (u64, u64),
    cleanup: bool,
}

impl OwnedStage {
    fn create(parent: &Path) -> io::Result<Self> {
        let temporary = stage_in(parent)?;
        let path = temporary.keep();
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("skill stage has no name"))?
            .to_os_string();
        let directory = PackageDirectory::open(&path)?;
        let metadata = directory_metadata(&directory)?;
        Ok(Self {
            path,
            name,
            identity: (metadata.dev(), metadata.ino()),
            cleanup: true,
        })
    }

    fn disarm(&mut self) {
        self.cleanup = false;
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OwnedStage {
    fn drop(&mut self) {
        if self.cleanup {
            let _ = remove_owned_tree(&self.path, self.identity);
        }
    }
}

/// A staged Skill package whose publication has not started.
/// Dropping it removes only the unpublished stage.
pub(in crate::local_runtime) struct PreparedSkillTransfer {
    source_target: SourceTarget,
    destination_target: SourceTarget,
    operation: TransferOperation,
    source_name: String,
    destination_name: String,
    expected: String,
    stage: OwnedStage,
    tree: Tree,
    source_identity: (u64, u64),
}

fn publish(
    stage: &mut OwnedStage,
    destination: &SourceDirectory,
    installed: &Path,
) -> Result<PackageDirectory, SettingsError> {
    let name = installed.file_name().ok_or(SettingsError::Invalid)?;
    destination.confirm().map_err(SettingsError::Io)?;
    if !stage_name_is(destination, &stage.name, stage.identity) {
        return Err(SettingsError::Io(std::io::Error::other(
            "stage directory replaced externally",
        )));
    }
    // Same-parent rename does not update `..`, so a non-writable package root
    // can be published without widening its mode.
    match renameat_with(
        destination.descriptor(),
        &stage.name,
        destination.descriptor(),
        name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => {
            stage.disarm();
            destination.sync().map_err(|_| SettingsError::Committed)?;
        }
        Err(rustix::io::Errno::EXIST) => {
            return Err(SettingsError::TransferRejected(
                TransferRejection::NameTaken,
            ));
        }
        Err(error) => return Err(SettingsError::Io(error.into())),
    }
    let directory = PackageDirectory::open(installed).map_err(|_| SettingsError::Committed)?;
    let identity = directory_metadata(&directory).map_err(|_| SettingsError::Committed)?;
    if (identity.dev(), identity.ino()) != stage.identity {
        return Err(SettingsError::Committed);
    }
    Ok(directory)
}

fn stage_name_is(parent: &SourceDirectory, name: &OsStr, identity: (u64, u64)) -> bool {
    // `O_DIRECTORY | O_NOFOLLOW` rejects a symlink without following it.
    // The stored identity comes from `std` metadata, whose `dev`/`ino` are
    // already normalized; rustix `st_dev` is `i32` on Darwin and must not be
    // compared with that tuple directly.
    let Ok(opened) = rustix::fs::openat(parent.descriptor(), name, OWNED_DIRECTORY, Mode::empty())
    else {
        return false;
    };
    let Ok(listed) = rustix::fs::statat(parent.descriptor(), name, AtFlags::SYMLINK_NOFOLLOW)
    else {
        return false;
    };
    let Ok(current) = rustix::fs::fstat(&opened) else {
        return false;
    };
    if (listed.st_dev, listed.st_ino) != (current.st_dev, current.st_ino) {
        return false;
    }
    File::from(opened)
        .metadata()
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == identity)
}

/// First remove the directory from discovery with a same-parent rename.
/// Retain a replaced entry if the name changed owners during the rename.
fn retire(path: &Path, directory: &PackageDirectory) -> Result<(), SettingsError> {
    let parent = path.parent().ok_or(SettingsError::Invalid)?;
    let source = SourceDirectory::observe(parent)
        .map_err(SettingsError::Io)?
        .ok_or(SettingsError::Invalid)?;
    let name = path.file_name().ok_or(SettingsError::Invalid)?;
    let metadata = directory_metadata(directory).map_err(SettingsError::Io)?;
    let identity = (metadata.dev(), metadata.ino());
    // Keep the special bits (setgid/sticky) as well as the access bits:
    // 0o777 would silently drop them when the mode is restored on failure.
    let original_mode = Mode::from_raw_mode((metadata.mode() & 0o7777) as _);
    source.confirm().map_err(SettingsError::Io)?;
    directory.verify().map_err(|_| SettingsError::Invalid)?;
    // Renaming a non-writable directory fails with EACCES on XNU (seen on the
    // macOS-15 runners): widen the root before moving it aside. If the move
    // never happens the package stays in place, so the recorded mode must be
    // put back on every failure path below.
    rustix::fs::fchmod(directory.descriptor(), Mode::from_raw_mode(0o700))
        .map_err(|error| SettingsError::Io(error.into()))?;
    let stage_name = match rename_aside(&source, name) {
        Ok(stage_name) => stage_name,
        Err(error) => {
            let _ = rustix::fs::fchmod(directory.descriptor(), original_mode);
            return Err(error);
        }
    };
    let stage_path = parent.join(&stage_name);
    let Ok(retired) = PackageDirectory::open(&stage_path) else {
        return Err(SettingsError::Committed);
    };
    let moved = directory_metadata(&retired).map_err(|_| SettingsError::Committed)?;
    if (moved.dev(), moved.ino()) != identity {
        if renameat_with(
            source.descriptor(),
            &stage_name,
            source.descriptor(),
            name,
            RenameFlags::NOREPLACE,
        )
        .is_err()
            || source.sync().is_err()
        {
            return Err(SettingsError::Committed);
        }
        let _ = rustix::fs::fchmod(directory.descriptor(), original_mode);
        return Err(SettingsError::Invalid);
    }
    if source.sync().is_err() {
        return Err(SettingsError::Committed);
    }
    remove_owned_tree(&stage_path, identity).map_err(|_| SettingsError::Committed)
}

fn rename_aside(parent: &SourceDirectory, name: &OsStr) -> Result<OsString, SettingsError> {
    for _ in 0..8 {
        let stage_name = format!(".rustx-skill-{}", uuid::Uuid::now_v7().simple());
        match renameat_with(
            parent.descriptor(),
            name,
            parent.descriptor(),
            stage_name.as_str(),
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => return Ok(OsString::from(stage_name)),
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(SettingsError::Io(error.into())),
        }
    }
    Err(SettingsError::Io(std::io::Error::other(
        "stage names exhausted",
    )))
}

/// Remove one unpublished or retired tree after proving it is still the inode
/// this owner created or renamed aside. A replaced symlink or directory is
/// left untouched, including its mode.
fn remove_owned_tree(path: &Path, identity: (u64, u64)) -> io::Result<()> {
    let directory = PackageDirectory::open(path)?;
    directory.verify()?;
    let metadata = directory_metadata(&directory)?;
    if (metadata.dev(), metadata.ino()) != identity {
        return Err(io::Error::other("refusing to clean a replaced skill stage"));
    }
    clear_owned_directory(directory.descriptor())?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("skill stage has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("skill stage has no name"))?;
    let parent = PackageDirectory::open(parent)?;
    parent.verify()?;
    let listed = rustix::fs::statat(parent.descriptor(), name, AtFlags::SYMLINK_NOFOLLOW)?;
    let opened = rustix::fs::fstat(directory.descriptor())?;
    if (listed.st_dev, listed.st_ino) != (opened.st_dev, opened.st_ino) {
        return Err(io::Error::other(
            "refusing to remove a replaced skill stage",
        ));
    }
    rustix::fs::unlinkat(parent.descriptor(), name, AtFlags::REMOVEDIR)?;
    Ok(())
}

fn clear_owned_directory(fd: &OwnedFd) -> io::Result<()> {
    let identity = rustix::fs::fstat(fd)?;
    rustix::fs::fchmod(fd, Mode::from_raw_mode(0o700))?;
    let after = rustix::fs::fstat(fd)?;
    if (after.st_dev, after.st_ino) != (identity.st_dev, identity.st_ino) {
        return Err(io::Error::other(
            "skill directory identity changed during cleanup",
        ));
    }
    let mut names = Vec::new();
    for entry in rustix::fs::Dir::read_from(fd)? {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        names.push(OsStr::from_bytes(bytes).to_os_string());
    }
    for name in names {
        let listed = rustix::fs::statat(fd, &name, AtFlags::SYMLINK_NOFOLLOW)?;
        let kind = FileType::from_raw_mode(listed.st_mode);
        if !kind.is_dir() || kind.is_symlink() {
            rustix::fs::unlinkat(fd, &name, AtFlags::empty())?;
            continue;
        }
        let child = rustix::fs::openat(fd, &name, OWNED_DIRECTORY, Mode::empty())?;
        let opened = rustix::fs::fstat(&child)?;
        if (opened.st_dev, opened.st_ino) != (listed.st_dev, listed.st_ino) {
            return Err(io::Error::other(
                "refusing to follow a replaced skill entry",
            ));
        }
        clear_owned_directory(&child)?;
        let again = rustix::fs::statat(fd, &name, AtFlags::SYMLINK_NOFOLLOW)?;
        if (again.st_dev, again.st_ino) != (opened.st_dev, opened.st_ino) {
            return Err(io::Error::other(
                "refusing to remove a replaced skill entry",
            ));
        }
        rustix::fs::unlinkat(fd, &name, AtFlags::REMOVEDIR)?;
    }
    Ok(())
}

impl UserConfigManager {
    fn skill_collection(&self, target: &SourceTarget) -> PathBuf {
        self.resource_root(target).join(SKILLS_ROOT)
    }

    fn discover_skill_scope(&self, target: &SourceTarget, root: &Path) -> SkillDiscoveryOutcome {
        let source = match target.scope() {
            SourceScope::User => SkillSource::User,
            SourceScope::Workspace => SkillSource::Workspace,
        };
        SkillDiscovery::with_config(SkillDiscoveryConfig {
            automatic: vec![AutomaticSkillRoot {
                source,
                root: root.to_path_buf(),
            }],
        })
        .with_cancellation(self.resource_io.clone())
        .discover()
    }

    fn skill_for_operation(
        &self,
        target: &SourceTarget,
        collection: &Path,
        name: &str,
        expected: &str,
    ) -> Result<SkillPackage, SettingsError> {
        let package = self
            .discover_skill_scope(target, collection)
            .packages
            .into_iter()
            .find(|package| package.name() == name);
        let actual = package
            .as_ref()
            .and_then(|package| operation_revision(package, collection))
            .unwrap_or_else(|| "missing".into());
        if actual != expected {
            return Err(SettingsError::Conflict {
                scope: target.scope(),
                expected: expected.into(),
                actual,
            });
        }
        package.ok_or(SettingsError::Invalid)
    }

    pub(in crate::local_runtime) fn delete_skill(
        &self,
        target: &SourceTarget,
        name: &str,
        expected: &str,
    ) -> Result<String, SettingsError> {
        let collection = self.skill_collection(target);
        let directory = SourceDirectory::observe(&collection)
            .map_err(SettingsError::Io)?
            .ok_or(SettingsError::Invalid)?;
        let _lock = directory
            .lock(OsStr::new(SKILLS_ROOT))
            .map_err(SettingsError::Io)?;
        let package = self.skill_for_operation(target, &collection, name, expected)?;
        let root = managed_root(&package, &collection).ok_or(SettingsError::Invalid)?;
        let package_dir = PackageDirectory::open(root).map_err(SettingsError::Io)?;
        Tree::scan(&package_dir, &self.resource_io, false).map_err(|_| SettingsError::Invalid)?;
        self.skill_for_operation(target, &collection, name, expected)?;
        retire(root, &package_dir)?;
        Ok("missing".into())
    }

    /// Stage a Skill transfer before the caller takes the application lock.
    ///
    /// Non-Skill mutations return `Ok(None)` and are committed by the caller.
    pub(in crate::local_runtime) fn prepare_skill_content(
        &self,
        target: &SourceTarget,
        expected: &str,
        mutation: &SourceMutation,
    ) -> Result<Option<PreparedSkillTransfer>, SettingsError> {
        let SourceMutation::ResourceTransfer {
            from,
            operation,
            resource:
                ResourceTransfer::Skill {
                    source,
                    destination,
                },
        } = mutation
        else {
            return Ok(None);
        };
        if target.scope() != SourceScope::Workspace {
            return Err(SettingsError::Invalid);
        }
        let user = SourceTarget::User;
        let (source_target, destination_target) = if *from == SourceScope::User {
            (user, target.clone())
        } else {
            (target.clone(), user)
        };
        self.prepare_skill_transfer(
            &source_target,
            &destination_target,
            *operation,
            source,
            destination,
            expected,
        )
        .map(Some)
    }

    /// Stage one Skill package without publishing it.
    ///
    /// Collection locks are not held across the copy or sync. The caller can
    /// therefore take the process-wide application lock, and a concurrent
    /// skill mutation can take the collection locks, without waiting for this
    /// staging to finish. Publication is [`Self::commit_skill_transfer`].
    pub(in crate::local_runtime) fn prepare_skill_transfer(
        &self,
        source_target: &SourceTarget,
        destination_target: &SourceTarget,
        operation: TransferOperation,
        source_name: &str,
        destination_name: &str,
        expected: &str,
    ) -> Result<PreparedSkillTransfer, SettingsError> {
        validate_skill_name(destination_name).map_err(|_| SettingsError::Invalid)?;
        if operation == TransferOperation::Copy && source_name == destination_name {
            return Err(SettingsError::TransferRejected(
                TransferRejection::NameTaken,
            ));
        }
        let source_collection = self.skill_collection(source_target);
        let destination_collection = self.skill_collection(destination_target);
        let source = SourceDirectory::observe(&source_collection)
            .map_err(SettingsError::Io)?
            .ok_or(SettingsError::Invalid)?;
        let destination = SourceDirectory::create_directory(&destination_collection)
            .map_err(SettingsError::Io)?;
        if source.within(&destination).map_err(SettingsError::Io)?
            || destination.within(&source).map_err(SettingsError::Io)?
        {
            return Err(SettingsError::TransferRejected(
                TransferRejection::SameDirectory,
            ));
        }
        let package =
            self.skill_for_operation(source_target, &source_collection, source_name, expected)?;
        let root = managed_root(&package, &source_collection).ok_or(SettingsError::Invalid)?;
        let package_dir = PackageDirectory::open(root).map_err(SettingsError::Io)?;
        let moving = operation == TransferOperation::Move;
        self.admit_skill_destination(
            source_target,
            destination_target,
            source_name,
            destination_name,
            moving,
        )?;
        let identity = directory_metadata(&package_dir).map_err(SettingsError::Io)?;
        let source_identity = (identity.dev(), identity.ino());
        let stage = OwnedStage::create(&destination_collection).map_err(SettingsError::Io)?;
        let tree = Tree::scan(&package_dir, &self.resource_io, true)
            .map_err(|_| SettingsError::Invalid)?;
        if let Err(error) = tree.copy_to(&package_dir, stage.path(), &self.resource_io, || {
            #[cfg(test)]
            self.test_hooks.reach("skill_content_preparing");
        }) {
            return Err(self.classify_staging_failure(
                source_target,
                &source_collection,
                source_name,
                expected,
                &tree,
                error,
            ));
        }
        if (!moving || source_name != destination_name)
            && let Err(error) = crate::skills::frontmatter::rewrite(
                &stage.path().join(SKILL_MARKDOWN_FILE),
                destination_name,
                !moving,
                &self.resource_io,
            )
        {
            return Err(self.classify_staging_failure(
                source_target,
                &source_collection,
                source_name,
                expected,
                &tree,
                error,
            ));
        }
        // Directory modes are restored only after the staged tree is complete.
        tree.restore_directory_modes(stage.path())
            .map_err(SettingsError::Io)?;
        tree.sync_directories(stage.path())
            .map_err(SettingsError::Io)?;
        Ok(PreparedSkillTransfer {
            source_target: source_target.clone(),
            destination_target: destination_target.clone(),
            operation,
            source_name: source_name.to_owned(),
            destination_name: destination_name.to_owned(),
            expected: expected.to_owned(),
            stage,
            tree,
            source_identity,
        })
    }

    /// Revalidate and publish a prepared Skill package.
    ///
    /// Callers that own configuration application responsibility acquire that
    /// lock before this commit. Collection locks are taken here, User before
    /// Workspace, and every identity, revision, conflict, and selection check
    /// is repeated against the live source before publication.
    pub(in crate::local_runtime) fn commit_skill_transfer(
        &self,
        prepared: PreparedSkillTransfer,
    ) -> Result<String, SettingsError> {
        let PreparedSkillTransfer {
            source_target,
            destination_target,
            operation,
            source_name,
            destination_name,
            expected,
            mut stage,
            tree,
            source_identity,
        } = prepared;
        let source_collection = self.skill_collection(&source_target);
        let destination_collection = self.skill_collection(&destination_target);
        let source = SourceDirectory::observe(&source_collection)
            .map_err(SettingsError::Io)?
            .ok_or(SettingsError::Invalid)?;
        let destination = SourceDirectory::create_directory(&destination_collection)
            .map_err(SettingsError::Io)?;
        if source.within(&destination).map_err(SettingsError::Io)?
            || destination.within(&source).map_err(SettingsError::Io)?
        {
            return Err(SettingsError::TransferRejected(
                TransferRejection::SameDirectory,
            ));
        }
        let (_first, _second) = lock_skill_collections(&source_target, &source, &destination)?;
        let package =
            self.skill_for_operation(&source_target, &source_collection, &source_name, &expected)?;
        let root = managed_root(&package, &source_collection).ok_or(SettingsError::Invalid)?;
        let package_dir = PackageDirectory::open(root).map_err(SettingsError::Io)?;
        let identity = directory_metadata(&package_dir).map_err(SettingsError::Io)?;
        if (identity.dev(), identity.ino()) != source_identity || identity.mode() != tree.root_mode
        {
            return Err(SettingsError::Invalid);
        }
        let moving = operation == TransferOperation::Move;
        self.admit_skill_destination(
            &source_target,
            &destination_target,
            &source_name,
            &destination_name,
            moving,
        )?;
        let latest = Tree::scan(&package_dir, &self.resource_io, true)
            .map_err(|_| SettingsError::Invalid)?;
        if latest.entries != tree.entries || latest.root_mode != tree.root_mode || !source.holds() {
            return Err(SettingsError::Invalid);
        }
        let installed = destination_collection.join(&destination_name);
        let installed_directory = publish(&mut stage, &destination, &installed)?;
        // The stage root stayed writable through the publish rename (see
        // `restore_directory_modes`); the published package now takes the
        // recorded root mode. Chmod through the descriptor so a local writer
        // cannot redirect the call by replacing `installed` after publish.
        rustix::fs::fchmod(
            installed_directory.descriptor(),
            Mode::from_raw_mode((tree.root_mode & 0o777) as _),
        )
        .map_err(|_| SettingsError::Committed)?;
        let visible = self
            .discover_skill_scope(&destination_target, &destination_collection)
            .packages
            .into_iter()
            .find(|package| {
                package.name() == destination_name
                    && package.instruction_source() == installed.join(SKILL_MARKDOWN_FILE)
            });
        let Some(revision) = visible
            .as_ref()
            .and_then(|package| operation_revision(package, &destination_collection))
        else {
            retire(&installed, &installed_directory).map_err(|_| SettingsError::Committed)?;
            return Err(SettingsError::Invalid);
        };
        if moving {
            installed_directory
                .verify()
                .map_err(|_| SettingsError::Committed)?;
            let final_source = Tree::scan(&package_dir, &self.resource_io, true)
                .map_err(|_| SettingsError::Committed)?;
            if final_source.entries != tree.entries || final_source.root_mode != tree.root_mode {
                return Err(SettingsError::Committed);
            }
            retire(root, &package_dir).map_err(|_| SettingsError::Committed)?;
        }
        Ok(revision)
    }

    pub(in crate::local_runtime) fn transfer_skill(
        &self,
        source_target: &SourceTarget,
        destination_target: &SourceTarget,
        operation: TransferOperation,
        source_name: &str,
        destination_name: &str,
        expected: &str,
    ) -> Result<String, SettingsError> {
        let prepared = self.prepare_skill_transfer(
            source_target,
            destination_target,
            operation,
            source_name,
            destination_name,
            expected,
        )?;
        self.commit_skill_transfer(prepared)
    }

    #[cfg(test)]
    pub(crate) fn skill_operation_revision(
        &self,
        target: &SourceTarget,
        name: &str,
    ) -> Option<String> {
        let collection = self.skill_collection(target);
        let package = self
            .discover_skill_scope(target, &collection)
            .packages
            .into_iter()
            .find(|package| package.name() == name)?;
        operation_revision(&package, &collection)
    }

    fn classify_staging_failure(
        &self,
        source_target: &SourceTarget,
        collection: &Path,
        name: &str,
        expected: &str,
        snapshot: &Tree,
        _error: io::Error,
    ) -> SettingsError {
        let package = match self.skill_for_operation(source_target, collection, name, expected) {
            Ok(package) => package,
            Err(error) => return error,
        };
        let Some(root) = managed_root(&package, collection) else {
            return SettingsError::Invalid;
        };
        let changed = PackageDirectory::open(root)
            .and_then(|directory| Tree::scan(&directory, &self.resource_io, true));
        match changed {
            Ok(latest)
                if latest.entries != snapshot.entries || latest.root_mode != snapshot.root_mode =>
            {
                SettingsError::Invalid
            }
            Ok(_) => SettingsError::Io(std::io::Error::other(
                "post-operation tree still equals its snapshot",
            )),
            Err(error) => match self.skill_for_operation(source_target, collection, name, expected)
            {
                Err(error @ SettingsError::Conflict { .. }) => error,
                _ => SettingsError::Io(error),
            },
        }
    }

    fn admit_skill_destination(
        &self,
        source: &SourceTarget,
        destination: &SourceTarget,
        source_name: &str,
        name: &str,
        moving: bool,
    ) -> Result<(), SettingsError> {
        for target in [source, destination] {
            let collection = self.skill_collection(target);
            let discovered = self.discover_skill_scope(target, &collection);
            if !(target == source && source_name == name && moving)
                && discovered
                    .provenance
                    .iter()
                    .chain(&discovered.invalid)
                    .any(|row| row.name == name)
            {
                return Err(SettingsError::TransferRejected(
                    TransferRejection::NameTaken,
                ));
            }
            if !moving {
                let (settings, _) = self.capture_authored_settings(target, None)?;
                let selection = settings
                    .ok_or(SettingsError::Invalid)?
                    .agent
                    .and_then(|agent| agent.skills);
                let (agents, _) = crate::local_runtime::agent_resources::load_authorized(
                    target.workspace(),
                    &self.resource_root(&SourceTarget::User).join("agents"),
                )
                .map_err(|_| SettingsError::Invalid)?;
                // Exact selectors explicitly enable a package, including a
                // copy whose default is disabled. Check named Agents as well
                // as the root before introducing a formerly missing identity.
                let named_selected = agents
                    .definitions()
                    .any(|agent| agent.skills().iter().any(|selected| selected == name));
                if named_selected
                    || selection.is_some_and(|selection| {
                        selection.names().iter().any(|selected| selected == name)
                    })
                {
                    return Err(SettingsError::TransferRejected(
                        TransferRejection::NameEnabled,
                    ));
                }
            }
        }
        Ok(())
    }
}
