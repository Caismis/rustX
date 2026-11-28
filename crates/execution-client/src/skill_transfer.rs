//! Staged, bounded copies into an ownership-marked User Skill cache.
//!
//! Published versions are immutable so replacing a Skill never removes a
//! directory still referenced by another bound session. The cache is an
//! isolated child of the official collection; cleanup touches private stages
//! only, never the standing User root or an unmarked directory.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use rustix::fs::{AtFlags, FlockOperation, Mode, OFlags};
use rustx_confined_fs::PackageDirectory;
use rustx_execution_protocol::MAX_RAW_CHUNK_BYTES;
use rustx_execution_protocol::skills::{
    CACHE_DIRECTORY as CACHE, CACHE_LOCK, LeaseId, MAX_PACKAGE_ENTRIES, MAX_SKILL_BYTES,
    MAX_SYNC_BYTES, Reply, Request, valid_cache_id, valid_package_path,
};

use super::mutation::Arbitration;

const MARKER: &str = ".rustx-owned";
const MARKER_CONTENT: &[u8] = b"rustx user skill cache v1\n";
const AUTHORITY_LOCK: &str = ".lock";
const RETIRED_PREFIX: &str = ".retired-";
const VERSION_MARKER: &str = ".version";
const INTENT_SUFFIX: &str = ".intent";
const LEASE: &str = ".lease";
const SIZE: &str = ".bytes";
const MAX_CACHE_VERSIONS: usize = 1024;
const MAX_CACHE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_CACHE_AUTHORITIES: usize = 64;

#[derive(Default)]
pub(super) struct Transfer {
    sync: Option<Sync>,
    leases: BTreeMap<LeaseId, BTreeMap<String, File>>,
}

struct Sync {
    authority: String,
    owner: PackageDirectory,
    _lock: File,
    stage: Option<Stage>,
    bytes: u64,
    retained: BTreeMap<String, File>,
}

struct Stage {
    name: String,
    version: String,
    data: PackageDirectory,
    file: Option<PendingFile>,
    bytes: u64,
    entries: usize,
    lease: File,
}

struct PendingFile {
    file: File,
    length: u64,
    received: u64,
    executable: bool,
}

impl Transfer {
    /// Discard unfinished work and keep leases held by catalog readers.
    pub(super) fn lane_lost(&mut self) -> io::Result<()> {
        let cleanup = self.abort();
        self.sync = None;
        cleanup
    }

    pub(super) fn dispatch(
        &mut self,
        root: &Path,
        selected_collection: &Path,
        request: &Request,
        arbitration: &Arbitration,
    ) -> io::Result<Reply> {
        match request {
            Request::BeginSync { authority } => {
                self.begin_sync(root, selected_collection, authority)
            }
            Request::BeginPackage { version } => self.begin_package(version),
            Request::Retain { version } => self.retain(version),
            Request::Directory { path } => self.directory(path),
            Request::BeginFile {
                path,
                length,
                executable,
            } => self.begin_file(path, length.get(), *executable),
            Request::Chunk {
                offset,
                data_base64,
            } => self.chunk(offset.get(), data_base64),
            Request::EndFile => self.end_file(),
            Request::Publish => self.publish(arbitration),
            Request::AbortPackage => {
                self.abort()?;
                Ok(Reply::Accepted)
            }
            Request::EndSync { lease } => self.end_sync(*lease),
            _ => Err(io::ErrorKind::InvalidInput.into()),
        }
    }

    fn end_sync(&mut self, lease: Option<LeaseId>) -> io::Result<Reply> {
        if let Some(lease) = lease {
            if self.leases.contains_key(&lease) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            let sync = self.sync.take().ok_or(io::ErrorKind::InvalidInput)?;
            if let Some(stage) = sync.stage {
                cleanup_work_item(&sync.owner, &stage.name)?;
                return Err(io::ErrorKind::InvalidInput.into());
            }
            if !sync.retained.is_empty() {
                self.leases.insert(lease, sync.retained);
            }
        } else {
            self.lane_lost()?;
        }
        Ok(Reply::SyncEnded)
    }

    pub(super) fn release_lease(&mut self, lease: LeaseId) -> Reply {
        self.leases.remove(&lease);
        Reply::LeaseReleased
    }

