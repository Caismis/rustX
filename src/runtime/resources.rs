//! Immutable process-local runtime resources (Issue #106).
//!
//! Resource discovery belongs to runtime creation and explicit reload. An
//! admitted attempt receives one [`RuntimeResourceSnapshot`] by `Arc` and
//! never consults the loader or filesystem again. Historical request values
//! remain frozen by [`crate::model::RequestSnapshot`]; this process-local
//! snapshot is never persisted as a second history plane.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::capabilities::{CapabilityCoordinator, CapabilitySnapshot, PreparedCapabilityCandidate};
use crate::context::ContextAssembly;
use crate::runtime::identity::{CapabilityRevision, RuntimeResourceRevision};

const PROJECT_CONTEXT_FILENAMES: [&str; 5] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];
const MAX_RESOURCE_DIAGNOSTIC_BYTES: usize = 4096;

/// One runtime-loaded project instruction file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextFile {
    /// The deterministic absolute source path.
    pub path: PathBuf,
    /// The exact UTF-8 content loaded for this generation, with an optional
    /// UTF-8 BOM removed in the same way as Pi's resource loader.
    pub content: String,
}

/// The complete immutable resource generation observed by an attempt.
#[derive(Clone)]
pub struct RuntimeResourceSnapshot {
    revision: RuntimeResourceRevision,
    project_context_files: Arc<[ProjectContextFile]>,
    project_instructions: Option<Arc<str>>,
    skill_catalog: Option<Arc<str>>,
    skill_sources: Arc<[PathBuf]>,
    agent_profile: Option<Arc<str>>,
    context_assembly: ContextAssembly,
    capability: Arc<CapabilitySnapshot>,
}

impl core::fmt::Debug for RuntimeResourceSnapshot {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RuntimeResourceSnapshot")
            .field("revision", &self.revision)
            .field("project_context_files", &self.project_context_files)
            .field("skill_sources", &self.skill_sources)
            .field("agent_profile", &self.agent_profile)
            .field("context_assembly", &self.context_assembly)
            .field("capability_revision", &self.capability.revision())
            .finish_non_exhaustive()
    }
}

impl RuntimeResourceSnapshot {
    /// Builds one immutable generation from explicit already-loaded values
    /// and its compatible committed capability snapshot.
    #[must_use]
    pub fn new(
        revision: RuntimeResourceRevision,
        project_context_files: Vec<ProjectContextFile>,
        agent_profile: Option<String>,
        context_assembly: ContextAssembly,
        capability: Arc<CapabilitySnapshot>,
    ) -> Self {
        let project_instructions =
            concatenate_project_instructions(&project_context_files).map(Arc::<str>::from);
        let skill_catalog = capability.skill_catalog().map(Arc::<str>::from);
        let skill_sources = capability
            .skills()
            .locations()
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        Self {
            revision,
            project_context_files: project_context_files.into(),
            project_instructions,
            skill_catalog,
            skill_sources: skill_sources.into(),
            agent_profile: agent_profile.map(Arc::<str>::from),
            context_assembly,
            capability,
        }
    }

    /// Completes a fully prepared generation after its compatible capability
    /// candidate has committed.
    #[must_use]
    pub(crate) fn from_prepared(
        revision: RuntimeResourceRevision,
        prepared: PreparedRuntimeResourceData,
        capability: Arc<CapabilitySnapshot>,
    ) -> Self {
        Self::new(
            revision,
            prepared.project_context_files,
            prepared.agent_profile,
            prepared.context_assembly,
            capability,
        )
    }

    /// The process-local generation identity.
    #[must_use]
    pub const fn revision(&self) -> RuntimeResourceRevision {
        self.revision
    }

    /// Ordered project instruction sources, global/root-most to workspace.
    #[must_use]
    pub fn project_context_files(&self) -> &[ProjectContextFile] {
        &self.project_context_files
    }

    /// The exact concatenated project instruction text.
    #[must_use]
    pub fn project_instructions(&self) -> Option<&str> {
        self.project_instructions.as_deref()
    }

    /// The exact compact Skill catalog frozen for this generation.
    #[must_use]
    pub fn skill_catalog(&self) -> Option<&str> {
        self.skill_catalog.as_deref()
    }

    /// Canonical source identities of the discovered `SKILL.md` files.
    #[must_use]
    pub fn skill_sources(&self) -> &[PathBuf] {
        &self.skill_sources
    }

