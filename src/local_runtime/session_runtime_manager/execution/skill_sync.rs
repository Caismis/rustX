//! Host/Client User Skill filesystem identity.
//!
//! Device and inode values alone are not globally unique across machines or
//! filesystem namespaces. A fresh, private challenge must be visible at
//! both roots and the opened file's identity and content must agree.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

use base64::Engine as _;
use rustix::fs::{AtFlags, Mode, OFlags};
use rustx_confined_fs::PackageDirectory;
use rustx_execution_protocol::save::Sha256Hex;
use rustx_execution_protocol::skills::{
    MAX_DIGEST_BYTES, MAX_PACKAGE_ENTRIES, MAX_SKILL_BYTES, MAX_SYNC_BYTES, Reply, Request,
};
use rustx_execution_protocol::{DecimalU64, MAX_RAW_CHUNK_BYTES};
use rustx_execution_server::LocalConnection;
use sha2::{Digest, Sha256};

use super::LocalExecutionEndpoint;

struct ProbeFile<'a> {
    root: &'a PackageDirectory,
    name: String,
}

#[derive(Clone)]
struct UserSkillSync {
    collection: PathBuf,
    paths: BTreeMap<PathBuf, PathBuf>,
    capture: crate::skills::package::UserSkillCapture,
}

#[derive(Default)]
pub(super) struct SyncFlights {
    active: std::sync::Mutex<BTreeMap<PathBuf, std::sync::Arc<SyncFlight>>>,
}

struct SyncFlight {
    state: std::sync::Mutex<FlightState>,
    ready: std::sync::Condvar,
    #[cfg(test)]
    waiters: std::sync::atomic::AtomicUsize,
}

struct FlightState {
    result: Option<Result<UserSkillSync, FlightError>>,
    progress: Instant,
}

impl Default for SyncFlight {
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(FlightState {
                result: None,
                progress: Instant::now(),
            }),
            ready: std::sync::Condvar::new(),
            #[cfg(test)]
            waiters: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl SyncFlight {
    fn progressed(&self) {
        self.state.lock().expect("User Skill flight state").progress = Instant::now();
        self.ready.notify_all();
    }
}

fn wire_deadline() -> Instant {
    Instant::now() + std::time::Duration::from_secs(30)
}

#[derive(Clone)]
struct FlightError {
    kind: io::ErrorKind,
    message: String,
    outcome_unknown: bool,
}

impl FlightError {
    fn from_error(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
            outcome_unknown: error.get_ref().is_some_and(
                <dyn std::error::Error + Send + Sync>::is::<
                    rustx_execution_server::WriteOutcomeUnknown,
                >,
            ),
        }
    }

    fn into_error(self) -> io::Error {
        if self.outcome_unknown {
            io::Error::other(rustx_execution_server::WriteOutcomeUnknown)
        } else {
            io::Error::new(self.kind, self.message)
        }
    }
}

impl SyncFlights {
    fn run(
        &self,
        key: &Path,
        signal: &crate::runtime::CancellationSignal,
        deadline: Instant,
        work: impl FnOnce(&SyncFlight) -> io::Result<UserSkillSync>,
    ) -> io::Result<UserSkillSync> {
        crate::package_io::check_cancelled(signal)?;
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let (flight, leader) = {
            let mut active = self.active.lock().expect("User Skill flight registry");
            if let Some(flight) = active.get(key) {
                (flight.clone(), false)
            } else {
                let flight = std::sync::Arc::new(SyncFlight::default());
                active.insert(key.to_owned(), flight.clone());
                (flight, true)
            }
        };
        if leader {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&flight)))
                .unwrap_or_else(|_| {
                    Err(io::Error::other(
                        rustx_execution_server::WriteOutcomeUnknown,
                    ))
                });
            let shared = outcome
                .as_ref()
                .map(Clone::clone)
                .map_err(FlightError::from_error);
            let mut active = self.active.lock().expect("User Skill flight registry");
            flight.state.lock().expect("User Skill flight state").result = Some(shared);
            active.remove(key);
            drop(active);
            flight.ready.notify_all();
            outcome
        } else {
            #[cfg(test)]
            flight
                .waiters
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let idle_budget = deadline.saturating_duration_since(Instant::now());
            let mut cutoff = deadline;
            let mut state = flight.state.lock().expect("User Skill flight state");
            let mut observed_progress = state.progress;
            loop {
                crate::package_io::check_cancelled(signal)?;
                if state.progress > observed_progress {
                    observed_progress = state.progress;
                    cutoff = state.progress + idle_budget;
                }
                if Instant::now() >= cutoff {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                if let Some(outcome) = &state.result {
                    return outcome.clone().map_err(FlightError::into_error);
                }
                let wait = cutoff
                    .saturating_duration_since(Instant::now())
                    .min(std::time::Duration::from_millis(25));
                state = flight
                    .ready
                    .wait_timeout(state, wait)
                    .expect("User Skill flight wait")
                    .0;
            }
        }
    }
}

