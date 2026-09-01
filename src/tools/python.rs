//! Managed Python tool packages (Issue #174).
//!
//! A Python tool package is a workspace folder that compiles into the
//! generic MCP runtime: it is **not** a second runtime protocol.
//!
//! ```text
//! <workspace>/.agents/tools/<package>/     one folder = one package
//! ├── server.py                            the `FastMCP` server (entrypoint
//! │                                        is fixed: server.py:mcp)
//! ├── requirements.txt                     REQUIRED, even when empty
//! └── *.py                                 supporting modules
//! ```
//!
//! rustX discovers each package, freezes its bytes into a fingerprint,
//! prepares an isolated uv environment in the runtime-private store, and
//! synthesizes one [`McpServerBinding`] per package. From there everything
//! — connect, `tools/list`, `tools/call`, epochs, availability, commit,
//! leases, subagent crossing — is the generic MCP machinery of
//! [`crate::tools::mcp`]; this module ends at the launch specification.
//!
//! # Store shape and ownership
//!
//! ```text
//! <store-root>/                                     (under the runtime's
//! ├── packages/<fingerprint>/                        environment store)
//! │   ├── source/                    # immutable frozen package source copy
//! │   ├── pyproject.toml             # rustX-generated project stub
//! │   ├── uv.lock                    # rustX-generated lock
//! │   ├── venv/                      # UV_PROJECT_ENVIRONMENT target
//! │   └── manifest.json              # rustX frozen manifest
//! └── uv-cache/                      # shared uv cache (scratch state)
//! ```
//!
//! The fingerprint covers exactly the material inputs: the package identity
//! (the synthesized `python:<folder>` MCP server identity), every package
//! source file (path and bytes, sorted — this includes `requirements.txt`),
//! the probed Python interpreter identity (path + version), the probed uv
//! identity (path + version), [`MANAGED_FASTMCP_VERSION`], and OS/arch. No
//! timestamps and no host paths: the package folder name is the logical
//! identity, so moving the whole workspace to another host path never
//! changes a package's prepared identity, while two distinct folders can
//! never share one prepared environment even when every user-authored byte
//! is identical. The same fingerprint reuses the prepared state after
//! validating the manifest and re-hashing the frozen source; a corrupt
//! state fails closed and is never mutated. A changed fingerprint prepares
//! a new state directory; the old one is left untouched (no GC exists).
//!
//! The managed `FastMCP` build is pinned to exactly one version
//! ([`MANAGED_FASTMCP_VERSION`]) and a package may never declare `fastmcp`
//! itself: the dependency identity of the MCP wire implementation is
//! rustX-owned, so a workspace edit cannot silently change the protocol
//! peer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::runtime::identity::McpServerId;
use crate::runtime::process_runner::{
    ProcessOutcomeIntent, RunnerBackedProcessRunner, SupervisedCommandSpec, SupervisedProcessRunner,
};
use crate::skills::environments::{ENVIRONMENT_COMMAND_TIMEOUT, RUNTIME_PROBE_TIMEOUT};
use crate::tools::environment::ToolEnvironment;
use crate::tools::mcp::{McpServerBinding, McpTransportConfig};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::workspace::Workspace;

/// The exact `FastMCP` build every managed Python package is prepared with.
///
/// Pinned exactly: the `FastMCP` SDK is rustX's MCP protocol peer, so its
/// identity is a fingerprint input, never a workspace-declared dependency.
/// Verified against the rustX MCP client (rmcp) before pinning: a real
/// `FastMCP` 3.4.7 stdio server negotiates the `2025-11-25` revision with the
/// legacy `initialize` handshake and serves `tools/list`/`tools/call`.
pub const MANAGED_FASTMCP_VERSION: &str = "3.4.7";

/// The fixed custom tool root below a Workspace.
pub const TOOLS_DIRECTORY: &str = ".agents";
/// The reserved `mcpServers` namespace owned by rustX-managed Python
/// packages (Issue #174).
///
/// Every synthesized package server identity is `python:<folder>`; a
/// configured `mcpServers` entry may never claim this namespace (rejected
/// at configuration validation), so one [`McpServerId`] can never have two
/// owners.
pub const MANAGED_MCP_NAMESPACE: &str = "python:";
/// The fixed package container below [`TOOLS_DIRECTORY`].
pub const TOOLS_ROOT: &str = "tools";
/// The fixed `FastMCP` server module of a package.
pub const SERVER_FILE: &str = "server.py";
/// The fixed dependency manifest of a package (required, even when empty).
pub const REQUIREMENTS_FILE: &str = "requirements.txt";

/// The fixed entrypoint every managed server is launched with.
const ENTRYPOINT: &str = "server.py:mcp";
const PACKAGES_DIRECTORY: &str = "packages";
const UV_CACHE_DIRECTORY: &str = "uv-cache";
const SOURCE_DIRECTORY: &str = "source";
const VENV_DIRECTORY: &str = "venv";
const MANIFEST_FILE: &str = "manifest.json";
const MANIFEST_FORMAT: &str = "rustx-managed-python-package-v1";
/// The finite deadline of one runtime identity probe (mirrors the M6
/// `RUNTIME_PROBE_TIMEOUT`).
const PYTHON_TOOL_PROBE_TIMEOUT: std::time::Duration = RUNTIME_PROBE_TIMEOUT;
/// The finite deadline of one uv lock/materialization command (mirrors the
/// M6 `ENVIRONMENT_COMMAND_TIMEOUT`).
const PYTHON_TOOL_UV_TIMEOUT: std::time::Duration = ENVIRONMENT_COMMAND_TIMEOUT;

/// A package discovery/preparation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PythonToolError {
    /// The package is malformed.
    InvalidPackage(String),
    /// Prepared state could not be read or published.
    Storage(String),
    /// The dependency environment could not be checked or materialized.
    Environment(String),
}

