//! Physical source capture and prepared-environment publication for Client-owned Python tools.

use rustx_confined_fs::PackageDirectory;
use rustx_process_supervisor::process_runner::{
    ProcessOutcomeIntent, RunnerBackedProcessRunner, SupervisedCommandSpec, SupervisedProcessRunner,
};
use rustx_process_supervisor::signal::ProcessSignal;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod catalog;
mod requirements;

pub(crate) use catalog::{Catalog, SourceCleanup};

pub(super) const MANAGED_FASTMCP_VERSION: &str = "4.0.3";
const MANAGED_MCP_NAMESPACE: &str = "python:";
const SERVER_FILE: &str = "server.py";
const REQUIREMENTS_FILE: &str = "requirements.txt";
const PERMISSIONS_FILE: &str = "permissions.toml";
const ENTRYPOINT: &str = "server.py:mcp";
const PACKAGES_DIRECTORY: &str = "packages";
const UV_CACHE_DIRECTORY: &str = "uv-cache";
const SOURCE_DIRECTORY: &str = "source";
const VENV_DIRECTORY: &str = "venv";
const MANIFEST_FILE: &str = "manifest.json";
const MANIFEST_FORMAT: &str = "rustx-managed-python-package-v1";
/// Finite deadline of one managed-Python runtime identity probe.
const PYTHON_TOOL_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);
/// Finite deadline of one managed-Python uv lock or materialization command.
const PYTHON_TOOL_UV_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(10);

/// A package discovery/preparation failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PythonToolError {
    /// The package is malformed.
    #[error("invalid Python tool package: {0}")]
    InvalidPackage(String),
    /// Prepared state could not be read or published.
    #[error("Python tool storage failed: {0}")]
    Storage(String),
    /// The dependency environment could not be checked or materialized.
    #[error("Python tool environment failed: {0}")]
    Environment(String),
    #[error("Python physical cleanup is unconfirmed: {0}")]
    Unconfirmed(String),
}

/// The Client's frozen bytes of one admitted source. The Server receives only a
/// digest and the bounded permission document for its own policy validation.
#[derive(Debug, Clone)]
pub(crate) struct PythonToolPackage {
    name: String,
    root: PathBuf,
    files: Vec<PathBuf>,
    source: Arc<PythonSource>,
    pub(super) source_digest: String,
    requirements: Vec<String>,
}

/// One anonymous backing file, with bounded in-memory file coordinates. Its last
/// Arc closes the descriptor; the kernel owns cleanup even after cancellation.
#[derive(Debug)]
struct PythonSource {
    file: std::fs::File,
    entries: BTreeMap<PathBuf, (u64, u64, std::fs::Permissions)>,
    _quota: SnapshotQuota,
}

#[derive(Debug)]
struct SnapshotQuota {
    bytes: Arc<AtomicU64>,
    owned: u64,
}

impl SnapshotQuota {
    fn reserve(&mut self, length: u64) -> std::io::Result<()> {
        const PER_SOURCE: u64 = 512 * 1024 * 1024;
        const PER_BINDING: u64 = 1024 * 1024 * 1024;
        let quota = || std::io::Error::from(std::io::ErrorKind::QuotaExceeded);
        let owned = self
            .owned
            .checked_add(length)
            .filter(|value| *value <= PER_SOURCE)
            .ok_or_else(quota)?;
        self.bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(length)
                    .filter(|value| *value <= PER_BINDING)
            })
            .map_err(|_| quota())?;
        self.owned = owned;
        Ok(())
    }
}

impl Drop for SnapshotQuota {
    fn drop(&mut self) {
        self.bytes.fetch_sub(self.owned, Ordering::AcqRel);
    }
}
impl PythonSource {
    fn copy_to(
        &self,
        relative: &Path,
        destination: &Path,
        cancellation: &ProcessSignal,
    ) -> Result<(), PythonToolError> {
        use std::os::unix::fs::FileExt;
        let (offset, length, permissions) = self
            .entries
            .get(relative)
            .ok_or_else(|| PythonToolError::Storage("unknown captured Python file".into()))?;
        std::fs::create_dir_all(
            destination
                .parent()
                .ok_or_else(|| PythonToolError::Storage("destination has no parent".into()))?,
        )
        .map_err(io_error)?;
        let mut output = std::fs::File::create_new(destination).map_err(io_error)?;
        let mut copied = 0;
        let mut buffer = vec![0; CHUNK_BYTES];
        while copied < *length {
            check_cancelled(cancellation).map_err(io_error)?;
            let count =
                usize::try_from((*length - copied).min(buffer.len() as u64)).map_err(io_error)?;
            let read = self
                .file
                .read_at(&mut buffer[..count], offset + copied)
                .map_err(io_error)?;
            if read == 0 {
                return Err(PythonToolError::Storage(
                    "captured Python file was truncated".into(),
                ));
            }
            output.write_all(&buffer[..read]).map_err(io_error)?;
            copied += read as u64;
        }
        output
            .set_permissions(permissions.clone())
            .map_err(io_error)?;
        check_cancelled(cancellation).map_err(io_error)
    }
}