    fn begin_sync(
        &mut self,
        root: &Path,
        selected_collection: &Path,
        authority: &str,
    ) -> io::Result<Reply> {
        if self.sync.is_some() || !valid_cache_id(authority) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if super::skills::UserSkills::collection_path(root)? != selected_collection {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        // This root is chosen locally by the Client, not supplied on the
        // wire. Resolve its configured aliases before pinning it; all cache
        // descendants still use no-follow, descriptor-relative operations.
        let product = ensure_selected_collection(root)?;
        let skills = ensure_selected_collection(selected_collection)?;
        product.verify()?;
        // Serialize creation and marker verification across Client processes.
        // Never truncate or unlink an existing file at this reserved name.
        let initializer = File::from(rustix::fs::openat(
            skills.descriptor(),
            CACHE_LOCK,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        if !initializer.metadata()?.is_file() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        match rustix::fs::flock(&initializer, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(rustix::io::Errno::AGAIN) => return Ok(Reply::Busy),
            Err(error) => return Err(error.into()),
        }
        let cache = ensure_owned(&skills, OsStr::new(CACHE))?;
        cleanup_retired_authorities(&cache)?;
        make_authority_room(&cache, authority)?;
        let owner = ensure_owned(&cache, OsStr::new(authority))?;
        let Some(lock) = lock_authority(&owner)? else {
            return Ok(Reply::Busy);
        };
        cleanup_stages(&owner)?;
        self.sync = Some(Sync {
            authority: authority.to_owned(),
            owner,
            _lock: lock,
            stage: None,
            bytes: 0,
            retained: BTreeMap::new(),
        });
        Ok(Reply::SyncStarted)
    }

    fn begin_package(&mut self, version: &str) -> io::Result<Reply> {
        let sync = self.sync.as_mut().ok_or(io::ErrorKind::InvalidInput)?;
        if sync.stage.is_some() || !valid_cache_id(version) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        sync.owner.verify()?;
        match rustix::fs::statat(sync.owner.descriptor(), version, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Ok(Reply::VersionExists),
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(error.into()),
        }
        let name = reserve_work(&sync.owner, ".stage-")?;
        if let Err(error) = rustix::fs::mkdirat(
            sync.owner.descriptor(),
            name.as_str(),
            Mode::from_raw_mode(0o700),
        ) {
            remove_intent(&sync.owner, &name)?;
            return Err(error.into());
        }
        let setup = (|| {
            let stage = sync.owner.child(OsStr::new(&name))?;
            write_new(&stage, VERSION_MARKER, MARKER_CONTENT)?;
            let lease = open_new(&stage, LEASE)?;
            rustix::fs::flock(&lease, FlockOperation::NonBlockingLockShared)?;
            rustix::fs::mkdirat(stage.descriptor(), "data", Mode::from_raw_mode(0o700))?;
            let data = stage.child(OsStr::new("data"))?;
            Ok::<_, io::Error>((data, lease))
        })();
        let (data, lease) = match setup {
            Ok(stage) => stage,
            Err(error) => {
                cleanup_work_item(&sync.owner, &name)?;
                return Err(error);
            }
        };
        sync.stage = Some(Stage {
            name,
            version: version.to_owned(),
            data,
            file: None,
            bytes: 0,
            entries: 0,
            lease,
        });
        Ok(Reply::Accepted)
    }

    fn retain(&mut self, version: &str) -> io::Result<Reply> {
        if !valid_cache_id(version) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let sync = self.sync.as_mut().ok_or(io::ErrorKind::InvalidInput)?;
        let version_dir = sync.owner.child(OsStr::new(version))?;
        verify_marker(&version_dir, VERSION_MARKER)?;
        let bytes = version_size(&version_dir)?;
        if !sync.retained.contains_key(version) {
            if bytes > MAX_SYNC_BYTES.saturating_sub(sync.bytes) {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            let lease = open_existing(&version_dir, LEASE)?;
            rustix::fs::flock(&lease, FlockOperation::NonBlockingLockShared)?;
            sync.retained.insert(version.to_owned(), lease);
            sync.bytes += bytes;
        }
        Ok(Reply::Retained {
            package: PathBuf::from(CACHE)
                .join(&sync.authority)
                .join(version)
                .join("data"),
            bytes: rustx_execution_protocol::DecimalU64::new(bytes),
        })
    }

    fn directory(&mut self, path: &Path) -> io::Result<Reply> {
        let stage = self.stage()?;
        if stage.file.is_some() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let (parent, leaf) = stage_parent(stage, path)?;
        rustix::fs::mkdirat(parent.descriptor(), leaf, Mode::from_raw_mode(0o700))?;
        stage.entries += 1;
        Ok(Reply::Accepted)
    }

    fn begin_file(&mut self, path: &Path, length: u64, executable: bool) -> io::Result<Reply> {
        let sync = self.sync.as_mut().ok_or(io::ErrorKind::InvalidInput)?;
        let stage = sync.stage.as_mut().ok_or(io::ErrorKind::InvalidInput)?;
        if stage.file.is_some()
            || length > MAX_SKILL_BYTES.saturating_sub(stage.bytes)
            || length > MAX_SYNC_BYTES.saturating_sub(sync.bytes)
        {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        let (parent, leaf) = stage_parent(stage, path)?;
        parent.verify()?;
        let file = File::from(rustix::fs::openat(
            parent.descriptor(),
            leaf,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        stage.entries += 1;
        stage.bytes += length;
        sync.bytes += length;
        stage.file = Some(PendingFile {
            file,
            length,
            received: 0,
            executable,
        });
        Ok(Reply::Accepted)
    }

    fn chunk(&mut self, offset: u64, encoded: &str) -> io::Result<Reply> {
        if encoded.len() > MAX_RAW_CHUNK_BYTES.div_ceil(3) * 4 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let pending = self
            .stage()?
            .file
            .as_mut()
            .ok_or(io::ErrorKind::InvalidInput)?;
        if offset != pending.received {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        if bytes.is_empty()
            || bytes.len() > MAX_RAW_CHUNK_BYTES
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                > pending.length.saturating_sub(pending.received)
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        pending.file.write_all_at(&bytes, offset)?;
        pending.received += bytes.len() as u64;
        Ok(Reply::Accepted)
    }

    fn end_file(&mut self) -> io::Result<Reply> {
        let stage = self.stage()?;
        let pending = stage.file.as_ref().ok_or(io::ErrorKind::InvalidInput)?;
        if pending.received != pending.length {
            return Err(io::ErrorKind::InvalidData.into());
        }
        rustix::fs::fchmod(
            &pending.file,
            Mode::from_raw_mode(if pending.executable { 0o700 } else { 0o600 }),
        )?;
        stage.file = None;
        Ok(Reply::Accepted)
    }

    fn publish(&mut self, arbitration: &Arbitration) -> io::Result<Reply> {
        let sync = self.sync.as_mut().ok_or(io::ErrorKind::InvalidInput)?;
        let stage = sync.stage.as_ref().ok_or(io::ErrorKind::InvalidInput)?;
        if stage.file.is_some() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        sync.owner.verify()?;
        stage.data.verify()?;
        let stage_directory = sync.owner.child(OsStr::new(&stage.name))?;
        write_new(&stage_directory, SIZE, stage.bytes.to_string().as_bytes())?;
        prune(&sync.owner, stage.bytes, MAX_CACHE_BYTES)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let moved = arbitration.with_grant(|| {
            rustix::fs::renameat_with(
                sync.owner.descriptor(),
                stage.name.as_str(),
                sync.owner.descriptor(),
                stage.version.as_str(),
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(Into::into)
        });
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let moved: io::Result<()> = Err(io::ErrorKind::Unsupported.into());
        match moved {
            Ok(()) => {
                let stage = sync.stage.take().expect("stage is present");
                // The published version is already visible. A leftover intent
                // is harmless and will be removed on the next sync.
                let _ = remove_intent(&sync.owner, &stage.name);
                sync.retained.insert(stage.version.clone(), stage.lease);
                Ok(Reply::Published {
                    package: PathBuf::from(CACHE)
                        .join(&sync.authority)
                        .join(stage.version)
                        .join("data"),
                })
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.abort()?;
                Ok(Reply::VersionExists)
            }
            Err(error) => Err(error),
        }
    }

    fn stage(&mut self) -> io::Result<&mut Stage> {
        self.sync
            .as_mut()
            .and_then(|sync| sync.stage.as_mut())
            .ok_or(io::ErrorKind::InvalidInput.into())
    }

    fn abort(&mut self) -> io::Result<()> {
        if let Some(sync) = &mut self.sync
            && let Some(stage) = sync.stage.take()
        {
            cleanup_work_item(&sync.owner, &stage.name)?;
        }
        Ok(())
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        let _ = self.abort();
    }
}

fn stage_parent<'a>(stage: &Stage, path: &'a Path) -> io::Result<(PackageDirectory, &'a OsStr)> {
    if stage.entries >= MAX_PACKAGE_ENTRIES || !valid_package_path(path) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let parent = stage.data.descend(path.parent().unwrap_or(Path::new("")))?;
    Ok((parent, path.file_name().ok_or(io::ErrorKind::InvalidInput)?))
}

fn ensure_directory(
    parent: &PackageDirectory,
    name: &OsStr,
) -> io::Result<(PackageDirectory, bool)> {
    parent.verify()?;
    let created = match rustix::fs::mkdirat(parent.descriptor(), name, Mode::from_raw_mode(0o700)) {
        Ok(()) => true,
        Err(rustix::io::Errno::EXIST) => false,
        Err(error) => return Err(error.into()),
    };
    Ok((parent.child(name)?, created))
}

/// Create only the missing suffix of the Client-selected physical collection.
fn ensure_selected_collection(path: &Path) -> io::Result<PackageDirectory> {
    if !rustx_execution_protocol::is_normalized_absolute_path(path) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut ancestor = path;
    let mut missing = Vec::new();
    let mut directory = loop {
        match PackageDirectory::open(ancestor) {
            Ok(directory) => break directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .ok_or(io::ErrorKind::InvalidInput)?
                        .to_owned(),
                );
                ancestor = ancestor.parent().ok_or(io::ErrorKind::InvalidInput)?;
            }
            Err(error) => return Err(error),
        }
    };
    for name in missing.iter().rev() {
        directory = ensure_directory(&directory, name)?.0;
    }
    Ok(directory)
}

fn ensure_owned(parent: &PackageDirectory, name: &OsStr) -> io::Result<PackageDirectory> {
    let (directory, created) = ensure_directory(parent, name)?;
    if created {
        write_new(&directory, MARKER, MARKER_CONTENT)?;
    } else {
        verify_marker(&directory, MARKER)?;
    }
    Ok(directory)
}

fn open_new(directory: &PackageDirectory, name: &str) -> io::Result<File> {
    directory.verify()?;
    Ok(File::from(rustix::fs::openat(
        directory.descriptor(),
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?))
}

fn open_existing(directory: &PackageDirectory, name: &str) -> io::Result<File> {
    directory.verify()?;
    let file = File::from(rustix::fs::openat(
        directory.descriptor(),
        name,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    if !file.metadata()?.is_file() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}

fn write_new(directory: &PackageDirectory, name: &str, bytes: &[u8]) -> io::Result<()> {
    open_new(directory, name)?.write_all(bytes)
}

fn verify_marker(directory: &PackageDirectory, name: &str) -> io::Result<()> {
    let mut marker = directory.file(Path::new(name))?;
    let mut content = Vec::new();
    Read::by_ref(&mut marker)
        .take(128)
        .read_to_end(&mut content)?;
    if content != MARKER_CONTENT {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

fn version_size(directory: &PackageDirectory) -> io::Result<u64> {
    let mut file = directory.file(Path::new(SIZE))?;
    let mut size = String::new();
    Read::by_ref(&mut file).take(32).read_to_string(&mut size)?;
    let size: u64 = size.parse().map_err(|_| io::ErrorKind::InvalidData)?;
    if size > MAX_SKILL_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(size)
}

fn lock_authority(owner: &PackageDirectory) -> io::Result<Option<File>> {
    owner.verify()?;
    let file = File::from(rustix::fs::openat(
        owner.descriptor(),
        AUTHORITY_LOCK,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?);
    if !file.metadata()?.is_file() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(file)),
        Err(rustix::io::Errno::AGAIN) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

// The collection initializer stays locked through eviction and new-owner
// creation. A resumed sync cannot reopen an authority while it is retired.
fn make_authority_room(cache: &PackageDirectory, requested: &str) -> io::Result<()> {
    let owners: Vec<_> = cache
        .names(MAX_CACHE_AUTHORITIES * 2 + 1)?
        .into_iter()
        .filter(|name| name.to_str().is_some_and(valid_cache_id))
        .collect();
    let mut remaining = owners.len();
    if remaining < MAX_CACHE_AUTHORITIES || owners.iter().any(|name| name == OsStr::new(requested))
    {
        return Ok(());
    }
    for name in owners {
        remaining -= usize::from(retire_inactive_authority(cache, name.as_os_str())?);
        if remaining < MAX_CACHE_AUTHORITIES {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::FileTooLarge,
        "live or unowned User Skill authorities exhaust the cache bound",
    ))
}

fn retire_inactive_authority(cache: &PackageDirectory, name: &OsStr) -> io::Result<bool> {
    let Ok(owner) = cache.child(name) else {
        return Ok(false);
    };
    if verify_marker(&owner, MARKER).is_err() {
        return Ok(false);
    }
    let Some(_lock) = lock_authority(&owner)? else {
        return Ok(false);
    };
    cleanup_stages(&owner)?;
    for child in owner.names(MAX_CACHE_VERSIONS * 2 + 4)? {
        let Some(text) = child.to_str() else {
            return Ok(false);
        };
        if matches!(text, MARKER | AUTHORITY_LOCK) {
            continue;
        }
        let version = valid_cache_id(text);
        if !version && !stage_name(text) {
            return Ok(false);
        }
        let Ok(directory) = owner.child(child.as_os_str()) else {
            return Ok(false);
        };
        if verify_marker(&directory, VERSION_MARKER).is_err() {
            return Ok(false);
        }
        if version {
            let Ok(lease) = open_existing(&directory, LEASE) else {
                return Ok(false);
            };
            // Retain and Publish both require the authority lock. Until we
            // drop it, a version that has no lease cannot acquire one.
            match rustix::fs::flock(&lease, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => {}
                Err(rustix::io::Errno::AGAIN) => return Ok(false),
                Err(error) => return Err(error.into()),
            }
        }
    }
    owner.verify()?;
    cache.verify()?;
    let retired = mark_retired(cache, name)?;
    cleanup_retired_authority(cache, OsStr::new(&retired))?;
    Ok(true)
}

fn mark_retired(cache: &PackageDirectory, name: &OsStr) -> io::Result<String> {
    let authority = name.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    let retired = format!("{RETIRED_PREFIX}{authority}");
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    rustix::fs::renameat_with(
        cache.descriptor(),
        name,
        cache.descriptor(),
        retired.as_str(),
        rustix::fs::RenameFlags::NOREPLACE,
    )?;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return Err(io::ErrorKind::Unsupported.into());
    Ok(retired)
}

fn cleanup_retired_authorities(cache: &PackageDirectory) -> io::Result<()> {
    for name in cache.names(MAX_CACHE_AUTHORITIES * 2 + 1)? {
        if name
            .to_str()
            .and_then(|name| name.strip_prefix(RETIRED_PREFIX))
            .is_some_and(valid_cache_id)
        {
            cleanup_retired_authority(cache, name.as_os_str())?;
        }
    }
    Ok(())
}

fn cleanup_retired_authority(cache: &PackageDirectory, name: &OsStr) -> io::Result<()> {
    let retired = cache.child(name)?;
    let children = retired.names(MAX_CACHE_VERSIONS * 2 + 4)?;
    match verify_marker(&retired, MARKER) {
        Ok(()) => {
            for child in children {
                let text = child.to_str().ok_or(io::ErrorKind::InvalidData)?;
                match text {
                    MARKER => {}
                    AUTHORITY_LOCK => {
                        rustix::fs::unlinkat(retired.descriptor(), text, AtFlags::empty())?;
                    }
                    _ if valid_cache_id(text) || stage_name(text) => {
                        // A prior cleanup may have removed a child marker.
                        // The enclosing authority's marker owns the retirement.
                        remove_tree(&retired, child.as_os_str(), &mut (MAX_PACKAGE_ENTRIES + 5))?;
                    }
                    _ => return Err(io::ErrorKind::PermissionDenied.into()),
                }
            }
            retired.verify()?;
            rustix::fs::unlinkat(retired.descriptor(), MARKER, AtFlags::empty())?;
        }
        // The last step of an interrupted cleanup can leave an empty
        // reserved directory. No child is removed without an ownership marker.
        Err(error) if error.kind() == io::ErrorKind::NotFound && children.is_empty() => {}
        Err(error) => return Err(error),
    }
    cache.verify()?;
    rustix::fs::unlinkat(cache.descriptor(), name, AtFlags::REMOVEDIR)?;
    Ok(())
}

/// Evict oldest unleased versions when an incoming package would exceed the
/// per-authority retention bound. Metadata reads are shallow and bounded.
fn prune(owner: &PackageDirectory, incoming: u64, byte_limit: u64) -> io::Result<()> {
    let mut versions = Vec::new();
    let mut bytes = 0u64;
    for name in owner.names(MAX_CACHE_VERSIONS * 2 + 4)? {
        let Some(text) = name.to_str() else { continue };
        if !valid_cache_id(text) {
            continue;
        }
        let directory = owner.child(name.as_os_str())?;
        verify_marker(&directory, VERSION_MARKER)?;
        let size = version_size(&directory)?;
        bytes = bytes.checked_add(size).ok_or(io::ErrorKind::FileTooLarge)?;
        let modified = directory.file(Path::new(SIZE))?.metadata()?.modified()?;
        versions.push((modified, name, size));
    }
    versions.sort_by_key(|(modified, _, _)| *modified);
    let mut count = versions.len();
    for (_, name, size) in versions {
        if count < MAX_CACHE_VERSIONS && bytes.saturating_add(incoming) <= byte_limit {
            break;
        }
        let directory = owner.child(name.as_os_str())?;
        let lease = open_existing(&directory, LEASE)?;
        match rustix::fs::flock(&lease, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {
                let retired = reserve_work(owner, ".prune-")?;
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                let renamed = rustix::fs::renameat_with(
                    owner.descriptor(),
                    name.as_os_str(),
                    owner.descriptor(),
                    retired.as_str(),
                    rustix::fs::RenameFlags::NOREPLACE,
                );
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                let renamed: Result<(), rustix::io::Errno> = Err(rustix::io::Errno::NOTSUP);
                if let Err(error) = renamed {
                    remove_intent(owner, &retired)?;
                    return Err(error.into());
                }
                cleanup_work_item(owner, &retired)?;
                bytes -= size;
                count -= 1;
            }
            Err(rustix::io::Errno::AGAIN) => {}
            Err(error) => return Err(error.into()),
        }
    }
    if count >= MAX_CACHE_VERSIONS || bytes.saturating_add(incoming) > byte_limit {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "live User Skill versions exhaust the cache bound",
        ));
    }
    Ok(())
}

// Reserve a random work name with an ownership intent before creating or
// moving any directory there. Only complete intents authorize cleanup.
fn reserve_work(owner: &PackageDirectory, prefix: &str) -> io::Result<String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let mut name = prefix.to_owned();
    for byte in random {
        use std::fmt::Write as _;
        write!(&mut name, "{byte:02x}").expect("writing to String");
    }
    owner.verify()?;
    match rustix::fs::statat(owner.descriptor(), name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => return Err(io::ErrorKind::AlreadyExists.into()),
        Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    // Creation itself is the complete intent, so a crash cannot leave a
    // partially written reservation that recovery cannot recognize.
    open_new(owner, &format!("{name}{INTENT_SUFFIX}"))?;
    Ok(name)
}

fn remove_intent(owner: &PackageDirectory, name: &str) -> io::Result<()> {
    owner.verify()?;
    rustix::fs::unlinkat(
        owner.descriptor(),
        format!("{name}{INTENT_SUFFIX}").as_str(),
        AtFlags::empty(),
    )?;
    Ok(())
}

fn verify_intent(owner: &PackageDirectory, name: &str) -> io::Result<()> {
    let intent = owner.file(Path::new(name))?;
    let metadata = intent.metadata()?;
    if !metadata.is_file() || metadata.len() != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

fn cleanup_work_item(owner: &PackageDirectory, name: &str) -> io::Result<()> {
    verify_intent(owner, &format!("{name}{INTENT_SUFFIX}"))?;
    match rustix::fs::statat(owner.descriptor(), name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => remove_tree(owner, OsStr::new(name), &mut (MAX_PACKAGE_ENTRIES + 5))?,
        Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    remove_intent(owner, name)
}

fn cleanup_stages(owner: &PackageDirectory) -> io::Result<()> {
    for name in owner.names(MAX_CACHE_VERSIONS * 2 + 4)? {
        let Some(text) = name.to_str() else { continue };
        let Some(work) = text.strip_suffix(INTENT_SUFFIX) else {
            continue;
        };
        if !stage_name(work) && !prune_name(work) {
            continue;
        }
        // Do not claim unmarked files or directories left by another actor.
        if verify_intent(owner, text).is_ok() {
            cleanup_work_item(owner, work)?;
        }
    }
    Ok(())
}

fn stage_name(name: &str) -> bool {
    work_name(name, ".stage-")
}

fn prune_name(name: &str) -> bool {
    work_name(name, ".prune-")
}

fn work_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn remove_tree(parent: &PackageDirectory, name: &OsStr, remaining: &mut usize) -> io::Result<()> {
    let directory = parent.child(name)?;
    for entry in directory.names(*remaining)? {
        if *remaining == 0 {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        *remaining -= 1;
        let leaf = entry.as_os_str();
        let metadata = rustix::fs::statat(directory.descriptor(), leaf, AtFlags::SYMLINK_NOFOLLOW)?;
        if rustix::fs::FileType::from_raw_mode(metadata.st_mode) == rustix::fs::FileType::Directory
        {
            remove_tree(&directory, leaf, remaining)?;
        } else {
            rustix::fs::unlinkat(directory.descriptor(), leaf, AtFlags::empty())?;
        }
    }
    parent.verify()?;
    rustix::fs::unlinkat(parent.descriptor(), name, AtFlags::REMOVEDIR)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustx_execution_protocol::DecimalU64;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn authority() -> String {
        "a".repeat(64)
    }

    #[test]
    fn missing_home_is_created_only_for_a_required_transfer_and_never_cleaned() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("missing/one/two/three");
        let root = home.join("rustx");
        let mut skills = super::super::skills::UserSkills::from_home(Some(home.clone()));
        let arbitration = Arbitration::new();
        assert!(matches!(
            skills
                .dispatch(&Request::Collection { probe: None }, &arbitration)
                .unwrap(),
            Reply::Collection { identity: None, .. }
        ));
        assert!(!home.exists());
        let mut transfer = Transfer::default();
        assert_eq!(
            call(
                &mut transfer,
                &root,
                &arbitration,
                Request::BeginSync {
                    authority: authority()
                }
            )
            .unwrap(),
            Reply::SyncStarted
        );
        assert!(root.join(".agents/skills/.rustx-sync").is_dir());
        assert_eq!(fs::metadata(&home).unwrap().permissions().mode() & 0o077, 0);
        transfer.lane_lost().unwrap();
        transfer.lane_lost().unwrap();
        assert!(home.is_dir());
    }
    fn version(byte: char) -> String {
        byte.to_string().repeat(64)
    }
    #[allow(
        clippy::needless_pass_by_value,
        reason = "test helper owns each constructed request"
    )]
    fn call(
        transfer: &mut Transfer,
        root: &Path,
        arbitration: &Arbitration,
        request: Request,
    ) -> io::Result<Reply> {
        let collection = super::super::skills::UserSkills::collection_path(root)?;
        transfer.dispatch(root, &collection, &request, arbitration)
    }

    #[test]
    fn authority_limit_reclaims_inactive_owners_and_preserves_live_versions() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap().join("rustx");
        let arbitration = Arbitration::new();
        let begin = |transfer: &mut Transfer, authority| {
            call(
                transfer,
                &root,
                &arbitration,
                Request::BeginSync { authority },
            )
            .unwrap()
        };
        let authority = |index: usize| format!("{index:064x}");
        let bytes = b"live instructions";
        let mut reader = Transfer::default();
        assert_eq!(begin(&mut reader, authority(0)), Reply::SyncStarted);
        let published = [
            Request::BeginPackage {
                version: version('a'),
            },
            Request::BeginFile {
                path: "SKILL.md".into(),
                length: DecimalU64::new(bytes.len() as u64),
                executable: false,
            },
            Request::Chunk {
                offset: DecimalU64::new(0),
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
            Request::EndFile,
        ];
        for request in published {
            assert_eq!(
                call(&mut reader, &root, &arbitration, request).unwrap(),
                Reply::Accepted
            );
        }
        let Reply::Published { package } =
            call(&mut reader, &root, &arbitration, Request::Publish).unwrap()
        else {
            panic!("expected a published version");
        };
        reader.end_sync(Some(LeaseId::new_v4())).unwrap();

        let mut active = Transfer::default();
        assert_eq!(begin(&mut active, authority(1)), Reply::SyncStarted);
        let mut inactive = Transfer::default();
        for index in 2..MAX_CACHE_AUTHORITIES {
            assert_eq!(begin(&mut inactive, authority(index)), Reply::SyncStarted);
            inactive.lane_lost().unwrap();
        }
        assert_eq!(
            begin(&mut inactive, authority(MAX_CACHE_AUTHORITIES)),
            Reply::SyncStarted
        );
        let collection = root.join(".agents/skills");
        assert_eq!(
            fs::read(collection.join(package).join("SKILL.md")).unwrap(),
            bytes
        );
        let cache = collection.join(CACHE);
        assert!(cache.join(authority(1)).is_dir());
        assert_eq!(
            fs::read_dir(cache)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
                .count(),
            MAX_CACHE_AUTHORITIES
        );
    }

    #[test]
    fn interrupted_authority_retirement_allows_the_same_authority_to_bind() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap().join("rustx");
        let arbitration = Arbitration::new();
        let request = || Request::BeginSync {
            authority: authority(),
        };
        let mut transfer = Transfer::default();
        assert_eq!(
            call(&mut transfer, &root, &arbitration, request()).unwrap(),
            Reply::SyncStarted
        );
        let cache_path = root.join(".agents/skills").join(CACHE);
        let cache = PackageDirectory::open(&cache_path).unwrap();
        let authority = authority();
        let owner = cache.child(OsStr::new(&authority)).unwrap();
        let version = version('a');
        let stage = ensure_directory(&owner, OsStr::new(&version)).unwrap().0;
        write_new(&stage, VERSION_MARKER, MARKER_CONTENT).unwrap();
        let data = ensure_directory(&stage, OsStr::new("data")).unwrap().0;
        write_new(&data, "SKILL.md", b"retired instructions").unwrap();
        transfer.lane_lost().unwrap();

        let retired = mark_retired(&cache, OsStr::new(&authority)).unwrap();
        let retired_path = cache_path.join(retired);
        fs::remove_file(retired_path.join(&version).join(VERSION_MARKER)).unwrap();

        assert_eq!(
            call(&mut transfer, &root, &arbitration, request()).unwrap(),
            Reply::SyncStarted
        );
        assert!(!retired_path.exists());
        let owner = cache.child(OsStr::new(&authority)).unwrap();
        verify_marker(&owner, MARKER).unwrap();
        assert!(!cache_path.join(authority).join(version).exists());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one staged package lifecycle with two bound clients"
    )]
    fn packages_are_staged_immutable_and_do_not_touch_personal_skills() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let personal = root.join(".agents/skills/personal");
        fs::create_dir_all(&personal).unwrap();
        fs::write(personal.join("SKILL.md"), b"personal").unwrap();
        let arbitration = Arbitration::new();
        let mut first = Transfer::default();
        assert_eq!(
            call(
                &mut first,
                &root,
                &arbitration,
                Request::BeginSync {
                    authority: authority()
                }
            )
            .unwrap(),
            Reply::SyncStarted
        );
        let mut second = Transfer::default();
        assert_eq!(
            call(
                &mut second,
                &root,
                &arbitration,
                Request::BeginSync {
                    authority: authority()
                }
            )
            .unwrap(),
            Reply::Busy
        );
        assert_eq!(
            call(
                &mut first,
                &root,
                &arbitration,
                Request::BeginPackage {
                    version: version('b')
                }
            )
            .unwrap(),
            Reply::Accepted
        );
        assert_eq!(
            call(
                &mut first,
                &root,
                &arbitration,
                Request::Directory {
                    path: "scripts".into()
                }
            )
            .unwrap(),
            Reply::Accepted
        );
        for (name, bytes, executable) in [
            ("SKILL.md", b"instructions".as_slice(), false),
            ("scripts/run", b"script".as_slice(), true),
        ] {
            call(
                &mut first,
                &root,
                &arbitration,
                Request::BeginFile {
                    path: name.into(),
                    length: DecimalU64::new(bytes.len() as u64),
                    executable,
                },
            )
            .unwrap();
            call(
                &mut first,
                &root,
                &arbitration,
                Request::Chunk {
                    offset: DecimalU64::new(0),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                },
            )
            .unwrap();
            call(&mut first, &root, &arbitration, Request::EndFile).unwrap();
        }
        let Reply::Published { package } =
            call(&mut first, &root, &arbitration, Request::Publish).unwrap()
        else {
            panic!("published")
        };
        let data = root.join(".agents/skills").join(&package);
        assert_eq!(fs::read(data.join("SKILL.md")).unwrap(), b"instructions");
        assert_eq!(fs::read(data.join("scripts/run")).unwrap(), b"script");
        assert_ne!(
            fs::metadata(data.join("scripts/run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert_eq!(
            call(
                &mut first,
                &root,
                &arbitration,
                Request::EndSync {
                    lease: Some(LeaseId::new_v4())
                }
            )
            .unwrap(),
            Reply::SyncEnded
        );
        assert_eq!(
            call(
                &mut second,
                &root,
                &arbitration,
                Request::BeginSync {
                    authority: authority()
                }
            )
            .unwrap(),
            Reply::SyncStarted
        );
        assert_eq!(
            call(
                &mut second,
                &root,
                &arbitration,
                Request::Retain {
                    version: version('b')
                }
            )
            .unwrap(),
            Reply::Retained {
                package,
                bytes: DecimalU64::new(18)
            }
        );
        call(
            &mut second,
            &root,
            &arbitration,
            Request::BeginPackage {
                version: version('c'),
            },
        )
        .unwrap();
        call(
            &mut second,
            &root,
            &arbitration,
            Request::BeginFile {
                path: "SKILL.md".into(),
                length: DecimalU64::new(3),
                executable: false,
            },
        )
        .unwrap();
        call(
            &mut second,
            &root,
            &arbitration,
            Request::Chunk {
                offset: DecimalU64::new(0),
                data_base64: "bmV3".into(),
            },
        )
        .unwrap();
        call(&mut second, &root, &arbitration, Request::EndFile).unwrap();
        call(&mut second, &root, &arbitration, Request::Publish).unwrap();
        drop(second);
        drop(first);
        assert_eq!(fs::read(data.join("SKILL.md")).unwrap(), b"instructions");
        assert_eq!(fs::read(personal.join("SKILL.md")).unwrap(), b"personal");
    }

    #[test]
    fn refuses_unowned_cache_and_never_cleans_personal_content() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let cache = root.join(".agents/skills/.rustx-sync");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("personal"), b"keep").unwrap();
        let mut transfer = Transfer::default();
        assert!(
            call(
                &mut transfer,
                &root,
                &Arbitration::new(),
                Request::BeginSync {
                    authority: authority()
                }
            )
            .is_err()
        );
        drop(transfer);
        assert_eq!(fs::read(cache.join("personal")).unwrap(), b"keep");
    }

    #[test]
    fn caps_and_cancelled_stages_leave_no_published_version() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let arbitration = Arbitration::new();
        let mut skills =
            super::super::skills::UserSkills::from_home(Some(home.path().to_path_buf()));
        skills
            .dispatch(
                &Request::BeginSync {
                    authority: authority(),
                },
                &arbitration,
            )
            .unwrap();
        skills
            .dispatch(
                &Request::BeginPackage {
                    version: version('d'),
                },
                &arbitration,
            )
            .unwrap();
        assert!(
            skills
                .dispatch(
                    &Request::BeginFile {
                        path: "large".into(),
                        length: DecimalU64::new(MAX_SKILL_BYTES + 1),
                        executable: false
                    },
                    &arbitration
                )
                .is_err()
        );
        assert!(
            skills
                .dispatch(
                    &Request::BeginFile {
                        path: "../outside".into(),
                        length: DecimalU64::new(0),
                        executable: false
                    },
                    &arbitration
                )
                .is_err()
        );
        arbitration.revoke();
        assert!(skills.dispatch(&Request::Publish, &arbitration).is_err());
        drop(skills);
        let owner = root.join(".agents/skills/.rustx-sync").join(authority());
        assert!(!owner.join(version('d')).exists());
        assert!(fs::read_dir(owner).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-")
        }));
    }

    #[test]
    fn interrupted_stages_recover_before_and_after_the_version_marker() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let collection = super::super::skills::UserSkills::collection_path(&root).unwrap();
        let authority = authority();
        let mut transfer = Transfer::default();
        transfer.begin_sync(&root, &collection, &authority).unwrap();
        let owner = &transfer.sync.as_ref().unwrap().owner;
        let before_creation = reserve_work(owner, ".stage-").unwrap();
        let before_marker = reserve_work(owner, ".stage-").unwrap();
        rustix::fs::mkdirat(
            owner.descriptor(),
            before_marker.as_str(),
            Mode::from_raw_mode(0o700),
        )
        .unwrap();
        let after_marker = reserve_work(owner, ".stage-").unwrap();
        rustix::fs::mkdirat(
            owner.descriptor(),
            after_marker.as_str(),
            Mode::from_raw_mode(0o700),
        )
        .unwrap();
        let stage = owner.child(OsStr::new(&after_marker)).unwrap();
        write_new(&stage, VERSION_MARKER, MARKER_CONTENT).unwrap();
        write_new(&stage, "partial", b"work").unwrap();
        let unknown = format!(".stage-{}", "f".repeat(32));
        rustix::fs::mkdirat(
            owner.descriptor(),
            unknown.as_str(),
            Mode::from_raw_mode(0o700),
        )
        .unwrap();
        let unknown_path = collection.join(CACHE).join(&authority).join(&unknown);
        fs::write(unknown_path.join("personal"), b"keep").unwrap();
        transfer.lane_lost().unwrap();

        assert_eq!(
            transfer.begin_sync(&root, &collection, &authority).unwrap(),
            Reply::SyncStarted
        );
        let owner_path = collection.join(CACHE).join(&authority);
        for name in [before_creation, before_marker, after_marker] {
            assert!(!owner_path.join(&name).exists());
            assert!(!owner_path.join(format!("{name}{INTENT_SUFFIX}")).exists());
        }
        assert_eq!(fs::read(unknown_path.join("personal")).unwrap(), b"keep");
        assert_eq!(
            transfer.begin_package(&version('a')).unwrap(),
            Reply::Accepted
        );
    }

    #[test]
    fn interrupted_pruning_recovers_after_version_marker_removal() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let collection = super::super::skills::UserSkills::collection_path(&root).unwrap();
        let authority = authority();
        let mut transfer = Transfer::default();
        transfer.begin_sync(&root, &collection, &authority).unwrap();
        let owner = &transfer.sync.as_ref().unwrap().owner;
        let original = version('b');
        let directory = ensure_directory(owner, OsStr::new(&original)).unwrap().0;
        write_new(&directory, VERSION_MARKER, MARKER_CONTENT).unwrap();
        write_new(&directory, LEASE, b"").unwrap();
        write_new(&directory, SIZE, b"1").unwrap();
        let data = ensure_directory(&directory, OsStr::new("data")).unwrap().0;
        write_new(&data, "SKILL.md", b"x").unwrap();
        let retired = reserve_work(owner, ".prune-").unwrap();
        rustix::fs::renameat_with(
            owner.descriptor(),
            original.as_str(),
            owner.descriptor(),
            retired.as_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .unwrap();
        let owner_path = collection.join(CACHE).join(&authority);
        fs::remove_file(owner_path.join(&retired).join(VERSION_MARKER)).unwrap();
        transfer.lane_lost().unwrap();

        assert_eq!(
            transfer.begin_sync(&root, &collection, &authority).unwrap(),
            Reply::SyncStarted
        );
        assert!(!owner_path.join(&retired).exists());
        assert!(
            !owner_path
                .join(format!("{retired}{INTENT_SUFFIX}"))
                .exists()
        );
        assert_eq!(transfer.begin_package(&original).unwrap(), Reply::Accepted);
    }

    #[test]
    fn eviction_waits_for_the_last_catalog_lease() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let arbitration = Arbitration::new();
        let mut transfer = Transfer::default();
        call(
            &mut transfer,
            &root,
            &arbitration,
            Request::BeginSync {
                authority: authority(),
            },
        )
        .unwrap();
        call(
            &mut transfer,
            &root,
            &arbitration,
            Request::BeginPackage {
                version: version('e'),
            },
        )
        .unwrap();
        call(
            &mut transfer,
            &root,
            &arbitration,
            Request::BeginFile {
                path: "SKILL.md".into(),
                length: DecimalU64::new(1),
                executable: false,
            },
        )
        .unwrap();
        call(
            &mut transfer,
            &root,
            &arbitration,
            Request::Chunk {
                offset: DecimalU64::new(0),
                data_base64: "eA==".into(),
            },
        )
        .unwrap();
        call(&mut transfer, &root, &arbitration, Request::EndFile).unwrap();
        call(&mut transfer, &root, &arbitration, Request::Publish).unwrap();
        let older = LeaseId::new_v4();
        transfer.end_sync(Some(older)).unwrap();
        let collection = super::super::skills::UserSkills::collection_path(&root).unwrap();
        transfer
            .begin_sync(&root, &collection, &authority())
            .unwrap();
        transfer.retain(&version('e')).unwrap();
        let current = LeaseId::new_v4();
        transfer.end_sync(Some(current)).unwrap();
        transfer
            .begin_sync(&root, &collection, &authority())
            .unwrap();
        let owner = PackageDirectory::open(&collection.join(CACHE).join(authority())).unwrap();
        assert!(prune(&owner, 1, 1).is_err());
        assert_eq!(transfer.release_lease(older), Reply::LeaseReleased);
        assert!(prune(&owner, 1, 1).is_err());
        let readable = collection
            .join(CACHE)
            .join(authority())
            .join(version('e'))
            .join("data/SKILL.md");
        assert_eq!(fs::read(&readable).unwrap(), b"x");
        assert_eq!(transfer.release_lease(current), Reply::LeaseReleased);
        assert_eq!(transfer.release_lease(current), Reply::LeaseReleased);
        prune(&owner, 1, 1).unwrap();
        assert!(!readable.exists());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one lane loss across staged and published versions"
    )]
    fn lane_loss_discards_only_the_stage_and_keeps_published_leases() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let arbitration = Arbitration::new();
        let mut first = Transfer::default();
        call(
            &mut first,
            &root,
            &arbitration,
            Request::BeginSync {
                authority: authority(),
            },
        )
        .unwrap();
        call(
            &mut first,
            &root,
            &arbitration,
            Request::BeginPackage {
                version: version('e'),
            },
        )
        .unwrap();
        call(
            &mut first,
            &root,
            &arbitration,
            Request::BeginFile {
                path: "SKILL.md".into(),
                length: DecimalU64::new(1),
                executable: false,
            },
        )
        .unwrap();
        call(
            &mut first,
            &root,
            &arbitration,
            Request::Chunk {
                offset: DecimalU64::new(0),
                data_base64: "eA==".into(),
            },
        )
        .unwrap();
        call(&mut first, &root, &arbitration, Request::EndFile).unwrap();
        call(&mut first, &root, &arbitration, Request::Publish).unwrap();
        let lease = LeaseId::new_v4();
        first.end_sync(Some(lease)).unwrap();
        let collection = super::super::skills::UserSkills::collection_path(&root).unwrap();
        first.begin_sync(&root, &collection, &authority()).unwrap();
        call(
            &mut first,
            &root,
            &arbitration,
            Request::BeginPackage {
                version: version('f'),
            },
        )
        .unwrap();
        call(
            &mut first,
            &root,
            &arbitration,
            Request::BeginFile {
                path: "SKILL.md".into(),
                length: DecimalU64::new(4),
                executable: false,
            },
        )
        .unwrap();
        call(
            &mut first,
            &root,
            &arbitration,
            Request::Chunk {
                offset: DecimalU64::new(0),
                data_base64: "eA==".into(),
            },
        )
        .unwrap();
        let owner = root.join(".agents/skills/.rustx-sync").join(authority());
        assert!(fs::read_dir(&owner).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-")
        }));
        first.lane_lost().unwrap();
        first.lane_lost().unwrap();
        assert!(first.sync.is_none());
        assert!(first.leases.contains_key(&lease));
        assert!(!owner.join(version('f')).exists());
        assert!(fs::read_dir(&owner).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-")
        }));
        assert_eq!(
            fs::read(owner.join(version('e')).join("data/SKILL.md")).unwrap(),
            b"x"
        );

        let mut second = Transfer::default();
        assert_eq!(
            call(
                &mut second,
                &root,
                &arbitration,
                Request::BeginSync {
                    authority: authority()
                }
            )
            .unwrap(),
            Reply::SyncStarted
        );
        assert!(matches!(
            call(
                &mut second,
                &root,
                &arbitration,
                Request::Retain {
                    version: version('e')
                }
            )
            .unwrap(),
            Reply::Retained { .. }
        ));
        assert_eq!(
            second.begin_package(&version('e')).unwrap(),
            Reply::VersionExists
        );
        assert!(second.sync.as_ref().unwrap().stage.is_none());
        drop(second);
        let lease = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(owner.join(version('e')).join(LEASE))
            .unwrap();
        assert_eq!(
            rustix::fs::flock(&lease, FlockOperation::NonBlockingLockExclusive).unwrap_err(),
            rustix::io::Errno::AGAIN
        );
        drop(first);
        rustix::fs::flock(&lease, FlockOperation::NonBlockingLockExclusive).unwrap();
    }

    #[test]
    fn a_file_remains_pending_until_its_declared_bytes_have_arrived() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let arbitration = Arbitration::new();
        let mut transfer = Transfer::default();
        let collection = super::super::skills::UserSkills::collection_path(&root).unwrap();
        transfer
            .begin_sync(&root, &collection, &authority())
            .unwrap();
        transfer.begin_package(&version('b')).unwrap();
        transfer
            .begin_file(Path::new("SKILL.md"), 2, false)
            .unwrap();
        transfer.chunk(0, "YQ==").unwrap();
        assert_eq!(
            transfer.end_file().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            transfer.publish(&arbitration).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        transfer.chunk(1, "Yg==").unwrap();
        transfer.end_file().unwrap();
        let Reply::Published { package } = transfer.publish(&arbitration).unwrap() else {
            panic!("completed stage must publish");
        };
        assert_eq!(
            fs::read(collection.join(package).join("SKILL.md")).unwrap(),
            b"ab"
        );
    }

    #[test]
    fn ending_sync_releases_the_lock_when_stage_cleanup_fails() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("rustx");
        let arbitration = Arbitration::new();
        let mut transfer = Transfer::default();
        let collection = super::super::skills::UserSkills::collection_path(&root).unwrap();
        let authority = authority();
        transfer.begin_sync(&root, &collection, &authority).unwrap();
        transfer.begin_package(&version('b')).unwrap();
        let owner = collection.join(CACHE).join(&authority);
        let moved = home.path().join("moved-owner");
        fs::rename(&owner, &moved).unwrap();
        assert!(
            transfer
                .dispatch(
                    &root,
                    &collection,
                    &Request::EndSync { lease: None },
                    &arbitration
                )
                .is_err()
        );
        fs::rename(moved, owner).unwrap();
        assert_eq!(
            transfer.begin_sync(&root, &collection, &authority).unwrap(),
            Reply::SyncStarted
        );
    }

    #[test]
    fn transfer_resolves_local_home_and_product_aliases() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let home = base.join("home");
        let home_alias = base.join("home-alias");
        let product = base.join("configured-product");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&product).unwrap();
        symlink(&home, &home_alias).unwrap();
        symlink(&product, home.join("rustx")).unwrap();
        let arbitration = Arbitration::new();
        let mut skills = crate::skills::UserSkills::from_home(Some(home_alias));
        let Reply::Collection { path, .. } = skills
            .dispatch(&Request::Collection { probe: None }, &arbitration)
            .unwrap()
        else {
            panic!("expected the Client-selected physical collection");
        };
        assert_eq!(path, product.canonicalize().unwrap().join(".agents/skills"));
        assert_eq!(
            skills
                .dispatch(
                    &Request::BeginSync {
                        authority: authority()
                    },
                    &arbitration,
                )
                .unwrap(),
            Reply::SyncStarted
        );
        assert!(
            product
                .join(".agents/skills/.rustx-sync")
                .join(authority())
                .is_dir()
        );
        skills.lane_lost().unwrap();
    }

    #[test]
    fn transfer_resolves_collection_and_agents_aliases() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let arbitration = Arbitration::new();
        let mut transfer = Transfer::default();
        let alternate_root = base.join("alternate-product");
        let collection_target = base.join("collection-target");
        fs::create_dir_all(alternate_root.join(".agents")).unwrap();
        fs::create_dir(&collection_target).unwrap();
        symlink(&collection_target, alternate_root.join(".agents/skills")).unwrap();
        assert_eq!(
            call(
                &mut transfer,
                &alternate_root,
                &arbitration,
                Request::BeginSync {
                    authority: authority()
                }
            )
            .unwrap(),
            Reply::SyncStarted
        );
        assert!(
            collection_target
                .join(".rustx-sync")
                .join(authority())
                .is_dir()
        );
        transfer.lane_lost().unwrap();

        let third_root = base.join("third-product");
        let agents_target = base.join("agents-target");
        fs::create_dir(&third_root).unwrap();
        fs::create_dir(&agents_target).unwrap();
        symlink(&agents_target, third_root.join(".agents")).unwrap();
        assert_eq!(
            call(
                &mut transfer,
                &third_root,
                &arbitration,
                Request::BeginSync {
                    authority: authority()
                }
            )
            .unwrap(),
            Reply::SyncStarted
        );
        assert!(
            agents_target
                .join("skills/.rustx-sync")
                .join(authority())
                .is_dir()
        );
        transfer.lane_lost().unwrap();
    }

    #[test]
    fn transfer_refuses_symlinked_cache_directory() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let arbitration = Arbitration::new();
        let mut transfer = Transfer::default();
        let unowned = base.join("unowned");
        fs::create_dir(&unowned).unwrap();
        let linked = base.join("linked-product");
        fs::create_dir_all(linked.join(".agents/skills")).unwrap();
        symlink(&unowned, linked.join(".agents/skills/.rustx-sync")).unwrap();
        assert!(
            call(
                &mut transfer,
                &linked,
                &arbitration,
                Request::BeginSync {
                    authority: authority()
                }
            )
            .is_err()
        );
        assert!(fs::read_dir(unowned).unwrap().next().is_none());
    }

    #[test]
    fn transfer_refuses_a_retargeted_collection_before_creating_cache() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("rustx");
        let first = temporary.path().join("first");
        let second = temporary.path().join("second");
        fs::create_dir_all(root.join(".agents")).unwrap();
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let link = root.join(".agents/skills");
        symlink(&first, &link).unwrap();
        let selected = super::super::skills::UserSkills::collection_path(&root).unwrap();
        fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();
        let error = Transfer::default()
            .dispatch(
                &root,
                &selected,
                &Request::BeginSync {
                    authority: authority(),
                },
                &Arbitration::new(),
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!first.join(".rustx-sync").exists());
        assert!(!second.join(".rustx-sync").exists());

        // Once a stage was authorized, a later alias change must not move
        // its cleanup to a different collection.
        fs::remove_file(&link).unwrap();
        symlink(&first, &link).unwrap();
        let mut transfer = Transfer::default();
        let arbitration = Arbitration::new();
        assert_eq!(
            transfer
                .dispatch(
                    &root,
                    &selected,
                    &Request::BeginSync {
                        authority: authority()
                    },
                    &arbitration
                )
                .unwrap(),
            Reply::SyncStarted
        );
        assert_eq!(
            transfer
                .dispatch(
                    &root,
                    &selected,
                    &Request::BeginPackage {
                        version: version('f')
                    },
                    &arbitration
                )
                .unwrap(),
            Reply::Accepted
        );
        fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();
        transfer.lane_lost().unwrap();
        let owner = first.join(".rustx-sync").join(authority());
        assert!(fs::read_dir(owner).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-")
        }));
        assert!(!second.join(".rustx-sync").exists());
    }
}