    /// The immutable agent profile/persona of this runtime generation.
    #[must_use]
    pub fn agent_profile(&self) -> Option<&str> {
        self.agent_profile.as_deref()
    }

    /// The certified-extension registry frozen with this generation.
    #[must_use]
    pub fn context_assembly(&self) -> &ContextAssembly {
        &self.context_assembly
    }

    /// The compatible immutable capability snapshot published with this
    /// resource generation.
    #[must_use]
    pub fn capability(&self) -> &Arc<CapabilitySnapshot> {
        &self.capability
    }

    /// The compatible capability revision.
    #[must_use]
    pub fn capability_revision(&self) -> CapabilityRevision {
        self.capability.revision()
    }
}

/// A complete off-side resource candidate. Nothing in this value is visible
/// to an admitted attempt until the runtime publishes it.
pub struct PreparedRuntimeResources {
    project_context_files: Vec<ProjectContextFile>,
    agent_profile: Option<String>,
    context_assembly: ContextAssembly,
    capability: PreparedCapabilityCandidate,
}

/// The non-capability half of a prepared resource candidate after the
/// capability candidate has been moved into its commit boundary.
pub(crate) struct PreparedRuntimeResourceData {
    project_context_files: Vec<ProjectContextFile>,
    agent_profile: Option<String>,
    context_assembly: ContextAssembly,
}

impl PreparedRuntimeResources {
    /// Builds one complete prepared resource candidate.
    #[must_use]
    pub fn new(
        project_context_files: Vec<ProjectContextFile>,
        agent_profile: Option<String>,
        context_assembly: ContextAssembly,
        capability: PreparedCapabilityCandidate,
    ) -> Self {
        Self {
            project_context_files,
            agent_profile,
            context_assembly,
            capability,
        }
    }

    pub(crate) fn into_parts(self) -> (PreparedCapabilityCandidate, PreparedRuntimeResourceData) {
        let Self {
            project_context_files,
            agent_profile,
            context_assembly,
            capability,
        } = self;
        (
            capability,
            PreparedRuntimeResourceData {
                project_context_files,
                agent_profile,
                context_assembly,
            },
        )
    }
}

/// Runtime-owned resource loading. Implementations may read explicit current
/// configuration and filesystem inputs, but are invoked only at runtime
/// creation or through the semantic reload operation.
pub trait RuntimeResourceLoader: Send + Sync {
    /// Builds a complete candidate off-side.
    fn prepare<'a>(
        &'a self,
        capability: &'a CapabilityCoordinator,
    ) -> BoxFuture<'a, Result<PreparedRuntimeResources, RuntimeResourceLoadError>>;
}

/// The built-in filesystem loader for a fixed runtime composition.
///
/// It reuses the coordinator's explicit current capability inputs and owns
/// only filesystem discovery timing. Local product composition may use a
/// richer loader that reparses its current config on explicit reload.
#[derive(Clone)]
pub struct FilesystemRuntimeResourceLoader {
    workspace: PathBuf,
    agent_profile: Option<String>,
    context_assembly: ContextAssembly,
    base_only: bool,
}

impl FilesystemRuntimeResourceLoader {
    /// Creates the normal runtime loader.
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            agent_profile: None,
            context_assembly: ContextAssembly::new(),
            base_only: false,
        }
    }

    /// Creates a base-capability-only loader, used by deny-by-construction
    /// subagent child profiles.
    #[must_use]
    pub fn base_only(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            agent_profile: None,
            context_assembly: ContextAssembly::new(),
            base_only: true,
        }
    }

    /// Freezes the runtime agent profile into each generation.
    #[must_use]
    pub fn with_agent_profile(mut self, profile: impl Into<String>) -> Self {
        self.agent_profile = Some(profile.into());
        self
    }

    /// Freezes the certified-extension registry into each generation.
    #[must_use]
    pub fn with_context_assembly(mut self, assembly: ContextAssembly) -> Self {
        self.context_assembly = assembly;
        self
    }
}

impl RuntimeResourceLoader for FilesystemRuntimeResourceLoader {
    fn prepare<'a>(
        &'a self,
        capability: &'a CapabilityCoordinator,
    ) -> BoxFuture<'a, Result<PreparedRuntimeResources, RuntimeResourceLoadError>> {
        Box::pin(async move {
            let project_context_files = load_project_context_files(&self.workspace)?;
            let candidate = if self.base_only {
                capability.prepare_base_only_candidate().map_err(|error| {
                    RuntimeResourceLoadError::new(format!(
                        "cannot prepare base capability resources: {error}"
                    ))
                })?
            } else {
                capability.prepare_candidate().await.map_err(|error| {
                    RuntimeResourceLoadError::new(format!(
                        "cannot prepare capability resources: {error}"
                    ))
                })?
            };
            Ok(PreparedRuntimeResources::new(
                project_context_files,
                self.agent_profile.clone(),
                self.context_assembly.clone(),
                candidate,
            ))
        })
    }
}