impl Drop for ProbeFile<'_> {
    fn drop(&mut self) {
        // Remove only the fresh probe itself, relative to the retained Host
        // root. Client cleanup never owns the standing User root.
        let _ = rustix::fs::unlinkat(self.root.descriptor(), self.name.as_str(), AtFlags::empty());
    }
}

impl LocalExecutionEndpoint {
    /// Return the Client's root and whether it is demonstrably the same
    /// physical standing root as the Host's. A missing or non-writable Host
    /// root cannot prove sharing and therefore never enables the skip.
    pub(crate) fn shared_user_skill_root(
        &self,
        host_home: &Path,
        deadline: Instant,
    ) -> io::Result<(PathBuf, bool)> {
        let host_root = host_home.join("rustx");
        let root = match PackageDirectory::open(&host_root) {
            Ok(root) => root,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) =>
            {
                let (path, _) = self.inspect_user_skill_root(None, deadline)?;
                return Ok((path, false));
            }
            Err(error) => return Err(error),
        };
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let name = format!(".rustx-skill-probe-{}", uuid::Uuid::new_v4().simple());
        let descriptor = match rustix::fs::openat(
            root.descriptor(),
            name.as_str(),
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(descriptor) => descriptor,
            Err(error)
                if error == rustix::io::Errno::ACCESS || error == rustix::io::Errno::ROFS =>
            {
                let (path, _) = self.inspect_user_skill_root(None, deadline)?;
                return Ok((path, false));
            }
            Err(error) => return Err(error.into()),
        };
        let _cleanup = ProbeFile {
            root: &root,
            name: name.clone(),
        };
        let mut probe = File::from(descriptor);
        probe.write_all(&random)?;
        let metadata = probe.metadata()?;
        let expected = Sha256Hex::from_digest(Sha256::digest(random).into());
        let (path, observed) = self.inspect_user_skill_root(Some(name), deadline)?;
        let shared = observed.is_some_and(|observed| {
            observed.device.get() == metadata.dev()
                && observed.inode.get() == metadata.ino()
                && observed.digest == expected
        });
        root.verify()?;
        Ok((path, shared))
    }

    /// Eagerly synchronize every discoverable Host User Skill and return a
    /// mapping from collection-relative instruction paths to Client paths.
    /// A matching marker skips the entire package without visiting assets.
    pub(crate) fn sync_user_skills(
        &self,
        host_home: &Path,
        signal: &crate::runtime::CancellationSignal,
        deadline: Instant,
    ) -> io::Result<(
        PathBuf,
        BTreeMap<PathBuf, PathBuf>,
        crate::skills::package::UserSkillCapture,
    )> {
        let sync = self
            .user_skill_flights
            .run(host_home, signal, deadline, |flight| {
                self.sync_user_skills_owned(host_home, signal, deadline, flight)
            })?;
        Ok((sync.collection, sync.paths, sync.capture))
    }

    fn sync_user_skills_owned(
        &self,
        host_home: &Path,
        signal: &crate::runtime::CancellationSignal,
        deadline: Instant,
        flight: &SyncFlight,
    ) -> io::Result<UserSkillSync> {
        let (client_root, shared) = self.shared_user_skill_root(host_home, deadline)?;
        flight.progressed();
        let client_collection = client_root.join(".agents/skills");
        let host_collection = host_home.join("rustx/.agents/skills");
        let capture = crate::skills::package::capture_user_skills(&host_collection, signal)?;
        flight.progressed();
        let host_collection = match host_collection.canonicalize() {
            Ok(root) => root,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(UserSkillSync {
                    collection: client_collection,
                    paths: BTreeMap::new(),
                    capture,
                });
            }
            Err(error) => return Err(error),
        };
        let found: Vec<_> = capture.candidates().collect();
        if shared {
            let paths = found
                .into_iter()
                .map(|(marker, _)| {
                    let relative = marker
                        .strip_prefix(&host_collection)
                        .map_err(|_| io::ErrorKind::InvalidData)?;
                    Ok((relative.to_path_buf(), client_collection.join(relative)))
                })
                .collect::<io::Result<_>>()?;
            return Ok(UserSkillSync {
                collection: client_collection,
                paths,
                capture,
            });
        }
        let mut invalid_paths = BTreeMap::new();
        let mut valid_files = Vec::new();
        for (marker, valid) in found {
            if valid {
                valid_files.push(marker);
            } else {
                let relative = marker
                    .strip_prefix(&host_collection)
                    .map_err(|_| io::ErrorKind::InvalidData)?;
                invalid_paths.insert(relative.to_owned(), client_collection.join(relative));
            }
        }
        if valid_files.is_empty() {
            return Ok(UserSkillSync {
                collection: client_collection,
                paths: invalid_paths,
                capture,
            });
        }
        let authority = authority_id(&host_home.join("rustx"), &host_collection)?;
        let paths = self.with_bounded_data_connection(deadline, |connection| {
            let result = sync_packages(
                connection,
                &authority,
                &host_collection,
                &client_collection,
                &valid_files,
                signal,
                deadline,
                flight,
            );
            // `wait_for_connection` retries WouldBlock, which is only safe
            // before an exchange. Never replay a partially sent sync.
            result.map_err(|error| {
                if error.kind() == io::ErrorKind::WouldBlock {
                    io::Error::other(error)
                } else {
                    error
                }
            })
        })?;
        invalid_paths.extend(paths);
        Ok(UserSkillSync {
            collection: client_collection,
            paths: invalid_paths,
            capture,
        })
    }
}