impl std::fmt::Display for PythonToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPackage(message) => {
                write!(formatter, "invalid Python tool package: {message}")
            }
            Self::Storage(message) => write!(formatter, "Python tool storage failed: {message}"),
            Self::Environment(message) => {
                write!(formatter, "Python tool environment failed: {message}")
            }
        }
    }
}

impl std::error::Error for PythonToolError {}

/// One discovered package: the frozen in-memory snapshot of every package
/// byte, already validated against the package contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonToolPackage {
    /// The package name: its folder name, validated identifier-safe.
    pub name: String,
    /// The workspace folder the snapshot was read from (diagnostics only;
    /// the server never runs from this live root).
    pub root: PathBuf,
    /// Every package file, sorted by package-relative path.
    pub files: Vec<(PathBuf, Vec<u8>)>,
    /// The normalized dependency lines of `requirements.txt` (comments and
    /// blank lines removed), in file order.
    pub requirements: Vec<String>,
}

/// The discovery outcome of one folder below `.agents/tools/`: either the
/// validated frozen package or its package-identifying diagnostic.
#[derive(Debug)]
pub struct DiscoveredPythonPackage {
    /// The synthesized MCP server identity of this folder
    /// (`python:<folder-name>`), present even when the package is invalid so
    /// the failure lands on the folder's own availability state.
    pub server_id: McpServerId,
    /// The validated package, or the diagnostic rejecting it.
    pub outcome: Result<PythonToolPackage, PythonToolError>,
}

/// The synthesized MCP server identity of one package folder.
#[must_use]
pub fn python_server_id(folder_name: &str) -> McpServerId {
    McpServerId::new(format!("{MANAGED_MCP_NAMESPACE}{folder_name}"))
}

/// Discovers every Python tool package of the Workspace, in deterministic
/// folder-name order.
///
/// Per-folder failures are returned in place (`DiscoveredPythonPackage`):
/// one malformed package never suppresses its siblings. Only walking the
/// container itself can fail the whole call (the same layering the Skill
/// discovery already has).
///
/// # Errors
///
/// Returns [`PythonToolError::Storage`] when `.agents/tools/` exists but
/// cannot be walked.
pub fn discover_python_packages(
    workspace: &Workspace,
) -> Result<Vec<DiscoveredPythonPackage>, PythonToolError> {
    let tools_root = workspace.root().join(TOOLS_DIRECTORY).join(TOOLS_ROOT);
    if !tools_root.exists() {
        return Ok(Vec::new());
    }
    let mut entries = std::fs::read_dir(&tools_root)
        .map_err(io_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error)?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut discovered = Vec::with_capacity(entries.len());
    for entry in entries {
        let folder_name = entry.file_name().to_string_lossy().into_owned();
        let server_id = python_server_id(&folder_name);
        discovered.push(DiscoveredPythonPackage {
            server_id,
            outcome: discover_package(&entry.path(), &folder_name),
        });
    }
    Ok(discovered)
}

fn discover_package(root: &Path, name: &str) -> Result<PythonToolPackage, PythonToolError> {
    let invalid = |message: String| {
        PythonToolError::InvalidPackage(format!("package {name:?} ({}): {message}", root.display()))
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
    let files = collect_files(root)?;
    let server_key = Path::new(SERVER_FILE);
    if !files.iter().any(|(path, _)| path == server_key) {
        return Err(invalid(format!(
            "the package has no {SERVER_FILE} (the managed FastMCP server)"
        )));
    }
    let requirements_key = Path::new(REQUIREMENTS_FILE);
    let Some((_, requirements_bytes)) = files.iter().find(|(path, _)| path == requirements_key)
    else {
        return Err(invalid(format!(
            "the package has no {REQUIREMENTS_FILE} (required, even when empty)"
        )));
    };
    let requirements = parse_requirements(requirements_bytes)
        .map_err(|message| invalid(format!("{REQUIREMENTS_FILE}: {message}")))?;
    Ok(PythonToolPackage {
        name: name.to_owned(),
        root: root.to_path_buf(),
        files,
        requirements,
    })
}

/// Reads one package tree into memory: sorted, deterministic, symlink-free.
fn collect_files(root: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, PythonToolError> {
    fn walk(
        root: &Path,
        current: &Path,
        output: &mut Vec<(PathBuf, Vec<u8>)>,
    ) -> Result<(), PythonToolError> {
        let mut entries = std::fs::read_dir(current)
            .map_err(io_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io_error)?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(io_error)?;
            if metadata.file_type().is_symlink() {
                return Err(PythonToolError::InvalidPackage(format!(
                    "package symlink is rejected: {}",
                    path.display()
                )));
            }
            if metadata.is_dir() {
                walk(root, &path, output)?;
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| {
                        PythonToolError::Storage("cannot relativize package file".to_owned())
                    })?
                    .to_path_buf();
                if relative.to_str().is_none() {
                    return Err(PythonToolError::InvalidPackage(
                        "package paths must be valid UTF-8".to_owned(),
                    ));
                }
                output.push((relative, std::fs::read(&path).map_err(io_error)?));
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    walk(root, root, &mut output)?;
    output.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(output)
}

fn validate_identifier(identifier: &str) -> Result<(), PythonToolError> {
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
fn parse_requirements(bytes: &[u8]) -> Result<Vec<String>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the file is not valid UTF-8".to_owned())?;
    let mut requirements = Vec::new();
    for (index, raw_line) in text.lines().enumerate() {
        // Strip end-of-line comments the way pip does (a `#` preceded by
        // whitespace or at line start).
        let line = strip_requirement_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('-') {
            return Err(format!(
                "line {}: option lines are not supported; declare PEP 508 dependencies only",
                index + 1
            ));
        }
        let name = requirement_name(line)
            .ok_or_else(|| format!("line {}: not a requirement line: {line:?}", index + 1))?;
        if name.eq_ignore_ascii_case("fastmcp") {
            return Err(format!(
                "line {}: the `fastmcp` dependency is managed by rustX \
                 (pinned to fastmcp=={MANAGED_FASTMCP_VERSION}); remove it",
                index + 1
            ));
        }
        requirements.push(line.to_owned());
    }
    Ok(requirements)
}

fn strip_requirement_comment(line: &str) -> &str {
    let mut previous: Option<char> = None;
    for (index, character) in line.char_indices() {
        if character == '#' && (index == 0 || previous.is_some_and(char::is_whitespace)) {
            return &line[..index];
        }
        previous = Some(character);
    }
    line
}

/// The requirement name of one PEP 508 line: everything before the extras,
/// version specifier, direct reference, or environment marker.
fn requirement_name(line: &str) -> Option<&str> {
    let end = line
        .find(|character: char| {
            matches!(character, '[' | '=' | '<' | '>' | '!' | '~' | ';' | '@')
                || character.is_whitespace()
        })
        .unwrap_or(line.len());
    let name = &line[..end];
    if name.is_empty()
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        return None;
    }
    Some(name)
}

/// The deterministic content digest of the package source bytes alone.
fn source_digest(files: &[(PathBuf, Vec<u8>)]) -> String {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"rustx:managed-python-source:v1\0");
    for (path, bytes) in files {
        append_bytes(&mut canonical, path.to_string_lossy().as_bytes());
        append_bytes(&mut canonical, bytes);
    }
    format!("sha256:{}", hex_digest(&sha2::Sha256::digest(canonical)))
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
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"rustx:managed-python-package:v2\0");
    append_bytes(
        &mut canonical,
        python_server_id(&package.name).as_str().as_bytes(),
    );
    append_bytes(&mut canonical, source_digest(&package.files).as_bytes());
    append_bytes(&mut canonical, python_identity.as_bytes());
    append_bytes(&mut canonical, uv_identity.as_bytes());
    append_bytes(&mut canonical, MANAGED_FASTMCP_VERSION.as_bytes());
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
        self.finish(Err(PythonToolError::Environment(
            "the Python build owner exited before terminal publication".to_owned(),
        )));
    }
}