pub(super) fn discover_package(
    root: &Path,
    name: &str,
    cancellation: &ProcessSignal,
    quota: Arc<AtomicU64>,
) -> Result<(PythonToolPackage, Option<Vec<u8>>), PythonToolError> {
    let invalid = |message: String| {
        PythonToolError::InvalidPackage(format!("package {name:?} ({}): {message}", root.display()))
    };
    let requirements_error = |error| match error {
        PythonToolError::InvalidPackage(message) => {
            invalid(format!("{REQUIREMENTS_FILE}: {message}"))
        }
        other => other,
    };
    let metadata = std::fs::symlink_metadata(root).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid(format!(
            "package symlink is rejected: {}",
            root.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(invalid("package entry is not a directory".to_owned()));
    }
    validate_identifier(name).map_err(|error| match error {
        PythonToolError::InvalidPackage(message) => invalid(message),
        other => other,
    })?;
    let mut files = collect_files(root, cancellation)?;
    // Membership is already complete. Reject malformed winning candidates
    // before creating capture storage or reading any supporting asset bytes.
    if !files.iter().any(|path| path == Path::new(SERVER_FILE)) {
        return Err(invalid(format!(
            "the package has no {SERVER_FILE} (the managed FastMCP server)"
        )));
    }
    if !files
        .iter()
        .any(|path| path == Path::new(REQUIREMENTS_FILE))
    {
        return Err(invalid(format!(
            "the package has no {REQUIREMENTS_FILE} (required, even when empty)"
        )));
    }
    // Resource permissions are package metadata, not frozen server source:
    // parse them off the winning folder, then exclude the document from the
    // code/dependency fingerprint and prepared env cache.
    let permissions = if files.iter().any(|path| path == Path::new(PERMISSIONS_FILE)) {
        files.retain(|path| path != Path::new(PERMISSIONS_FILE));
        let directory = PackageDirectory::open(root).map_err(io_error)?;
        Some(bounded_read(
            &directory,
            Path::new(PERMISSIONS_FILE),
            512 * 1024,
        )?)
    } else {
        None
    };
    let (source, source_digest, requirements) =
        capture_package_source(root, &files, cancellation, quota, requirements_error)?;
    Ok((
        PythonToolPackage {
            name: name.to_owned(),
            root: root.to_path_buf(),
            files,
            source: Arc::new(source),
            source_digest,
            requirements,
        },
        permissions,
    ))
}

/// Captures package source bytes into an anonymous backing file and parses
/// `requirements.txt` declarations from the same stream.
fn capture_package_source(
    root: &Path,
    files: &[PathBuf],
    cancellation: &ProcessSignal,
    bytes: Arc<AtomicU64>,
    requirements_error: impl Fn(PythonToolError) -> PythonToolError,
) -> Result<(PythonSource, String, Vec<String>), PythonToolError> {
    let mut quota = SnapshotQuota { bytes, owned: 0 };
    let mut source_file = tempfile::tempfile().map_err(io_error)?;
    let directory = PackageDirectory::open(root).map_err(io_error)?;
    let mut hash = sha2::Sha256::new();
    hash.update(b"rustx:managed-python-source:v2\0");
    let mut entries = BTreeMap::new();
    let mut offset = 0_u64;
    let mut requirements = requirements::Parser::default();
    for path in files {
        if path == Path::new(REQUIREMENTS_FILE) {
            requirements = requirements::Parser::new(offset);
        }
        let mut parsing_error = None;
        let name = path.as_os_str().as_encoded_bytes();
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name);
        let evidence = stream(&directory, path, cancellation, |bytes| {
            quota.reserve(bytes.len() as u64)?;
            source_file.write_all(bytes)?;
            if path == Path::new(REQUIREMENTS_FILE)
                && let Err(message) = requirements.push(bytes, &source_file)
            {
                parsing_error = Some(message);
                return Err(std::io::Error::other("invalid requirements"));
            }
            Ok(())
        })
        .map_err(|error| match parsing_error {
            Some(error) => requirements_error(error),
            None => io_error(error),
        })?;
        hash.update(evidence.metadata.len().to_be_bytes());
        hash.update(evidence.digest);
        let metadata = evidence.metadata;
        entries.insert(
            path.clone(),
            (offset, metadata.len(), metadata.permissions()),
        );
        offset = offset
            .checked_add(metadata.len())
            .ok_or_else(|| PythonToolError::Storage("captured package length overflow".into()))?;
    }
    let source_digest = format!("sha256:{}", hex_digest(&hash.finalize()));
    let source = PythonSource {
        file: source_file,
        entries,
        _quota: quota,
    };
    // Only declarations require text parsing. Binary supporting assets are copied
    // and hashed as bytes and are never held in the parsed package representation.
    let requirements = requirements
        .finish(&source.file)
        .map_err(requirements_error)?;
    Ok((source, source_digest, requirements))
}

/// Enumerate metadata through anchored directory handles, without opening links.
fn collect_files(
    root: &Path,
    cancellation: &ProcessSignal,
) -> Result<Vec<PathBuf>, PythonToolError> {
    fn walk(
        directory: &PackageDirectory,
        prefix: &Path,
        files: &mut Vec<PathBuf>,
        remaining: &mut usize,
        depth: usize,
        cancellation: &ProcessSignal,
    ) -> Result<(), PythonToolError> {
        check_cancelled(cancellation).map_err(io_error)?;
        if depth > 64 {
            return Err(PythonToolError::InvalidPackage(
                "package nesting exceeds 64".into(),
            ));
        }
        for name in directory.names(1024).map_err(io_error)? {
            *remaining = remaining.checked_sub(1).ok_or_else(|| {
                PythonToolError::InvalidPackage("package exceeds 4096 entries".into())
            })?;
            let path = prefix.join(&name);
            if path.to_str().is_none() {
                return Err(PythonToolError::InvalidPackage(
                    "package paths must be valid UTF-8".into(),
                ));
            }
            if let Ok(child) = directory.child(name.as_os_str()) {
                walk(&child, &path, files, remaining, depth + 1, cancellation)?;
            } else {
                // Includes symlinks, FIFOs and unsupported entry kinds: the
                // opened descriptor, not a prior path check, decides.
                directory.file(&name).map_err(|e| PythonToolError::InvalidPackage(format!("package entry {} is not a regular file or directory (symlinks are rejected): {e}", path.display())))?;
                files.push(path);
            }
        }
        Ok(())
    }
    let directory = PackageDirectory::open(root).map_err(io_error)?;
    let mut files = Vec::new();
    walk(
        &directory,
        Path::new(""),
        &mut files,
        &mut 4096,
        0,
        cancellation,
    )?;
    files.sort();
    Ok(files)
}

pub(super) fn validate_identifier(identifier: &str) -> Result<(), PythonToolError> {
    if identifier.is_empty()
        || identifier.starts_with('-')
        || identifier.ends_with('-')
        || identifier.contains("--")
        || !identifier.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(PythonToolError::InvalidPackage(format!(
            "invalid package name {identifier:?}"
        )));
    }
    Ok(())
}

/// Parses the normalized requirement lines of one `requirements.txt`.
///
/// This is deliberately conservative: the only dependency semantics rustX
/// must own is the `fastmcp` conflict check — the managed `FastMCP` build is
/// rustX-pinned, so a package may never declare it. Everything else is
/// validated only far enough to become a `pyproject.toml` dependency entry;
/// real dependency semantics (resolution, markers, indexes) belong to uv.
fn parse_requirement(line: &str, number: usize) -> Result<String, String> {
    if line.starts_with('-') {
        return Err(format!(
            "line {number}: option lines are not supported; declare PEP 508 dependencies only"
        ));
    }
    if line.ends_with('\\') {
        return Err(format!(
            "line {number}: line continuations are not supported"
        ));
    }
    if contains_environment_variable_reference(line) {
        return Err(format!(
            "line {number}: environment-variable expansion is not supported"
        ));
    }
    let requirement = pep_508::parse(line)
        .map_err(|error| format!("line {number}: {}", safe_pep508_diagnostic(&error)))?;
    let name = uv_normalize::PackageName::from_str(requirement.name)
        .map_err(|_| format!("line {number}: invalid dependency declaration"))?;
    if name.as_ref() == "fastmcp" {
        return Err(format!(
            "line {number}: the `fastmcp` dependency is managed by rustX \
                 (pinned to fastmcp=={MANAGED_FASTMCP_VERSION}); remove it"
        ));
    }
    Ok(line.to_owned())
}

/// The requirements-file layer rejects only pip's documented `${NAME}`
/// expansion directive. Other dollar signs belong to the PEP 508 parser: in
/// particular, `$TOKEN` is ordinary quoted-marker text rather than pip
/// expansion syntax.
fn contains_environment_variable_reference(line: &str) -> bool {
    line.match_indices("${").any(|(index, _)| {
        let remainder = &line[index + 2..];
        let Some(end) = remainder.find('}') else {
            return false;
        };
        let name = &remainder[..end];
        let mut characters = name.chars();
        matches!(characters.next(), Some(character) if character == '_' || character.is_ascii_alphabetic())
            && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    })
}