fn authority_id(standing_root: &Path, collection: &Path) -> io::Result<String> {
    let metadata = collection.metadata()?;
    let mut hash = Sha256::new();
    hash.update(b"rustx user skill authority v1\0");
    hash.update(standing_root_authority(standing_root)?);
    hash.update(collection.as_os_str().as_bytes());
    hash.update(metadata.dev().to_be_bytes());
    hash.update(metadata.ino().to_be_bytes());
    // Machine identity, when present, adds another distinction between
    // otherwise similar Host installations. The random standing-root ID
    // remains sufficient for stable reuse when a platform has no machine ID.
    let machine = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/etc/machine-id")
        .ok()
        .and_then(|file| {
            if !file.metadata().ok()?.is_file() {
                return None;
            }
            let mut value = Vec::new();
            file.take(1025).read_to_end(&mut value).ok()?;
            (value.len() <= 1024 && !value.is_empty()).then_some(value)
        })
        .unwrap_or_default();
    hash.update(machine);
    Ok(String::from(Sha256Hex::from_digest(hash.finalize().into())))
}

const AUTHORITY_FILE: &str = ".rustx-skill-authority";

fn process_authority() -> [u8; 32] {
    static PROCESS_ID: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    *PROCESS_ID.get_or_init(|| {
        let mut value = [0u8; 32];
        getrandom::fill(&mut value).expect("OS entropy for process Skill authority");
        value
    })
}

fn read_authority(root: &PackageDirectory) -> io::Result<[u8; 32]> {
    let mut file = root.file(Path::new(AUTHORITY_FILE))?;
    let metadata = file.metadata()?;
    let entry = rustx_confined_fs::FileMetadata::of(&file)?;
    if metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.len() != 32
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe User Skill authority identity",
        ));
    }
    let mut value = [0u8; 32];
    file.read_exact(&mut value)?;
    let after = file.metadata()?;
    if file_stamp(&metadata) != file_stamp(&after)
        || after.mode() != metadata.mode()
        || root.entry_metadata(Path::new(AUTHORITY_FILE))? != entry
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    root.verify()?;
    Ok(value)
}