#[derive(Clone)]
struct PythonToolStoreInner {
    root: PathBuf,
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
pub struct PythonToolStore {
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
    pub fn new(root: PathBuf) -> Result<Self, PythonToolError> {
        Self::establish(&root)?;
        Ok(Self {
            inner: Arc::new(PythonToolStoreInner {
                root,
                runner: Arc::new(RunnerBackedProcessRunner::default()),
                uv_binary: resolve_executable("uv"),
                python_binary: resolve_executable("python3"),
                in_flight: Arc::new(Mutex::new(BTreeMap::new())),
            }),
        })
    }

    /// Test constructor for deterministic recorded process backends.
    #[cfg(test)]
    pub(crate) fn with_binaries_and_runner(
        root: PathBuf,
        uv_binary: PathBuf,
        python_binary: PathBuf,
        runner: Arc<dyn SupervisedProcessRunner>,
    ) -> Result<Self, PythonToolError> {
        Self::establish(&root)?;
        Ok(Self {
            inner: Arc::new(PythonToolStoreInner {
                root,
                runner,
                uv_binary,
                python_binary,
                in_flight: Arc::new(Mutex::new(BTreeMap::new())),
            }),
        })
    }

    fn establish(root: &Path) -> Result<(), PythonToolError> {
        std::fs::create_dir_all(root.join(PACKAGES_DIRECTORY)).map_err(io_error)?;
        std::fs::create_dir_all(root.join(UV_CACHE_DIRECTORY)).map_err(io_error)?;
        Ok(())
    }

    /// The root of this store.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Test-only observation of the store identity: distinguishes a reused
    /// store from a re-initialized one.
    #[cfg(test)]
    pub(crate) fn identity_token(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
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
    pub async fn ensure_prepared(
        &self,
        package: &PythonToolPackage,
        cancellation: &crate::runtime::CancellationSignal,
    ) -> Result<PreparedPythonPackage, PythonToolError> {
        let (python_identity, uv_identity) =
            probe_runtime_identity(&self.inner, cancellation).await?;
        let fingerprint = package_fingerprint(package, &python_identity, &uv_identity);
        let state_dir = self.inner.root.join(PACKAGES_DIRECTORY).join(&fingerprint);
        if let Some(prepared) = read_prepared_state(
            &state_dir,
            package,
            &fingerprint,
            &python_identity,
            &uv_identity,
        )? {
            return Ok(prepared);
        }
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
                let result = materialize_package(
                    &inner,
                    &package,
                    &build_dir,
                    &build_fingerprint,
                    &python_identity,
                    &uv_identity,
                    &build_cancellation,
                )
                .await;
                owner_guard.finish(result);
            }));
        }
        // The no-lost-wakeup wait: the notified future is registered before
        // the result check, so a result published between the check and the
        // registration is observed by the next iteration.
        let mut notified = Box::pin(state.notify.notified());
        loop {
            if let Some(result) = state
                .result
                .lock()
                .expect("Python build result lock")
                .clone()
            {
                return result;
            }
            notified.as_mut().enable();
            if state
                .result
                .lock()
                .expect("Python build result lock")
                .is_some()
            {
                continue;
            }
            notified.await;
            notified = Box::pin(state.notify.notified());
        }
    }
}

/// One prepared managed package: the fingerprint-keyed immutable state the
/// synthesized MCP binding launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPythonPackage {
    /// The preparation fingerprint of this state.
    pub fingerprint: String,
    /// The state directory (`packages/<fingerprint>/`).
    pub state_dir: PathBuf,
}

