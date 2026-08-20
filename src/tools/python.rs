//! Immutable custom Python `ToolVersion`s and their canonical executor.
//!
//! A package is discovered from exactly one Workspace-local root. Discovery
//! reads every package-owned byte before capability commit, publishes that
//! finite snapshot into the private runtime store, and only then constructs
//! the executor. Workspace edits therefore cannot change an old foreground or
//! detached background call.
//!
//! # Store shape and ownership
//!
//! ```text
//! <store-root>/
//! ├── tool-versions/<ToolVersionId>/
//! │   ├── source/                     # canonical ToolVersion source (immutable)
//! │   │   ├── TOOL.toml
//! │   │   ├── input.schema.json
//! │   │   ├── pyproject.toml
//! │   │   ├── uv.lock
//! │   │   └── ...                     # may include package-owned input.json/harness.py
//! │   └── RUSTX_TOOL_VERSION.json     # format + claimed ToolVersionId
//! ├── python-tool-envs/<PythonToolEnvironmentDigest>/
//! │   └── RUSTX_ENV_MANIFEST.json     # exact deterministic input lock
//! ├── python-tool-bindings/<ToolVersionId>/<PythonToolEnvironmentDigest>.json
//! ├── uv-cache/                       # store-private uv cache (scratch state)
//! └── python-invocations/execution-N/ # one invocation-private execution bundle
//!     ├── source/                     # writable copy of the canonical source
//!     ├── harness.py                  # runtime-owned harness bytes
//!     └── input.json                  # runtime-owned invocation arguments
//! ```
//!
//! On reuse a published `ToolVersion` is validated by recomputing the
//! deterministic content digest over its `source/` files and comparing it
//! against the claimed identity — never by trusting a marker string alone.
//! A corrupt published `ToolVersion` fails preparation explicitly and is
//! never mutated. There is exactly one identity authority:
//! `ToolVersionId` is derived once from the canonical source bytes
//! (sorted package-relative paths, lengths, and raw bytes). The published
//! `source/` directory is the **immutable canonical authority**: no
//! execution ever uses it as a working directory. Each invocation claims a
//! unique execution bundle from the store's monotonic allocation domain —
//! the store is a coordinator-lifetime-stable identity, so two executor
//! generations can never collide — and materializes its own `source/`
//! copy, `harness.py`, and `input.json` inside it. The three ownership
//! domains never share a namespace: runtime-owned invocation files live
//! outside `source/`, so package-owned files named `input.json` or
//! `harness.py` are copied and preserved like any other canonical byte.
//! The Python process runs with the bundle's `source/` as module root and
//! working directory, and the bundle is removed when that invocation — and
//! only that invocation — settles; a bundle left behind by a crash is
//! stale scratch that is never reused or deleted (no GC exists). The
//! harness additionally disables bytecode caches so imports never write
//! `__pycache__` into any runtime-owned directory. Ordinary
//! tool writes — relative paths or `__file__` — therefore land in the
//! invocation-private `source/` copy and can never drift the canonical
//! bytes, so the persisted representation always validates identically
//! across executions and restarts. The environment marker records
//! every deterministic input that derives the environment identity
//! (format, OS, architecture, digest, lock digest, Python runtime
//! identity, uv identity); each `ToolVersion -> environment digest`
//! binding is recorded deterministically outside the environment's
//! immutable dependency identity, so a reusable environment never claims
//! one `ToolVersion` as complete GC reference metadata. No GC exists in
//! M7.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::runtime::identity::{PythonToolEnvironmentDigest, ToolVersionId};
use crate::runtime::process_runner::{
    CapturedProcessResult, ProcessOutcomeIntent, RunnerBackedProcessRunner, SupervisedCommandSpec,
    SupervisedProcessRunner,
};
use crate::skills::environments::{ENVIRONMENT_COMMAND_TIMEOUT, RUNTIME_PROBE_TIMEOUT};
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentOverlay};
use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::types::{
    ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionResult, ToolExecutionStatus,
    ToolInvocation, ToolInvocationPolicy, ToolResultContent,
};
use crate::tools::workspace::Workspace;

/// The fixed custom tool root below a Workspace.
pub const TOOLS_DIRECTORY: &str = ".agents";
pub const TOOLS_ROOT: &str = "tools";
pub const TOOL_MANIFEST_FILE: &str = "TOOL.toml";
pub const INPUT_SCHEMA_FILE: &str = "input.schema.json";
pub const PYPROJECT_FILE: &str = "pyproject.toml";
pub const UV_LOCK_FILE: &str = "uv.lock";
const TOOL_VERSION_MARKER: &str = "RUSTX_TOOL_VERSION.json";
const TOOL_SOURCE_DIRECTORY: &str = "source";
const ENVIRONMENT_MARKER: &str = "RUSTX_ENV_MANIFEST.json";
const ENVIRONMENT_MARKER_FORMAT: &str = "rustx-python-tool-environment-v1";
const TOOL_VERSION_MARKER_FORMAT: u32 = 1;
const BINDING_FORMAT: u32 = 1;
/// The finite deadline of one runtime identity probe (mirrors the M6
/// `RUNTIME_PROBE_TIMEOUT`).
const PYTHON_TOOL_PROBE_TIMEOUT: Duration = RUNTIME_PROBE_TIMEOUT;
/// The finite deadline of one uv lock/materialization command (mirrors the
/// M6 `ENVIRONMENT_COMMAND_TIMEOUT`).
const PYTHON_TOOL_UV_TIMEOUT: Duration = ENVIRONMENT_COMMAND_TIMEOUT;

const PYTHON_HARNESS: &str = r#"import contextlib
import importlib
import json
import pathlib
import sys

source_root = pathlib.Path(sys.argv[1]).resolve()
entrypoint = sys.argv[2]
input_path = pathlib.Path(sys.argv[3]).resolve()
sys.path.insert(0, str(source_root))

# The source root the harness receives is the invocation-private
# `source/` copy inside this invocation's own execution bundle, and it is
# also the process working directory: ordinary tool writes may mutate the
# private copy but can never reach the canonical published source. The
# harness itself is runtime-owned code materialized per invocation beside
# (never inside) `source/`. Bytecode caches stay disabled so imports never
# write `__pycache__` into the private copy or the immutable dependency
# environment.
sys.dont_write_bytecode = True

def emit(value):
    sys.__stdout__.write(json.dumps(value, separators=(",", ":"), ensure_ascii=False) + "\n")
    sys.__stdout__.flush()

try:
    module_name, function_name = entrypoint.split(":", 1)
    arguments = json.loads(input_path.read_text(encoding="utf-8"))
    with contextlib.redirect_stdout(sys.stderr):
        module = importlib.import_module(module_name)
        function = getattr(module, function_name)
        value = function(arguments)
    emit({"ok": True, "value": value})
except BaseException as error:
    emit({"ok": False, "error": f"{type(error).__name__}: {error}"})
"#;

/// A package discovery/publication failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PythonToolError {
    /// The package is malformed.
    InvalidPackage(String),
    /// A source snapshot could not be read or published.
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

/// The manifest accepted by M7.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolManifest {
    schema_version: u32,
    name: String,
    description: String,
    entrypoint: String,
    execution: String,
    concurrency: String,
}

/// An immutable finite package snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonToolPackage {
    /// Logical model-facing name.
    pub name: String,
    /// Model-facing description.
    pub description: String,
    /// Python module/function entrypoint.
    pub entrypoint: String,
    /// Canonical invocation policy.
    pub policy: ToolInvocationPolicy,
    /// Content identity of the complete package snapshot.
    pub tool_version_id: ToolVersionId,
    /// Sorted package-relative files and exact bytes.
    pub files: Vec<(PathBuf, Vec<u8>)>,
    /// Canonical model-call input schema from `input.schema.json`.
    pub input_schema: serde_json::Value,
    /// Dependency-relevant project and lock inputs.
    pyproject: Vec<u8>,
    uv_lock: Vec<u8>,
}

/// Discovers one-level packages under `<workspace>/.agents/tools/`.
#[derive(Debug, Clone)]
pub struct PythonToolDiscovery {
    workspace: Workspace,
}

impl PythonToolDiscovery {
    /// Creates Workspace-anchored discovery.
    #[must_use]
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            workspace: workspace.clone(),
        }
    }

    /// Reads and validates every candidate atomically as a complete result.
    ///
    /// # Errors
    ///
    /// Returns an error if the tools root, any package, or any package-owned
    /// file is malformed, symlinked, or unreadable.
    pub fn discover(&self) -> Result<Vec<PythonToolPackage>, PythonToolError> {
        let root = self.workspace.root().join(TOOLS_DIRECTORY).join(TOOLS_ROOT);
        if !root.exists() {
            return Ok(Vec::new());
        }
        if !root.is_dir() {
            return Err(PythonToolError::InvalidPackage(
                "the tools root is not a directory".to_owned(),
            ));
        }
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(&root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(io_error)?;
            if metadata.file_type().is_symlink() {
                return Err(PythonToolError::InvalidPackage(format!(
                    "symlinked tool package is rejected: {}",
                    entry.path().display()
                )));
            }
            if metadata.is_dir() {
                candidates.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                ));
            }
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        let mut packages = candidates
            .into_iter()
            .map(|(directory, root)| discover_package(&directory, &root))
            .collect::<Result<Vec<_>, _>>()?;
        packages.sort_by(|left, right| left.name.cmp(&right.name));
        let mut names = std::collections::BTreeSet::new();
        for package in &packages {
            if !names.insert(package.name.clone()) {
                return Err(PythonToolError::InvalidPackage(format!(
                    "duplicate Python model-facing name {:?}",
                    package.name
                )));
            }
        }
        Ok(packages)
    }
}

fn discover_package(directory: &str, root: &Path) -> Result<PythonToolPackage, PythonToolError> {
    validate_identifier(directory)?;
    let manifest_bytes = regular_file(root, TOOL_MANIFEST_FILE)?;
    let input_schema = regular_file(root, INPUT_SCHEMA_FILE)?;
    let pyproject = regular_file(root, PYPROJECT_FILE)?;
    let uv_lock = regular_file(root, UV_LOCK_FILE)?;
    let manifest: ToolManifest =
        toml::from_str(std::str::from_utf8(&manifest_bytes).map_err(|error| {
            PythonToolError::InvalidPackage(format!("TOOL.toml is not UTF-8: {error}"))
        })?)
        .map_err(|error| PythonToolError::InvalidPackage(format!("TOOL.toml: {error}")))?;
    if manifest.schema_version != 1 || manifest.name != directory {
        return Err(PythonToolError::InvalidPackage(
            "TOOL.toml schema_version/name does not match the package".to_owned(),
        ));
    }
    if manifest.description.trim().is_empty() {
        return Err(PythonToolError::InvalidPackage(
            "description must be non-empty".to_owned(),
        ));
    }
    validate_entrypoint(root, &manifest.entrypoint)?;
    let policy = ToolInvocationPolicy::new(
        parse_execution(&manifest.execution)?,
        parse_concurrency(&manifest.concurrency)?,
    );
    let schema: serde_json::Value = serde_json::from_slice(&input_schema)
        .map_err(|error| PythonToolError::InvalidPackage(format!("input.schema.json: {error}")))?;
    crate::tools::schema::validate_canonical_schema(&schema)
        .map_err(|error| PythonToolError::InvalidPackage(format!("input.schema.json: {error}")))?;
    let files = collect_files(root)?;
    let tool_version_id = tool_version_id(&files);
    Ok(PythonToolPackage {
        name: manifest.name,
        description: manifest.description,
        entrypoint: manifest.entrypoint,
        policy,
        tool_version_id,
        files,
        input_schema: schema,
        pyproject,
        uv_lock,
    })
}

fn regular_file(root: &Path, name: &str) -> Result<Vec<u8>, PythonToolError> {
    let path = root.join(name);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| PythonToolError::InvalidPackage(format!("missing {name}: {error}")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(PythonToolError::InvalidPackage(format!(
            "{name} must be a regular non-symlink file"
        )));
    }
    std::fs::read(&path).map_err(io_error)
}

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

fn validate_entrypoint(root: &Path, entrypoint: &str) -> Result<(), PythonToolError> {
    let Some((module, function)) = entrypoint.split_once(':') else {
        return Err(PythonToolError::InvalidPackage(
            "entrypoint must be module:function".to_owned(),
        ));
    };
    if module.is_empty()
        || function.is_empty()
        || !function
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
        || module.split('.').any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|character| character == '_' || character.is_ascii_alphanumeric())
        })
    {
        return Err(PythonToolError::InvalidPackage(
            "entrypoint contains an invalid Python identifier".to_owned(),
        ));
    }
    let module_path = root.join(module.replace('.', "/") + ".py");
    if !module_path.is_file() {
        return Err(PythonToolError::InvalidPackage(format!(
            "entrypoint module is not present: {module}"
        )));
    }
    Ok(())
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