fn standing_root_authority(path: &Path) -> io::Result<[u8; 32]> {
    let root = PackageDirectory::open(path)?;
    match read_authority(&root) {
        Ok(value) => return Ok(value),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let name = format!(".rustx-skill-authority-{}", uuid::Uuid::new_v4().simple());
    let fd = match rustix::fs::openat(
        root.descriptor(),
        name.as_str(),
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::ACCESS || error == rustix::io::Errno::ROFS => {
            return Ok(process_authority());
        }
        Err(error) => return Err(error.into()),
    };
    let _cleanup = ProbeFile {
        root: &root,
        name: name.clone(),
    };
    let mut value = [0u8; 32];
    getrandom::fill(&mut value).map_err(io::Error::other)?;
    let mut file = File::from(fd);
    file.write_all(&value)?;
    file.sync_all()?;
    root.verify()?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let published = rustix::fs::renameat_with(
        root.descriptor(),
        name.as_str(),
        root.descriptor(),
        AUTHORITY_FILE,
        rustix::fs::RenameFlags::NOREPLACE,
    );
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let published: rustix::io::Result<()> = Err(rustix::io::Errno::NOSYS);
    match published {
        Ok(()) | Err(rustix::io::Errno::EXIST) => read_authority(&root),
        Err(rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL | rustix::io::Errno::ROFS) => {
            Ok(process_authority())
        }
        Err(error) => Err(error.into()),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one binding's sync context and cancellation authority"
)]
fn sync_packages(
    connection: &mut LocalConnection,
    authority: &str,
    host_collection: &Path,
    client_collection: &Path,
    files: &[PathBuf],
    signal: &crate::runtime::CancellationSignal,
    deadline: Instant,
    flight: &SyncFlight,
) -> io::Result<BTreeMap<PathBuf, PathBuf>> {
    loop {
        crate::package_io::check_cancelled(signal)?;
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match connection.user_skill_transfer_until(
            &Request::BeginSync {
                authority: authority.into(),
            },
            deadline.min(wire_deadline()),
        )? {
            Reply::SyncStarted => {
                flight.progressed();
                break;
            }
            Reply::Busy => std::thread::sleep(std::time::Duration::from_millis(25)),
            _ => return Err(io::ErrorKind::InvalidData.into()),
        }
    }
    let result = sync_locked(
        connection,
        authority,
        host_collection,
        client_collection,
        files,
        signal,
        flight,
    );
    if result.as_ref().err().is_some_and(|error| {
        error
            .get_ref()
            .is_some_and(<dyn std::error::Error + Send + Sync>::is::<rustx_execution_server::WriteOutcomeUnknown>)
    }) {
        return result;
    }
    if result.is_err() {
        connection.user_skill_transfer_until(&Request::AbortPackage, wire_deadline())?;
    }
    let ended = connection.user_skill_transfer_until(&Request::EndSync, wire_deadline());
    match (result, ended) {
        (Ok(paths), Ok(Reply::SyncEnded)) => Ok(paths),
        (_, Err(error)) | (Err(error), _) => Err(error),
        _ => Err(io::ErrorKind::InvalidData.into()),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "each package's digest, retain, copy and publication form one sequence"
)]
fn sync_locked(
    connection: &mut LocalConnection,
    authority: &str,
    host_collection: &Path,
    client_collection: &Path,
    files: &[PathBuf],
    signal: &crate::runtime::CancellationSignal,
    flight: &SyncFlight,
) -> io::Result<BTreeMap<PathBuf, PathBuf>> {
    let mut mapped = BTreeMap::new();
    let mut total = 0u64;
    for marker in files {
        crate::package_io::check_cancelled(signal)?;
        flight.progressed();
        let relative = marker
            .strip_prefix(host_collection)
            .map_err(|_| io::ErrorKind::InvalidData)?;
        let package_dir = marker.file_name().is_some_and(|name| name == "SKILL.md");
        let selected_marker = marker.canonicalize()?;
        let (source, source_name) = marker_source(marker, package_dir)?;
        let digest = if package_dir {
            marker_digest(&source, &source_name, signal)?
        } else {
            None
        };
        let before = source.file(Path::new(&source_name))?.metadata()?;
        let marker_stamp = file_stamp(&before);
        let mut hash = Sha256::new();
        hash.update(b"rustx user skill version v1\0");
        hash.update(relative.as_os_str().as_bytes());
        if let Some(digest) = digest {
            hash.update(digest.bytes());
        } else {
            // No marker digest can establish a reusable cache hit. Keep the
            // same staged path for repeated captures in this one binding,
            // while every new Client bind copies it again.
            hash.update(connection.admission().instance.as_str().as_bytes());
            hash.update(format!("{marker_stamp:?}").as_bytes());
        }
        let mut version = String::from(Sha256Hex::from_digest(hash.finalize().into()));
        let fallback = String::from(Sha256Hex::of(
            format!("{version}:{}", connection.admission().instance.as_str()).as_bytes(),
        ));
        let mut client_package = cache_path(authority, &version);
        if let Some(expected) = digest {
            let mut cached = None;
            for candidate in [&version, &fallback] {
                let path = cache_path(authority, candidate);
                if matches!(connection.user_skills_until(&Request::Digest { package: path.clone() }, wire_deadline())?, Reply::Digest { digest: Some(actual) } if actual == expected)
                {
                    match connection.user_skill_transfer_until(
                        &Request::Retain {
                            version: candidate.clone(),
                        },
                        wire_deadline(),
                    )? {
                        Reply::Retained { package, bytes } if package == path => {
                            charge_retained(&mut total, bytes.get())?;
                            cached = Some(path);
                        }
                        _ => return Err(io::ErrorKind::InvalidData.into()),
                    }
                    break;
                }
            }
            if let Some(path) = cached {
                if marker_digest(&source, &source_name, signal)? != Some(expected)
                    || file_stamp(&source.file(Path::new(&source_name))?.metadata()?)
                        != marker_stamp
                    || marker.canonicalize()? != selected_marker
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Skill marker changed during inquiry",
                    ));
                }
                mapped.insert(
                    relative.to_path_buf(),
                    client_collection.join(path).join("SKILL.md"),
                );
                continue;
            }
        } else {
            match connection.user_skill_transfer_until(
                &Request::Retain {
                    version: version.clone(),
                },
                wire_deadline(),
            ) {
                Ok(Reply::Retained { package, bytes }) if package == client_package => {
                    charge_retained(&mut total, bytes.get())?;
                    if file_stamp(&source.file(Path::new(&source_name))?.metadata()?)
                        != marker_stamp
                        || marker.canonicalize()? != selected_marker
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Skill marker changed during inquiry",
                        ));
                    }
                    mapped.insert(
                        relative.to_path_buf(),
                        client_collection
                            .join(package)
                            .join(marker.file_name().ok_or(io::ErrorKind::InvalidData)?),
                    );
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                _ => return Err(io::ErrorKind::InvalidData.into()),
            }
        }
        loop {
            expect_accepted(
                connection,
                &Request::BeginPackage {
                    version: version.clone(),
                },
                wire_deadline(),
            )?;
            let mut budget = CopyBudget {
                bytes: 0,
                entries: 0,
                total: &mut total,
                signal,
                flight,
            };
            if package_dir {
                copy_directory(connection, &source, Path::new(""), &mut budget)?;
            } else {
                copy_file(
                    connection,
                    &source,
                    Path::new(&source_name),
                    Path::new(marker.file_name().ok_or(io::ErrorKind::InvalidData)?),
                    &mut budget,
                )?;
            }
            if digest.is_some() && marker_digest(&source, &source_name, signal)? != digest {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Skill marker changed during copy",
                ));
            }
            if file_stamp(&source.file(Path::new(&source_name))?.metadata()?) != marker_stamp {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Skill marker changed during copy",
                ));
            }
            source.verify()?;
            if marker.canonicalize()? != selected_marker {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Skill source changed during copy",
                ));
            }
            match connection.user_skill_transfer_until(&Request::Publish, wire_deadline())? {
                Reply::Published { package } if package == client_package => break,
                Reply::VersionExists => {
                    // Never replace a published version: a different Session
                    // may still hold its paths. Use a fresh immutable version.
                    version = if version == fallback {
                        String::from(Sha256Hex::of(uuid::Uuid::new_v4().as_bytes()))
                    } else {
                        fallback.clone()
                    };
                    client_package = cache_path(authority, &version);
                }
                _ => return Err(io::ErrorKind::InvalidData.into()),
            }
        }
        let filename = marker.file_name().ok_or(io::ErrorKind::InvalidData)?;
        mapped.insert(
            relative.to_path_buf(),
            client_collection.join(client_package).join(filename),
        );
    }
    Ok(mapped)
}

