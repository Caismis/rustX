//! Immutable custom Python `ToolVersion`s and their canonical executor.
//!
//! A package is discovered from exactly one Workspace-local root. Discovery
//! reads every package-owned byte before capability commit, publishes that
//! finite snapshot into the private runtime store, and only then constructs
//! the executor. Workspace edits therefore cannot change an old foreground or
//! detached background call.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use sha2::Digest;

use crate::runtime::identity::{PythonToolEnvironmentDigest, ToolVersionId};
use crate::runtime::process_runner::{
    CapturedProcessResult, ProcessOutcomeIntent, RunnerBackedProcessRunner, SupervisedCommandSpec,
    SupervisedProcessRunner,
};
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
const ENVIRONMENT_MARKER: &str = "RUSTX_ENV_MANIFEST.json";

const PYTHON_HARNESS: &str = r#"import contextlib
import importlib
import json
import pathlib
import sys

source_root = pathlib.Path(sys.argv[1]).resolve()
entrypoint = sys.argv[2]
input_path = pathlib.Path(sys.argv[3]).resolve()
sys.path.insert(0, str(source_root))

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
    /// Private immutable source path.
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

#[derive(Debug)]
struct BuildCell {
    result: Mutex<Option<Result<PythonToolEnvironment, PythonToolError>>>,
    notify: tokio::sync::Notify,
}