fn parse_execution(value: &str) -> Result<ToolExecutionPolicy, PythonToolError> {
    match value {
        "foreground_only" => Ok(ToolExecutionPolicy::ForegroundOnly),
        "background_only" => Ok(ToolExecutionPolicy::BackgroundOnly),
        "model_selectable" => Ok(ToolExecutionPolicy::ModelSelectable),
        _ => Err(PythonToolError::InvalidPackage(format!(
            "invalid execution policy {value:?}"
        ))),
    }
}

fn parse_concurrency(value: &str) -> Result<ToolConcurrencyPolicy, PythonToolError> {
    match value {
        "sequential" => Ok(ToolConcurrencyPolicy::Sequential),
        "parallel" => Ok(ToolConcurrencyPolicy::Parallel),
        _ => Err(PythonToolError::InvalidPackage(format!(
            "invalid concurrency policy {value:?}"
        ))),
    }
}

fn tool_version_id(files: &[(PathBuf, Vec<u8>)]) -> ToolVersionId {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"rustx:python-tool-version:v1\0");
    for (path, bytes) in files {
        append_bytes(&mut canonical, path.to_string_lossy().as_bytes());
        append_bytes(&mut canonical, bytes);
    }
    ToolVersionId::new(format!(
        "sha256:{}",
        hex_digest(&sha2::Sha256::digest(canonical))
    ))
}

/// Generates the logical Python tool identity independently of its version.
#[must_use]
pub fn python_tool_id(name: &str) -> String {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"rustx:python-tool-id:v1\0");
    append_bytes(&mut canonical, name.as_bytes());
    format!(
        "python:sha256:{}",
        hex_digest(&sha2::Sha256::digest(canonical))
    )
}

/// Computes the dependency environment identity from only material inputs.
#[must_use]
pub fn python_tool_environment_digest(
    os: &str,
    architecture: &str,
    python_identity: &str,
    uv_identity: &str,
    pyproject: &[u8],
    uv_lock: &[u8],
) -> PythonToolEnvironmentDigest {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"rustx:python-tool-environment:v1\0");
    for value in [
        os.as_bytes(),
        architecture.as_bytes(),
        python_identity.as_bytes(),
        uv_identity.as_bytes(),
        pyproject,
        uv_lock,
    ] {
        append_bytes(&mut canonical, value);
    }
    PythonToolEnvironmentDigest::new(format!(
        "sha256:{}",
        hex_digest(&sha2::Sha256::digest(canonical))
    ))
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

/// A published immutable `ToolVersion` source snapshot.
#[derive(Debug, Clone)]
pub struct PublishedPythonTool {
    /// Original package metadata and identity.
    pub package: PythonToolPackage,
    /// Private immutable source root (`tool-versions/<id>/source/`): the
    /// canonical authority. Every uv preparation command uses it as its
    /// root; the executor reads it only to materialize each
    /// invocation-private execution copy — it is never an execution
    /// working directory.
    pub root: PathBuf,
}

/// A published immutable Python environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonToolEnvironment {
    /// Environment identity.
    pub digest: PythonToolEnvironmentDigest,
    /// Final immutable environment path.
    pub root: PathBuf,
}

/// The shared in-flight build state of one environment digest.
#[derive(Debug)]
struct BuildState {
    result: Mutex<Option<Result<PythonToolEnvironment, PythonToolError>>>,
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
    fn finish(&mut self, result: Result<PythonToolEnvironment, PythonToolError>) {
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
    next_invocation: Arc<AtomicU64>,
}

/// Runtime-private source/environment store for custom Python tools.
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
    /// Creates the production store below a runtime-private root.
    ///
    /// # Errors
    ///
    /// Returns an error if the store directories cannot be created.
    pub fn new(root: PathBuf) -> Result<Self, PythonToolError> {
        std::fs::create_dir_all(root.join("tool-versions")).map_err(io_error)?;
        std::fs::create_dir_all(root.join("python-tool-envs")).map_err(io_error)?;
        std::fs::create_dir_all(root.join("python-tool-bindings")).map_err(io_error)?;
        std::fs::create_dir_all(root.join("python-invocations")).map_err(io_error)?;
        Ok(Self {
            inner: Arc::new(PythonToolStoreInner {
                root,
                runner: Arc::new(RunnerBackedProcessRunner::default()),
                uv_binary: resolve_executable("uv"),
                python_binary: resolve_executable("python3"),
                in_flight: Arc::new(Mutex::new(BTreeMap::new())),
                next_invocation: Arc::new(AtomicU64::new(0)),
            }),
        })
    }

    /// Test constructor for deterministic recorded process backends.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn with_runner(
        root: PathBuf,
        runner: Arc<dyn SupervisedProcessRunner>,
    ) -> Result<Self, PythonToolError> {
        std::fs::create_dir_all(root.join("tool-versions")).map_err(io_error)?;
        std::fs::create_dir_all(root.join("python-tool-envs")).map_err(io_error)?;
        std::fs::create_dir_all(root.join("python-tool-bindings")).map_err(io_error)?;
        std::fs::create_dir_all(root.join("python-invocations")).map_err(io_error)?;
        Ok(Self {
            inner: Arc::new(PythonToolStoreInner {
                root,
                runner,
                uv_binary: resolve_executable("uv"),
                python_binary: resolve_executable("python3"),
                in_flight: Arc::new(Mutex::new(BTreeMap::new())),
                next_invocation: Arc::new(AtomicU64::new(0)),
            }),
        })
    }

    /// Test-only identity token of this store's process-local coordination
    /// domain: two handles over the same coordination identity return the
    /// same token.
    #[cfg(test)]
    pub(crate) fn identity_token(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    /// Allocates one unique, freshly claimed execution bundle directory for
    /// one invocation: `python-invocations/execution-N/`.
    ///
    /// The store is the single process-local allocation domain (Issue #81):
    /// one coordinator-lifetime-stable store hands out strictly monotonically
    /// increasing identifiers, so two executors from different capability
    /// generations can never claim the same bundle. The claim is the
    /// atomic `create_dir` itself: an already-existing path is **never
    /// deleted or reused** — it is stale scratch from a previous process
    /// lifetime (there is deliberately no scratch GC), so the allocator
    /// skips it. Identifier exhaustion is an **absorbing terminal state**
    /// for this store identity: the counter is never allowed to transition
    /// `MAX -> 0`, so no later invocation can wrap, reuse an identifier, or
    /// create a lower-numbered bundle — every later allocation attempt
    /// fails with the same explicit exhaustion error.
    ///
    /// The atomic counter only allocates unique, monotonically increasing
    /// names within this process-local store domain — `Ordering::Relaxed`
    /// is sufficient for that; the actual bundle-ownership claim is the
    /// filesystem `create_dir`.
    fn allocate_execution_bundle(&self) -> Result<PathBuf, PythonToolError> {
        let root = self.inner.root.join("python-invocations");
        let exhausted = || {
            PythonToolError::Storage(
                "the Python invocation identifier space is exhausted".to_owned(),
            )
        };
        loop {
            // Checked claim: at `u64::MAX` the counter stays at `MAX`
            // (absorbing) and allocation fails; it never wraps to 0.
            let number = self
                .inner
                .next_invocation
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    current.checked_add(1)
                })
                .map_err(|_| exhausted())?;
            let bundle = root.join(format!("execution-{number}"));
            match std::fs::create_dir(&bundle) {
                Ok(()) => return Ok(bundle),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Stale scratch from a previous process lifetime:
                    // unknown ownership, never destroyed — skip to the next
                    // monotonic identifier.
                }
                Err(error) => return Err(io_error(error)),
            }
        }
    }

    /// Publishes exact package bytes by content identity.
    ///
    /// The published shape is `tool-versions/<ToolVersionId>/source/...`
    /// plus the version marker; the executor's source root is exactly
    /// `.../source/`. On reuse the published `source/` content digest is
    /// recomputed and compared against the claimed identity, so a marker
    /// string alone never validates a `ToolVersion`; a corrupt publication
    /// fails preparation explicitly and is never mutated.
    ///
    /// # Errors
    ///
    /// Returns an error if the immutable snapshot cannot be staged, written,
    /// validated, or atomically installed.
    pub fn publish(
        &self,
        package: &PythonToolPackage,
    ) -> Result<PublishedPythonTool, PythonToolError> {
        static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);
        let destination = self
            .inner
            .root
            .join("tool-versions")
            .join(package.tool_version_id.as_str());
        let source_root = destination.join(TOOL_SOURCE_DIRECTORY);
        if destination.exists() {
            let marker = destination.join(TOOL_VERSION_MARKER);
            let marker_value = serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(&marker).map_err(io_error)?,
            )
            .map_err(|error| {
                PythonToolError::Storage(format!(
                    "published ToolVersion marker is invalid: {error}"
                ))
            })?;
            let valid = marker_value.get("format")
                == Some(&serde_json::json!(TOOL_VERSION_MARKER_FORMAT))
                && marker_value
                    .get("tool_version_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(package.tool_version_id.as_str());
            if !valid {
                return Err(PythonToolError::Storage(
                    "published ToolVersion marker is invalid".to_owned(),
                ));
            }
            let published_digest = published_source_digest(&source_root)?;
            if published_digest != package.tool_version_id {
                return Err(PythonToolError::Storage(format!(
                    "published ToolVersion source does not match its claimed identity: \
                     marker claims {}, published source digest is {}",
                    package.tool_version_id.as_str(),
                    published_digest.as_str(),
                )));
            }
            return Ok(PublishedPythonTool {
                package: package.clone(),
                root: source_root,
            });
        }
        let staging = self.inner.root.join(format!(
            ".tool-version-{}-{}-{}",
            package.tool_version_id.as_str(),
            std::process::id(),
            NEXT_STAGING.fetch_add(1, Ordering::Relaxed)
        ));
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(io_error)?;
        }
        let staging_source = staging.join(TOOL_SOURCE_DIRECTORY);
        std::fs::create_dir_all(&staging_source).map_err(io_error)?;
        for (relative, bytes) in &package.files {
            let path = staging_source.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io_error)?;
            }
            std::fs::write(path, bytes).map_err(io_error)?;
        }
        let marker = serde_json::json!({
            "format": TOOL_VERSION_MARKER_FORMAT,
            "tool_version_id": package.tool_version_id,
        });
        let marker_bytes = serde_json::to_vec(&marker).map_err(io_error)?;
        std::fs::write(staging.join(TOOL_VERSION_MARKER), marker_bytes).map_err(io_error)?;
        match std::fs::rename(&staging, &destination) {
            Ok(()) => {}
            Err(_error) if destination.exists() => {
                std::fs::remove_dir_all(&staging).map_err(io_error)?;
                if !destination.join(TOOL_VERSION_MARKER).is_file() {
                    return Err(PythonToolError::Storage(
                        "concurrent ToolVersion publication produced an invalid marker".to_owned(),
                    ));
                }
            }
            Err(error) => return Err(io_error(error)),
        }
        Ok(PublishedPythonTool {
            package: package.clone(),
            root: source_root,
        })
    }

    /// Ensures one immutable environment, coalescing concurrent same-digest
    /// builds behind one store-owned task.
    ///
    /// Exactly one store-owned logical build owns a digest until terminal
    /// publication. Candidate callers only wait for the result; dropping a
    /// waiter never releases ownership, and owner failure always publishes a
    /// terminal error and removes the in-flight entry.
    ///
    /// # Errors
    ///
    /// Returns an error if runtime probes, lock validation, frozen
    /// materialization, validation, or publication fails.
    ///
    /// # Panics
    ///
    /// Panics only if the store's internal synchronization lock is poisoned.
    #[allow(clippy::too_many_lines)] // one coherent probe/coalesce/wait/publish pipeline
    pub async fn ensure_environment(
        &self,
        tool: &PublishedPythonTool,
    ) -> Result<PythonToolEnvironment, PythonToolError> {
        let (python_runtime, uv_identity) = probe_runtime_identity(&self.inner, tool).await?;
        let digest = python_tool_environment_digest(
            std::env::consts::OS,
            std::env::consts::ARCH,
            &python_runtime,
            &uv_identity,
            &tool.package.pyproject,
            &tool.package.uv_lock,
        );
        let final_root = self
            .inner
            .root
            .join("python-tool-envs")
            .join(digest.as_str());
        if let Some(environment) = Self::read_published_environment(
            &final_root,
            &digest,
            &python_runtime,
            &uv_identity,
            &tool.package.uv_lock,
        )? {
            Self::record_tool_version_binding(&self.inner, tool, &digest)?;
            return Ok(environment);
        }
        let key = digest.as_str().to_owned();
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
            let tool = tool.clone();
            let build_digest = digest.clone();
            let build_root = final_root.clone();
            let build_python = python_runtime.clone();
            let build_uv = uv_identity.clone();
            // Dropping a JoinHandle detaches the task; it does not abort it.
            // The caller therefore cannot become the physical materialization
            // owner merely by being cancelled while waiting below.
            std::mem::drop(tokio::spawn(async move {
                let mut owner_guard = owner_guard;
                let result = materialize_environment(
                    &inner,
                    &tool,
                    &build_root,
                    &build_digest,
                    &build_python,
                    &build_uv,
                )
                .await;
                let result = match result {
                    Ok(environment) => {
                        if let Err(error) =
                            Self::record_tool_version_binding(&inner, &tool, &build_digest)
                        {
                            Err(error)
                        } else {
                            Ok(environment)
                        }
                    }
                    Err(error) => Err(error),
                };
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

    /// Records the deterministic `ToolVersion -> environment digest` binding
    /// outside the environment's immutable dependency identity. The write is
    /// idempotent (same inputs produce the same record); a conflicting record
    /// is an explicit storage failure.
    fn record_tool_version_binding(
        inner: &PythonToolStoreInner,
        tool: &PublishedPythonTool,
        digest: &PythonToolEnvironmentDigest,
    ) -> Result<(), PythonToolError> {
        let directory = inner
            .root
            .join("python-tool-bindings")
            .join(tool.package.tool_version_id.as_str());
        std::fs::create_dir_all(&directory).map_err(io_error)?;
        let record = serde_json::json!({
            "format": BINDING_FORMAT,
            "tool_version_id": tool.package.tool_version_id,
            "environment_digest": digest,
        });
        let bytes = serde_json::to_vec(&record).map_err(io_error)?;
        let path = directory.join(format!("{}.json", digest.as_str()));
        if path.exists() {
            let existing = std::fs::read(&path).map_err(io_error)?;
            if existing != bytes {
                return Err(PythonToolError::Storage(format!(
                    "conflicting ToolVersion binding record for {}",
                    digest.as_str()
                )));
            }
            return Ok(());
        }
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, &bytes).map_err(io_error)?;
        std::fs::rename(&temporary, &path).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            io_error(error)
        })?;
        Ok(())
    }

    /// Validates a published environment marker against the exact
    /// deterministic inputs that derive the environment identity, and
    /// returns the environment handle. A marker that does not match every
    /// input is an explicit preparation failure, never a silent reuse.
    #[allow(clippy::too_many_arguments)] // one deterministic marker boundary
    fn read_published_environment(
        root: &Path,
        digest: &PythonToolEnvironmentDigest,
        python_runtime: &str,
        uv_identity: &str,
        lock_bytes: &[u8],
    ) -> Result<Option<PythonToolEnvironment>, PythonToolError> {
        let marker = root.join(ENVIRONMENT_MARKER);
        if !marker.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&marker).map_err(io_error)?;
        let value: EnvironmentMarker = serde_json::from_slice(&bytes).map_err(|error| {
            PythonToolError::Environment(format!("invalid environment marker: {error}"))
        })?;
        let valid = value.format == ENVIRONMENT_MARKER_FORMAT
            && value.ready
            && value.os == std::env::consts::OS
            && value.arch == std::env::consts::ARCH
            && value.digest == digest.as_str()
            && value.lock_digest == lock_digest_bytes(lock_bytes)
            && value.python_runtime == python_runtime
            && value.uv == uv_identity;
        if !valid {
            return Err(PythonToolError::Environment(
                "published environment marker does not match the expected digest inputs".to_owned(),
            ));
        }
        if !root.join("bin/python").is_file() {
            return Err(PythonToolError::Environment(
                "published environment marker has no Python interpreter".to_owned(),
            ));
        }
        Ok(Some(PythonToolEnvironment {
            digest: digest.clone(),
            root: root.to_path_buf(),
        }))
    }
}