fn cache_path(authority: &str, version: &str) -> PathBuf {
    PathBuf::from(".rustx-sync")
        .join(authority)
        .join(version)
        .join("data")
}

fn marker_source(
    marker: &Path,
    package_dir: bool,
) -> io::Result<(PackageDirectory, std::ffi::OsString)> {
    // A link offered as the package directory is an explicit source choice.
    // Pin that resolved directory; links *inside* it are never followed.
    if package_dir {
        let root = marker
            .parent()
            .ok_or(io::ErrorKind::InvalidData)?
            .canonicalize()?;
        Ok((
            PackageDirectory::open(&root)?,
            marker
                .file_name()
                .ok_or(io::ErrorKind::InvalidData)?
                .to_owned(),
        ))
    } else {
        // A root-level Markdown entry names just this one file.
        let file = marker.canonicalize()?;
        let root = file.parent().ok_or(io::ErrorKind::InvalidData)?;
        Ok((
            PackageDirectory::open(root)?,
            file.file_name()
                .ok_or(io::ErrorKind::InvalidData)?
                .to_owned(),
        ))
    }
}

fn marker_digest(
    source: &PackageDirectory,
    name: &std::ffi::OsStr,
    signal: &crate::runtime::CancellationSignal,
) -> io::Result<Option<Sha256Hex>> {
    let mut file = source.file(Path::new(name))?;
    let entry = source.entry_metadata(Path::new(name))?;
    if rustx_confined_fs::FileMetadata::of(&file)? != entry {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    if before.len() > MAX_DIGEST_BYTES {
        return Ok(None);
    }
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 16 * 1024];
    let deadline = wire_deadline();
    loop {
        crate::package_io::check_cancelled(signal)?;
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count += u64::try_from(read).expect("bounded read");
        if count > MAX_DIGEST_BYTES {
            return Ok(None);
        }
        digest.update(&buffer[..read]);
    }
    let after = file.metadata()?;
    if count != after.len()
        || file_stamp(&before) != file_stamp(&after)
        || source.entry_metadata(Path::new(name))? != entry
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "skill marker changed during inquiry",
        ));
    }
    Ok(Some(Sha256Hex::from_digest(digest.finalize().into())))
}

