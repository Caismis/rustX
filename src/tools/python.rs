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
//! ├── permissions.toml                     OPTIONAL resource permissions
//! │                                        (`permissions` + `tools` map;
//! │                                        same shape as MCP server docs)
//! └── *.py                                 supporting modules
//! ```
//!
//! The admitted Execution Client freezes the source, verifies and prepares
//! the private Store, and launches the virtual environment interpreter over
//! the generic stdio MCP path. Server snapshots retain the source identity,
//! digest, and authored permissions for authorization and canonical results.
//! The Server can also inspect authoring declarations without preparing the
//! environment. Physical package and environment fixtures compile only for
//! Rust tests.
//!
//! The managed `FastMCP` build is pinned to exactly one version
//! ([`MANAGED_FASTMCP_VERSION`]) and a package may never declare `fastmcp`
//! itself: the dependency identity of the MCP wire implementation is
//! rustX-owned, so a workspace edit cannot silently change the protocol
//! peer.
//!
//! # Wire ownership
//!
//! Package stdout is protocol-owned — it is the MCP wire and nothing
//! else — and stderr is the supported diagnostics channel. A package whose
//! stdout violates the protocol is diagnosed by the **generic** MCP
//! runtime's corruption observation seam through the package's synthesized
//! `python:<folder>` MCP server identity; this module contains no
//! Python-specific framing, parsing, or recovery.

use std::collections::BTreeMap;
#[cfg(any(test, feature = "test-support"))]
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "test-support"))]
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
#[cfg(any(test, feature = "test-support"))]
use sha2::Digest;

use crate::runtime::identity::McpServerId;
use crate::tools::mcp::{McpServerBinding, McpTransportConfig};
use crate::tools::types::ToolInvocationPolicy;

#[cfg(any(test, feature = "test-support"))]
mod fixture_store;
#[cfg(any(test, feature = "test-support"))]
mod requirements;
#[cfg(any(test, feature = "test-support"))]
pub use fixture_store::{PreparedPythonPackage, PythonToolStore};

/// The exact `FastMCP` build every managed Python package is prepared with.
///
/// Pinned exactly: the `FastMCP` SDK is rustX's MCP protocol peer, so its
/// identity is a fingerprint input, never a workspace-declared dependency.
/// One version, never a range and never a workspace-selectable one: rustX
/// owns the wire implementation, so there is no `FastMCP` mode, fallback, or
/// compatibility shim to choose between.
///
/// Verified against the rustX MCP client (rmcp) before pinning (Issue #241):
/// a real `FastMCP` 4.0.3 stdio child answers the modern `server/discover`
/// probe advertising `supportedVersions: ["2026-07-28"]`, so the generic
/// rustX MCP connect negotiates the `2026-07-28` revision inline and serves
/// `tools/list`/`tools/call` on it. That evidence is a property of the
/// *connection*, not of this constant: it is asserted from the negotiated
/// revision of a real child in `tests/tools/mcp_managed.rs`. rustX's generic
/// negotiated fallback to genuinely older external peers is unchanged and
/// unrelated to this pin.
pub const MANAGED_FASTMCP_VERSION: &str = "4.0.3";

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
/// Optional package-local resource permission document.
///
/// Same authoring shape as an MCP server permissions document: a source-default
/// [`ResourcePermissionSet`](crate::runtime::permission::ResourcePermissionSet) under `permissions`, plus exact-tool exceptions
/// under `tools.<name>.permissions`. Absent means product/matrix defaults.
pub const PERMISSIONS_FILE: &str = "permissions.toml";

/// A package discovery/preparation failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PythonToolError {
    /// The package is malformed.
    #[error("invalid Python tool package: {0}")]
    InvalidPackage(String),
    /// Prepared state could not be read or published.
    #[error("Python tool storage failed: {0}")]
    Storage(String),
    /// The dependency environment could not be checked or materialized.
    #[error("Python tool environment failed: {0}")]
    Environment(String),
}