#[derive(Clone)]
struct PythonToolStoreInner {
    root: PathBuf,
    runner: Arc<dyn SupervisedProcessRunner>,
    uv_binary: PathBuf,
    python_binary: PathBuf,
    in_flight: Arc<tokio::sync::Mutex<BTreeMap<String, Arc<BuildCell>>>>,
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
        Ok(Self {
            inner: Arc::new(PythonToolStoreInner {
                root,
                runner: Arc::new(RunnerBackedProcessRunner::default()),
                uv_binary: resolve_executable("uv"),
                python_binary: resolve_executable("python3"),
                in_flight: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
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
        Ok(Self {
            inner: Arc::new(PythonToolStoreInner {
                root,
                runner,
                uv_binary: resolve_executable("uv"),
                python_binary: resolve_executable("python3"),
                in_flight: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
                next_invocation: Arc::new(AtomicU64::new(0)),
            }),
        })
    }

    /// Publishes exact package bytes by content identity.
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
        if destination.exists() {
            let marker = destination.join(TOOL_VERSION_MARKER);
            let valid = marker.is_file()
                && serde_json::from_slice::<serde_json::Value>(
                    &std::fs::read(&marker).map_err(io_error)?,
                )
                .ok()
                .and_then(|value| {
                    value
                        .get("tool_version_id")
                        .and_then(serde_json::Value::as_str)
                        .map(|value| value == package.tool_version_id.as_str())
                }) == Some(true);
            if !valid {
                return Err(PythonToolError::Storage(
                    "published ToolVersion marker is invalid".to_owned(),
                ));
            }
            return Ok(PublishedPythonTool {
                package: package.clone(),
                root: destination,
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
        std::fs::create_dir_all(&staging).map_err(io_error)?;
        for (relative, bytes) in &package.files {
            let path = staging.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io_error)?;
            }
            std::fs::write(path, bytes).map_err(io_error)?;
        }
        let marker = serde_json::json!({
            "format": 1,
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
            root: destination,
        })
    }

    /// Ensures one immutable environment, coalescing concurrent same-digest
    /// builds behind one store-owned task.
    ///
    /// # Errors
    ///
    /// Returns an error if runtime probes, lock validation, frozen
    /// materialization, validation, or publication fails.
    ///
    /// # Panics
    ///
    /// Panics only if the store's internal synchronization lock is poisoned.
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
        if let Some(environment) = Self::read_published_environment(&final_root, &digest)? {
            return Ok(environment);
        }
        let (cell, leader) = {
            let mut builds = self.inner.in_flight.lock().await;
            if let Some(cell) = builds.get(digest.as_str()) {
                (cell.clone(), false)
            } else {
                let cell = Arc::new(BuildCell {
                    result: Mutex::new(None),
                    notify: tokio::sync::Notify::new(),
                });
                builds.insert(digest.as_str().to_owned(), cell.clone());
                (cell, true)
            }
        };
        if leader {
            let inner = self.inner.clone();
            let tool = tool.clone();
            let digest_clone = digest.clone();
            let cell_clone = cell.clone();
            tokio::spawn(async move {
                let result = materialize_environment(
                    &inner,
                    &tool,
                    &final_root,
                    &digest_clone,
                    &python_runtime,
                    &uv_identity,
                )
                .await;
                *cell_clone.result.lock().expect("Python build cell lock") = Some(result);
                cell_clone.notify.notify_waiters();
                inner.in_flight.lock().await.remove(digest_clone.as_str());
            });
        }
        loop {
            if let Some(result) = cell.result.lock().expect("Python build cell lock").clone() {
                return result;
            }
            cell.notify.notified().await;
        }
    }

    fn read_published_environment(
        root: &Path,
        digest: &PythonToolEnvironmentDigest,
    ) -> Result<Option<PythonToolEnvironment>, PythonToolError> {
        let marker = root.join(ENVIRONMENT_MARKER);
        if !marker.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&marker).map_err(io_error)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            PythonToolError::Environment(format!("invalid environment marker: {error}"))
        })?;
        if value.get("digest").and_then(serde_json::Value::as_str) != Some(digest.as_str())
            || value.get("ready").and_then(serde_json::Value::as_bool) != Some(true)
        {
            return Err(PythonToolError::Environment(
                "published environment marker is corrupt".to_owned(),
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
                        timeout: None,
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
        if result.exit_code != Some(0) || !matches!(result.intent, ProcessOutcomeIntent::Completed)
        {
            return Err(PythonToolError::Environment(format!(
                "{command} failed: {}",
                bounded_output(&result.stderr)
            )));
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
                    timeout: None,
                    cancellation: crate::runtime::CancellationSignal::new(),
                },
                None,
            )
            .await
            .map_err(PythonToolError::Environment)?;
        if result.exit_code != Some(0) || !matches!(result.intent, ProcessOutcomeIntent::Completed)
        {
            return Err(PythonToolError::Environment(format!(
                "{command} failed: {}",
                bounded_output(&result.stderr)
            )));
        }
    }
    let marker = serde_json::json!({
        "ready": true,
        "format": 1,
        "digest": digest,
        "tool_version_id": tool.package.tool_version_id,
        "lock_digest": format!("sha256:{}", hex_digest(&sha2::Sha256::digest(&tool.package.uv_lock))),
        "python_runtime": python_runtime,
        "uv": uv_identity,
    });
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

/// Canonical Python executor using the immutable `ToolVersion` source and
/// environment handles captured at candidate preparation.
pub struct PythonToolExecutor {
    tool: PublishedPythonTool,
    environment: PythonToolEnvironment,
    runner: Arc<dyn SupervisedProcessRunner>,
    harness: PathBuf,
    invocation_root: PathBuf,
    next_invocation: Arc<AtomicU64>,
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
    /// # Errors
    ///
    /// Returns an error if the harness or invocation directory cannot be
    /// installed.
    pub fn new(
        store: &PythonToolStore,
        tool: PublishedPythonTool,
        environment: PythonToolEnvironment,
    ) -> Result<Self, PythonToolError> {
        let harness = store.inner.root.join("python-tool-harness.py");
        if !harness.exists() {
            std::fs::write(&harness, PYTHON_HARNESS).map_err(io_error)?;
        }
        let invocation_root = store.inner.root.join("python-invocations");
        std::fs::create_dir_all(&invocation_root).map_err(io_error)?;
        Ok(Self {
            tool,
            environment,
            runner: store.inner.runner.clone(),
            harness,
            invocation_root,
            next_invocation: store.inner.next_invocation.clone(),
        })
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
            let number = self.next_invocation.fetch_add(1, Ordering::Relaxed);
            let input_path = self.invocation_root.join(format!("input-{number}.json"));
            if let Err(error) = write_private_input(&input_path, &invocation.arguments) {
                return failed_python(&error.to_string(), started);
            }
            let python = self.environment.root.join("bin/python");
            let command = format!(
                "{} {} {} {}",
                shell_quote(&python),
                shell_quote(&self.harness),
                shell_quote(&self.tool.root),
                shell_quote_str(&self.tool.package.entrypoint),
            ) + &format!(" {}", shell_quote(&input_path));
            let runtime_environment = context
                .environment
                .with_replacement_overlay(&ToolEnvironmentOverlay::python(&self.environment.root));
            let result = self
                .runner
                .run(
                    SupervisedCommandSpec {
                        command,
                        cwd: self.tool.root.clone(),
                        environment: runtime_environment
                            .child_environment(context.workspace.root()),
                        timeout: None,
                        cancellation: context.cancellation.clone(),
                    },
                    None,
                )
                .await;
            let _ = std::fs::remove_file(&input_path);
            match result {
                Ok(result) => translate_python_result(result, started),
                Err(error) => failed_python(&error, started),
            }
        })
    }
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

fn translate_python_result(result: CapturedProcessResult, started: Instant) -> ToolExecutionResult {
    let status = match result.intent {
        ProcessOutcomeIntent::Cancelled => ToolExecutionStatus::Cancelled {
            reason: crate::runtime::types::CancellationReason::UserRequested,
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
    let Some(path) = std::env::var_os("PATH") else {
        return PathBuf::from(name);
    };
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
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
    use super::python_tool_environment_digest;

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
}