fn charge_retained(total: &mut u64, bytes: u64) -> io::Result<()> {
    if bytes > MAX_SKILL_BYTES || bytes > MAX_SYNC_BYTES.saturating_sub(*total) {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "User Skills exceed the sync cap; exclude a package with .ignore",
        ));
    }
    *total += bytes;
    Ok(())
}

fn file_stamp(metadata: &std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

struct CopyBudget<'a> {
    bytes: u64,
    entries: usize,
    total: &'a mut u64,
    signal: &'a crate::runtime::CancellationSignal,
    flight: &'a SyncFlight,
}

impl CopyBudget<'_> {
    fn entry(&mut self) -> io::Result<()> {
        crate::package_io::check_cancelled(self.signal)?;
        self.flight.progressed();
        self.entries += 1;
        if self.entries > MAX_PACKAGE_ENTRIES {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        Ok(())
    }
}

fn copy_directory(
    connection: &mut LocalConnection,
    source: &PackageDirectory,
    relative: &Path,
    budget: &mut CopyBudget<'_>,
) -> io::Result<()> {
    if relative.components().count() > rustx_execution_protocol::skills::MAX_PACKAGE_DEPTH {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    for child in source.names(MAX_PACKAGE_ENTRIES.saturating_sub(budget.entries))? {
        budget.entry()?;
        let child_relative = relative.join(&child);
        let metadata = source_entry(source, &child)?;
        if metadata.is_directory {
            expect_accepted(
                connection,
                &Request::Directory {
                    path: child_relative.clone(),
                },
                wire_deadline(),
            )?;
            let child_dir = source.child(child.as_os_str())?;
            if child_dir.entry_metadata(Path::new(""))? != metadata {
                return Err(io::ErrorKind::InvalidData.into());
            }
            copy_directory(connection, &child_dir, &child_relative, budget)?;
        } else if metadata.is_file {
            copy_file(connection, source, &child, &child_relative, budget)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Skill package contains a link or special file",
            ));
        }
    }
    source.verify()?;
    Ok(())
}