/// Converts parser-owned failure metadata into a short rustX-owned reason.
///
/// `pep-508` owns acceptance and grammar. Its `Simple` errors expose only a
/// byte span and whether parsing found a token, not grammar-production labels;
/// rustX selects the furthest `span().start`, breaking a same-position tie in
/// favor of a found token. This makes Chumsky's error-vector order irrelevant
/// while emitting only the parser-supported distinction between an unexpected
/// end and another syntax error. Never format the parser error: its display
/// form includes the authored token and span, which can disclose URL
/// credentials.
const MAX_REQUIREMENTS_DIAGNOSTIC_BYTE_OFFSET: usize = 16 * 1024 * 1024;
const MAX_REQUIREMENTS_PARSE_REASON_BYTES: usize =
    "unexpected end of dependency declaration near byte 16777216+".len();

fn safe_pep508_diagnostic(errors: &[chumsky::error::Simple<'_, char>]) -> String {
    let error = errors
        .iter()
        .max_by_key(|error| (error.span().start, error.found().is_some()))
        .expect("pep-508 parse failures include structured error metadata");
    let offset = error
        .span()
        .start
        .min(MAX_REQUIREMENTS_DIAGNOSTIC_BYTE_OFFSET);
    let offset = if error.span().start > MAX_REQUIREMENTS_DIAGNOSTIC_BYTE_OFFSET {
        format!("{offset}+")
    } else {
        offset.to_string()
    };
    let reason = if error.found().is_none() {
        format!("unexpected end of dependency declaration near byte {offset}")
    } else {
        format!("invalid dependency syntax near byte {offset}")
    };
    debug_assert!(reason.len() <= MAX_REQUIREMENTS_PARSE_REASON_BYTES);
    reason
}

/// The deterministic content digest of the package source bytes alone.
fn source_digest(
    root: &Path,
    files: &[PathBuf],
    cancellation: &ProcessSignal,
) -> Result<String, PythonToolError> {
    let directory = PackageDirectory::open(root).map_err(io_error)?;
    let mut hash = sha2::Sha256::new();
    hash.update(b"rustx:managed-python-source:v2\0");
    for path in files {
        let name = path.as_os_str().as_encoded_bytes();
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name);
        let evidence = stream(&directory, path, cancellation, |_| Ok(())).map_err(io_error)?;
        hash.update(evidence.metadata.len().to_be_bytes());
        hash.update(evidence.digest);
    }
    Ok(format!("sha256:{}", hex_digest(&hash.finalize())))
}

/// The full preparation fingerprint: the package identity (the synthesized
/// `python:<folder>` MCP server identity), the source bytes (including
/// `requirements.txt`), the probed interpreter and uv identities, the
/// managed `FastMCP` pin, and the platform. No timestamps, no host paths.
///
/// The package identity is a first-class input (Issue #174 invariant):
/// «different Python package identities always have different prepared
/// environment identities, even when every user-authored source byte is
/// identical». Two distinct folders must never collapse into one prepared
/// environment merely because their bytes happen to match — cross-package
/// environment deduplication is explicitly out of scope. The folder name
/// (not the absolute path) is the identity, so relocating the workspace
/// never changes a package's logical identity.
fn package_fingerprint(
    package: &PythonToolPackage,
    python_identity: &str,
    uv_identity: &str,
) -> String {
    // The managed pin is a constant, never a caller-selectable input: the
    // only production fingerprint is the one derived from
    // `MANAGED_FASTMCP_VERSION`.
    package_fingerprint_with(
        package,
        python_identity,
        uv_identity,
        MANAGED_FASTMCP_VERSION,
    )
}

/// The fingerprint derivation with the managed `FastMCP` pin as an explicit
/// argument.
///
/// The argument exists so the pin's materiality is provable — a changed
/// managed `FastMCP` build must always be a different prepared-state
/// identity — without making the production pin configurable: the only
/// non-test caller is [`package_fingerprint`], which always passes
/// [`MANAGED_FASTMCP_VERSION`].
fn package_fingerprint_with(
    package: &PythonToolPackage,
    python_identity: &str,
    uv_identity: &str,
    fastmcp_version: &str,
) -> String {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"rustx:managed-python-package:v2\0");
    append_bytes(
        &mut canonical,
        format!("{MANAGED_MCP_NAMESPACE}{}", package.name).as_bytes(),
    );
    append_bytes(&mut canonical, package.source_digest.as_bytes());
    append_bytes(&mut canonical, python_identity.as_bytes());
    append_bytes(&mut canonical, uv_identity.as_bytes());
    append_bytes(&mut canonical, fastmcp_version.as_bytes());
    append_bytes(&mut canonical, std::env::consts::OS.as_bytes());
    append_bytes(&mut canonical, std::env::consts::ARCH.as_bytes());
    format!("sha256:{}", hex_digest(&sha2::Sha256::digest(canonical)))
}

fn append_bytes(target: &mut Vec<u8>, bytes: &[u8]) {
    target.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    target.extend_from_slice(bytes);
}

fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

fn io_error(error: impl std::fmt::Display) -> PythonToolError {
    PythonToolError::Storage(error.to_string())
}

fn resolve_executable(name: &str) -> PathBuf {
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    // The child environment uses the fixed runtime-approved PATH; resolve
    // there so the probed identity and the `UV_PYTHON` pin always name the
    // exact same binary.
    for directory in ["/usr/local/bin", "/usr/bin", "/bin"] {
        let candidate = std::path::Path::new(directory).join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(name)
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn bounded_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).into_owned()
}

/// The rustX-owned frozen manifest of one prepared package state
/// (`manifest.json`): everything needed to explain the state and to prove
/// on reopen that it still is what it claims to be.
///
/// Authoritative identity fields — verified verbatim by
/// [`read_prepared_state`] against the live discovery and the probed
/// runtime identities: `format`, `fingerprint`, `package`, `entrypoint`,
/// `fastmcp`, `python`, `uv`, `os`, `arch`, `source_digest`. `origin` is
/// deliberately non-authoritative: it records which workspace folder the
/// frozen source was read from for diagnostics only, and reuse must not
/// depend on it (relocating the workspace must not invalidate an
/// otherwise-identical prepared state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedManifest {
    format: String,
    /// The full preparation fingerprint this state answers to.
    fingerprint: String,
    /// The package (folder) name of the preparing discovery.
    package: String,
    /// The workspace folder the frozen source was read from.
    origin: String,
    /// The fixed managed entrypoint (`server.py:mcp`).
    entrypoint: String,
    /// The rustX-pinned `FastMCP` build this environment contains.
    fastmcp: String,
    /// The probed Python interpreter identity (path + version).
    python: String,
    /// The probed uv identity (path + version).
    uv: String,
    os: String,
    arch: String,
    /// The content digest of the frozen `source/` copy.
    source_digest: String,
}