/// One bounded resource-load failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResourceLoadError {
    /// Bounded diagnostic safe for Runtime Client presentation.
    pub message: String,
}

impl RuntimeResourceLoadError {
    /// Creates a bounded diagnostic.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > MAX_RESOURCE_DIAGNOSTIC_BYTES {
            let mut boundary = MAX_RESOURCE_DIAGNOSTIC_BYTES;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
            message.push('…');
        }
        Self { message }
    }
}

impl core::fmt::Display for RuntimeResourceLoadError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeResourceLoadError {}

/// Loads Pi-style project context files from `workspace` and every ancestor.
/// Each directory contributes at most one file using first-match precedence:
/// `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD`.
/// The returned order is root-to-leaf and paths are deduplicated.
///
/// # Errors
///
/// Returns a bounded diagnostic when the workspace cannot be canonicalized,
/// a candidate cannot be inspected/read, or selected content is not UTF-8.
pub fn load_project_context_files(
    workspace: &Path,
) -> Result<Vec<ProjectContextFile>, RuntimeResourceLoadError> {
    let workspace = std::fs::canonicalize(workspace).map_err(|error| {
        RuntimeResourceLoadError::new(format!(
            "cannot canonicalize workspace {}: {error}",
            workspace.display()
        ))
    })?;
    let mut directories = workspace
        .ancestors()
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    directories.reverse();
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for directory in directories {
        if let Some(file) = load_context_file_from_directory(&directory)?
            && seen.insert(file.path.clone())
        {
            files.push(file);
        }
    }
    Ok(files)
}

fn load_context_file_from_directory(
    directory: &Path,
) -> Result<Option<ProjectContextFile>, RuntimeResourceLoadError> {
    for filename in PROJECT_CONTEXT_FILENAMES {
        let path = directory.join(filename);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(RuntimeResourceLoadError::new(format!(
                    "cannot inspect project context file {}: {error}",
                    path.display()
                )));
            }
        };
        if !metadata.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            RuntimeResourceLoadError::new(format!(
                "cannot read project context file {}: {error}",
                path.display()
            ))
        })?;
        let mut content = String::from_utf8(bytes).map_err(|error| {
            RuntimeResourceLoadError::new(format!(
                "project context file {} is not UTF-8: {error}",
                path.display()
            ))
        })?;
        if let Some(without_bom) = content.strip_prefix('\u{feff}') {
            content = without_bom.to_owned();
        }
        return Ok(Some(ProjectContextFile { path, content }));
    }
    Ok(None)
}

fn concatenate_project_instructions(files: &[ProjectContextFile]) -> Option<String> {
    let parts = files
        .iter()
        .map(|file| file.content.as_str())
        .filter(|content| !content.trim().is_empty())
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::{concatenate_project_instructions, load_project_context_files};

    #[test]
    fn project_context_precedence_and_order_are_deterministic() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("root");
        let child = root.join("a/b");
        std::fs::create_dir_all(&child).expect("workspace");
        std::fs::write(root.join("AGENTS.md"), "\u{feff}root agents").expect("root AGENTS");
        std::fs::write(root.join("CLAUDE.md"), "shadowed root claude").expect("root CLAUDE");
        std::fs::write(root.join("a/AGENTS.MD"), "middle agents").expect("middle AGENTS");
        std::fs::write(child.join("AGENTS.md"), "shadowed child agents").expect("child AGENTS");
        std::fs::write(child.join("AGENTS.override.md"), "child override").expect("child override");

        let files = load_project_context_files(&child).expect("project context");
        assert_eq!(
            files
                .iter()
                .filter(|file| file.path.starts_with(&root))
                .map(|file| (
                    file.path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    file.content.clone()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("AGENTS.md".to_owned(), "root agents".to_owned()),
                ("AGENTS.MD".to_owned(), "middle agents".to_owned()),
                ("AGENTS.override.md".to_owned(), "child override".to_owned()),
            ]
        );
        assert_eq!(
            concatenate_project_instructions(&files),
            Some("root agents\n\nmiddle agents\n\nchild override".to_owned())
        );
    }
}