fn source_entry(
    source: &PackageDirectory,
    child: &Path,
) -> io::Result<rustx_confined_fs::FileMetadata> {
    let metadata = source.entry_metadata(child)?;
    if !metadata.is_directory && !metadata.is_file {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Skill package contains a link or special file",
        ));
    }
    Ok(metadata)
}

fn copy_file(
    connection: &mut LocalConnection,
    source: &PackageDirectory,
    name: &Path,
    relative: &Path,
    budget: &mut CopyBudget<'_>,
) -> io::Result<()> {
    let entry = source.entry_metadata(name)?;
    if !entry.is_file {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut file = source.file(name)?;
    if rustx_confined_fs::FileMetadata::of(&file)? != entry {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let before = file.metadata()?;
    if !before.is_file()
        || before.len() > MAX_SKILL_BYTES.saturating_sub(budget.bytes)
        || before.len() > MAX_SYNC_BYTES.saturating_sub(*budget.total)
    {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "User Skill exceeds the sync cap; exclude the package with .ignore",
        ));
    }
    budget.bytes += before.len();
    *budget.total += before.len();
    expect_accepted(
        connection,
        &Request::BeginFile {
            path: relative.to_owned(),
            length: DecimalU64::new(before.len()),
            executable: before.mode() & 0o111 != 0,
        },
        wire_deadline(),
    )?;
    let mut offset = 0;
    let mut buffer = vec![0u8; MAX_RAW_CHUNK_BYTES];
    loop {
        crate::package_io::check_cancelled(budget.signal)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if count as u64 > before.len().saturating_sub(offset) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        expect_accepted(
            connection,
            &Request::Chunk {
                offset: DecimalU64::new(offset),
                data_base64: base64::engine::general_purpose::STANDARD.encode(&buffer[..count]),
            },
            wire_deadline(),
        )?;
        budget.flight.progressed();
        offset += count as u64;
    }
    if offset != before.len()
        || file_stamp(&before) != file_stamp(&file.metadata()?)
        || source.entry_metadata(name)? != entry
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Skill file changed during copy",
        ));
    }
    expect_accepted(connection, &Request::EndFile, wire_deadline())
}