async fn probe_runtime_identity(
    inner: &PythonToolStoreInner,
    tool: &PublishedPythonTool,
) -> Result<(String, String), PythonToolError> {
    let environment = ToolEnvironment::new();
    let child_environment = environment.child_environment(&tool.root);
    let uv_command = format!("{} --version", shell_quote(&inner.uv_binary));
    let python_command = format!("{} --version", shell_quote(&inner.python_binary));
    let run = |command: String| {
        let runner = inner.runner.clone();
        let cwd = tool.root.clone();
        let environment = child_environment.clone();
        async move {
            runner
                .run(
                    SupervisedCommandSpec {
                        command: command.clone(),
                        cwd,
                        environment,
                        timeout: Some(PYTHON_TOOL_PROBE_TIMEOUT),
                        cancellation: crate::runtime::CancellationSignal::new(),
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
    let uv_identity = bounded_output(&uv.stdout).trim().to_owned();
    let python_identity = bounded_output(&python.stdout).trim().to_owned();
    if uv_identity.is_empty() || python_identity.is_empty() {
        return Err(PythonToolError::Environment(
            "runtime identity probe returned empty output".to_owned(),
        ));
    }
    Ok((python_identity, uv_identity))
}

async fn materialize_environment(
    inner: &PythonToolStoreInner,
    tool: &PublishedPythonTool,
    final_root: &Path,
    digest: &PythonToolEnvironmentDigest,
    python_runtime: &str,
    uv_identity: &str,
) -> Result<PythonToolEnvironment, PythonToolError> {
    if final_root.exists() {
        std::fs::remove_dir_all(final_root).map_err(io_error)?;
    }
    let environment = ToolEnvironment::new();
    let child_environment = environment.child_environment(&tool.root);
    let commands = [
        format!("{} lock --check --no-config", shell_quote(&inner.uv_binary)),
        format!(
            "{} sync --frozen --no-install-project --no-default-groups --no-config",
            shell_quote(&inner.uv_binary)
        ),
    ];
    for command in commands {
        let mut environment_entries = child_environment.clone();
        environment_entries.push((
            "UV_PROJECT_ENVIRONMENT".to_owned(),
            final_root.display().to_string(),
        ));
        // The uv cache is store-private scratch state: without this pin,
        // a cache lookup could resolve relative to the working directory
        // and write `.cache/uv` into the immutable published ToolVersion
        // source, corrupting its canonical bytes.
        environment_entries.push((
            "UV_CACHE_DIR".to_owned(),
            inner.root.join("uv-cache").display().to_string(),
        ));
        // The exact interpreter selection: uv must materialize with the same
        // runtime whose identity entered the environment digest. Project-local
        // interpreter selection and uv heuristics are never permitted to pick
        // another Python while retaining the probed identity in the digest.
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
                    cwd: tool.root.clone(),
                    environment: environment_entries,
                    timeout: Some(PYTHON_TOOL_UV_TIMEOUT),
                    cancellation: crate::runtime::CancellationSignal::new(),
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
    let marker = EnvironmentMarker {
        format: ENVIRONMENT_MARKER_FORMAT.to_owned(),
        ready: true,
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        digest: digest.as_str().to_owned(),
        lock_digest: lock_digest_bytes(&tool.package.uv_lock),
        python_runtime: python_runtime.to_owned(),
        uv: uv_identity.to_owned(),
    };
    std::fs::create_dir_all(final_root).map_err(io_error)?;
    let temporary = final_root.with_extension("incomplete");
    if temporary.exists() {
        std::fs::remove_dir_all(&temporary).map_err(io_error)?;
    }
    std::fs::create_dir_all(&temporary).map_err(io_error)?;
    std::fs::write(
        temporary.join(ENVIRONMENT_MARKER),
        serde_json::to_vec(&marker).map_err(io_error)?,
    )
    .map_err(io_error)?;
    std::fs::rename(
        temporary.join(ENVIRONMENT_MARKER),
        final_root.join(ENVIRONMENT_MARKER),
    )
    .map_err(io_error)?;
    std::fs::remove_dir_all(temporary).map_err(io_error)?;
    Ok(PythonToolEnvironment {
        digest: digest.clone(),
        root: final_root.to_path_buf(),
    })
}

/// The deterministic environment marker: every input that derives the
/// environment identity, plus the `ready` publication gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EnvironmentMarker {
    format: String,
    ready: bool,
    os: String,
    arch: String,
    digest: String,
    lock_digest: String,
    python_runtime: String,
    uv: String,
}

/// Recomputes the `ToolVersion` identity over a published `source/` directory:
/// sorted relative paths, lengths, and raw bytes — the same canonical input
/// as [`tool_version_id`].
fn published_source_digest(source_root: &Path) -> Result<ToolVersionId, PythonToolError> {
    let files = collect_files(source_root)?;
    Ok(tool_version_id(&files))
}

fn lock_digest_bytes(lock: &[u8]) -> String {
    format!("sha256:{}", hex_digest(&sha2::Sha256::digest(lock)))
}

/// Canonical Python executor using the immutable `ToolVersion` source and
/// environment handles captured at candidate preparation.
///
/// The canonical published source is the immutable authority and is never
/// a working directory: every invocation claims a unique execution bundle
/// from the stable store's allocation domain and materializes its own
/// private source copy, harness, and input there (see
/// [`PythonToolStore::allocate_execution_bundle`]).
pub struct PythonToolExecutor {
    tool: PublishedPythonTool,
    environment: PythonToolEnvironment,
    store: PythonToolStore,
}

impl std::fmt::Debug for PythonToolExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PythonToolExecutor")
            .field("tool_version_id", &self.tool.package.tool_version_id)
            .field("environment_digest", &self.environment.digest)
            .finish_non_exhaustive()
    }
}

impl PythonToolExecutor {
    /// Creates an executor from published immutable handles.
    ///
    /// The constructor performs no filesystem writes: the harness is
    /// runtime-owned executable code materialized into each invocation's
    /// own execution bundle, so preparing a new executor generation can
    /// never rewrite executable bytes an older detached execution is about
    /// to launch.
    #[must_use]
    pub fn new(
        store: &PythonToolStore,
        tool: PublishedPythonTool,
        environment: PythonToolEnvironment,
    ) -> Self {
        Self {
            tool,
            environment,
            store: store.clone(),
        }
    }
}

impl ToolExecutor for PythonToolExecutor {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move {
            let started = Instant::now();
            // The invocation-private execution bundle:
            //
            //   python-invocations/execution-N/
            //       source/     the writable copy of the immutable
            //                   canonical ToolVersion source — the module
            //                   root *and* the working directory
            //       harness.py  the runtime-owned harness bytes
            //       input.json  the runtime-owned invocation arguments
            //
            // The three ownership domains never share a namespace: a
            // package-owned `input.json` or `harness.py` inside the
            // ToolVersion source is copied into `source/` like any other
            // canonical file and is never overwritten by runtime-owned
            // invocation files. Ordinary tool writes (relative paths,
            // `__file__`) land in `source/` and can never mutate the
            // published ToolVersion. The bundle is this invocation's own
            // claim; it is removed when the invocation settles, and no
            // other invocation or capability generation can reuse or
            // delete it while it is live.
            let bundle = match self.store.allocate_execution_bundle() {
                Ok(bundle) => bundle,
                Err(error) => return failed_python(&error.to_string(), started),
            };
            let source_dir = bundle.join("source");
            let harness_path = bundle.join("harness.py");
            let input_path = bundle.join("input.json");
            let materialization = (|| -> Result<(), PythonToolError> {
                materialize_invocation_source(&self.tool.root, &source_dir)?;
                std::fs::write(&harness_path, PYTHON_HARNESS).map_err(io_error)?;
                write_private_input(&input_path, &invocation.arguments)?;
                Ok(())
            })();
            if let Err(error) = materialization {
                // The bundle is this invocation's own freshly claimed
                // directory, so settling it is always safe.
                let _ = std::fs::remove_dir_all(&bundle);
                return failed_python(&error.to_string(), started);
            }
            let python = self.environment.root.join("bin/python");
            let command = format!(
                "{} {} {} {}",
                shell_quote(&python),
                shell_quote(&harness_path),
                shell_quote(&source_dir),
                shell_quote_str(&self.tool.package.entrypoint),
            ) + &format!(" {}", shell_quote(&input_path));
            let runtime_environment = context
                .environment
                .with_replacement_overlay(&ToolEnvironmentOverlay::python(&self.environment.root));
            let result = self
                .store
                .inner
                .runner
                .run(
                    SupervisedCommandSpec {
                        command,
                        cwd: source_dir,
                        environment: runtime_environment
                            .child_environment(context.workspace.root()),
                        timeout: None,
                        cancellation: context.cancellation.signal(),
                    },
                    None,
                )
                .await;
            let _ = std::fs::remove_dir_all(&bundle);
            match result {
                Ok(result) => {
                    translate_python_result(result, started, context.cancellation.reason())
                }
                Err(error) => failed_python(&error, started),
            }
        })
    }
}