impl PreparedPythonPackage {
    /// The synthesized MCP server binding of this prepared package: a stdio
    /// launch of the prepared virtualenv's interpreter running the `FastMCP`
    /// CLI module against the frozen source copy, with the default
    /// unconfigured-server invocation policy.
    ///
    /// The launch never re-resolves dependencies: it names the prepared
    /// venv's interpreter directly (never `uv run`), and `--skip-env` pins
    /// that decision inside the CLI. It runs the interpreter with
    /// `-m fastmcp.cli` rather than the venv's `fastmcp` console script
    /// because uv writes the script's shebang with the build-time (staging)
    /// path, which the atomic publication rename invalidates; the
    /// interpreter itself is a symlink to the probed base runtime and stays
    /// valid. `cwd` stays `None` (the workspace root, per the generic stdio
    /// launch rules); the `fastmcp run <file>:<object>` resolution puts the
    /// frozen source directory on `sys.path` itself, so sibling package
    /// modules import regardless of cwd. The environment keeps stdout clean
    /// for the MCP wire (banner and update check silenced, bytecode caches
    /// disabled).
    #[must_use]
    pub fn server_binding(&self) -> McpServerBinding {
        let program = venv_python(&self.state_dir);
        let server = self.state_dir.join(SOURCE_DIRECTORY).join(SERVER_FILE);
        let mut environment = BTreeMap::new();
        environment.insert("FASTMCP_SHOW_SERVER_BANNER".to_owned(), "false".to_owned());
        environment.insert("FASTMCP_CHECK_FOR_UPDATES".to_owned(), "off".to_owned());
        environment.insert("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned());
        McpServerBinding {
            transport: McpTransportConfig::Stdio {
                program: program.display().to_string(),
                args: vec![
                    "-m".to_owned(),
                    "fastmcp.cli".to_owned(),
                    "run".to_owned(),
                    format!("{}:mcp", server.display()),
                    "--skip-env".to_owned(),
                    "--no-banner".to_owned(),
                ],
                cwd: None,
                environment,
            },
            policy: ToolInvocationPolicy::default(),
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

/// Validates a published state against the exact deterministic inputs that
/// derive it, and returns the handle. A state whose directory exists but
/// does not verify — missing/invalid manifest, mismatched identity inputs
/// (fingerprint, package identity, fixed entrypoint, managed `FastMCP` pin,
/// probed Python/uv identities, OS/arch), frozen source that no longer
/// hashes to the manifest, missing launch executable — is an explicit
/// preparation failure, never a silent reuse and never a rebuild.
///
/// Every identity-bearing manifest claim is verified: a frozen manifest
/// must not record semantic claims that reuse does not check. The
/// `origin` field is the one deliberately non-authoritative record (host
/// provenance for diagnostics; reuse must not depend on it).
fn read_prepared_state(
    state_dir: &Path,
    package: &PythonToolPackage,
    fingerprint: &str,
    python_identity: &str,
    uv_identity: &str,
) -> Result<Option<PreparedPythonPackage>, PythonToolError> {
    if !state_dir.exists() {
        return Ok(None);
    }
    let manifest_path = state_dir.join(MANIFEST_FILE);
    let bytes = std::fs::read(&manifest_path).map_err(io_error)?;
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
    let files = collect_files(&source_root)?;
    if source_digest(&files) != manifest.source_digest {
        return Err(PythonToolError::Storage(format!(
            "prepared state source {} does not hash back to its manifest",
            source_root.display()
        )));
    }
    // The reopened package must be the one whose fingerprint this state
    // answers to: same source digest.
    if source_digest(&package.files) != manifest.source_digest {
        return Err(PythonToolError::Storage(format!(
            "prepared state {} source digest does not match the discovered package",
            state_dir.display()
        )));
    }
    let program = venv_python(state_dir);
    if !program.is_file() {
        return Err(PythonToolError::Storage(format!(
            "prepared state {} has no venv interpreter",
            state_dir.display()
        )));
    }
    Ok(Some(PreparedPythonPackage {
        fingerprint: fingerprint.to_owned(),
        state_dir: state_dir.to_path_buf(),
    }))
}

async fn probe_runtime_identity(
    inner: &PythonToolStoreInner,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<(String, String), PythonToolError> {
    let environment = ToolEnvironment::new();
    let child_environment = environment.child_environment(&inner.root);
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
                        environment,
                        timeout: Some(PYTHON_TOOL_PROBE_TIMEOUT),
                        // The caller's lifecycle authority, never a fresh
                        // detached signal: a settled preparation physically
                        // cancels the probe, and the result resolves only
                        // after the unit's settlement.
                        cancellation,
                    },
                    None,
                )
                .await
                .map_err(PythonToolError::Environment)
        }
    };
    let uv = run(uv_command.clone()).await?;
    let python = run(python_command.clone()).await?;
    for (command, result) in [(uv_command, &uv), (python_command, &python)] {
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
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<PreparedPythonPackage, PythonToolError> {
    let staging =
        state_dir.with_file_name(format!(".build-{}-{}", fingerprint, std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(io_error)?;
    }
    // On every failure below the staging directory is scratch: it is never
    // renamed into place, so a failed build can never publish a partial
    // state. A crashed process may leave the staging directory behind; it
    // is removed by the next build attempt of the same fingerprint.
    let result = stage_and_build(
        inner,
        package,
        &staging,
        fingerprint,
        python_identity,
        uv_identity,
        cancellation,
    )
    .await;
    let staging_result = match result {
        Ok(manifest) => {
            let manifest_bytes = serde_json::to_vec(&manifest).map_err(io_error)?;
            std::fs::write(staging.join(MANIFEST_FILE), manifest_bytes).map_err(io_error)?;
            match std::fs::rename(&staging, state_dir) {
                Ok(()) => Ok(()),
                Err(error) if state_dir.exists() => {
                    // A concurrent builder won the publish. The staging
                    // scratch is removed and the winner's state is validated
                    // by the caller-side reopen, exactly as if this process
                    // had found it published.
                    let _ = std::fs::remove_dir_all(&staging);
                    let _ = error;
                    Ok(())
                }
                Err(error) => Err(io_error(error)),
            }
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(error)
        }
    };
    staging_result?;
    // Publish through validation, never through trust: the handle returned
    // is the re-validated on-disk state, whoever built it.
    read_prepared_state(
        state_dir,
        package,
        fingerprint,
        python_identity,
        uv_identity,
    )?
    .ok_or_else(|| {
        PythonToolError::Storage(format!(
            "prepared state {} did not materialize",
            state_dir.display()
        ))
    })
}

async fn stage_and_build(
    inner: &PythonToolStoreInner,
    package: &PythonToolPackage,
    staging: &Path,
    fingerprint: &str,
    python_identity: &str,
    uv_identity: &str,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<PreparedManifest, PythonToolError> {
    let source_root = staging.join(SOURCE_DIRECTORY);
    std::fs::create_dir_all(&source_root).map_err(io_error)?;
    // The frozen source copy: the server runs these snapshotted bytes,
    // never the live workspace files.
    for (relative, bytes) in &package.files {
        let destination = source_root.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        std::fs::write(&destination, bytes).map_err(io_error)?;
    }
    std::fs::write(
        staging.join("pyproject.toml"),
        generated_pyproject(package, python_identity),
    )
    .map_err(io_error)?;

    let environment = ToolEnvironment::new();
    let child_environment = environment.child_environment(&inner.root);
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
                    environment: environment_entries,
                    timeout: Some(PYTHON_TOOL_UV_TIMEOUT),
                    // The build-owner domain's lifecycle authority: its
                    // cancellation physically cancels this uv unit, and the
                    // result resolves only after the unit settled.
                    cancellation: cancellation.clone(),
                },
                None,
            )
            .await
            .map_err(PythonToolError::Environment)?;
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
            return Err(PythonToolError::Environment(failure));
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
        source_digest: source_digest(&package.files),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::CancellationSignal;
    use crate::runtime::process_runner::CapturedProcessResult;

    type PackageFiles = (&'static str, &'static [u8]);

    fn workspace_with(packages: &[(&str, &[PackageFiles])]) -> (tempfile::TempDir, Workspace) {
        let directory = tempfile::tempdir().expect("workspace");
        for (name, files) in packages {
            let root = directory.path().join(".agents/tools").join(name);
            std::fs::create_dir_all(&root).expect("package directory");
            for (relative, bytes) in *files {
                std::fs::write(root.join(relative), bytes).expect("package file");
            }
        }
        let workspace = Workspace::new(directory.path()).expect("workspace");
        (directory, workspace)
    }

    fn valid_package_files() -> Vec<PackageFiles> {
        vec![
            (
                SERVER_FILE,
                b"from fastmcp import FastMCP\nmcp = FastMCP('demo')\n",
            ),
            (REQUIREMENTS_FILE, b"# none\n"),
        ]
    }

    fn package(name: &str) -> PythonToolPackage {
        let (_directory, workspace) = workspace_with(&[(name, &valid_package_files())]);
        let discovered = discover_python_packages(&workspace).expect("discover");
        discovered
            .into_iter()
            .next()
            .expect("one package")
            .outcome
            .expect("valid package")
    }

    #[test]
    fn discovery_rejects_a_folder_without_server_py_in_place() {
        let (_directory, workspace) =
            workspace_with(&[("demo", &[(REQUIREMENTS_FILE, b"# none\n".as_slice())])]);
        let discovered = discover_python_packages(&workspace).expect("discover");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].server_id, python_server_id("demo"));
        let Err(PythonToolError::InvalidPackage(message)) = &discovered[0].outcome else {
            panic!(
                "missing server.py rejects the package: {:?}",
                discovered[0].outcome
            );
        };
        assert!(
            message.contains("\"demo\""),
            "the diagnostic names the package: {message}"
        );
        assert!(
            message.contains(SERVER_FILE),
            "the diagnostic names the contract: {message}"
        );
    }

    #[test]
    fn discovery_rejects_a_folder_without_requirements_txt_in_place() {
        let (_directory, workspace) =
            workspace_with(&[("demo", &[(SERVER_FILE, b"mcp = None\n".as_slice())])]);
        let discovered = discover_python_packages(&workspace).expect("discover");
        let Err(PythonToolError::InvalidPackage(message)) = &discovered[0].outcome else {
            panic!("missing requirements.txt rejects the package");
        };
        assert!(
            message.contains("\"demo\""),
            "the diagnostic names the package: {message}"
        );
        assert!(
            message.contains(REQUIREMENTS_FILE),
            "the diagnostic names the contract: {message}"
        );
    }

    #[test]
    fn discovery_rejects_invalid_folder_names() {
        for name in ["Bad", "under_score", "-lead", "trail-", "double--dash"] {
            let (_directory, workspace) = workspace_with(&[(name, &valid_package_files())]);
            let discovered = discover_python_packages(&workspace).expect("discover");
            assert!(
                matches!(
                    &discovered[0].outcome,
                    Err(PythonToolError::InvalidPackage(message))
                        if message.contains("invalid package name")
                ),
                "{name:?} is rejected: {:?}",
                discovered[0].outcome
            );
        }
    }

    #[test]
    fn one_malformed_package_never_suppresses_its_siblings() {
        let (_directory, workspace) = workspace_with(&[
            ("alpha", &valid_package_files()),
            ("broken", &[(SERVER_FILE, b"mcp = None\n".as_slice())]),
            ("zeta", &valid_package_files()),
        ]);
        let discovered = discover_python_packages(&workspace).expect("discover");
        assert_eq!(
            discovered
                .iter()
                .map(|entry| entry.server_id.as_str().to_owned())
                .collect::<Vec<_>>(),
            ["python:alpha", "python:broken", "python:zeta"]
        );
        assert!(discovered[0].outcome.is_ok());
        assert!(discovered[1].outcome.is_err());
        assert!(discovered[2].outcome.is_ok());
    }

    #[test]
    fn requirements_parse_normalizes_comments_and_blank_lines() {
        let parsed = parse_requirements(
            b"# heading\n\nsix==1.16.0  # pinned\nrequests[socks]>=2 ; python_version >= '3.10'\n",
        )
        .expect("valid requirements");
        assert_eq!(
            parsed,
            [
                "six==1.16.0",
                "requests[socks]>=2 ; python_version >= '3.10'"
            ]
        );
    }

    #[test]
    fn requirements_reject_the_managed_fastmcp_dependency() {
        for line in ["fastmcp", "fastmcp==3.4.7", "FastMCP[cli]>=2"] {
            let error = parse_requirements(line.as_bytes()).expect_err("fastmcp is managed");
            assert!(
                error.contains(MANAGED_FASTMCP_VERSION),
                "the diagnostic names the managed pin: {error}"
            );
        }
    }

    #[test]
    fn requirements_reject_option_lines_and_unparseable_lines() {
        let error = parse_requirements(b"-r other.txt\n").expect_err("option line");
        assert!(
            error.contains("line 1"),
            "the diagnostic locates the line: {error}"
        );
        assert!(error.contains("option lines"), "{error}");
        let error =
            parse_requirements(b"\nsix==1.16.0\n=== garbage ===\n").expect_err("unparseable");
        assert!(
            error.contains("line 3"),
            "the diagnostic locates the line: {error}"
        );
        let error = parse_requirements(b"\xff\xfe").expect_err("not UTF-8");
        assert!(error.contains("UTF-8"), "{error}");
    }

    #[test]
    fn the_fingerprint_is_stable_and_tracks_every_material_input() {
        let package = package("demo");
        let python_identity = "/usr/bin/python3 (Python 3.12.13)";
        let uv_identity = "/usr/bin/uv (uv 0.11.12)";
        let fingerprint = package_fingerprint(&package, python_identity, uv_identity);
        assert!(fingerprint.starts_with("sha256:"));
        assert_eq!(
            fingerprint,
            package_fingerprint(&package, python_identity, uv_identity),
            "the fingerprint is deterministic"
        );

        // The package identity (the synthesized `python:<folder>` server
        // identity) is a first-class fingerprint input: renaming the folder
        // is a new environment identity even with byte-identical content.
        let mut renamed = package.clone();
        renamed.name = "renamed".to_owned();
        assert_ne!(
            fingerprint,
            package_fingerprint(&renamed, python_identity, uv_identity),
            "the package identity is a fingerprint input"
        );
        // Relocating the workspace on the host is not: the live path is
        // deliberately excluded so moving the workspace never changes a
        // package's logical identity.
        let mut relocated = package.clone();
        relocated.root = PathBuf::from("/elsewhere/workspace");
        assert_eq!(
            fingerprint,
            package_fingerprint(&relocated, python_identity, uv_identity),
            "the live host path is not a fingerprint input"
        );

        let mut changed_bytes = package.clone();
        changed_bytes.files[0].1.push(b'\n');
        assert_ne!(
            fingerprint,
            package_fingerprint(&changed_bytes, python_identity, uv_identity),
            "a source edit is a new fingerprint"
        );
        let mut changed_requirements = package.clone();
        changed_requirements.files[1] =
            (PathBuf::from(REQUIREMENTS_FILE), b"six==1.16.0\n".to_vec());
        assert_ne!(
            fingerprint,
            package_fingerprint(&changed_requirements, python_identity, uv_identity),
            "a requirements edit is a new fingerprint"
        );
        assert_ne!(
            fingerprint,
            package_fingerprint(&package, "/usr/bin/python3 (Python 3.13.0)", uv_identity),
            "the interpreter identity is a fingerprint input"
        );
        assert_ne!(
            fingerprint,
            package_fingerprint(&package, python_identity, "/usr/bin/uv (uv 0.12.0)"),
            "the uv identity is a fingerprint input"
        );
    }

    /// Issue #174 environment-identity invariant: two distinct package
    /// folders with byte-identical contents must still receive distinct
    /// fingerprints (and therefore distinct state directories and distinct
    /// environment identities). Cross-package environment deduplication is
    /// explicitly out of scope.
    #[test]
    fn byte_identical_folders_have_distinct_environment_identities() {
        let alpha = package("alpha");
        let beta = package("beta");
        assert_eq!(
            alpha.files, beta.files,
            "the fixture writes byte-identical package contents"
        );
        assert_eq!(
            alpha.requirements, beta.requirements,
            "byte-identical requirements"
        );
        assert_ne!(alpha.name, beta.name);
        let python_identity = "/usr/bin/python3 (Python 3.12.13)";
        let uv_identity = "/usr/bin/uv (uv 0.11.12)";
        let alpha_fingerprint = package_fingerprint(&alpha, python_identity, uv_identity);
        let beta_fingerprint = package_fingerprint(&beta, python_identity, uv_identity);
        assert_ne!(
            alpha_fingerprint, beta_fingerprint,
            "byte-identical folders never share one environment identity"
        );
    }

    #[test]
    fn the_generated_pyproject_pins_the_probed_series_and_the_managed_fastmcp() {
        let mut package = package("demo");
        package.requirements = vec!["six==1.16.0".to_owned()];
        let pyproject = generated_pyproject(&package, "/usr/bin/python3 (Python 3.12.13)");
        assert!(
            pyproject.contains("name = \"rustx-python-tool-demo\""),
            "{pyproject}"
        );
        assert!(
            pyproject.contains("requires-python = \"==3.12.*\""),
            "{pyproject}"
        );
        assert!(pyproject.contains("\"six==1.16.0\""), "{pyproject}");
        assert!(
            pyproject.contains(&format!("\"fastmcp=={MANAGED_FASTMCP_VERSION}\"")),
            "{pyproject}"
        );
        assert_eq!(
            probed_python_series("/opt/py (not a version)"),
            None,
            "an unparseable identity degrades to the fallback bound"
        );
    }

    #[test]
    fn the_server_binding_launches_the_prepared_venv_interpreter_directly() {
        let prepared = PreparedPythonPackage {
            fingerprint: "sha256:abc".to_owned(),
            state_dir: PathBuf::from("/state/sha256:abc"),
        };
        let binding = prepared.server_binding();
        let McpTransportConfig::Stdio {
            program,
            args,
            cwd,
            environment,
        } = &binding.transport
        else {
            panic!("the binding is a stdio launch");
        };
        if cfg!(windows) {
            assert_eq!(program, "/state/sha256:abc/venv/Scripts/python.exe");
        } else {
            assert_eq!(program, "/state/sha256:abc/venv/bin/python");
        }
        assert_eq!(
            args,
            &[
                "-m".to_owned(),
                "fastmcp.cli".to_owned(),
                "run".to_owned(),
                "/state/sha256:abc/source/server.py:mcp".to_owned(),
                "--skip-env".to_owned(),
                "--no-banner".to_owned(),
            ]
        );
        assert!(cwd.is_none());
        assert_eq!(
            environment
                .get("FASTMCP_SHOW_SERVER_BANNER")
                .map(String::as_str),
            Some("false")
        );
        assert_eq!(
            environment
                .get("FASTMCP_CHECK_FOR_UPDATES")
                .map(String::as_str),
            Some("off")
        );
        assert_eq!(
            environment
                .get("PYTHONDONTWRITEBYTECODE")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(binding.policy, ToolInvocationPolicy::default());
    }

    /// One fully valid on-disk prepared state of `package` under
    /// `packages/<fingerprint>` (with a stub `venv/bin/python`).
    fn seed_prepared_state(
        store_root: &Path,
        package: &PythonToolPackage,
        python_identity: &str,
        uv_identity: &str,
    ) -> (String, PathBuf) {
        let fingerprint = package_fingerprint(package, python_identity, uv_identity);
        let state_dir = store_root.join(PACKAGES_DIRECTORY).join(&fingerprint);
        let source_root = state_dir.join(SOURCE_DIRECTORY);
        std::fs::create_dir_all(&source_root).expect("source root");
        for (relative, bytes) in &package.files {
            std::fs::write(source_root.join(relative), bytes).expect("frozen source");
        }
        let program = venv_python(&state_dir);
        std::fs::create_dir_all(program.parent().expect("bin")).expect("venv bin");
        std::fs::write(&program, b"#!/bin/sh\n").expect("interpreter stub");
        let manifest = PreparedManifest {
            format: MANIFEST_FORMAT.to_owned(),
            fingerprint: fingerprint.clone(),
            package: package.name.clone(),
            origin: package.root.display().to_string(),
            entrypoint: ENTRYPOINT.to_owned(),
            fastmcp: MANAGED_FASTMCP_VERSION.to_owned(),
            python: python_identity.to_owned(),
            uv: uv_identity.to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            source_digest: source_digest(&package.files),
        };
        std::fs::write(
            state_dir.join(MANIFEST_FILE),
            serde_json::to_vec(&manifest).expect("manifest"),
        )
        .expect("manifest write");
        (fingerprint, state_dir)
    }

    #[test]
    fn a_published_state_reopens_only_when_every_claim_verifies() {
        let directory = tempfile::tempdir().expect("store");
        let package = package("demo");
        let (fingerprint, state_dir) = seed_prepared_state(directory.path(), &package, "py", "uv");

        let reopened = read_prepared_state(&state_dir, &package, &fingerprint, "py", "uv")
            .expect("valid state")
            .expect("state exists");
        assert_eq!(reopened.fingerprint, fingerprint);
        assert_eq!(reopened.state_dir, state_dir);

        // An unknown fingerprint directory is simply absent, not an error.
        assert_eq!(
            read_prepared_state(
                &directory
                    .path()
                    .join(PACKAGES_DIRECTORY)
                    .join("sha256:other"),
                &package,
                "sha256:other",
                "py",
                "uv",
            )
            .expect("absent state"),
            None
        );
    }

    #[test]
    fn a_tampered_published_state_fails_closed() {
        let directory = tempfile::tempdir().expect("store");
        let package = package("demo");
        let (fingerprint, state_dir) = seed_prepared_state(directory.path(), &package, "py", "uv");

        // A tampered manifest (a different claimed fingerprint).
        let manifest_path = state_dir.join(MANIFEST_FILE);
        let original = std::fs::read(&manifest_path).expect("manifest");
        let mut manifest: PreparedManifest =
            serde_json::from_slice(&original).expect("manifest parses");
        manifest.package = "impostor".to_owned();
        // The manifest claim check covers the fingerprint/format/platform;
        // a source-digest-consistent but fingerprint-foreign manifest fails.
        manifest.fingerprint = "sha256:foreign".to_owned();
        std::fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("manifest"),
        )
        .expect("tamper");
        let error = read_prepared_state(&state_dir, &package, &fingerprint, "py", "uv")
            .expect_err("a tampered manifest fails closed");
        assert!(
            matches!(&error, PythonToolError::Storage(message) if message.contains("does not match its claimed identity")),
            "{error}"
        );

        // Restored manifest, tampered frozen source.
        std::fs::write(&manifest_path, &original).expect("restore manifest");
        std::fs::write(
            state_dir.join(SOURCE_DIRECTORY).join(SERVER_FILE),
            b"mcp = 'tampered'\n",
        )
        .expect("tamper source");
        let error = read_prepared_state(&state_dir, &package, &fingerprint, "py", "uv")
            .expect_err("a tampered source fails closed");
        assert!(
            matches!(&error, PythonToolError::Storage(message) if message.contains("does not hash back")),
            "{error}"
        );

        // The tampered bytes are never mutated into looking valid.
        let persisted =
            std::fs::read(state_dir.join(SOURCE_DIRECTORY).join(SERVER_FILE)).expect("persisted");
        assert_eq!(persisted, b"mcp = 'tampered'\n");
    }

    /// Every identity-bearing manifest claim is verified on reopen: a frozen
    /// manifest must not record semantic claims that reuse does not check.
    /// A contradicting `package`, `entrypoint`, `python`, or `uv` claim fails
    /// closed even when the fingerprint itself matches.
    #[test]
    fn a_manifest_claiming_identity_inputs_that_reuse_does_not_verify_fails_closed() {
        let directory = tempfile::tempdir().expect("store");
        let package = package("demo");
        let (fingerprint, state_dir) = seed_prepared_state(directory.path(), &package, "py", "uv");
        let manifest_path = state_dir.join(MANIFEST_FILE);
        let original = std::fs::read(&manifest_path).expect("manifest");

        let tamper = |mutate: &dyn Fn(&mut PreparedManifest)| {
            let mut manifest: PreparedManifest =
                serde_json::from_slice(&original).expect("manifest parses");
            mutate(&mut manifest);
            std::fs::write(
                &manifest_path,
                serde_json::to_vec(&manifest).expect("manifest"),
            )
            .expect("tamper");
        };
        let expect_rejection = |label: &str| {
            let error = read_prepared_state(&state_dir, &package, &fingerprint, "py", "uv")
                .expect_err(label);
            assert!(
                matches!(&error, PythonToolError::Storage(message) if message.contains("does not match its claimed identity")),
                "{label}: {error}"
            );
        };

        tamper(&|manifest| manifest.package = "other-package".to_owned());
        expect_rejection("a foreign package claim fails closed");

        tamper(&|manifest| manifest.entrypoint = "tool.py:run".to_owned());
        expect_rejection("a foreign entrypoint claim fails closed");

        tamper(&|manifest| manifest.python = "other-python".to_owned());
        expect_rejection("a foreign Python identity claim fails closed");

        tamper(&|manifest| manifest.uv = "other-uv".to_owned());
        expect_rejection("a foreign uv identity claim fails closed");

        // The last tampered claim is never repaired into looking valid: the
        // on-disk manifest still carries the foreign uv claim.
        let persisted = std::fs::read_to_string(&manifest_path).expect("persisted manifest");
        assert!(
            persisted.contains("\"uv\":\"other-uv\""),
            "the manifest is never rewritten by validation: {persisted}"
        );
    }

    #[test]
    fn a_published_state_without_the_launch_executable_fails_closed() {
        let directory = tempfile::tempdir().expect("store");
        let package = package("demo");
        let (fingerprint, state_dir) = seed_prepared_state(directory.path(), &package, "py", "uv");
        std::fs::remove_file(venv_python(&state_dir)).expect("remove the interpreter");
        let error = read_prepared_state(&state_dir, &package, &fingerprint, "py", "uv")
            .expect_err("a missing venv interpreter fails closed");
        assert!(
            matches!(&error, PythonToolError::Storage(message) if message.contains("no venv interpreter")),
            "{error}"
        );
    }

    /// A recorded process backend: probes succeed with fixed versions, and
    /// the `uv sync` command materializes the stub `venv/bin/python` launch
    /// executable inside the staging directory it runs in.
    #[derive(Debug, Default)]
    struct FakeRunner {
        commands: Mutex<Vec<String>>,
    }

    impl SupervisedProcessRunner for FakeRunner {
        fn run(
            &self,
            spec: SupervisedCommandSpec,
            _control: Option<crate::runtime::process_runner::RunnerTestControl>,
        ) -> futures_util::future::BoxFuture<'_, Result<CapturedProcessResult, String>> {
            self.commands
                .lock()
                .expect("commands")
                .push(spec.command.clone());
            if spec.command.contains("sync") {
                let program = venv_python(&spec.cwd);
                std::fs::create_dir_all(program.parent().expect("bin")).expect("venv bin");
                std::fs::write(program, b"#!/bin/sh\n").expect("interpreter stub");
            }
            let stdout = if spec.command.contains("python3") {
                b"Python 3.12.13\n".to_vec()
            } else if spec.command.contains("uv") && spec.command.contains("--version") {
                b"uv 0.11.12\n".to_vec()
            } else {
                Vec::new()
            };
            Box::pin(async move {
                Ok(CapturedProcessResult {
                    exit_code: Some(0),
                    intent: ProcessOutcomeIntent::Completed,
                    stdout,
                    stderr: Vec::new(),
                })
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_preparations_coalesce_onto_one_physical_build() {
        let directory = tempfile::tempdir().expect("store");
        let runner = Arc::new(FakeRunner::default());
        let store = PythonToolStore::with_binaries_and_runner(
            directory.path().to_path_buf(),
            PathBuf::from("/usr/bin/uv"),
            PathBuf::from("/usr/bin/python3"),
            runner.clone(),
        )
        .expect("store");
        let package = package("demo");

        let cancellation = CancellationSignal::new();
        let (first, second) = tokio::join!(
            store.ensure_prepared(&package, &cancellation),
            store.ensure_prepared(&package, &cancellation),
        );
        let first = first.expect("first preparation");
        let second = second.expect("second preparation");
        assert_eq!(
            first, second,
            "both callers observe the same published state"
        );
        assert!(venv_python(&first.state_dir).is_file());
        assert!(first.state_dir.join(MANIFEST_FILE).is_file());
        assert!(first.state_dir.join("source").join(SERVER_FILE).is_file());

        let commands = runner.commands.lock().expect("commands").clone();
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.contains(" lock "))
                .count(),
            1,
            "concurrent builds of one fingerprint coalesce: {commands:?}"
        );

        // A later preparation of the unchanged package reuses the validated
        // state: no further uv lock runs.
        let reused = store
            .ensure_prepared(&package, &CancellationSignal::new())
            .await
            .expect("reuse");
        assert_eq!(reused, first);
        let commands = runner.commands.lock().expect("commands").clone();
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.contains(" lock "))
                .count(),
            1,
            "a valid published state is reused without rebuilding: {commands:?}"
        );

        // A changed package is a new fingerprint and a new build.
        let mut changed = package.clone();
        changed.files[0].1.push(b'\n');
        let rebuilt = store
            .ensure_prepared(&changed, &CancellationSignal::new())
            .await
            .expect("rebuild");
        assert_ne!(rebuilt.fingerprint, first.fingerprint);
    }
}