fn expect_accepted(
    connection: &mut LocalConnection,
    request: &Request,
    deadline: Instant,
) -> io::Result<()> {
    if connection.user_skill_transfer_until(request, deadline)? == Reply::Accepted {
        Ok(())
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn standing_authorities_are_distinct_stable_and_malformed_files_fail_closed() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let one = standing_root_authority(&first).unwrap();
        let two = standing_root_authority(&second).unwrap();
        assert_ne!(one, two);
        assert_eq!(one, standing_root_authority(&first).unwrap());
        assert_eq!(
            std::fs::metadata(first.join(AUTHORITY_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::write(first.join(AUTHORITY_FILE), b"malformed").unwrap();
        assert!(standing_root_authority(&first).is_err());
    }

    #[test]
    fn concurrent_authority_initialization_publishes_one_complete_identity() {
        let temp = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                barrier.wait();
                standing_root_authority(temp.path()).unwrap()
            });
            let second = scope.spawn(|| {
                barrier.wait();
                standing_root_authority(temp.path()).unwrap()
            });
            barrier.wait();
            assert_eq!(first.join().unwrap(), second.join().unwrap());
        });
    }

    #[test]
    fn read_only_standing_root_uses_process_identity_without_creating_a_file() {
        use std::os::unix::fs::PermissionsExt as _;
        if nix::unistd::geteuid().is_root() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        assert_eq!(
            standing_root_authority(temp.path()).unwrap(),
            process_authority()
        );
        assert!(!temp.path().join(AUTHORITY_FILE).exists());
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn concurrent_same_target_joins_one_capture_and_later_refresh_runs_again() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let flights = std::sync::Arc::new(SyncFlights::default());
        let captures = std::sync::Arc::new(AtomicUsize::new(0));
        let directory = tempfile::tempdir().unwrap();
        let capture = crate::skills::package::capture_user_skills(
            &directory.path().join("missing"),
            &crate::runtime::CancellationSignal::new(),
        )
        .unwrap();
        let key = PathBuf::from("/home/example");
        let (entered, started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        std::thread::scope(|scope| {
            let owner_flights = flights.clone();
            let owner_key = key.clone();
            let owner_captures = captures.clone();
            let owner_capture = capture.clone();
            let owner = scope.spawn(move || {
                owner_flights.run(
                    &owner_key,
                    &crate::runtime::CancellationSignal::new(),
                    deadline,
                    |_| {
                        owner_captures.fetch_add(1, Ordering::SeqCst);
                        entered.send(()).unwrap();
                        released.recv().unwrap();
                        Ok(UserSkillSync {
                            collection: "/client/rustx/.agents/skills".into(),
                            paths: BTreeMap::new(),
                            capture: owner_capture,
                        })
                    },
                )
            });
            started.recv().unwrap();
            let waiter = scope.spawn(|| {
                flights.run(
                    &key,
                    &crate::runtime::CancellationSignal::new(),
                    deadline,
                    |_| panic!("concurrent caller must join"),
                )
            });
            let cancel_signal = crate::runtime::CancellationSignal::new();
            let wait_signal = cancel_signal.clone();
            let cancelled_flights = flights.clone();
            let cancelled_key = key.clone();
            let cancelled = scope.spawn(move || {
                cancelled_flights.run(&cancelled_key, &wait_signal, deadline, |_| {
                    panic!("cancelled caller must join")
                })
            });
            loop {
                let joined = flights
                    .active
                    .lock()
                    .unwrap()
                    .get(&key)
                    .unwrap()
                    .waiters
                    .load(Ordering::SeqCst)
                    >= 2;
                if joined {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            cancel_signal.cancel();
            assert!(cancelled.join().unwrap().is_err());
            release.send(()).unwrap();
            assert_eq!(
                owner.join().unwrap().unwrap().collection,
                waiter.join().unwrap().unwrap().collection
            );
        });
        assert_eq!(captures.load(Ordering::SeqCst), 1);
        flights
            .run(
                &key,
                &crate::runtime::CancellationSignal::new(),
                deadline,
                |_| {
                    captures.fetch_add(1, Ordering::SeqCst);
                    Ok(UserSkillSync {
                        collection: "/next".into(),
                        paths: BTreeMap::new(),
                        capture,
                    })
                },
            )
            .unwrap();
        assert_eq!(captures.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn source_tree_rejects_links_and_fifos_but_accepts_collection_package_links() {
        let temporary = tempfile::tempdir().unwrap();
        let package = temporary.path().join("actual");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("SKILL.md"), b"instructions").unwrap();
        let collection_link = temporary.path().join("offered");
        symlink(&package, &collection_link).unwrap();
        let marker = collection_link.join("SKILL.md");
        let (root, name) = marker_source(&marker, true).unwrap();
        assert_eq!(
            marker_digest(&root, &name, &crate::runtime::CancellationSignal::new()).unwrap(),
            Some(Sha256Hex::of(b"instructions"))
        );
        let cancelled = crate::runtime::CancellationSignal::new();
        cancelled.cancel();
        assert!(marker_digest(&root, &name, &cancelled).is_err());
        symlink(temporary.path(), package.join("outside-link")).unwrap();
        assert!(source_entry(&root, Path::new("outside-link")).is_err());
        nix::unistd::mkfifo(&package.join("pipe"), nix::sys::stat::Mode::S_IRUSR).unwrap();
        assert!(source_entry(&root, Path::new("pipe")).is_err());
        std::fs::remove_file(package.join("SKILL.md")).unwrap();
        symlink(temporary.path().join("outside"), package.join("SKILL.md")).unwrap();
        assert!(marker_digest(&root, &name, &crate::runtime::CancellationSignal::new()).is_err());
    }

    #[test]
    fn retained_and_new_bytes_share_the_total_cap() {
        let mut total = 0;
        charge_retained(&mut total, MAX_SKILL_BYTES).unwrap();
        charge_retained(&mut total, MAX_SKILL_BYTES).unwrap();
        assert!(charge_retained(&mut total, 1).is_err());
    }
}