/// The shared in-flight build state of one fingerprint.
#[derive(Debug)]
struct BuildState {
    result: Mutex<Option<Result<PreparedPythonPackage, PythonToolError>>>,
    notify: tokio::sync::Notify,
}

impl BuildState {
    async fn wait(&self) -> Result<PreparedPythonPackage, PythonToolError> {
        // The no-lost-wakeup wait: the notified future is registered before
        // the result check, so a result published between the check and the
        // registration is observed by the next iteration.
        let mut notified = Box::pin(self.notify.notified());
        loop {
            if let Some(result) = self
                .result
                .lock()
                .expect("Python build result lock")
                .clone()
            {
                return result;
            }
            notified.as_mut().enable();
            if self
                .result
                .lock()
                .expect("Python build result lock")
                .is_some()
            {
                continue;
            }
            notified.await;
            notified = Box::pin(self.notify.notified());
        }
    }
}

/// Completes a process-local build entry if its store-owned task exits
/// unexpectedly. The guard lives inside that detached owner task, never in a
/// candidate preparation caller, so caller cancellation cannot release an
/// in-flight build while its physical materialization is running.
struct BuildOwnerGuard {
    in_flight: Arc<Mutex<BTreeMap<String, Arc<BuildState>>>>,
    key: String,
    state: Arc<BuildState>,
    completed: bool,
}

impl BuildOwnerGuard {
    fn finish(&mut self, result: Result<PreparedPythonPackage, PythonToolError>) {
        *self.state.result.lock().expect("Python build result lock") = Some(result);
        let mut in_flight = self.in_flight.lock().expect("Python build lock");
        if in_flight
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.state))
        {
            in_flight.remove(&self.key);
        }
        self.completed = true;
        drop(in_flight);
        self.state.notify.notify_waiters();
    }
}

impl Drop for BuildOwnerGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.finish(Err(PythonToolError::Unconfirmed(
            "the Python build owner exited before terminal publication".to_owned(),
        )));
    }
}

struct PythonToolStoreInner {
    root: PathBuf,
    pending_cleanup: Mutex<std::collections::BTreeSet<PathBuf>>,
    runner: Arc<dyn SupervisedProcessRunner>,
    uv_binary: PathBuf,
    python_binary: PathBuf,
    in_flight: Arc<Mutex<BTreeMap<String, Arc<BuildState>>>>,
}

/// The runtime-private prepared-environment store of managed Python tool
/// packages.
///
/// The store is a coordinator-lifetime-stable identity: its in-flight map
/// is the one process-local coordination domain that coalesces concurrent
/// preparations of the same fingerprint onto a single physical build.
#[derive(Clone)]
pub(crate) struct PythonToolStore {
    inner: Arc<PythonToolStoreInner>,
}

impl std::fmt::Debug for PythonToolStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PythonToolStore")
            .field("root", &self.inner.root)
            .finish()
    }
}

impl PythonToolStore {
    /// Creates the production store rooted at `root`
    /// (`<environment-store>/python-tools`).
    ///
    /// # Errors
    ///
    /// Returns [`PythonToolError::Storage`] if the store directories cannot
    /// be created.
    pub(super) fn new(root: PathBuf) -> Result<Self, PythonToolError> {
        Self::establish(&root)?;
        let root = std::fs::canonicalize(root).map_err(io_error)?;
        Ok(Self {
            inner: Arc::new(PythonToolStoreInner {
                root,
                pending_cleanup: Mutex::default(),
                runner: Arc::new(RunnerBackedProcessRunner),
                uv_binary: resolve_executable("uv"),
                python_binary: resolve_executable("python3"),
                in_flight: Arc::new(Mutex::new(BTreeMap::new())),
            }),
        })
    }

    fn establish(root: &Path) -> Result<(), PythonToolError> {
        std::fs::create_dir_all(root.join(PACKAGES_DIRECTORY)).map_err(io_error)?;
        std::fs::create_dir_all(root.join(UV_CACHE_DIRECTORY)).map_err(io_error)?;
        Ok(())
    }