/// Package-local permission authoring document (`permissions.toml`).
///
/// Mirrors the MCP server permissions document surface: source-default
/// `permissions` plus exact-tool exceptions under `tools`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct ManagedPythonPermissionsDocument {
    /// Source-default three-mode permission object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
    /// Exact-tool permission exceptions keyed by remote tool name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, crate::local_runtime::config::McpToolPermissionDocument>,
}

/// One validated package with an owned, file-backed source snapshot.
#[derive(Debug, Clone)]
pub struct PythonToolPackage {
    /// The package name: its folder name, validated identifier-safe.
    name: String,
    /// The workspace folder the snapshot was read from (diagnostics only;
    /// the server never runs from this live root).
    root: PathBuf,
    /// Every package file, sorted by package-relative path.
    ///
    /// Excludes [`PERMISSIONS_FILE`]: that document is resource metadata
    /// captured onto this package, not frozen server source.
    files: Vec<PathBuf>,
    source: PackageSource,
    source_digest: String,
    /// The normalized dependency lines of `requirements.txt` (comments and
    /// blank lines removed), in file order.
    requirements: Vec<String>,
    /// Source-default three-mode permission object from [`PERMISSIONS_FILE`].
    permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
    /// Exact-tool permission exceptions from [`PERMISSIONS_FILE`].
    tool_permissions: BTreeMap<String, crate::runtime::permission::ResourcePermissionSet>,
}

#[derive(Debug, Clone)]
enum PackageSource {
    #[cfg(any(test, feature = "test-support"))]
    Local(Arc<PythonSource>),
    Client(Arc<ClientSource>),
    #[cfg(not(test))]
    Unprepared,
}

