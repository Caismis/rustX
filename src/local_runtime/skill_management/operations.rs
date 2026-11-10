use super::super::configuration::{
    UserConfigManager,
    observation::digest,
    settings::{SettingsError, SourceTarget, TransferOperation, TransferRejection},
};
use super::super::settings::{SourceDirectory, is_writer_artifact};
use crate::package_io::{self, PackageDirectory};
use crate::runtime::{CancellationSignal, resources::SourceScope};
use crate::skills::{
    AutomaticSkillRoot, SKILLS_ROOT, SkillDiscovery, SkillDiscoveryConfig, SkillDiscoveryOutcome,
    SkillPackage, SkillSource,
    package::{SKILL_MARKDOWN_FILE, validate_skill_name},
};
use rustix::fs::{RenameFlags, renameat_with};
use std::ffi::OsStr;
use std::fs::{self, File, Metadata, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const MAX_ENTRIES: usize = 4096;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const PACKAGE: &str = "package";

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
        let mut tree = Self {
            device: directory_metadata(directory)?.dev(),
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
    ) -> io::Result<()> {
        fs::create_dir(destination)?;
        for entry in &self.entries {
            let path = destination.join(&entry.path);
            if entry.directory {
                fs::create_dir(path)?;
            } else {
                let mut output = File::options().write(true).create_new(true).open(path)?;
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
        Ok(())
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

fn publish(
    stage: &PackageDirectory,
    destination: &SourceDirectory,
    installed: &Path,
) -> Result<PackageDirectory, SettingsError> {
    let name = installed.file_name().ok_or(SettingsError::Invalid)?;
    let staged_identity = stage
        .child(OsStr::new(PACKAGE))
        .and_then(|staged| directory_metadata(&staged))
        .map_err(|_| SettingsError::Io)?;
    stage.verify().map_err(|_| SettingsError::Io)?;
    destination.confirm().map_err(|_| SettingsError::Io)?;
    match renameat_with(
        stage.descriptor(),
        PACKAGE,
        destination.descriptor(),
        name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => destination.sync().map_err(|_| SettingsError::Committed)?,
        Err(rustix::io::Errno::EXIST) => {
            return Err(SettingsError::TransferRejected(
                TransferRejection::NameTaken,
            ));
        }
        Err(_) => return Err(SettingsError::Io),
    }
    let directory = PackageDirectory::open(installed).map_err(|_| SettingsError::Committed)?;
    let identity = directory_metadata(&directory).map_err(|_| SettingsError::Committed)?;
    if (staged_identity.dev(), staged_identity.ino()) != (identity.dev(), identity.ino()) {
        return Err(SettingsError::Committed);
    }
    Ok(directory)
}

/// First remove the directory from discovery with a single rename. Retain a
/// replaced entry if the name changed owners during the rename.
fn retire(path: &Path, directory: &PackageDirectory) -> Result<(), SettingsError> {
    let parent = path.parent().ok_or(SettingsError::Invalid)?;
    let source = SourceDirectory::observe(parent)
        .map_err(|_| SettingsError::Io)?
        .ok_or(SettingsError::Invalid)?;
    let name = path.file_name().ok_or(SettingsError::Invalid)?;
    let temporary = stage_in(parent).map_err(|_| SettingsError::Io)?;
    let retired = PackageDirectory::open(temporary.path()).map_err(|_| SettingsError::Io)?;
    let identity = directory_metadata(directory).map_err(|_| SettingsError::Io)?;
    source.confirm().map_err(|_| SettingsError::Io)?;
    directory.verify().map_err(|_| SettingsError::Invalid)?;
    renameat_with(
        source.descriptor(),
        name,
        retired.descriptor(),
        PACKAGE,
        RenameFlags::NOREPLACE,
    )
    .map_err(|_| SettingsError::Io)?;
    let same = retired
        .child(OsStr::new(PACKAGE))
        .and_then(|child| directory_metadata(&child))
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == (identity.dev(), identity.ino()));
    if !same {
        if renameat_with(
            retired.descriptor(),
            PACKAGE,
            source.descriptor(),
            name,
            RenameFlags::NOREPLACE,
        )
        .is_err()
            || source.sync().is_err()
        {
            let _ = temporary.keep();
            return Err(SettingsError::Committed);
        }
        return Err(SettingsError::Invalid);
    }
    if source.sync().is_err() {
        let _ = temporary.keep();
        return Err(SettingsError::Committed);
    }
    temporary.close().map_err(|_| SettingsError::Committed)
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
            .map_err(|_| SettingsError::Io)?
            .ok_or(SettingsError::Invalid)?;
        let _lock = directory
            .lock(OsStr::new(SKILLS_ROOT))
            .map_err(|_| SettingsError::Io)?;
        let package = self.skill_for_operation(target, &collection, name, expected)?;
        let root = managed_root(&package, &collection).ok_or(SettingsError::Invalid)?;
        let package_dir = PackageDirectory::open(root).map_err(|_| SettingsError::Io)?;
        Tree::scan(&package_dir, &self.resource_io, false).map_err(|_| SettingsError::Invalid)?;
        self.skill_for_operation(target, &collection, name, expected)?;
        retire(root, &package_dir)?;
        Ok("missing".into())
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
        validate_skill_name(destination_name).map_err(|_| SettingsError::Invalid)?;
        let source_collection = self.skill_collection(source_target);
        let destination_collection = self.skill_collection(destination_target);
        let source = SourceDirectory::observe(&source_collection)
            .map_err(|_| SettingsError::Io)?
            .ok_or(SettingsError::Invalid)?;
        let destination = SourceDirectory::create_directory(&destination_collection)
            .map_err(|_| SettingsError::Io)?;
        if source.within(&destination).map_err(|_| SettingsError::Io)?
            || destination.within(&source).map_err(|_| SettingsError::Io)?
        {
            return Err(SettingsError::TransferRejected(
                TransferRejection::SameDirectory,
            ));
        }
        let (first, second) = if source_target.scope() == SourceScope::User {
            (&source, &destination)
        } else {
            (&destination, &source)
        };
        let _first = first
            .lock(OsStr::new(SKILLS_ROOT))
            .map_err(|_| SettingsError::Io)?;
        let _second = second
            .lock(OsStr::new(SKILLS_ROOT))
            .map_err(|_| SettingsError::Io)?;
        let package =
            self.skill_for_operation(source_target, &source_collection, source_name, expected)?;
        let root = managed_root(&package, &source_collection).ok_or(SettingsError::Invalid)?;
        let package_dir = PackageDirectory::open(root).map_err(|_| SettingsError::Io)?;
        let moving = operation == TransferOperation::Move;
        self.admit_skill_destination(
            source_target,
            destination_target,
            source_name,
            destination_name,
            moving,
        )?;
        let stage = stage_in(&destination_collection).map_err(|_| SettingsError::Io)?;
        let staged_root = stage.path().join(PACKAGE);
        let tree = Tree::scan(&package_dir, &self.resource_io, true)
            .map_err(|_| SettingsError::Invalid)?;
        tree.copy_to(&package_dir, &staged_root, &self.resource_io)
            .map_err(|_| SettingsError::Io)?;
        if !moving || source_name != destination_name {
            crate::skills::frontmatter::rewrite(
                &staged_root.join(SKILL_MARKDOWN_FILE),
                destination_name,
                !moving,
                &self.resource_io,
            )
            .map_err(|_| SettingsError::Invalid)?;
        }
        tree.sync_directories(&staged_root)
            .map_err(|_| SettingsError::Io)?;
        self.skill_for_operation(source_target, &source_collection, source_name, expected)?;
        let latest = Tree::scan(&package_dir, &self.resource_io, true)
            .map_err(|_| SettingsError::Invalid)?;
        if latest.entries != tree.entries || !source.holds() {
            return Err(SettingsError::Invalid);
        }
        let staging = PackageDirectory::open(stage.path()).map_err(|_| SettingsError::Io)?;
        let installed = destination_collection.join(destination_name);
        let installed_directory = publish(&staging, &destination, &installed)?;
        let visible = self
            .discover_skill_scope(destination_target, &destination_collection)
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
            if final_source.entries != tree.entries {
                return Err(SettingsError::Committed);
            }
            retire(root, &package_dir).map_err(|_| SettingsError::Committed)?;
        }
        Ok(revision)
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
                if selection.is_some_and(|selection| {
                    selection.names().iter().any(|selected| selected == name)
                }) {
                    return Err(SettingsError::TransferRejected(
                        TransferRejection::NameEnabled,
                    ));
                }
            }
        }
        Ok(())
    }
}