    async fn settle_pending_cleanup(&self) -> Result<(), PythonToolError> {
        let cleanup_owner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut pending = cleanup_owner
                .pending_cleanup
                .lock()
                .expect("Python cleanup");
            let paths: Vec<_> = pending.iter().cloned().collect();
            for path in paths {
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => {
                        pending.remove(&path);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        pending.remove(&path);
                    }
                    Err(error) => {
                        return Err(PythonToolError::Unconfirmed(format!(
                            "cannot clean retained Python staging {}: {error}",
                            path.display()
                        )));
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(io_error)??;
        Ok(())
    }

    /// Ensures the prepared environment state of one frozen package:
    /// probes the runtime identities, computes the fingerprint, reuses a
    /// valid published state, or builds and publishes a new one.
    ///
    /// The server is deliberately **not** validate-launched here: the
    /// generic MCP connect + `tools/list` that immediately follows is the
    /// validation, and the candidate-generation machinery already
    /// guarantees a failure leaves the previously committed generation
    /// intact.
    ///
    /// # Errors
    ///
    /// Returns [`PythonToolError::Environment`] when the runtime identities
    /// cannot be probed or the uv build fails, and
    /// [`PythonToolError::Storage`] when existing state claims the
    /// fingerprint but fails verification (fail closed, never repaired).
    ///
    /// # Panics
    ///
    /// Panics if the process-internal build coordination lock is poisoned.
    pub(crate) async fn ensure_prepared(
        &self,
        package: &PythonToolPackage,
        cancellation: &ProcessSignal,
    ) -> Result<PreparedPythonPackage, PythonToolError> {
        self.settle_pending_cleanup().await?;
        let (python_identity, uv_identity) =
            probe_runtime_identity(&self.inner, cancellation).await?;
        let fingerprint = package_fingerprint(package, &python_identity, &uv_identity);
        let state_dir = self.inner.root.join(PACKAGES_DIRECTORY).join(&fingerprint);
        let key = fingerprint.clone();
        let (state, owner) = {
            let mut builds = self.inner.in_flight.lock().expect("Python build lock");
            if let Some(state) = builds.get(&key) {
                (state.clone(), false)
            } else {
                let state = Arc::new(BuildState {
                    result: Mutex::new(None),
                    notify: tokio::sync::Notify::new(),
                });
                builds.insert(key.clone(), state.clone());
                (state, true)
            }
        };
        if owner {
            let owner_guard = BuildOwnerGuard {
                in_flight: self.inner.in_flight.clone(),
                key,
                state: state.clone(),
                completed: false,
            };
            let inner = self.inner.clone();
            let package = package.clone();
            let build_fingerprint = fingerprint.clone();
            let build_dir = state_dir.clone();
            // The physical build observes the lifecycle cancellation
            // authority of the caller that established it: cancelling the
            // owning domain's authority cancels the uv units themselves.
            let build_cancellation = cancellation.clone();
            // Dropping a JoinHandle detaches the task; it does not abort it.
            // The caller therefore cannot become the physical materialization
            // owner merely by being cancelled while waiting below.
            std::mem::drop(tokio::spawn(async move {
                let mut owner_guard = owner_guard;
                let verification = (
                    build_dir.clone(),
                    package.clone(),
                    build_fingerprint.clone(),
                    python_identity.clone(),
                    uv_identity.clone(),
                    inner.python_binary.clone(),
                    build_cancellation.clone(),
                );
                let verified = tokio::task::spawn_blocking(move || {
                    let (directory, package, fingerprint, python, uv, interpreter, signal) =
                        verification;
                    read_prepared_state(
                        &directory,
                        &package,
                        &fingerprint,
                        &python,
                        &uv,
                        &interpreter,
                        &signal,
                    )
                })
                .await
                .map_err(io_error)
                .and_then(|outcome| outcome);
                let result = match verified {
                    Ok(Some(prepared)) => Ok(prepared),
                    Ok(None) => {
                        materialize_package(
                            &inner,
                            &package,
                            &build_dir,
                            &build_fingerprint,
                            &python_identity,
                            &uv_identity,
                            &build_cancellation,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                owner_guard.finish(result);
            }));
        }
        let result = state.wait().await?;
        check_cancelled(cancellation).map_err(io_error)?;
        Ok(result)
    }
}

/// One verified immutable environment. Launch continues through the regular stdio owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedPythonPackage {
    fingerprint: String,
    state_dir: PathBuf,
}

impl PreparedPythonPackage {
    pub(crate) fn launch(
        &self,
        cwd: PathBuf,
    ) -> rustx_process_supervisor::interactive_process::InteractiveProcessSpec {
        let server = self.state_dir.join(SOURCE_DIRECTORY).join(SERVER_FILE);
        rustx_process_supervisor::interactive_process::InteractiveProcessSpec {
            program: venv_python(&self.state_dir),
            args: vec![
                "-m".into(),
                "fastmcp.cli".into(),
                "run".into(),
                format!("{}:mcp", server.display()),
                "--skip-env".into(),
                "--no-banner".into(),
            ],
            cwd,
            environment: vec![
                ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
                ("FASTMCP_SHOW_SERVER_BANNER".into(), "false".into()),
                ("FASTMCP_CHECK_FOR_UPDATES".into(), "off".into()),
                ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            ],
        }
    }
}

/// The prepared virtualenv's interpreter: the launch executable of a
/// prepared package.
fn venv_python(state_dir: &Path) -> PathBuf {
    state_dir.join(VENV_DIRECTORY).join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    })
}

fn read_prepared_state(
    state_dir: &Path,
    package: &PythonToolPackage,
    fingerprint: &str,
    python_identity: &str,
    uv_identity: &str,
    interpreter: &Path,
    cancellation: &ProcessSignal,
) -> Result<Option<PreparedPythonPackage>, PythonToolError> {
    match state_dir.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(PythonToolError::Storage(
                "published Python state must be a directory".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    }
    let manifest_path = state_dir.join(MANIFEST_FILE);
    check_cancelled(cancellation).map_err(io_error)?;
    let directory = PackageDirectory::open(state_dir).map_err(io_error)?;
    let bytes = bounded_read(&directory, Path::new(MANIFEST_FILE), 1024 * 1024)?;
    let manifest: PreparedManifest = serde_json::from_slice(&bytes).map_err(|error| {
        PythonToolError::Storage(format!(
            "invalid prepared-state manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let valid = manifest.format == MANIFEST_FORMAT
        && manifest.fingerprint == fingerprint
        && manifest.package == package.name
        && manifest.entrypoint == ENTRYPOINT
        && manifest.fastmcp == MANAGED_FASTMCP_VERSION
        && manifest.python == python_identity
        && manifest.uv == uv_identity
        && manifest.os == std::env::consts::OS
        && manifest.arch == std::env::consts::ARCH;
    if !valid {
        return Err(PythonToolError::Storage(format!(
            "prepared state {} does not match its claimed identity inputs \
             (fingerprint={fingerprint}, package={:?}, entrypoint={ENTRYPOINT}, \
             fastmcp={MANAGED_FASTMCP_VERSION}, python={python_identity:?}, \
             uv={uv_identity:?})",
            state_dir.display(),
            package.name
        )));
    }
    let source_root = state_dir.join(SOURCE_DIRECTORY);
    let files = collect_files(&source_root, cancellation)?;
    if source_digest(&source_root, &files, cancellation)? != manifest.source_digest {
        return Err(PythonToolError::Storage(format!(
            "prepared state source {} does not hash back to its manifest",
            source_root.display()
        )));
    }
    // The reopened package must be the one whose fingerprint this state
    // answers to: same source digest.
    if package.source_digest != manifest.source_digest {
        return Err(PythonToolError::Storage(format!(
            "prepared state {} source digest does not match the discovered package",
            state_dir.display()
        )));
    }
    let program = venv_python(state_dir);
    if !program.is_file()
        || program.canonicalize().map_err(io_error)?
            != interpreter.canonicalize().map_err(io_error)?
    {
        return Err(PythonToolError::Storage(format!(
            "prepared state {} has no matching venv interpreter",
            state_dir.display()
        )));
    }
    // Permissions stay unset here: the fingerprint directory is env-only.
    // `ensure_prepared` freezes them from the current package afterward.
    Ok(Some(PreparedPythonPackage {
        fingerprint: fingerprint.to_owned(),
        state_dir: state_dir.to_path_buf(),
    }))
}

async fn probe_runtime_identity(
    inner: &PythonToolStoreInner,
    cancellation: &ProcessSignal,
) -> Result<(String, String), PythonToolError> {
    let child_environment = preparation_environment(&inner.root);
    let uv_command = format!("{} --version", shell_quote(&inner.uv_binary));
    let python_command = format!("{} --version", shell_quote(&inner.python_binary));
    let run = |command: String| {
        let runner = inner.runner.clone();
        let cwd = inner.root.clone();
        let environment = child_environment.clone();
        let cancellation = cancellation.clone();
        async move {
            runner
                .run(
                    SupervisedCommandSpec {
                        command: command.clone(),
                        cwd,
                        command_environment: environment,
                        timeout: Some(PYTHON_TOOL_PROBE_TIMEOUT),
                        // The caller's lifecycle authority, never a fresh
                        // detached signal: a settled preparation physically
                        // cancels the probe, and the result resolves only
                        // after the unit's settlement.
                        cancellation: cancellation.clone(),
                    },
                    None,
                )
                .await
                .map_err(PythonToolError::Unconfirmed)
        }
    };
    let uv = run(uv_command.clone()).await?;
    let python = run(python_command.clone()).await?;
    for (command, result) in [(uv_command, &uv), (python_command, &python)] {
        if !result.physical_settlement_proven {
            return Err(PythonToolError::Unconfirmed(
                "runtime identity probe did not settle".into(),
            ));
        }
        let failure = match &result.intent {
            ProcessOutcomeIntent::Completed if result.exit_code != Some(0) => Some(format!(
                "{command} exited with code {:?}: {}",
                result.exit_code,
                bounded_output(&result.stderr)
            )),
            ProcessOutcomeIntent::Completed => None,
            ProcessOutcomeIntent::Cancelled => Some(format!(
                "{command} was cancelled during the runtime identity probe"
            )),
            ProcessOutcomeIntent::TimedOut => Some(format!(
                "{command} timed out after {PYTHON_TOOL_PROBE_TIMEOUT:?}"
            )),
            ProcessOutcomeIntent::ProcessControlFailed(error) => {
                Some(format!("{command} failed: {error}"))
            }
        };
        if let Some(failure) = failure {
            return Err(PythonToolError::Environment(failure));
        }
    }
    // The identity pins path *and* version: a reinstalled interpreter of the
    // same version at a different path is a different runtime.
    let uv_version = bounded_output(&uv.stdout).trim().to_owned();
    let python_version = bounded_output(&python.stdout).trim().to_owned();
    if uv_version.is_empty() || python_version.is_empty() {
        return Err(PythonToolError::Environment(
            "runtime identity probe returned empty output".to_owned(),
        ));
    }
    let uv_identity = format!("{} ({uv_version})", inner.uv_binary.display());
    let python_identity = format!("{} ({python_version})", inner.python_binary.display());
    Ok((python_identity, uv_identity))
}

/// The rustX-generated `pyproject.toml` of one package: the project stub
/// that lets uv lock/sync, with the package's declared dependencies plus
/// the rustX-pinned `FastMCP` build.
fn generated_pyproject(package: &PythonToolPackage, python_identity: &str) -> String {
    use std::fmt::Write as _;
    // `python_identity` is `<path> (Python X.Y.Z)`; requires-python tracks
    // the probed interpreter's minor series, and the interpreter identity
    // is itself a fingerprint input, so a toolchain change re-prepares.
    let requires_python = probed_python_series(python_identity).map_or_else(
        || toml_basic_string(">=3.10"),
        |series| toml_basic_string(&format!("=={series}.*")),
    );
    let mut dependencies = String::new();
    for requirement in &package.requirements {
        let _ = writeln!(dependencies, "    {},", toml_basic_string(requirement));
    }
    let _ = writeln!(
        dependencies,
        "    {},",
        toml_basic_string(&format!("fastmcp=={MANAGED_FASTMCP_VERSION}"))
    );
    format!(
        "[project]\nname = {}\nversion = \"0.0.0\"\nrequires-python = {requires_python}\ndependencies = [\n{dependencies}]\n",
        toml_basic_string(&format!("rustx-python-tool-{}", package.name)),
    )
}

/// The `<major>.<minor>` series of the probed interpreter identity
/// (`<path> (Python X.Y.Z)`), when it parses.
fn probed_python_series(python_identity: &str) -> Option<String> {
    let version = python_identity
        .rsplit("(Python ")
        .next()?
        .trim_end_matches(')');
    let mut parts = version.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    if major.chars().all(|c| c.is_ascii_digit()) && minor.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("{major}.{minor}"))
    } else {
        None
    }
}

fn toml_basic_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Builds one package state in a sibling staging directory and publishes it
/// with one atomic rename.
async fn materialize_package(
    inner: &PythonToolStoreInner,
    package: &PythonToolPackage,
    state_dir: &Path,
    fingerprint: &str,
    python_identity: &str,
    uv_identity: &str,
    cancellation: &ProcessSignal,
) -> Result<PreparedPythonPackage, PythonToolError> {
    use std::os::unix::fs::PermissionsExt as _;
    let package_root = state_dir
        .parent()
        .ok_or_else(|| PythonToolError::Storage("prepared state has no parent".into()))?;
    let directory = PackageDirectory::open(package_root).map_err(io_error)?;
    let staging = tempfile::Builder::new()
        .prefix(".build-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(package_root)
        .map_err(io_error)?
        .keep();
    // The existing Python store owns any failed cleanup and retries it before
    // its next preparation. In-flight staging is not in that cleanup set.
    let result = match stage_and_build(
        inner,
        package,
        &staging,
        fingerprint,
        python_identity,
        uv_identity,
        cancellation,
    )
    .await
    {
        Err(error @ PythonToolError::Unconfirmed(_)) => {
            // The physical domain can still own writes to this stage.
            // Retain it and report the unresolved responsibility.
            return Err(error);
        }
        outcome => outcome,
    };
    let owner = inner.root.clone();
    let package = package.clone();
    let destination = state_dir.to_path_buf();
    let fingerprint = fingerprint.to_owned();
    let python_identity = python_identity.to_owned();
    let uv_identity = uv_identity.to_owned();
    let interpreter = inner.python_binary.clone();
    let cancellation = cancellation.clone();
    let (result, residue) = tokio::task::spawn_blocking(move || {
        let publication = (|| {
            let manifest = result?;
            check_cancelled(&cancellation).map_err(io_error)?;
            std::fs::write(staging.join(MANIFEST_FILE), serde_json::to_vec(&manifest).map_err(io_error)?).map_err(io_error)?;
            // Verify the complete staged package again after dependency setup,
            // before making the directory addressable as prepared state.
            read_prepared_state(&staging, &package, &fingerprint, &python_identity, &uv_identity, &interpreter, &cancellation)?
                .ok_or_else(|| PythonToolError::Storage("Python staging did not materialize".into()))?;
            let staging_name = staging.file_name().expect("exclusive staging name");
            let destination_name = destination.file_name().expect("fingerprint name");
            directory.verify().map_err(io_error)?;
            let rename = cancellation.with_active(|| {
                rustix::fs::renameat_with(
                    directory.descriptor(),
                    staging_name,
                    directory.descriptor(),
                    destination_name,
                    rustix::fs::RenameFlags::NOREPLACE,
                )
            });
            match rename.ok_or_else(|| PythonToolError::Environment("Python preparation was cancelled before publication".into()))? {
                // Env publication is code/dependency only; permissions are
                // attached by `ensure_prepared` from the winning package.
                Ok(()) => Ok(PreparedPythonPackage {
                    fingerprint: fingerprint.clone(),
                    state_dir: destination.clone(),
                }),
                Err(rustix::io::Errno::EXIST) => read_prepared_state(&destination, &package, &fingerprint, &python_identity, &uv_identity, &interpreter, &cancellation)?
                    .ok_or_else(|| PythonToolError::Storage("concurrent Python publication disappeared".into())),
                Err(error) => Err(io_error(error)),
            }
        })();
        let cleanup = match std::fs::remove_dir_all(&staging) {
            Ok(()) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(error),
        };
        match cleanup {
            None => (publication, None),
            Some(error) => (Err(PythonToolError::Unconfirmed(format!("Python staging cleanup failed at {} (store {}): {error}; preparation: {publication:?}", staging.display(), owner.display()))), Some(staging)),
        }
    }).await.map_err(io_error)?;
    if let Some(path) = residue {
        inner
            .pending_cleanup
            .lock()
            .expect("Python cleanup")
            .insert(path);
    }
    result
}

async fn stage_source(
    package: &PythonToolPackage,
    source_root: &Path,
    cancellation: &ProcessSignal,
) -> Result<(), PythonToolError> {
    // The frozen source copy: the server runs these snapshotted bytes,
    // never the live workspace files.
    let snapshot = package.clone();
    let copy_root = source_root.to_path_buf();
    let copy_cancellation = cancellation.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&copy_root).map_err(io_error)?;
        for relative in &snapshot.files {
            snapshot
                .source
                .copy_to(relative, &copy_root.join(relative), &copy_cancellation)?;
        }
        if source_digest(&copy_root, &snapshot.files, &copy_cancellation)? != snapshot.source_digest
        {
            return Err(PythonToolError::Storage(
                "captured Python source changed before installation".into(),
            ));
        }
        Ok(())
    })
    .await
    .map_err(io_error)??;
    Ok(())
}

async fn stage_and_build(
    inner: &PythonToolStoreInner,
    package: &PythonToolPackage,
    staging: &Path,
    fingerprint: &str,
    python_identity: &str,
    uv_identity: &str,
    cancellation: &ProcessSignal,
) -> Result<PreparedManifest, PythonToolError> {
    let source_root = staging.join(SOURCE_DIRECTORY);
    stage_source(package, &source_root, cancellation).await?;
    std::fs::write(
        staging.join("pyproject.toml"),
        generated_pyproject(package, python_identity),
    )
    .map_err(io_error)?;

    let child_environment = preparation_environment(&inner.root);
    let commands = [
        // rustX generates the lock: `--check` would require a package-owned
        // uv.lock, which the contract deliberately does not have.
        format!("{} lock --no-config", shell_quote(&inner.uv_binary)),
        format!(
            "{} sync --frozen --no-install-project --no-default-groups --no-config",
            shell_quote(&inner.uv_binary)
        ),
    ];
    for command in commands {
        let mut environment_entries = child_environment.clone();
        environment_entries.push((
            "UV_PROJECT_ENVIRONMENT".to_owned(),
            staging.join(VENV_DIRECTORY).display().to_string(),
        ));
        // The uv cache is store-owned scratch state: without this pin, a
        // cache lookup could resolve relative to the working directory and
        // write `.cache/uv` into the staging source.
        environment_entries.push((
            "UV_CACHE_DIR".to_owned(),
            inner.root.join(UV_CACHE_DIRECTORY).display().to_string(),
        ));
        // The exact interpreter selection: uv must materialize with the same
        // runtime whose identity entered the fingerprint. Project-local
        // interpreter selection and uv heuristics are never permitted to pick
        // another Python while retaining the probed identity.
        environment_entries.push((
            "UV_PYTHON".to_owned(),
            inner.python_binary.display().to_string(),
        ));
        environment_entries.push(("UV_NO_PYTHON_DOWNLOADS".to_owned(), "1".to_owned()));
        environment_entries.push(("UV_PYTHON_DOWNLOADS".to_owned(), "0".to_owned()));
        environment_entries.push(("UV_NO_PROGRESS".to_owned(), "1".to_owned()));
        let result = inner
            .runner
            .run(
                SupervisedCommandSpec {
                    command: command.clone(),
                    cwd: staging.to_path_buf(),
                    command_environment: environment_entries,
                    timeout: Some(PYTHON_TOOL_UV_TIMEOUT),
                    // The build-owner domain's lifecycle authority: its
                    // cancellation physically cancels this uv unit, and the
                    // result resolves only after the unit settled.
                    cancellation: cancellation.clone(),
                },
                None,
            )
            .await
            .map_err(PythonToolError::Unconfirmed)?;
        if !result.physical_settlement_proven {
            return Err(PythonToolError::Unconfirmed(
                "dependency preparation did not settle".into(),
            ));
        }
        let failure = match result.intent {
            ProcessOutcomeIntent::Completed if result.exit_code != Some(0) => Some(format!(
                "{command} exited with code {:?}: {}",
                result.exit_code,
                bounded_output(&result.stderr)
            )),
            ProcessOutcomeIntent::Completed => None,
            ProcessOutcomeIntent::Cancelled => {
                Some(format!("{command} was cancelled during materialization"))
            }
            ProcessOutcomeIntent::TimedOut => Some(format!(
                "{command} timed out after {PYTHON_TOOL_UV_TIMEOUT:?}"
            )),
            ProcessOutcomeIntent::ProcessControlFailed(error) => {
                Some(format!("{command} failed: {error}"))
            }
        };
        if let Some(failure) = failure {
            // uv is the dependency-resolution authority; rustX only has to
            // make the failure attributable. The diagnostic names the
            // managed source identity and the phase, and carries uv's own
            // bounded explanation — a package whose declared dependencies
            // cannot be resolved against the managed `FastMCP` pin fails
            // exactly this one source.
            return Err(PythonToolError::Environment(format!(
                "{} dependency preparation failed: {failure}",
                package.name
            )));
        }
    }
    Ok(PreparedManifest {
        format: MANIFEST_FORMAT.to_owned(),
        fingerprint: fingerprint.to_owned(),
        package: package.name.clone(),
        origin: package.root.display().to_string(),
        entrypoint: ENTRYPOINT.to_owned(),
        fastmcp: MANAGED_FASTMCP_VERSION.to_owned(),
        python: python_identity.to_owned(),
        uv: uv_identity.to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        source_digest: package.source_digest.clone(),
    })
}

fn bounded_read(
    directory: &PackageDirectory,
    relative: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, PythonToolError> {
    let file = directory.file(relative).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > max_bytes {
        return Err(PythonToolError::Storage(String::from(
            "Python metadata exceeds its bound",
        )));
    }
    Ok(bytes)
}

fn preparation_environment(home: &Path) -> Vec<(String, String)> {
    vec![
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
        ("HOME".to_owned(), home.display().to_string()),
        ("LANG".to_owned(), String::from("C.UTF-8")),
        ("LC_ALL".to_owned(), String::from("C.UTF-8")),
    ]
}

fn private_store_path(root: &Path, workspace: &Path) -> Result<PathBuf, PythonToolError> {
    use std::os::unix::fs::DirBuilderExt as _;
    let workspace = workspace.canonicalize().map_err(io_error)?;
    let scratch = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .map(|path| path.canonicalize())
        .transpose()
        .map_err(io_error)?;
    let mut ancestor = root;
    while let Err(error) = ancestor.symlink_metadata() {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(io_error(error));
        }
        ancestor = ancestor.parent().ok_or_else(|| io_error(error))?;
    }
    let resolved = ancestor
        .canonicalize()
        .map_err(io_error)?
        .join(root.strip_prefix(ancestor).map_err(io_error)?);
    let check_overlap = |location: &Path| {
        if location.starts_with(&workspace) || workspace.starts_with(location) {
            return Err(PythonToolError::Storage(
                "environment store must be disjoint from Workspace".into(),
            ));
        }
        if scratch
            .as_ref()
            .is_some_and(|path| location.starts_with(path) || path.starts_with(location))
        {
            return Err(PythonToolError::Storage(
                "environment store must be disjoint from temporary storage".into(),
            ));
        }
        Ok(())
    };
    check_overlap(&resolved)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&resolved)
        .map_err(io_error)?;
    let resolved = resolved.canonicalize().map_err(io_error)?;
    check_overlap(&resolved)?;
    let metadata = resolved.metadata().map_err(io_error)?;
    if !metadata.is_dir()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(PythonToolError::Storage(
            "environment store must be an owner-only directory".into(),
        ));
    }
    PackageDirectory::open(&resolved).map_err(io_error)?;
    Ok(resolved)
}

use std::fs::{File, Metadata};
use std::io::Seek as _;
use std::os::unix::fs::MetadataExt as _;
const CHUNK_BYTES: usize = 64 * 1024;
fn stamp(meta: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

fn check_cancelled(signal: &ProcessSignal) -> std::io::Result<()> {
    if signal.is_cancelled() {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "package I/O cancelled",
        ))
    } else {
        Ok(())
    }
}