#[cfg(any(test, feature = "test-support"))]
impl PackageSource {
    fn local(&self) -> &PythonSource {
        match self {
            Self::Local(source) => source,
            Self::Client(_) => unreachable!("only an explicit fixture uses the local Store"),
            #[cfg(not(test))]
            Self::Unprepared => unreachable!("the authoring declaration is not prepared"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ClientSource {
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) endpoint: Arc<crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint>,
    pub(crate) key: rustx_execution_protocol::process::ProcessOperationKey,
}

impl Drop for ClientSource {
    fn drop(&mut self) {
        let endpoint = self.endpoint.clone();
        let key = self.key.clone();
        self.runtime
            .spawn(endpoint.release_managed_python_source(key));
    }
}

impl ClientSource {
    async fn prepare(
        &self,
        cancellation: &crate::runtime::CancellationSignal,
    ) -> Result<(), String> {
        use rustx_execution_protocol::{
            ReadError,
            mcp::PythonSourceReply as Reply,
            transport::{PAGE_REQUEST_TIMEOUT, recoverable_lane_loss},
        };
        use std::time::{Duration, Instant};
        // Each subprocess retains its own legacy hard limit. The waiter
        // additionally caps the full probe + lock + sync sequence.
        let cutoff = Instant::now() + Duration::from_mins(22);
        loop {
            if cancellation.is_cancelled() || Instant::now() >= cutoff {
                return Err(
                    "Managed Python preparation did not finish in the admitted lifetime".into(),
                );
            }
            let endpoint = self.endpoint.clone();
            let signal = cancellation.clone();
            let key = self.key.clone();
            let one_exchange = cutoff.min(Instant::now() + PAGE_REQUEST_TIMEOUT);
            let result = tokio::task::spawn_blocking(move || {
                endpoint.prepare_managed_python_source(key, one_exchange, &signal)
            })
            .await
            .map_err(|_| String::from("Managed Python preparation owner failed"))?;
            match result {
                Ok(Reply::Prepared) => return Ok(()),
                Ok(Reply::Capturing | Reply::Preparing | Reply::Refused(ReadError::Busy)) => {}
                Err(error)
                    if recoverable_lane_loss(&error)
                        || error.kind() == std::io::ErrorKind::WouldBlock => {}
                _ => {
                    return Err(
                        "the admitted Client could not prepare the frozen Python environment"
                            .into(),
                    );
                }
            }
            tokio::select! {
                () = cancellation.cancelled() => return Err("Managed Python preparation cancelled".into()),
                () = tokio::time::sleep(Duration::from_millis(50)) => {},
            }
        }
    }
}

/// One anonymous backing file, with bounded in-memory file coordinates. Its last
/// Arc closes the descriptor; the kernel owns cleanup even after cancellation.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
struct PythonSource {
    file: std::fs::File,
    entries: BTreeMap<PathBuf, (u64, u64, std::fs::Permissions)>,
}
#[cfg(any(test, feature = "test-support"))]
impl PythonSource {
    fn copy_to(
        &self,
        relative: &Path,
        destination: &Path,
        cancellation: &crate::runtime::CancellationSignal,
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
        let mut buffer = vec![0; crate::package_io::CHUNK_BYTES];
        while copied < *length {
            crate::package_io::check_cancelled(cancellation).map_err(io_error)?;
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
        crate::package_io::check_cancelled(cancellation).map_err(io_error)
    }
}

impl PythonToolPackage {
    #[cfg(not(test))]
    pub(crate) fn inspect_authoring(root: &Path, name: &str) -> Result<Self, PythonToolError> {
        validate_identifier(name)?;
        let directory = crate::package_io::PackageDirectory::open(root).map_err(io_error)?;
        for required in [SERVER_FILE, REQUIREMENTS_FILE] {
            directory.file(Path::new(required)).map_err(|_| {
                PythonToolError::InvalidPackage(format!(
                    "package {name:?} has no readable {required}"
                ))
            })?;
        }
        let (permissions, tool_permissions) = match directory.file(Path::new(PERMISSIONS_FILE)) {
            Ok(_) => parse_permissions_document(root, &PythonToolError::InvalidPackage)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, BTreeMap::new()),
            Err(error) => return Err(io_error(error)),
        };
        Ok(Self {
            name: name.to_owned(),
            root: root.to_path_buf(),
            files: Vec::new(),
            source: PackageSource::Unprepared,
            source_digest: String::new(),
            requirements: Vec::new(),
            permissions,
            tool_permissions,
        })
    }

    pub(crate) fn from_client(
        name: String,
        root: PathBuf,
        source_digest: String,
        permissions: Option<&[u8]>,
        source: Arc<ClientSource>,
    ) -> Result<Self, PythonToolError> {
        validate_identifier(&name)?;
        let (permissions, tool_permissions) = permissions.map_or_else(
            || Ok((None, BTreeMap::new())),
            |bytes| {
                parse_permissions_bytes(bytes, &|reason| PythonToolError::InvalidPackage(reason))
            },
        )?;
        Ok(Self {
            name,
            root,
            files: Vec::new(),
            source: PackageSource::Client(source),
            source_digest,
            requirements: Vec::new(),
            permissions,
            tool_permissions,
        })
    }

    pub(crate) fn client_binding(&self) -> Option<McpServerBinding> {
        let PackageSource::Client(source) = &self.source else {
            return None;
        };
        Some(McpServerBinding {
            credentials: crate::credentials::SourceCredentials::default(),
            transport: McpTransportConfig::ManagedPython {
                source: crate::tools::mcp::ManagedPythonSource::retain(source.clone()),
            },
            policy: ToolInvocationPolicy::default(),
            permissions: self.permissions,
            tool_permissions: self.tool_permissions.clone(),
        })
    }

    pub(crate) async fn prepare_on_client(
        &self,
        cancellation: &crate::runtime::CancellationSignal,
    ) -> Option<Result<McpServerBinding, String>> {
        let PackageSource::Client(owner) = &self.source else {
            return None;
        };
        Some(
            owner
                .prepare(cancellation)
                .await
                .map(|()| self.client_binding().expect("Client source")),
        )
    }

    /// Validated package identity, immutable after capture.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Normalized declarations parsed from the same captured source bytes.
    #[must_use]
    pub fn requirements(&self) -> &[String] {
        &self.requirements
    }

    /// Logical paths of the captured source; supporting bytes stay on disk.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
    /// Complete content identity of the owned captured bytes.
    #[must_use]
    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }
    /// Source-default three-mode permission object authored in
    /// [`PERMISSIONS_FILE`], when present.
    #[must_use]
    pub const fn permissions(&self) -> Option<crate::runtime::permission::ResourcePermissionSet> {
        self.permissions
    }
    /// Exact-tool permission exceptions authored in [`PERMISSIONS_FILE`].
    #[must_use]
    pub fn tool_permissions(
        &self,
    ) -> &BTreeMap<String, crate::runtime::permission::ResourcePermissionSet> {
        &self.tool_permissions
    }
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

#[cfg(test)]
thread_local! {
    // The package parser is synchronous. This owner-local hook counts entry,
    // including failures before reading bytes; it makes no cross-task claims.
    pub(crate) static PACKAGE_PARSE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn discover_package(
    root: &Path,
    name: &str,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<PythonToolPackage, PythonToolError> {
    #[cfg(test)]
    PACKAGE_PARSE_COUNT.with(|count| count.set(count.get() + 1));
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
    let (permissions, tool_permissions) = take_permissions_document(&mut files, root, &invalid)?;
    let (source, source_digest, requirements) =
        capture_package_source(root, &files, cancellation, requirements_error)?;
    Ok(PythonToolPackage {
        name: name.to_owned(),
        root: root.to_path_buf(),
        files,
        source: PackageSource::Local(Arc::new(source)),
        source_digest,
        requirements,
        permissions,
        tool_permissions,
    })
}

/// Parses optional [`PERMISSIONS_FILE`] and removes it from the frozen source set.
#[cfg(any(test, feature = "test-support"))]
fn take_permissions_document(
    files: &mut Vec<PathBuf>,
    root: &Path,
    invalid: &impl Fn(String) -> PythonToolError,
) -> Result<
    (
        Option<crate::runtime::permission::ResourcePermissionSet>,
        BTreeMap<String, crate::runtime::permission::ResourcePermissionSet>,
    ),
    PythonToolError,
> {
    if !files.iter().any(|path| path == Path::new(PERMISSIONS_FILE)) {
        return Ok((None, BTreeMap::new()));
    }
    files.retain(|path| path != Path::new(PERMISSIONS_FILE));
    parse_permissions_document(root, invalid)
}

/// Captures package source bytes into an anonymous backing file and parses
/// `requirements.txt` declarations from the same stream.
#[cfg(any(test, feature = "test-support"))]
fn capture_package_source(
    root: &Path,
    files: &[PathBuf],
    cancellation: &crate::runtime::CancellationSignal,
    requirements_error: impl Fn(PythonToolError) -> PythonToolError,
) -> Result<(PythonSource, String, Vec<String>), PythonToolError> {
    let mut source_file = tempfile::tempfile().map_err(io_error)?;
    let directory = crate::package_io::PackageDirectory::open(root).map_err(io_error)?;
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
        let evidence = crate::package_io::stream(&directory, path, cancellation, |bytes| {
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
    };
    // Only declarations require text parsing. Binary supporting assets are copied
    // and hashed as bytes and are never held in the parsed package representation.
    let requirements = requirements
        .finish(&source.file)
        .map_err(requirements_error)?;
    Ok((source, source_digest, requirements))
}

impl PartialEq for PythonToolPackage {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.root == other.root
            && self.files == other.files
            && self.source_digest == other.source_digest
            && self.requirements == other.requirements
            && self.permissions == other.permissions
            && self.tool_permissions == other.tool_permissions
    }
}
impl Eq for PythonToolPackage {}

/// Reads and validates optional [`PERMISSIONS_FILE`] from the package root.
fn parse_permissions_document(
    root: &Path,
    invalid: &impl Fn(String) -> PythonToolError,
) -> Result<
    (
        Option<crate::runtime::permission::ResourcePermissionSet>,
        BTreeMap<String, crate::runtime::permission::ResourcePermissionSet>,
    ),
    PythonToolError,
> {
    let path = root.join(PERMISSIONS_FILE);
    let bytes = crate::bounded_file::read_bounded(&path).map_err(|error| {
        invalid(format!(
            "{PERMISSIONS_FILE}: cannot read resource permissions: {error}"
        ))
    })?;
    parse_permissions_bytes(&bytes, invalid)
}

fn parse_permissions_bytes(
    bytes: &[u8],
    invalid: &impl Fn(String) -> PythonToolError,
) -> Result<
    (
        Option<crate::runtime::permission::ResourcePermissionSet>,
        BTreeMap<String, crate::runtime::permission::ResourcePermissionSet>,
    ),
    PythonToolError,
> {
    let document: ManagedPythonPermissionsDocument = crate::toml_authoring::parse(bytes)
        .map_err(|error| invalid(format!("{PERMISSIONS_FILE}: {error}")))?;
    let tool_permissions = document
        .tools
        .into_iter()
        .filter_map(|(name, tool)| tool.permissions.map(|permissions| (name, permissions)))
        .collect();
    Ok((document.permissions, tool_permissions))
}

/// Enumerate metadata through anchored directory handles, without opening links.
#[cfg(any(test, feature = "test-support"))]
fn collect_files(
    root: &Path,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<Vec<PathBuf>, PythonToolError> {
    fn walk(
        directory: &crate::package_io::PackageDirectory,
        prefix: &Path,
        files: &mut Vec<PathBuf>,
        remaining: &mut usize,
        depth: usize,
        cancellation: &crate::runtime::CancellationSignal,
    ) -> Result<(), PythonToolError> {
        crate::package_io::check_cancelled(cancellation).map_err(io_error)?;
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
    let directory = crate::package_io::PackageDirectory::open(root).map_err(io_error)?;
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

pub(crate) fn validate_identifier(identifier: &str) -> Result<(), PythonToolError> {
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
#[cfg(any(test, feature = "test-support"))]
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

#[cfg(test)]
fn parse_requirements(bytes: &[u8], _package_root: &Path) -> Result<Vec<String>, String> {
    let invalid = |error| match error {
        PythonToolError::InvalidPackage(message) => message,
        other => other.to_string(),
    };
    let mut file = tempfile::tempfile().unwrap();
    let mut parser = requirements::Parser::default();
    for chunk in bytes.chunks(7) {
        file.write_all(chunk).unwrap();
        parser.push(chunk, &file).map_err(invalid)?;
    }
    parser.finish(&file).map_err(invalid)
}

/// The requirements-file layer rejects only pip's documented `${NAME}`
/// expansion directive. Other dollar signs belong to the PEP 508 parser: in
/// particular, `$TOKEN` is ordinary quoted-marker text rather than pip
/// expansion syntax.
#[cfg(any(test, feature = "test-support"))]
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
#[cfg(any(test, feature = "test-support"))]
const MAX_REQUIREMENTS_DIAGNOSTIC_BYTE_OFFSET: usize = 16 * 1024 * 1024;
#[cfg(any(test, feature = "test-support"))]
const MAX_REQUIREMENTS_PARSE_REASON_BYTES: usize =
    "unexpected end of dependency declaration near byte 16777216+".len();

#[cfg(any(test, feature = "test-support"))]
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
#[cfg(any(test, feature = "test-support"))]
fn source_digest(
    root: &Path,
    files: &[PathBuf],
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<String, PythonToolError> {
    let directory = crate::package_io::PackageDirectory::open(root).map_err(io_error)?;
    let mut hash = sha2::Sha256::new();
    hash.update(b"rustx:managed-python-source:v2\0");
    for path in files {
        let name = path.as_os_str().as_encoded_bytes();
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name);
        let evidence = crate::package_io::stream(&directory, path, cancellation, |_| Ok(()))
            .map_err(io_error)?;
        hash.update(evidence.metadata.len().to_be_bytes());
        hash.update(evidence.digest);
    }
    Ok(format!("sha256:{}", hex_digest(&hash.finalize())))
}

#[cfg(any(test, feature = "test-support"))]
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