/// Materializes the invocation-private source copy of one immutable
/// published `ToolVersion`: a deterministic recursive copy of every regular
/// file, preserving the package-relative layout.
///
/// The destination is the `source/` directory inside the caller's freshly
/// claimed execution bundle, so it is expected not to exist; an unexpected
/// pre-existing destination is a storage failure, never a reason to delete
/// unknown state.
///
/// The published source was validated at discovery to contain only
/// directories and regular non-symlink files; anything else here means the
/// canonical store was tampered with and fails the invocation explicitly
/// rather than following a link outside the store.
fn materialize_invocation_source(
    source_root: &Path,
    destination: &Path,
) -> Result<(), PythonToolError> {
    if destination.exists() {
        return Err(PythonToolError::Storage(format!(
            "the invocation source directory already exists: {}",
            destination.display()
        )));
    }
    copy_invocation_tree(source_root, destination)
}

/// The deterministic recursive copy behind
/// [`materialize_invocation_source`].
fn copy_invocation_tree(source: &Path, destination: &Path) -> Result<(), PythonToolError> {
    std::fs::create_dir_all(destination).map_err(io_error)?;
    let mut entries = std::fs::read_dir(source)
        .map_err(io_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error)?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(io_error)?;
        let target = destination.join(entry.file_name());
        if metadata.is_dir() {
            copy_invocation_tree(&entry.path(), &target)?;
        } else if metadata.is_file() && !metadata.file_type().is_symlink() {
            std::fs::copy(entry.path(), &target).map_err(io_error)?;
        } else {
            return Err(PythonToolError::Storage(format!(
                "the published ToolVersion source contains a non-regular entry: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn write_private_input(path: &Path, arguments: &serde_json::Value) -> Result<(), PythonToolError> {
    let bytes = serde_json::to_vec(arguments)
        .map_err(|error| PythonToolError::Storage(error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true).mode(0o600);
        let mut file = options.open(path).map_err(io_error)?;
        file.write_all(&bytes).map_err(io_error)?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, bytes).map_err(io_error)?;
    Ok(())
}

fn translate_python_result(
    result: CapturedProcessResult,
    started: Instant,
    cancellation_reason: crate::runtime::types::CancellationReason,
) -> ToolExecutionResult {
    let status = match result.intent {
        ProcessOutcomeIntent::Cancelled => ToolExecutionStatus::Cancelled {
            reason: cancellation_reason,
        },
        ProcessOutcomeIntent::TimedOut => ToolExecutionStatus::TimedOut,
        ProcessOutcomeIntent::ProcessControlFailed(error) => ToolExecutionStatus::Failed { error },
        ProcessOutcomeIntent::Completed if result.exit_code != Some(0) => {
            ToolExecutionStatus::Failed {
                error: bounded_output(&result.stderr),
            }
        }
        ProcessOutcomeIntent::Completed => {
            let envelope: Result<serde_json::Value, _> = serde_json::from_slice(&result.stdout);
            match envelope {
                Ok(value) if value.get("ok") == Some(&serde_json::Value::Bool(true)) => {
                    return ToolExecutionResult {
                        status: ToolExecutionStatus::Success,
                        content: vec![ToolResultContent::Json {
                            value: value
                                .get("value")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        }],
                        duration_ms: duration_ms(started),
                        exit_code: result.exit_code,
                        artifacts: Vec::new(),
                        truncation: None,
                    };
                }
                Ok(value) => ToolExecutionStatus::Failed {
                    error: value
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Python tool failed")
                        .to_owned(),
                },
                Err(error) => ToolExecutionStatus::Failed {
                    error: format!("invalid Python result envelope: {error}"),
                },
            }
        }
    };
    ToolExecutionResult {
        status,
        content: Vec::new(),
        duration_ms: duration_ms(started),
        exit_code: result.exit_code,
        artifacts: Vec::new(),
        truncation: None,
    }
}

fn failed_python(message: &str, started: Instant) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Failed {
            error: bounded_output(message.as_bytes()),
        },
        content: Vec::new(),
        duration_ms: duration_ms(started),
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
    }
}

fn duration_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn bounded_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).into_owned()
}

fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    shell_quote_str(&value)
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

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{
        BuildOwnerGuard, BuildState, PublishedPythonTool, PythonToolPackage, PythonToolStore,
        PythonToolStoreInner, python_tool_environment_digest,
    };
    use crate::runtime::identity::ToolVersionId;
    use crate::runtime::process_runner::{
        CapturedProcessResult, ProcessOutcomeIntent, SupervisedCommandSpec, SupervisedProcessRunner,
    };
    use crate::tools::types::ToolInvocationPolicy;
    use futures_util::future::BoxFuture;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn environment_identity_excludes_source_description() {
        let first = python_tool_environment_digest(
            "linux",
            "x86_64",
            "Python 3.13",
            "uv 0.9",
            b"deps",
            b"lock",
        );
        let second = python_tool_environment_digest(
            "linux",
            "x86_64",
            "Python 3.13",
            "uv 0.9",
            b"deps",
            b"lock",
        );
        assert_eq!(first, second);
        assert_ne!(
            first,
            python_tool_environment_digest(
                "linux",
                "x86_64",
                "Python 3.13",
                "uv 0.9",
                b"other",
                b"lock"
            )
        );
        assert_ne!(
            first,
            python_tool_environment_digest(
                "linux",
                "aarch64",
                "Python 3.13",
                "uv 0.9",
                b"deps",
                b"lock"
            )
        );
    }

    /// A scripted runner that deterministically dispatches results by
    /// command, parks materialization commands behind a gate the test
    /// controls, records every invocation with its finite deadline, and
    /// materializes the `bin/python` fixture for successful sync results so
    /// the reuse path observes a real published environment. Result dispatch
    /// by command makes concurrent callers deterministic: probes always
    /// resolve the same identities regardless of interleaving.
    /// One recorded invocation: the program, its explicit environment, and
    /// the finite deadline it was given.
    type RecordedCommand = (String, Vec<(String, String)>, Option<Duration>);

    #[derive(Clone)]
    struct ScriptedRunner {
        materialization_results: Arc<Mutex<VecDeque<Result<CapturedProcessResult, String>>>>,
        fail_probes: Arc<std::sync::atomic::AtomicBool>,
        started: Arc<tokio::sync::Notify>,
        started_count: Arc<std::sync::atomic::AtomicUsize>,
        materialization_count: Arc<std::sync::atomic::AtomicUsize>,
        gate_tx: tokio::sync::watch::Sender<bool>,
        gate_rx: tokio::sync::watch::Receiver<bool>,
        commands: Arc<Mutex<Vec<RecordedCommand>>>,
    }

    impl ScriptedRunner {
        fn new(materialization_results: Vec<Result<CapturedProcessResult, String>>) -> Self {
            let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
            Self {
                materialization_results: Arc::new(Mutex::new(VecDeque::from(
                    materialization_results,
                ))),
                fail_probes: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                started: Arc::new(tokio::sync::Notify::new()),
                started_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                materialization_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                gate_tx,
                gate_rx,
                commands: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn set_probe_timeout(&self) {
            self.fail_probes
                .store(true, std::sync::atomic::Ordering::Release);
        }

        fn release_gate(&self) {
            let _ = self.gate_tx.send(true);
        }

        fn wait_for_runs(&self, count: usize) {
            tokio::task::block_in_place(|| {
                loop {
                    if self
                        .started_count
                        .load(std::sync::atomic::Ordering::Acquire)
                        >= count
                    {
                        return;
                    }
                    let notified = self.started.notified();
                    tokio::runtime::Handle::current().block_on(async {
                        tokio::time::timeout(std::time::Duration::from_secs(15), notified)
                            .await
                            .expect("the scripted runner must progress");
                    });
                }
            });
        }

        fn wait_for_materializations(&self, count: usize) {
            tokio::task::block_in_place(|| {
                loop {
                    if self
                        .materialization_count
                        .load(std::sync::atomic::Ordering::Acquire)
                        >= count
                    {
                        return;
                    }
                    let notified = self.started.notified();
                    tokio::runtime::Handle::current().block_on(async {
                        tokio::time::timeout(std::time::Duration::from_secs(15), notified)
                            .await
                            .expect("the scripted runner must progress");
                    });
                }
            });
        }

        fn materialization_count(&self) -> usize {
            self.materialization_count
                .load(std::sync::atomic::Ordering::Acquire)
        }

        /// The full environment of the first recorded `sync` invocation.
        fn sync_environment(&self) -> Option<Vec<(String, String)>> {
            self.commands
                .lock()
                .expect("recorded commands lock")
                .iter()
                .find(|(command, _, _)| command.contains(" sync --frozen "))
                .map(|(_, environment, _)| environment.clone())
        }
    }

    impl SupervisedProcessRunner for ScriptedRunner {
        fn run(
            &self,
            spec: SupervisedCommandSpec,
            _control: Option<crate::runtime::process_runner::RunnerTestControl>,
        ) -> BoxFuture<'_, Result<CapturedProcessResult, String>> {
            let is_materialization =
                spec.command.contains(" lock --check ") || spec.command.contains(" sync --frozen ");
            self.commands.lock().expect("recorded commands lock").push((
                spec.command.clone(),
                spec.environment.clone(),
                spec.timeout,
            ));
            let started_count = self.started_count.clone();
            let materialization_count = self.materialization_count.clone();
            let started = self.started.clone();
            let gate_rx = self.gate_rx.clone();
            let fail_probes = self.fail_probes.clone();
            let result = if is_materialization {
                self.materialization_results
                    .lock()
                    .expect("scripted result lock")
                    .pop_front()
                    .expect("scripted materialization result")
            } else if fail_probes.load(std::sync::atomic::Ordering::Acquire) {
                Ok(CapturedProcessResult {
                    exit_code: None,
                    intent: ProcessOutcomeIntent::TimedOut,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
            } else if spec.command.contains("python") {
                Ok(ok_output("Python 3.12.3\n"))
            } else {
                Ok(ok_output("uv 0.8.22\n"))
            };
            Box::pin(async move {
                let _ = started_count.fetch_add(1, std::sync::atomic::Ordering::Release);
                started.notify_waiters();
                if is_materialization {
                    let _ =
                        materialization_count.fetch_add(1, std::sync::atomic::Ordering::Release);
                    started.notify_waiters();
                    // Park every materialization command until the test
                    // releases the gate; probes pass through immediately.
                    let mut gate_rx = gate_rx;
                    if !*gate_rx.borrow() {
                        let _ = gate_rx.changed().await;
                    }
                    if spec.command.contains(" sync --frozen ") && result.as_ref().is_ok() {
                        // Materialize the environment fixture so the reuse
                        // path observes a real published environment.
                        let final_root = spec
                            .environment
                            .iter()
                            .find(|(key, _)| key == "UV_PROJECT_ENVIRONMENT")
                            .map(|(_, value)| std::path::PathBuf::from(value));
                        if let Some(final_root) = final_root {
                            let _ = std::fs::create_dir_all(final_root.join("bin"));
                            let _ = std::fs::write(final_root.join("bin/python"), b"#!/bin/sh\n");
                        }
                    }
                }
                result
            })
        }
    }

    fn ok_output(stdout: &str) -> CapturedProcessResult {
        CapturedProcessResult {
            exit_code: Some(0),
            intent: ProcessOutcomeIntent::Completed,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn test_package() -> PythonToolPackage {
        let files = vec![
            (
                PathBuf::from("TOOL.toml"),
                b"schema_version = 1\nname = \"alpha\"\ndescription = \"Alpha\"\nentrypoint = \"tool:main\"\nexecution = \"foreground_only\"\nconcurrency = \"sequential\"\n"
                    .to_vec(),
            ),
            (
                PathBuf::from("input.schema.json"),
                br#"{"type":"object","properties":{},"additionalProperties":false}"#.to_vec(),
            ),
            (
                PathBuf::from("pyproject.toml"),
                b"[project]\nname = \"alpha\"\nversion = \"0.1.0\"\nrequires-python = \">=3.11\"\n"
                    .to_vec(),
            ),
            (
                PathBuf::from("tool.py"),
                b"def main(arguments):\n    return arguments\n".to_vec(),
            ),
            (PathBuf::from("uv.lock"), b"version = 1\nrevision = 1\n".to_vec()),
        ];
        PythonToolPackage {
            name: "alpha".to_owned(),
            description: "Alpha".to_owned(),
            entrypoint: "tool:main".to_owned(),
            policy: ToolInvocationPolicy::default(),
            tool_version_id: super::tool_version_id(&files),
            files,
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            pyproject: b"[project]\nname = \"alpha\"\nversion = \"0.1.0\"\n".to_vec(),
            uv_lock: b"version = 1\nrevision = 1\n".to_vec(),
        }
    }

    fn store_with(
        runner: Arc<dyn SupervisedProcessRunner>,
    ) -> (tempfile::TempDir, PythonToolStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::with_runner(dir.path().join("store"), runner).expect("store");
        (dir, store)
    }

    /// Concurrent same-digest callers coalesce behind exactly one
    /// store-owned owner: while the first materialization is parked, a
    /// second caller waits on the same build and never starts a second
    /// materialization sequence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_digest_concurrent_callers_coalesce_behind_one_owner() {
        let scripted = Arc::new(ScriptedRunner::new(vec![
            Ok(ok_output("resolved 2 packages")),
            Ok(ok_output("installed 1 package")),
        ]));
        let (_dir, store) = store_with(scripted.clone());
        let published = store.publish(&test_package()).expect("publish");

        let first_store = store.clone();
        let first_published = published.clone();
        let first =
            tokio::spawn(async move { first_store.ensure_environment(&first_published).await });
        // The owner is inside the materialization while the gate is closed.
        scripted.wait_for_materializations(1);

        let second_store = store.clone();
        let second_published = published.clone();
        let second =
            tokio::spawn(async move { second_store.ensure_environment(&second_published).await });
        // Let the second caller finish its probes and reach the in-flight
        // coordination point, then assert it did not start a second
        // materialization sequence.
        // Runs: owner probes (2) + parked lock --check (1) + waiter probes
        // (2) = 5; the gated sync never starts before the release.
        scripted.wait_for_runs(5);
        assert_eq!(
            scripted.materialization_count(),
            1,
            "a second owner must never overlap an in-flight build"
        );

        scripted.release_gate();
        let first_result = first.await.expect("first caller task").expect("first env");
        let second_result = second
            .await
            .expect("second caller task")
            .expect("second env");
        assert_eq!(first_result.digest, second_result.digest);
        assert_eq!(first_result.root, second_result.root);
        assert_eq!(
            scripted.materialization_count(),
            2,
            "exactly one materialization sequence ran"
        );
    }

    /// Dropping a waiter while the build continues never releases the
    /// in-flight entry: the owner completes, publishes, and a later call
    /// observes the published environment without any new build.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_waiter_does_not_release_ownership_while_build_continues() {
        let scripted = Arc::new(ScriptedRunner::new(vec![
            Ok(ok_output("resolved 2 packages")),
            Ok(ok_output("installed 1 package")),
        ]));
        let (_dir, store) = store_with(scripted.clone());
        let published = store.publish(&test_package()).expect("publish");

        let waiter_store = store.clone();
        let waiter_published = published.clone();
        let waiter =
            tokio::spawn(async move { waiter_store.ensure_environment(&waiter_published).await });
        scripted.wait_for_materializations(1);
        waiter.abort();
        let _ = waiter.await;

        scripted.release_gate();
        scripted.wait_for_materializations(2);

        // The dropped waiter never released the in-flight entry: a retry
        // must coalesce with the still-published result instead of starting
        // a second build.
        let retry_store = store.clone();
        let retry_published = published.clone();
        let environment = retry_store
            .ensure_environment(&retry_published)
            .await
            .expect("the owner's result is still observed");
        assert_eq!(environment.digest.as_str().len(), 7 + 64);
        assert_eq!(
            scripted.materialization_count(),
            2,
            "no second build after waiter drop"
        );
    }

    /// An owner failure publishes a terminal error to every waiting caller
    /// and removes the in-flight entry, so a retry can acquire ownership
    /// without overlapping the failed owner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owner_error_wakes_all_waiters_and_retry_does_not_overlap() {
        let scripted = Arc::new(ScriptedRunner::new(vec![
            Ok(ok_output("resolved 2 packages")),
            Err("injected sync failure".to_owned()),
        ]));
        let (_dir, store) = store_with(scripted.clone());
        let published = store.publish(&test_package()).expect("publish");

        let first_store = store.clone();
        let first_published = published.clone();
        let first =
            tokio::spawn(async move { first_store.ensure_environment(&first_published).await });
        scripted.wait_for_materializations(1);
        let second_store = store.clone();
        let second_published = published.clone();
        let second =
            tokio::spawn(async move { second_store.ensure_environment(&second_published).await });
        scripted.wait_for_runs(5);
        assert_eq!(
            scripted.materialization_count(),
            1,
            "the waiter must not start a second owner"
        );
        scripted.release_gate();
        scripted.wait_for_materializations(2);

        let first_error = first
            .await
            .expect("first caller task")
            .expect_err("first fails");
        let second_error = second
            .await
            .expect("second caller task")
            .expect_err("second fails");
        assert!(first_error.to_string().contains("injected sync failure"));
        assert_eq!(
            first_error, second_error,
            "both waiters observe the same terminal error"
        );

        // The failed owner removed its in-flight entry; the retry starts a
        // fresh, non-overlapping materialization sequence.
        *scripted
            .materialization_results
            .lock()
            .expect("scripted results lock") = VecDeque::from(vec![
            Ok(ok_output("resolved 2 packages")),
            Ok(ok_output("installed 1 package")),
        ]);
        let retry_store = store.clone();
        let retry_published = published.clone();
        let retry =
            tokio::spawn(async move { retry_store.ensure_environment(&retry_published).await });
        scripted.wait_for_materializations(4);
        assert!(
            retry.await.expect("retry task").is_ok(),
            "the retry can acquire ownership after the failed owner published"
        );
        assert_eq!(
            scripted.materialization_count(),
            4,
            "the retry started only after the previous owner finished"
        );
    }

    /// The RAII owner guard: if the detached owner task exits before
    /// terminal publication, the in-flight entry is removed by
    /// pointer identity and every waiter receives a terminal error.
    #[test]
    fn owner_guard_early_exit_cannot_strand_an_in_flight_entry() {
        let in_flight: Arc<Mutex<BTreeMap<String, Arc<BuildState>>>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let state = Arc::new(BuildState {
            result: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        });
        let key = "digest-key".to_owned();
        in_flight
            .lock()
            .expect("in-flight lock")
            .insert(key.clone(), state.clone());
        {
            let guard = BuildOwnerGuard {
                in_flight: in_flight.clone(),
                key: key.clone(),
                state: state.clone(),
                completed: false,
            };
            drop(guard);
        }
        assert!(
            in_flight.lock().expect("in-flight lock").is_empty(),
            "the early-exited owner must remove its in-flight entry"
        );
        let result = state
            .result
            .lock()
            .expect("result lock")
            .clone()
            .expect("terminal result");
        assert!(result.is_err());
    }

    /// A foreign state for the same key is never removed by another owner's
    /// guard: removal is pointer-identity safe.
    #[test]
    fn owner_guard_removal_is_pointer_identity_safe() {
        let in_flight: Arc<Mutex<BTreeMap<String, Arc<BuildState>>>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let foreign = Arc::new(BuildState {
            result: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        });
        let key = "digest-key".to_owned();
        in_flight
            .lock()
            .expect("in-flight lock")
            .insert(key.clone(), foreign.clone());
        let own = Arc::new(BuildState {
            result: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        });
        {
            let mut guard = BuildOwnerGuard {
                in_flight: in_flight.clone(),
                key: key.clone(),
                state: own.clone(),
                completed: false,
            };
            guard.finish(Ok(crate::tools::python::PythonToolEnvironment {
                digest: crate::runtime::identity::PythonToolEnvironmentDigest::new(
                    "sha256:deadbeef".to_owned(),
                ),
                root: PathBuf::from("/unused"),
            }));
        }
        assert_eq!(
            in_flight.lock().expect("in-flight lock").len(),
            1,
            "a stale guard must never remove a newer owner's entry"
        );
        assert!(
            in_flight
                .lock()
                .expect("in-flight lock")
                .get(&key)
                .is_some_and(|current| Arc::ptr_eq(current, &foreign))
        );
    }

    /// The timeout of every materialization/probe command is finite, so a
    /// stuck package manager surfaces as an explicit preparation failure.
    #[tokio::test]
    async fn environment_commands_have_finite_deadlines() {
        let scripted = Arc::new(ScriptedRunner::new(vec![
            Ok(ok_output("resolved 2 packages")),
            Ok(ok_output("installed 1 package")),
        ]));
        scripted.release_gate();
        let (_dir, store) = store_with(scripted.clone());
        let published = store.publish(&test_package()).expect("publish");
        let _ = store.ensure_environment(&published).await;
        let recorded = scripted
            .commands
            .lock()
            .expect("recorded commands lock")
            .clone();
        assert_eq!(recorded.len(), 4, "two probes plus two uv commands");
        for (command, _, timeout) in &recorded {
            assert!(
                timeout.is_some(),
                "every M7 preparation command needs a finite deadline: {command}"
            );
        }
    }

    /// The exact interpreter whose identity enters the environment digest is
    /// pinned to uv via `UV_PYTHON`, so uv cannot silently select another
    /// interpreter while the digest claims the probed identity.
    #[tokio::test]
    async fn uv_is_pinned_to_the_probed_interpreter() {
        let scripted = Arc::new(ScriptedRunner::new(vec![
            Ok(ok_output("resolved 2 packages")),
            Ok(ok_output("installed 1 package")),
        ]));
        scripted.release_gate();
        let (_dir, store) = store_with(scripted.clone());
        let published = store.publish(&test_package()).expect("publish");
        let _ = store.ensure_environment(&published).await;

        let sync_env = scripted
            .sync_environment()
            .expect("sync command environment");
        let uv_python = sync_env
            .iter()
            .find(|(key, _)| key == "UV_PYTHON")
            .map(|(_, value)| value.clone())
            .expect("UV_PYTHON must pin the interpreter selection");
        let probes = sync_env
            .iter()
            .filter(|(key, _)| key == "UV_NO_PYTHON_DOWNLOADS" || key == "UV_PYTHON_DOWNLOADS")
            .collect::<Vec<_>>();
        assert!(
            probes
                .iter()
                .any(|(key, value)| { key == "UV_NO_PYTHON_DOWNLOADS" && value == "1" })
                && probes
                    .iter()
                    .any(|(key, value)| { key == "UV_PYTHON_DOWNLOADS" && value == "0" }),
            "managed Python downloads stay disabled"
        );
        assert!(
            !uv_python.is_empty() && !uv_python.contains(' '),
            "UV_PYTHON must name one exact executable"
        );
        assert!(
            std::path::Path::new(&uv_python).is_absolute(),
            "UV_PYTHON must be an absolute executable path"
        );
    }

    /// A different interpreter-selection input cannot alias to the same
    /// environment identity: the digest includes the probed Python identity,
    /// so a runtime selection change produces a different digest.
    #[test]
    fn different_interpreter_selection_cannot_alias_the_environment_identity() {
        let v1 = python_tool_environment_digest(
            std::env::consts::OS,
            std::env::consts::ARCH,
            "Python 3.12.3",
            "uv 0.8.22",
            b"pyproject",
            b"lock",
        );
        let v2 = python_tool_environment_digest(
            std::env::consts::OS,
            std::env::consts::ARCH,
            "Python 3.13.1",
            "uv 0.8.22",
            b"pyproject",
            b"lock",
        );
        assert_ne!(v1, v2);
    }

    /// The published `ToolVersion` shape is `tool-versions/<id>/source/` plus
    /// the version marker; the executor source root is exactly the
    /// `source/` directory.
    #[test]
    fn published_tool_version_uses_the_source_directory_shape() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        let package = test_package();
        let published = store.publish(&package).expect("publish");
        assert_eq!(
            published.root,
            dir.path()
                .join("store/tool-versions")
                .join(package.tool_version_id.as_str())
                .join("source")
        );
        assert!(published.root.join("TOOL.toml").is_file());
        assert!(published.root.join("uv.lock").is_file());
        let marker = dir
            .path()
            .join("store/tool-versions")
            .join(package.tool_version_id.as_str())
            .join("RUSTX_TOOL_VERSION.json");
        assert!(
            marker.is_file(),
            "the marker sits beside source/, never inside"
        );
    }

    /// A corrupt published `ToolVersion` — source mutated after publication —
    /// fails the reuse preparation explicitly instead of being trusted by
    /// its marker string.
    #[test]
    fn corrupt_published_tool_version_fails_reuse_explicitly() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        let package = test_package();
        let _ = store.publish(&package).expect("publish");
        let published_source = dir
            .path()
            .join("store/tool-versions")
            .join(package.tool_version_id.as_str())
            .join("source");
        std::fs::write(
            published_source.join("tool.py"),
            b"def main(arguments):\n    return \"tampered\"\n",
        )
        .expect("tamper with the published source");
        let error = store
            .publish(&package)
            .expect_err("corrupt reuse must fail");
        assert!(
            error
                .to_string()
                .contains("does not match its claimed identity"),
            "unexpected error: {error}"
        );
        // The corrupt publication is never mutated into validity.
        let tampered = std::fs::read_to_string(published_source.join("tool.py")).expect("read");
        assert!(tampered.contains("tampered"));
    }

    /// A valid published `ToolVersion` is reused without mutation and without
    /// re-staging.
    #[test]
    fn valid_published_tool_version_is_reused_unchanged() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        let package = test_package();
        let first = store.publish(&package).expect("first publish");
        let second = store.publish(&package).expect("second publish");
        assert_eq!(first.root, second.root);
        let source_files = std::fs::read_dir(&first.root).expect("source dir").count();
        assert_eq!(source_files, 5, "all package files published");
    }

    /// The environment ready marker locks every deterministic input of the
    /// environment identity; a marker that was written for a different
    /// Python runtime is rejected on reuse.
    #[test]
    fn environment_marker_locks_all_digest_inputs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        let package = test_package();
        let published = store.publish(&package).expect("publish");
        let digest = python_tool_environment_digest(
            std::env::consts::OS,
            std::env::consts::ARCH,
            "Python 3.12.3",
            "uv 0.8.22",
            &published.package.pyproject,
            &published.package.uv_lock,
        );
        let root = dir
            .path()
            .join("store/python-tool-envs")
            .join(digest.as_str());
        std::fs::create_dir_all(root.join("bin")).expect("bin dir");
        std::fs::write(root.join("bin/python"), b"#!/bin/sh\n").expect("python");
        let marker = super::EnvironmentMarker {
            format: super::ENVIRONMENT_MARKER_FORMAT.to_owned(),
            ready: true,
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            digest: digest.as_str().to_owned(),
            lock_digest: super::lock_digest_bytes(&package.uv_lock),
            python_runtime: "Python 9.9.9".to_owned(),
            uv: "uv 0.8.22".to_owned(),
        };
        std::fs::write(
            root.join(super::ENVIRONMENT_MARKER),
            serde_json::to_vec(&marker).expect("marker"),
        )
        .expect("marker write");
        let error = PythonToolStore::read_published_environment(
            &root,
            &digest,
            "Python 3.12.3",
            "uv 0.8.22",
            &package.uv_lock,
        )
        .expect_err("a marker with a different Python runtime must be rejected");
        assert!(
            error
                .to_string()
                .contains("does not match the expected digest inputs")
        );
    }

    /// Each `ToolVersion` -> environment binding is recorded deterministically
    /// outside the environment's immutable identity; a second `ToolVersion`
    /// reusing the same digest records its own binding.
    #[test]
    fn tool_version_environment_bindings_are_recorded_per_tool_version() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        let package = test_package();
        let digest = crate::runtime::identity::PythonToolEnvironmentDigest::new(
            "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_owned(),
        );
        PythonToolStore::record_tool_version_binding(
            &store.inner,
            &PublishedPythonTool {
                package: package.clone(),
                root: PathBuf::from("/unused"),
            },
            &digest,
        )
        .expect("first binding");
        let other = PythonToolPackage {
            name: "beta".to_owned(),
            tool_version_id: ToolVersionId::new("sha256:other".to_owned()),
            ..package.clone()
        };
        PythonToolStore::record_tool_version_binding(
            &store.inner,
            &PublishedPythonTool {
                package: other.clone(),
                root: PathBuf::from("/unused"),
            },
            &digest,
        )
        .expect("second binding");
        let first_record = dir
            .path()
            .join("store/python-tool-bindings")
            .join(package.tool_version_id.as_str())
            .join(format!("{}.json", digest.as_str()));
        let second_record = dir
            .path()
            .join("store/python-tool-bindings")
            .join(other.tool_version_id.as_str())
            .join(format!("{}.json", digest.as_str()));
        assert!(first_record.is_file(), "first binding recorded");
        assert!(second_record.is_file(), "second binding recorded");
        assert_ne!(first_record, second_record);
        let environment_marker = dir
            .path()
            .join("store/python-tool-envs")
            .join(digest.as_str())
            .join(super::ENVIRONMENT_MARKER);
        assert!(
            !environment_marker.exists(),
            "the environment's immutable identity never claims a ToolVersion"
        );
    }

    /// The `PythonToolStoreInner` construction keeps all coordination handles.
    #[test]
    fn store_inner_is_constructible() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        assert!(store.inner.root.join("tool-versions").is_dir());
        assert!(store.inner.root.join("python-tool-envs").is_dir());
        assert!(store.inner.root.join("python-tool-bindings").is_dir());
        assert!(store.inner.root.join("python-invocations").is_dir());
        let _ = PythonToolStoreInner {
            root: dir.path().join("other"),
            runner: Arc::new(ScriptedRunner::new(Vec::new())),
            uv_binary: PathBuf::from("uv"),
            python_binary: PathBuf::from("python3"),
            in_flight: Arc::new(Mutex::new(BTreeMap::new())),
            next_invocation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
    }

    /// A scripted execution runner that reproduces the tool's own
    /// filesystem behavior at whatever writable working directory the
    /// executor gave it — one relative write and one self-modification of
    /// the module file — then answers with a valid success envelope. It
    /// records every command with its cwd.
    #[derive(Clone, Default)]
    struct ToolWriteSimulatingRunner {
        commands: Arc<Mutex<Vec<(String, PathBuf)>>>,
    }

    impl SupervisedProcessRunner for ToolWriteSimulatingRunner {
        fn run(
            &self,
            spec: SupervisedCommandSpec,
            _control: Option<crate::runtime::process_runner::RunnerTestControl>,
        ) -> BoxFuture<'_, Result<CapturedProcessResult, String>> {
            self.commands
                .lock()
                .expect("recorded commands lock")
                .push((spec.command.clone(), spec.cwd.clone()));
            // The tool's ordinary writes land wherever its working
            // directory and module root are.
            std::fs::write(spec.cwd.join("runtime-cache.txt"), b"changed")
                .expect("the simulated relative write");
            std::fs::write(
                spec.cwd.join("tool.py"),
                b"def main(arguments):\n    return 'tampered'\n",
            )
            .expect("the simulated self-modification");
            Box::pin(async move {
                Ok(CapturedProcessResult {
                    exit_code: Some(0),
                    intent: ProcessOutcomeIntent::Completed,
                    stdout: br#"{"ok":true,"value":"ok"}"#.to_vec(),
                    stderr: Vec::new(),
                })
            })
        }
    }

    struct NoProgress;
    impl crate::tools::executor::ProgressReporter for NoProgress {
        fn report(&self, _progress: crate::tools::types::ToolProgress) {}
    }

    /// Builds an executor over a store with the given runner plus the
    /// conversation tool runtime the execution context borrows from.
    fn executor_fixture(
        dir: &tempfile::TempDir,
        runner: Arc<dyn SupervisedProcessRunner>,
        package: &PythonToolPackage,
    ) -> (
        PythonToolStore,
        PublishedPythonTool,
        crate::tools::python::PythonToolExecutor,
        crate::tools::runtime::ConversationToolRuntime,
    ) {
        let store = PythonToolStore::with_runner(dir.path().join("store"), runner).expect("store");
        let published = store.publish(package).expect("publish");
        let environment = crate::tools::python::PythonToolEnvironment {
            digest: crate::runtime::identity::PythonToolEnvironmentDigest::new(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_owned(),
            ),
            root: dir.path().join("env"),
        };
        let executor =
            crate::tools::python::PythonToolExecutor::new(&store, published.clone(), environment);
        std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            crate::runtime::identity::ConversationId::new("conv-python-immutability"),
            dir.path().join("workspace"),
            dir.path().join("artifacts"),
        )
        .expect("tool runtime");
        (store, published, executor, tool_runtime)
    }

    async fn execute_once(
        executor: &crate::tools::python::PythonToolExecutor,
        tool_runtime: &crate::tools::runtime::ConversationToolRuntime,
    ) -> crate::tools::types::ToolExecutionResult {
        crate::tools::executor::ToolExecutor::execute(
            executor,
            crate::tools::types::ToolInvocation {
                call_id: crate::runtime::identity::ToolCallId::new("call-1"),
                tool_id: crate::runtime::identity::ToolId::new("tool-alpha"),
                tool_name: "alpha".to_owned(),
                mode: crate::tools::types::ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            },
            crate::tools::executor::ToolExecutionContext {
                conversation_id: tool_runtime.conversation_id(),
                execution_id: None,
                cancellation: crate::runtime::ExecutionCancellation::detached(
                    crate::runtime::CancellationSignal::new(),
                    crate::runtime::types::CancellationReason::UserRequested,
                ),
                workspace: tool_runtime.workspace(),
                progress: &NoProgress,
                artifacts: tool_runtime.artifacts(),
                environment: tool_runtime.environment(),
            },
        )
        .await
    }

    /// `ToolVersion` immutability at execution (Issue #81): a tool that
    /// performs ordinary filesystem writes — a relative `cwd` write and a
    /// self-modification through `__file__`'s location — mutates only its
    /// invocation-private materialization. The canonical published source
    /// keeps its exact bytes, no extra file appears in it, the recomputed
    /// content digest still matches the committed `ToolVersionId`, and the
    /// invocation-private directory is settled after the execution.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_execution_cannot_mutate_the_canonical_published_source() {
        let runner = Arc::new(ToolWriteSimulatingRunner::default());
        let dir = tempfile::tempdir().expect("temp dir");
        let package = test_package();
        let original_tool_bytes = package
            .files
            .iter()
            .find(|(path, _)| path == &PathBuf::from("tool.py"))
            .expect("tool.py")
            .1
            .clone();
        let (store, published, executor, tool_runtime) =
            executor_fixture(&dir, runner.clone(), &package);

        let result = execute_once(&executor, &tool_runtime).await;
        assert!(
            matches!(
                result.status,
                crate::tools::types::ToolExecutionStatus::Success
            ),
            "the tool executed: {:?}",
            result.status
        );

        // The execution ran against an invocation-private materialization,
        // never against the canonical published root.
        let recorded = runner.commands.lock().expect("recorded commands lock");
        assert_eq!(recorded.len(), 1, "exactly one execution command ran");
        let (command, cwd) = &recorded[0];
        let invocation_root = store.inner.root.join("python-invocations");
        let bundle = cwd.parent().expect("the source directory's bundle");
        assert_eq!(
            cwd.file_name().expect("source dir name"),
            "source",
            "the execution cwd is the bundle's private source copy: {cwd:?}"
        );
        assert!(
            bundle.starts_with(&invocation_root) && *cwd != published.root,
            "the execution bundle is invocation-private: {bundle:?}"
        );
        assert!(
            command.contains(&cwd.display().to_string()),
            "the harness received the private source copy as its source root"
        );
        assert!(
            command.contains(&bundle.join("harness.py").display().to_string()),
            "each invocation executes its own bundle-private harness"
        );
        assert!(
            command.contains(&bundle.join("input.json").display().to_string()),
            "the runtime-owned input lives outside the source namespace"
        );
        assert!(
            !store.inner.root.join("python-tool-harness.py").exists(),
            "no shared writable harness path exists across executor generations"
        );
        drop(recorded);

        // The relative write never reached the canonical source.
        assert!(
            !published.root.join("runtime-cache.txt").exists(),
            "no tool-created file appears in the canonical published source"
        );
        // The self-modification never reached the canonical source.
        assert_eq!(
            std::fs::read(published.root.join("tool.py")).expect("canonical tool.py"),
            original_tool_bytes,
            "the canonical tool.py bytes are unchanged"
        );
        // The recomputed canonical digest still matches the committed
        // identity.
        assert_eq!(
            super::published_source_digest(&published.root).expect("canonical digest"),
            package.tool_version_id,
            "the published ToolVersion identity is stable after execution"
        );
        // The invocation-private materialization was settled.
        assert_eq!(
            std::fs::read_dir(&invocation_root)
                .expect("invocation root")
                .count(),
            0,
            "the invocation-private directory is removed after the execution"
        );
    }

    /// A gated execution runner: the first invocation records its command
    /// and cwd, signals that its execution bundle is live, then parks until
    /// the test releases it; every later invocation passes through. Both
    /// answer a valid success envelope. All synchronization is explicit —
    /// no sleeps.
    #[derive(Default)]
    struct FirstParkingRunner {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        seen: std::sync::atomic::AtomicUsize,
        commands: Mutex<Vec<(String, PathBuf)>>,
    }

    impl SupervisedProcessRunner for FirstParkingRunner {
        fn run(
            &self,
            spec: SupervisedCommandSpec,
            _control: Option<crate::runtime::process_runner::RunnerTestControl>,
        ) -> BoxFuture<'_, Result<CapturedProcessResult, String>> {
            self.commands
                .lock()
                .expect("recorded commands lock")
                .push((spec.command.clone(), spec.cwd.clone()));
            let first = self.seen.fetch_add(1, std::sync::atomic::Ordering::AcqRel) == 0;
            Box::pin(async move {
                if first {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                Ok(CapturedProcessResult {
                    exit_code: Some(0),
                    intent: ProcessOutcomeIntent::Completed,
                    stdout: br#"{"ok":true,"value":"ok"}"#.to_vec(),
                    stderr: Vec::new(),
                })
            })
        }
    }

    /// Invocation ownership across executor generations (Issue #81): an
    /// invocation from an older executor generation stays live — the exact
    /// condition of a detached background execution outliving its
    /// attempt's capability revision — while a new generation (what a
    /// capability refresh derives from the coordinator's one stable store)
    /// executes concurrently.
    ///
    /// This is the same lifetime condition as the
    /// `ConversationBackgroundRegistry` path — a detached execution holds
    /// its `Arc<dyn ToolExecutor>` beyond the attempt, and the next
    /// preparation constructs new executors — exercised here at the level
    /// where the ownership domain lives (the store's allocation domain),
    /// without mocking the ownership away: both executors allocate real
    /// bundles from the real store.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_live_older_generation_bundle_is_never_reused_or_deleted() {
        let runner = Arc::new(FirstParkingRunner::default());
        let dir = tempfile::tempdir().expect("temp dir");
        let package = test_package();
        let store =
            PythonToolStore::with_runner(dir.path().join("store"), runner.clone()).expect("store");
        let published = store.publish(&package).expect("publish");
        let environment = crate::tools::python::PythonToolEnvironment {
            digest: crate::runtime::identity::PythonToolEnvironmentDigest::new(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_owned(),
            ),
            root: dir.path().join("env"),
        };
        std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            crate::runtime::identity::ConversationId::new("conv-python-generations"),
            dir.path().join("workspace"),
            dir.path().join("artifacts"),
        )
        .expect("tool runtime");

        // R1: the older executor generation; its invocation is parked with
        // its execution bundle live.
        let executor_r1 = crate::tools::python::PythonToolExecutor::new(
            &store,
            published.clone(),
            environment.clone(),
        );
        let r1_runtime = tool_runtime.clone();
        let r1_task = tokio::spawn(async move { execute_once(&executor_r1, &r1_runtime).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            runner.entered.notified(),
        )
        .await
        .expect("the R1 execution bundle is live");
        let r1_cwd = runner.commands.lock().expect("commands lock")[0].1.clone();
        let r1_bundle = r1_cwd.parent().expect("R1 bundle").to_path_buf();
        assert!(r1_bundle.join("source/tool.py").is_file());
        assert!(r1_bundle.join("harness.py").is_file());
        assert!(r1_bundle.join("input.json").is_file());

        // R2: a new executor generation over the same stable store — what
        // the next capability preparation derives while R1 runs.
        let executor_r2 =
            crate::tools::python::PythonToolExecutor::new(&store, published.clone(), environment);
        let r2 = execute_once(&executor_r2, &tool_runtime).await;
        assert!(
            matches!(r2.status, crate::tools::types::ToolExecutionStatus::Success),
            "R2 executed: {:?}",
            r2.status
        );
        let r2_cwd = runner.commands.lock().expect("commands lock")[1].1.clone();
        let r2_bundle = r2_cwd.parent().expect("R2 bundle").to_path_buf();
        assert_ne!(
            r1_bundle, r2_bundle,
            "two executor generations never claim the same bundle"
        );
        assert!(
            r1_bundle.join("source/tool.py").is_file(),
            "R2 never deleted R1's live bundle"
        );
        assert!(!r2_bundle.exists(), "R2 settled its own bundle");

        // Releasing R1 settles it independently and removes only its own
        // bundle; the canonical published source is untouched throughout.
        runner.release.notify_one();
        let r1 = tokio::time::timeout(std::time::Duration::from_secs(60), r1_task)
            .await
            .expect("R1 settles")
            .expect("R1 task");
        assert!(matches!(
            r1.status,
            crate::tools::types::ToolExecutionStatus::Success
        ));
        assert!(!r1_bundle.exists(), "R1 settled its own bundle");
        assert_eq!(
            super::published_source_digest(&published.root).expect("canonical digest"),
            package.tool_version_id,
            "the canonical ToolVersion identity is unchanged across both generations"
        );
    }

    /// Namespace separation (Issue #81): a package-owned `input.json` and
    /// `harness.py` are canonical `ToolVersion` content copied into the
    /// bundle's `source/`; the runtime-owned harness and arguments live
    /// outside `source/` and never overwrite package content.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn package_owned_runtime_named_files_are_never_overwritten() {
        /// Inspects the live execution bundle during the run: the
        /// package-owned files must be intact inside `source/`, and the
        /// runtime-owned files must exist beside it with runtime content.
        struct BundleInspector;
        impl SupervisedProcessRunner for BundleInspector {
            fn run(
                &self,
                spec: SupervisedCommandSpec,
                _control: Option<crate::runtime::process_runner::RunnerTestControl>,
            ) -> BoxFuture<'_, Result<CapturedProcessResult, String>> {
                let source = spec.cwd.clone();
                let bundle = source.parent().expect("bundle").to_path_buf();
                assert_eq!(
                    std::fs::read(source.join("input.json")).expect("packaged input"),
                    br#"{"packaged": true}"#,
                    "the package-owned input.json reached the invocation source copy intact"
                );
                assert_eq!(
                    std::fs::read(source.join("harness.py")).expect("packaged harness"),
                    b"# package-owned harness\n",
                    "the package-owned harness.py reached the invocation source copy intact"
                );
                let runtime_input =
                    std::fs::read(bundle.join("input.json")).expect("runtime arguments");
                assert_ne!(
                    runtime_input, br#"{"packaged": true}"#,
                    "the runtime-owned arguments are a different file outside source/"
                );
                let runtime_harness =
                    std::fs::read_to_string(bundle.join("harness.py")).expect("runtime harness");
                assert!(
                    runtime_harness.contains("dont_write_bytecode"),
                    "the runtime-owned harness is the current harness bytes"
                );
                assert!(
                    spec.command
                        .contains(&bundle.join("harness.py").display().to_string()),
                    "the executed harness is the bundle's runtime-owned one"
                );
                Box::pin(async move {
                    Ok(CapturedProcessResult {
                        exit_code: Some(0),
                        intent: ProcessOutcomeIntent::Completed,
                        stdout: br#"{"ok":true,"value":"ok"}"#.to_vec(),
                        stderr: Vec::new(),
                    })
                })
            }
        }

        let mut package = test_package();
        package.files.push((
            PathBuf::from("input.json"),
            br#"{"packaged": true}"#.to_vec(),
        ));
        package.files.push((
            PathBuf::from("harness.py"),
            b"# package-owned harness\n".to_vec(),
        ));
        package.files.sort_by(|left, right| left.0.cmp(&right.0));
        package.tool_version_id = super::tool_version_id(&package.files);
        let dir = tempfile::tempdir().expect("temp dir");
        let (store, published, executor, tool_runtime) =
            executor_fixture(&dir, Arc::new(BundleInspector), &package);

        let result = execute_once(&executor, &tool_runtime).await;
        assert!(matches!(
            result.status,
            crate::tools::types::ToolExecutionStatus::Success
        ));
        // The canonical published package-owned files are byte-identical
        // and the identity still validates.
        assert_eq!(
            std::fs::read(published.root.join("input.json")).expect("canonical input"),
            br#"{"packaged": true}"#
        );
        assert_eq!(
            std::fs::read(published.root.join("harness.py")).expect("canonical harness"),
            b"# package-owned harness\n"
        );
        assert_eq!(
            super::published_source_digest(&published.root).expect("canonical digest"),
            package.tool_version_id
        );
        let _ = store;
    }

    /// Stale scratch safety (Issue #81): after a process restart the
    /// allocator may start from zero while unknown `execution-N/`
    /// directories remain. An existing path is stale scratch of unknown
    /// ownership — the allocator skips it and never deletes or reuses it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_scratch_from_a_previous_process_is_never_reused_or_deleted() {
        let runner = Arc::new(ToolWriteSimulatingRunner::default());
        let dir = tempfile::tempdir().expect("temp dir");
        let package = test_package();
        let (store, _published, executor, tool_runtime) =
            executor_fixture(&dir, runner.clone(), &package);
        let stale = store.inner.root.join("python-invocations/execution-0");
        std::fs::create_dir_all(&stale).expect("stale scratch");
        std::fs::write(stale.join("marker.txt"), b"unknown ownership").expect("marker");

        let result = execute_once(&executor, &tool_runtime).await;
        assert!(matches!(
            result.status,
            crate::tools::types::ToolExecutionStatus::Success
        ));
        let (_, cwd) = runner.commands.lock().expect("commands lock")[0].clone();
        assert!(
            !cwd.starts_with(&stale),
            "the allocator skipped the stale scratch directory: {cwd:?}"
        );
        assert!(
            stale.join("marker.txt").is_file(),
            "the unknown stale scratch was never deleted"
        );
    }

    /// Terminal exhaustion (Issue #81): once the allocator reaches
    /// exhaustion it remains exhausted forever for this store identity —
    /// the counter never transitions `MAX -> 0`, so no later call can
    /// succeed, wrap, or recreate `execution-0`.
    #[test]
    fn allocator_exhaustion_is_absorbing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        store
            .inner
            .next_invocation
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);

        for attempt in ["first", "second"] {
            let Err(super::PythonToolError::Storage(reason)) = store.allocate_execution_bundle()
            else {
                panic!("the {attempt} allocation must fail as exhausted");
            };
            assert!(
                reason.contains("identifier space is exhausted"),
                "the {attempt} allocation reports exhaustion: {reason}"
            );
        }
        assert_eq!(
            store
                .inner
                .next_invocation
                .load(std::sync::atomic::Ordering::Relaxed),
            u64::MAX,
            "the counter stays at MAX: exhaustion is absorbing"
        );
        assert!(
            !store
                .inner
                .root
                .join("python-invocations/execution-0")
                .exists(),
            "no lower-numbered bundle was ever created"
        );
    }

    /// The last valid identifier still allocates, then every later attempt
    /// fails — the terminal transition is `MAX - 1 -> MAX`, never a wrap.
    #[test]
    fn last_identifier_then_exhaustion_never_wraps() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        store
            .inner
            .next_invocation
            .store(u64::MAX - 1, std::sync::atomic::Ordering::Relaxed);

        let bundle = store
            .allocate_execution_bundle()
            .expect("the last valid identifier still allocates");
        assert_eq!(
            bundle,
            store
                .inner
                .root
                .join("python-invocations/execution-18446744073709551614"),
            "the final identifier names its bundle"
        );

        for attempt in ["second", "third"] {
            let Err(super::PythonToolError::Storage(reason)) = store.allocate_execution_bundle()
            else {
                panic!("the {attempt} allocation must fail as exhausted");
            };
            assert!(
                reason.contains("identifier space is exhausted"),
                "the {attempt} allocation reports exhaustion: {reason}"
            );
        }
        assert_eq!(
            store
                .inner
                .next_invocation
                .load(std::sync::atomic::Ordering::Relaxed),
            u64::MAX
        );
        assert!(
            !store
                .inner
                .root
                .join("python-invocations/execution-0")
                .exists(),
            "the allocator never wrapped to execution-0"
        );
        std::fs::remove_dir_all(&bundle).expect("remove the bundle this test owns");
    }

    /// Stale scratch and terminal exhaustion compose: skipping an
    /// already-existing final identifier consumes the last claim, and the
    /// next checked claim observes `MAX` and fails — never a wrap.
    #[test]
    fn stale_last_identifier_does_not_wrap() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        let stale = store
            .inner
            .root
            .join("python-invocations/execution-18446744073709551614");
        std::fs::create_dir_all(&stale).expect("stale final-identifier scratch");
        std::fs::write(stale.join("marker.txt"), b"unknown ownership").expect("marker");
        store
            .inner
            .next_invocation
            .store(u64::MAX - 1, std::sync::atomic::Ordering::Relaxed);

        let Err(super::PythonToolError::Storage(reason)) = store.allocate_execution_bundle() else {
            panic!("allocation past the stale final identifier must fail as exhausted");
        };
        assert!(
            reason.contains("identifier space is exhausted"),
            "the skip loop ends in explicit exhaustion: {reason}"
        );
        assert_eq!(
            store
                .inner
                .next_invocation
                .load(std::sync::atomic::Ordering::Relaxed),
            u64::MAX
        );
        assert!(
            !store
                .inner
                .root
                .join("python-invocations/execution-0")
                .exists(),
            "stale-scratch skipping never wrapped to execution-0"
        );
        assert!(
            stale.join("marker.txt").is_file(),
            "the stale scratch was never deleted"
        );
    }

    /// Reopen/restart validation (Issue #81): after a real execution the
    /// store instance is dropped, a fresh store over the same root
    /// revalidates the persisted `ToolVersion` from its bytes alone, and
    /// the identity is unchanged — execution left no drift.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn execution_then_store_reopen_revalidates_the_same_tool_version_identity() {
        let runner = Arc::new(ToolWriteSimulatingRunner::default());
        let dir = tempfile::tempdir().expect("temp dir");
        let package = test_package();
        let root = dir.path().join("store");
        let published_root = {
            let store = PythonToolStore::with_runner(root.clone(), runner).expect("first store");
            let published = store.publish(&package).expect("publish");
            let environment = crate::tools::python::PythonToolEnvironment {
                digest: crate::runtime::identity::PythonToolEnvironmentDigest::new(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .to_owned(),
                ),
                root: dir.path().join("env"),
            };
            let executor = crate::tools::python::PythonToolExecutor::new(
                &store,
                published.clone(),
                environment,
            );
            std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace");
            let tool_runtime = crate::tools::runtime::ConversationToolRuntime::new(
                crate::runtime::identity::ConversationId::new("conv-python-reopen"),
                dir.path().join("workspace"),
                dir.path().join("artifacts"),
            )
            .expect("tool runtime");
            let result = execute_once(&executor, &tool_runtime).await;
            assert!(matches!(
                result.status,
                crate::tools::types::ToolExecutionStatus::Success
            ));
            published.root
            // `store` and `executor` drop here: no in-memory handle remains.
        };
        let reopened =
            PythonToolStore::with_runner(root, Arc::new(ToolWriteSimulatingRunner::default()))
                .expect("reopened store");
        let republished = reopened
            .publish(&package)
            .expect("reuse must revalidate the persisted source after execution");
        assert_eq!(republished.root, published_root);
        assert_eq!(
            republished.package.tool_version_id, package.tool_version_id,
            "the persisted ToolVersion identity is unchanged across execution and reopen"
        );
    }

    /// The end-to-end execution architecture with a real interpreter
    /// (Issue #81): a tool that writes `runtime-cache.txt` relative to its
    /// cwd and rewrites `__file__` executes successfully against its
    /// invocation-private materialization while the canonical published
    /// source stays byte-identical.
    ///
    /// Opt-in by availability, mirroring the `m7_uv` pattern: without a
    /// real `python3` the acceptance is not exercised.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_execution_materializes_an_invocation_private_copy() {
        let python = super::resolve_executable("python3");
        if !python.is_file() {
            eprintln!("python3 unavailable; the real execution materialization is not exercised");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let mut package = test_package();
        let tool_source = b"from pathlib import Path\n\ndef main(arguments):\n    Path(\"runtime-cache.txt\").write_text(\"changed\")\n    Path(__file__).write_text(\"def main(arguments):\\n    return 'tampered'\\n\")\n    return \"ok\"\n".to_vec();
        for (path, bytes) in &mut package.files {
            if path == &PathBuf::from("tool.py") {
                *bytes = tool_source.clone();
            }
        }
        package.tool_version_id = super::tool_version_id(&package.files);
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        let published = store.publish(&package).expect("publish");
        let environment_root = dir.path().join("env");
        std::fs::create_dir_all(environment_root.join("bin")).expect("env bin");
        std::os::unix::fs::symlink(&python, environment_root.join("bin/python"))
            .expect("interpreter link");
        let executor = crate::tools::python::PythonToolExecutor::new(
            &store,
            published.clone(),
            crate::tools::python::PythonToolEnvironment {
                digest: crate::runtime::identity::PythonToolEnvironmentDigest::new(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .to_owned(),
                ),
                root: environment_root,
            },
        );
        std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            crate::runtime::identity::ConversationId::new("conv-python-real"),
            dir.path().join("workspace"),
            dir.path().join("artifacts"),
        )
        .expect("tool runtime");

        let result = execute_once(&executor, &tool_runtime).await;
        assert!(
            matches!(
                result.status,
                crate::tools::types::ToolExecutionStatus::Success
            ),
            "the real tool executed: {:?}",
            result.status
        );
        assert!(
            !published.root.join("runtime-cache.txt").exists(),
            "the relative write stayed inside the invocation-private copy"
        );
        assert_eq!(
            std::fs::read(published.root.join("tool.py")).expect("canonical tool.py"),
            tool_source,
            "the __file__ self-modification stayed inside the invocation-private copy"
        );
        assert_eq!(
            super::published_source_digest(&published.root).expect("canonical digest"),
            package.tool_version_id,
            "the canonical ToolVersion identity survived a real mutating execution"
        );
    }

    /// The end-to-end namespace separation with a real interpreter (Issue
    /// #81): a tool whose package ships its own `input.json` reads exactly
    /// its packaged bytes from its working directory, while the
    /// runtime-owned invocation arguments live outside `source/`.
    ///
    /// Opt-in by availability, mirroring the `m7_uv` pattern: without a
    /// real `python3` the acceptance is not exercised.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_execution_reads_the_packaged_input_json_from_source() {
        let python = super::resolve_executable("python3");
        if !python.is_file() {
            eprintln!("python3 unavailable; the packaged-input acceptance is not exercised");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let mut package = test_package();
        let tool_source = b"from pathlib import Path\n\ndef main(arguments):\n    return {\"packaged\": Path(\"input.json\").read_text(), \"arguments\": arguments}\n".to_vec();
        for (path, bytes) in &mut package.files {
            if path == &PathBuf::from("tool.py") {
                *bytes = tool_source.clone();
            }
        }
        package.files.push((
            PathBuf::from("input.json"),
            br#"{"packaged": true}"#.to_vec(),
        ));
        package.files.sort_by(|left, right| left.0.cmp(&right.0));
        package.tool_version_id = super::tool_version_id(&package.files);
        let store = PythonToolStore::new(dir.path().join("store")).expect("store");
        let published = store.publish(&package).expect("publish");
        let environment_root = dir.path().join("env");
        std::fs::create_dir_all(environment_root.join("bin")).expect("env bin");
        std::os::unix::fs::symlink(&python, environment_root.join("bin/python"))
            .expect("interpreter link");
        let executor = crate::tools::python::PythonToolExecutor::new(
            &store,
            published.clone(),
            crate::tools::python::PythonToolEnvironment {
                digest: crate::runtime::identity::PythonToolEnvironmentDigest::new(
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .to_owned(),
                ),
                root: environment_root,
            },
        );
        std::fs::create_dir_all(dir.path().join("workspace")).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::new(
            crate::runtime::identity::ConversationId::new("conv-python-packaged-input"),
            dir.path().join("workspace"),
            dir.path().join("artifacts"),
        )
        .expect("tool runtime");

        let result = execute_once(&executor, &tool_runtime).await;
        let crate::tools::types::ToolExecutionStatus::Success = result.status else {
            panic!("the real tool executed: {:?}", result.status);
        };
        let Some(crate::tools::types::ToolResultContent::Json { value }) = result.content.first()
        else {
            panic!("the tool returned a JSON result: {:?}", result.content);
        };
        assert_eq!(
            value.get("packaged").and_then(serde_json::Value::as_str),
            Some(r#"{"packaged": true}"#),
            "the tool read its own packaged input.json, not the runtime arguments: {value}"
        );
        assert_eq!(
            value.get("arguments"),
            Some(&serde_json::json!({})),
            "the runtime-owned arguments arrived separately: {value}"
        );
        assert_eq!(
            std::fs::read(published.root.join("input.json")).expect("canonical input"),
            br#"{"packaged": true}"#,
            "the canonical package-owned input.json is byte-identical"
        );
        assert_eq!(
            super::published_source_digest(&published.root).expect("canonical digest"),
            package.tool_version_id
        );
    }

    /// A timeout on a package-manager command is an explicit preparation
    /// failure.
    #[tokio::test]
    async fn timed_out_probe_is_an_explicit_preparation_failure() {
        let scripted = Arc::new(ScriptedRunner::new(Vec::new()));
        scripted.set_probe_timeout();
        scripted.release_gate();
        let (_dir, store) = store_with(scripted);
        let published = store.publish(&test_package()).expect("publish");
        let error = store
            .ensure_environment(&published)
            .await
            .expect_err("a timed-out probe must fail preparation");
        assert!(error.to_string().contains("timed out"));
    }

    /// Stable identity (Issue #81): the one canonical derivation produces
    /// exactly the same `ToolVersionId` for the same canonical source
    /// bytes, and any accepted source-content change produces a different
    /// identity.
    #[test]
    fn tool_version_identity_is_stable_and_content_sensitive() {
        let files = test_package().files;
        let first = super::tool_version_id(&files);
        let second = super::tool_version_id(&files);
        assert_eq!(
            first, second,
            "identical canonical source bytes derive an identical identity"
        );
        // The canonical representation is the path-sorted file list: the
        // collector sorts before deriving, so a reordered discovery reads
        // the same canonical bytes.
        let mut reordered = files.clone();
        reordered.reverse();
        reordered.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(first, super::tool_version_id(&reordered));

        let mut changed = files.clone();
        let tool = changed
            .iter_mut()
            .find(|(path, _)| path == &PathBuf::from("tool.py"))
            .expect("tool.py");
        tool.1.extend_from_slice(b"# changed\n");
        assert_ne!(
            first,
            super::tool_version_id(&changed),
            "an accepted source-content change must change the identity"
        );
        let mut renamed = files.clone();
        renamed[3].0 = PathBuf::from("renamed.py");
        assert_ne!(
            first,
            super::tool_version_id(&renamed),
            "a relative-path change must change the identity"
        );
    }

    /// Publication round-trip (Issue #81): a `ToolVersion` published by one
    /// store instance revalidates — from the persisted representation, not
    /// from in-memory state — under a fresh store over the same root, with
    /// the same identity.
    #[test]
    fn published_tool_version_revalidates_identically_across_a_store_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("store");
        let package = test_package();
        let first = PythonToolStore::new(root.clone())
            .expect("first store")
            .publish(&package)
            .expect("first publish");
        assert_eq!(first.package.tool_version_id, package.tool_version_id);
        // The store instance is dropped: the reuse path must validate the
        // persisted bytes alone.
        let reopened = PythonToolStore::new(root).expect("reopened store");
        let second = reopened
            .publish(&package)
            .expect("reuse must revalidate the persisted source");
        assert_eq!(second.root, first.root);
        assert_eq!(second.package.tool_version_id, package.tool_version_id);
    }

    /// The harness never mutates the source root it is given (Issue #81):
    /// importing the tool module must not write `__pycache__` bytecode
    /// caches. The executor additionally never hands the harness the
    /// canonical published root — it passes an invocation-private
    /// materialization — so this test is the second line of defense: even
    /// the copy must stay free of interpreter cache writes.
    ///
    /// Opt-in by availability, mirroring the `m7_uv` pattern: without a
    /// real `python3` the acceptance is not exercised.
    #[test]
    fn the_harness_leaves_the_published_source_root_pristine() {
        let python = super::resolve_executable("python3");
        if !python.is_file() {
            eprintln!("python3 unavailable; harness pristineness not exercised");
            return;
        }
        let dir = tempfile::tempdir().expect("temp dir");
        let source = dir.path().join("source");
        std::fs::create_dir_all(&source).expect("source dir");
        std::fs::write(
            source.join("tool.py"),
            "def main(arguments):\n    return {\"echo\": arguments}\n",
        )
        .expect("tool source");
        let harness = dir.path().join("harness.py");
        std::fs::write(&harness, super::PYTHON_HARNESS).expect("harness");
        let input = dir.path().join("input.json");
        std::fs::write(&input, br#"{"question": 42}"#).expect("input");
        let output = std::process::Command::new(&python)
            .arg(&harness)
            .arg(&source)
            .arg("tool:main")
            .arg(&input)
            .current_dir(&source)
            .output()
            .expect("run the harness");
        assert!(
            output.status.success(),
            "harness failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("harness stdout");
        assert!(
            stdout.contains("\"ok\":true"),
            "the tool executed through the harness: {stdout}"
        );
        let entries = super::collect_files(&source).expect("source files");
        assert_eq!(
            entries
                .iter()
                .map(|(path, _)| path.display().to_string())
                .collect::<Vec<_>>(),
            ["tool.py"],
            "the published source root must remain exactly the ToolVersion content"
        );
    }
}