/// Evidence over two complete matching reads. Metadata is additional mutation
/// evidence, never a substitute for content verification on a coarse clock.
#[derive(Debug)]
struct FileEvidence {
    metadata: Metadata,
    digest: [u8; 32],
}

/// Read the opened regular file, then verify the complete content again through
/// that descriptor. Callers consume chunks once; growth/truncation/replacement
/// and changed content fail closed. This does not claim an atomic snapshot of a
/// mutable directory. Publication verifies the actual copied package separately.
fn stream(
    directory: &PackageDirectory,
    relative: &Path,
    signal: &ProcessSignal,
    chunk: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<FileEvidence> {
    check_cancelled(signal)?;
    let mut file = directory.file(relative)?;
    let before = file.metadata()?;
    let mut buffer = vec![0; CHUNK_BYTES];
    let digest = read_pass(&mut file, before.len(), signal, &mut buffer, chunk)?;
    if stamp(&before) != stamp(&file.metadata()?)
        || stamp(&before) != stamp(&directory.file(relative)?.metadata()?)
    {
        return Err(std::io::Error::other(
            "package file changed during inspection",
        ));
    }
    file.rewind()?;
    let verified = read_pass(&mut file, before.len(), signal, &mut buffer, |_| Ok(()))?;
    if digest != verified
        || stamp(&before) != stamp(&file.metadata()?)
        || stamp(&before) != stamp(&directory.file(relative)?.metadata()?)
    {
        return Err(std::io::Error::other(
            "package file content changed during verification",
        ));
    }
    check_cancelled(signal)?;
    Ok(FileEvidence {
        metadata: before,
        digest,
    })
}

fn read_pass(
    file: &mut File,
    length: u64,
    signal: &ProcessSignal,
    buffer: &mut [u8],
    mut chunk: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<[u8; 32]> {
    let mut consumed = 0_u64;
    let mut hash = Sha256::new();
    loop {
        check_cancelled(signal)?;
        let read = file.read(buffer)?;
        if read == 0 {
            break;
        }
        consumed = consumed
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::other("package file length overflow"))?;
        if consumed > length {
            return Err(std::io::Error::other("package file grew during inspection"));
        }
        hash.update(&buffer[..read]);
        chunk(&buffer[..read])?;
    }
    if consumed != length {
        return Err(std::io::Error::other(
            "package file truncated during inspection",
        ));
    }
    Ok(hash.finalize().into())
}

#[cfg(test)]
mod store_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn source(root: &Path, name: &str) -> (PythonToolPackage, Option<Vec<u8>>) {
        discover_package(root, name, &ProcessSignal::new(), Arc::default()).unwrap()
    }

    fn authored(root: &Path) {
        std::fs::write(root.join(SERVER_FILE), b"from common import mcp\n").unwrap();
        std::fs::write(root.join(REQUIREMENTS_FILE), b"# no dependencies\n").unwrap();
        std::fs::write(root.join("common.py"), b"mcp = None\n").unwrap();
    }

    #[test]
    fn exact_store_identity_ignores_origin_and_permissions_but_keeps_runtime_and_source() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        authored(left.path());
        authored(right.path());
        std::fs::write(
            right.path().join(PERMISSIONS_FILE),
            b"# permission revision\n",
        )
        .unwrap();
        let (original, original_policy) = source(left.path(), "analytics");
        let (relocated, different_policy) = source(right.path(), "analytics");
        assert_ne!(original_policy, different_policy);
        let python = "/opt/python (Python 3.12.8)";
        let uv = "/opt/uv (uv 0.9.1)";
        let identity = package_fingerprint(&original, python, uv);
        assert_ne!(original.root, relocated.root);
        assert_eq!(identity, package_fingerprint(&relocated, python, uv));
        assert_ne!(
            identity,
            package_fingerprint(&source(right.path(), "reports").0, python, uv)
        );
        assert_ne!(
            identity,
            package_fingerprint(&original, "/new/python (Python 3.12.8)", uv)
        );
        assert_ne!(
            identity,
            package_fingerprint(&original, "/opt/python (Python 3.12.9)", uv)
        );
        assert_ne!(
            identity,
            package_fingerprint(&original, python, "/new/uv (uv 0.9.1)")
        );
        assert_ne!(
            identity,
            package_fingerprint(&original, python, "/opt/uv (uv 0.9.2)")
        );
        assert_ne!(
            identity,
            package_fingerprint_with(&original, python, uv, "4.0.4")
        );
        std::fs::rename(
            right.path().join("common.py"),
            right.path().join("extra.py"),
        )
        .unwrap();
        assert_ne!(
            identity,
            package_fingerprint(&source(right.path(), "analytics").0, python, uv)
        );
        std::fs::rename(
            right.path().join("extra.py"),
            right.path().join("common.py"),
        )
        .unwrap();
        std::fs::write(right.path().join(REQUIREMENTS_FILE), b"# different bytes\n").unwrap();
        assert_ne!(
            identity,
            package_fingerprint(&source(right.path(), "analytics").0, python, uv)
        );
        assert_eq!(identity, package_fingerprint(&original, python, uv));
    }

    #[test]
    fn a_dangling_fingerprint_entry_is_refused_without_attempting_repair() {
        let root = tempfile::tempdir().unwrap();
        authored(root.path());
        let (package, _) = source(root.path(), "analytics");
        let published = root.path().join("fingerprint");
        let interpreter = Path::new("/opt/python");
        let python = "/opt/python (Python 3.12.8)";
        let uv = "/opt/uv (uv 0.9.1)";
        let fingerprint = package_fingerprint(&package, python, uv);
        let inspect = || {
            read_prepared_state(
                &published,
                &package,
                &fingerprint,
                python,
                uv,
                interpreter,
                &ProcessSignal::new(),
            )
        };
        assert_eq!(inspect().unwrap(), None);
        let target = root.path().join("missing");
        std::os::unix::fs::symlink(&target, &published).unwrap();
        assert!(matches!(inspect(), Err(PythonToolError::Storage(_))));
        assert_eq!(published.read_link().unwrap(), target);
    }
}
