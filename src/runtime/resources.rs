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

use crate::capabilities::{
    CapabilityAvailability, CapabilityCoordinator, CapabilitySnapshot, PreparedCapabilityCandidate,
};
use crate::context::ContextAssembly;
use crate::runtime::identity::{CapabilityRevision, RuntimeResourceRevision};
use crate::runtime::subagent::catalog::{AgentCatalog, SubagentName};
use crate::runtime::workflow::WorkflowCatalog;
use crate::skills::SkillCatalogEntry;

const PROJECT_CONTEXT_FILENAMES: [&str; 5] = [
    "AGENTS.override.md",
    "AGENTS.md",
    "AGENTS.MD",
    "CLAUDE.md",
    "CLAUDE.MD",
];
const MAX_RESOURCE_DIAGNOSTIC_BYTES: usize = 4096;

/// One runtime-loaded project instruction file.
///
/// The value carries both the canonical source identity and the exact
/// content loaded for its generation, so a subagent child can be handed the
/// frozen chain by value and never has to rediscover or reinterpret it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextFile {
    /// The deterministic absolute source path.
    pub path: PathBuf,
    /// The exact UTF-8 content loaded for this generation, with an optional
    /// UTF-8 BOM removed in the same way as Pi's resource loader.
    pub content: String,
}

/// Inert canonical Managed Python sources frozen with the full generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedPythonCatalog {
    packages: std::collections::BTreeMap<crate::capabilities::ToolSourceId, PathBuf>,
}
impl ManagedPythonCatalog {
    /// Construct an inert catalog from canonical, authority-checked package paths.
    #[must_use]
    pub fn new(
        packages: std::collections::BTreeMap<crate::capabilities::ToolSourceId, PathBuf>,
    ) -> Self {
        Self { packages }
    }
    /// Discovered source identities and their canonical package directories.
    #[must_use]
    pub const fn packages(
        &self,
    ) -> &std::collections::BTreeMap<crate::capabilities::ToolSourceId, PathBuf> {
        &self.packages
    }
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
    /// The exact immutable named-subagent catalog admitted into this
    /// generation (Issue #144).
    subagents: Arc<AgentCatalog>,
    resolved_agents: std::collections::BTreeMap<
        SubagentName,
        Arc<crate::runtime::agent_profile::ResolvedAgentProfile>,
    >,
    /// The profiles explicitly admitted to the main Agent domain.
    /// The profiles explicitly admitted to Workflow Agent nodes.
    subagent_workflow: BTreeSet<SubagentName>,
    /// The immutable discovered Workflow programs of this generation.
    workflows: Arc<WorkflowCatalog>,
    managed_python: ManagedPythonCatalog,
    /// The capability-source availability state belonging to the *same*
    /// generation (Issue #144).
    ///
    /// Subagent resolution must distinguish "this generation does not
    /// authorize that capability" from "the optional source that would
    /// provide it is currently unavailable". `CapabilitySnapshot` stays
    /// focused on executable capability identity — its revision advances
    /// only when the executable set changes — so the control-plane
    /// availability that answers the second question is carried here,
    /// alongside the catalog that needs it.
    capability_availability: CapabilityAvailability,
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
            .field("subagents", &self.subagents.names())
            .finish_non_exhaustive()
    }
}

impl RuntimeResourceSnapshot {
    #[cfg(test)]
    pub(crate) fn with_test_root_agents(mut self, agents: BTreeSet<SubagentName>) -> Self {
        use crate::runtime::agent_profile::{
            AgentProfile, AgentProfileAuthority, AgentScope, resolve_agent_profile,
        };
        let profile = AgentProfile::from_document(
            &crate::local_runtime::config::AgentProfileDocument {
                agents: agents.into_iter().collect(),
                ..crate::local_runtime::config::builtin_root_profile()
            },
            Vec::new(),
        )
        .unwrap();
        let resolved = resolve_agent_profile(
            &profile,
            &AgentProfileAuthority {
                tools: self.capability.available_tools(),
                availability: &self.capability_availability,
                skills: self.capability.skills(),
                agents: &self.subagents.names().into_iter().cloned().collect(),
                workflows: &self.workflows.definitions().keys().cloned().collect(),
                scope: AgentScope::Root,
            },
        );
        self.capability = Arc::new(
            self.capability
                .as_ref()
                .clone()
                .with_resolved_profile(Some(Arc::new(resolved))),
        );
        self
    }

    fn resolve_profiles(&mut self) {
        use crate::runtime::agent_profile::{
            AgentProfileAuthority, AgentScope, resolve_agent_profile,
        };
        let agents = self.subagents.names().into_iter().cloned().collect();
        let workflows = self.workflows.definitions().keys().cloned().collect();
        let authority = AgentProfileAuthority {
            tools: self.capability.available_tools(),
            availability: &self.capability_availability,
            skills: self.capability.skills(),
            agents: &agents,
            workflows: &workflows,
            scope: AgentScope::OneShotChild,
        };
        self.resolved_agents = self
            .subagents
            .definitions()
            .map(|definition| {
                (
                    definition.name().clone(),
                    Arc::new(resolve_agent_profile(definition.profile(), &authority)),
                )
            })
            .collect();
    }
    pub fn resolved_agent(
        &self,
        name: &SubagentName,
    ) -> Option<&crate::runtime::agent_profile::ResolvedAgentProfile> {
        self.resolved_agents.get(name).map(AsRef::as_ref)
    }
    pub fn root_profile(&self) -> Option<&crate::runtime::agent_profile::ResolvedAgentProfile> {
        self.capability.resolved_profile()
    }

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
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::ResourcePublication,
        );
        let mut effective_project_files = project_context_files.clone();
        let mut agent_profile = agent_profile;
        if let Some(profile) = capability.resolved_profile() {
            if !profile.project_instructions.inherit {
                effective_project_files.clear();
            }
            effective_project_files.extend(profile.project_instructions.files.clone());
            if agent_profile.is_none() && !profile.instructions.is_empty() {
                agent_profile = Some(profile.instructions.clone());
            }
        }
        let project_instructions =
            concatenate_project_instructions(&effective_project_files).map(Arc::<str>::from);
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
            subagents: Arc::new(AgentCatalog::empty()),
            resolved_agents: std::collections::BTreeMap::new(),
            subagent_workflow: BTreeSet::new(),
            workflows: Arc::new(WorkflowCatalog::empty()),
            managed_python: ManagedPythonCatalog::default(),
            capability_availability: CapabilityAvailability::new(),
        }
    }

    /// Freezes the generation's admitted named-subagent catalog.
    #[must_use]
    pub fn with_subagent_catalog(mut self, catalog: AgentCatalog) -> Self {
        self.subagents = Arc::new(catalog);
        self.resolve_profiles();
        self
    }

    /// Freezes the independent main and Workflow profile admissions.
    #[must_use]
    pub fn with_workflow_admission(mut self, workflow: BTreeSet<SubagentName>) -> Self {
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::AuthorityMutation,
        );
        self.subagent_workflow = workflow;
        self
    }

    /// The inert package identities belonging to this generation.
    #[must_use]
    pub const fn managed_python_catalog(&self) -> &ManagedPythonCatalog {
        &self.managed_python
    }

    /// Freezes inert discovery, without granting preparation authority.
    #[must_use]
    pub fn with_managed_python_catalog(mut self, catalog: ManagedPythonCatalog) -> Self {
        self.managed_python = catalog;
        self
    }

    /// Freezes the discovered Workflow catalog for this generation.
    #[must_use]
    pub fn with_workflow_catalog(mut self, catalog: WorkflowCatalog) -> Self {
        self.workflows = Arc::new(catalog);
        self.resolve_profiles();
        self
    }

    /// Freezes the capability-source availability belonging to this exact
    /// generation.
    #[must_use]
    pub fn with_capability_availability(mut self, availability: CapabilityAvailability) -> Self {
        self.capability_availability = availability;
        self.resolve_profiles();
        self
    }

    /// Replaces the generation's model-visible Skill catalog with an exact
    /// frozen entry set.
    ///
    /// This is the subagent child composition path: a child's Skill catalog
    /// is the parent-resolved allowlist, handed over by value. The child
    /// therefore renders exactly the entries its invoking generation
    /// authorized and rediscovers nothing. Progressive disclosure is
    /// untouched: only catalog metadata is frozen, never a `SKILL.md` body.
    #[must_use]
    pub fn with_frozen_skill_catalog(mut self, entries: &[SkillCatalogEntry]) -> Self {
        let visible =
            crate::skills::admitted_skill_entries(entries, self.capability.tool_registry());
        self.skill_catalog = (!visible.is_empty())
            .then(|| Arc::<str>::from(crate::skills::render_skill_catalog(visible)));
        self.skill_sources = entries
            .iter()
            .map(|entry| PathBuf::from(&entry.location))
            .collect::<Vec<_>>()
            .into();
        self
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
        .with_subagent_catalog(prepared.subagents)
        .with_workflow_admission(prepared.subagent_workflow)
        .with_workflow_catalog(prepared.workflows)
        .with_managed_python_catalog(prepared.managed_python)
        .with_capability_availability(prepared.capability_availability)
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

    /// The immutable named-subagent catalog admitted into this generation.
    ///
    /// A subagent invocation resolves against exactly this catalog: an
    /// attempt that owns generation R1 keeps resolving R1 even after a
    /// reload has committed R2 as runtime-current.
    #[must_use]
    pub fn subagents(&self) -> &AgentCatalog {
        &self.subagents
    }

    /// The explicitly main-admitted profile ids.
    #[must_use]
    pub fn delegatable_agents(&self) -> &BTreeSet<SubagentName> {
        static EMPTY: BTreeSet<SubagentName> = BTreeSet::new();
        self.root_profile()
            .map_or(&EMPTY, |profile| &profile.agents)
    }

    /// The explicitly Workflow-admitted profile ids.
    #[must_use]
    pub fn subagent_workflow_admission(&self) -> &BTreeSet<SubagentName> {
        &self.subagent_workflow
    }

    /// The immutable discovered Workflow catalog.
    #[must_use]
    pub fn workflows(&self) -> &WorkflowCatalog {
        &self.workflows
    }

    /// The capability-source availability state of this exact generation.
    #[must_use]
    pub const fn capability_availability(&self) -> &CapabilityAvailability {
        &self.capability_availability
    }
}

/// A complete off-side resource candidate. Nothing in this value is visible
/// to an admitted attempt until the runtime publishes it.
pub struct PreparedRuntimeResources {
    project_context_files: Vec<ProjectContextFile>,
    agent_profile: Option<String>,
    context_assembly: ContextAssembly,
    subagents: AgentCatalog,
    subagent_workflow: BTreeSet<SubagentName>,
    workflows: WorkflowCatalog,
    managed_python: ManagedPythonCatalog,
    capability: PreparedCapabilityCandidate,
}

/// The non-capability half of a prepared resource candidate after the
/// capability candidate has been moved into its commit boundary.
pub(crate) struct PreparedRuntimeResourceData {
    project_context_files: Vec<ProjectContextFile>,
    agent_profile: Option<String>,
    context_assembly: ContextAssembly,
    subagents: AgentCatalog,
    subagent_workflow: BTreeSet<SubagentName>,
    workflows: WorkflowCatalog,
    managed_python: ManagedPythonCatalog,
    capability_availability: CapabilityAvailability,
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
            subagents: AgentCatalog::empty(),
            subagent_workflow: BTreeSet::new(),
            workflows: WorkflowCatalog::empty(),
            managed_python: ManagedPythonCatalog::default(),
            capability,
        }
    }

    /// Adds the candidate generation's validated named-subagent catalog.
    ///
    /// A loader validates its catalog against the *same* prepared candidate
    /// it publishes, so a definition naming an unknown capability, Skill, or
    /// model rejects the whole candidate off-side and the previous complete
    /// generation stays authoritative.
    #[must_use]
    pub fn with_subagent_catalog(mut self, catalog: AgentCatalog) -> Self {
        self.subagents = catalog;
        self
    }

    /// Adds the independent profile admissions to the candidate generation.
    #[must_use]
    pub fn with_workflow_admission(mut self, workflow: BTreeSet<SubagentName>) -> Self {
        self.subagent_workflow = workflow;
        self
    }

    /// Adds inert package discovery to the atomic candidate.
    #[must_use]
    pub fn with_managed_python_catalog(mut self, catalog: ManagedPythonCatalog) -> Self {
        self.managed_python = catalog;
        self
    }

    /// Adds the compiled Workflow catalog to the candidate generation.
    #[must_use]
    pub fn with_workflow_catalog(mut self, catalog: WorkflowCatalog) -> Self {
        self.workflows = catalog;
        self
    }

    /// The candidate capability plane a loader validates its catalog
    /// against, before either half is published.
    #[must_use]
    pub const fn capability_candidate(&self) -> &PreparedCapabilityCandidate {
        &self.capability
    }

    /// The candidate generation's named-subagent catalog.
    #[must_use]
    pub const fn subagent_catalog(&self) -> &AgentCatalog {
        &self.subagents
    }

    /// The candidate generation's compiled Workflow catalog.
    ///
    /// A loader validates Workflow-owned static references — including each
    /// Agent node's trusted invocation override (Issue #258) — against the
    /// same candidate it is about to publish.
    #[must_use]
    pub const fn workflow_catalog(&self) -> &WorkflowCatalog {
        &self.workflows
    }

    pub(crate) fn into_parts(self) -> (PreparedCapabilityCandidate, PreparedRuntimeResourceData) {
        let Self {
            project_context_files,
            agent_profile,
            context_assembly,
            subagents,
            subagent_workflow,
            workflows,
            managed_python,
            capability,
        } = self;
        let capability_availability = capability.availability().clone();
        (
            capability,
            PreparedRuntimeResourceData {
                project_context_files,
                agent_profile,
                context_assembly,
                subagents,
                subagent_workflow,
                workflows,
                managed_python,
                capability_availability,
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
    /// Optional authoritative local document context for offline diagnostics.
    pub source_file: Option<PathBuf>,
    pub field_path: Option<String>,
    /// Static category authored by the loader, never interpolated resource contents.
    pub diagnostic_reason: Option<&'static str>,
    pub inspection: Box<ResourceDiagnosticContext>,
}

/// Optional source context. Kept compact at the ordinary resource error boundary.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResourceDiagnosticContext {
    pub category: Option<&'static str>,
    pub correction: Option<&'static str>,
    pub detail: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
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
        Self {
            message,
            source_file: None,
            field_path: None,
            diagnostic_reason: None,
            inspection: Box::default(),
        }
    }
    /// Attach context at the resource owner, without parsing diagnostic text.
    #[must_use]
    pub fn at(mut self, file: &Path, field: impl Into<String>) -> Self {
        self.source_file = Some(file.into());
        self.field_path = Some(field.into());
        self
    }
    /// Attach a safe static explanation for offline diagnostics.
    #[must_use]
    pub fn because(mut self, reason: &'static str) -> Self {
        self.diagnostic_reason = Some(reason);
        self
    }
}

impl core::fmt::Display for RuntimeResourceLoadError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeResourceLoadError {}

/// Loads project context only from the resolved, trusted workspace boundary.
/// It contributes at most one file using first-match precedence:
/// `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD`.
/// Unrelated ancestor instructions are outside this workspace's authority.
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
    let files = load_context_file_from_directory(&workspace)?
        .into_iter()
        .collect();
    Ok(files)
}

/// Project resource reads follow canonical targets, never lexical prefixes.
/// Missing resources remain the loader's error; existing ancestors are still
/// checked so a missing leaf cannot hide an external symlink. No syscall-race
/// protection is claimed against an actively hostile local OS user.
pub(crate) fn validate_project_resource_path(
    workspace: &Path,
    path: &Path,
) -> Result<(), RuntimeResourceLoadError> {
    fn target(path: &Path) -> std::io::Result<PathBuf> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => std::fs::canonicalize(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let parent = path.parent().ok_or(e)?;
                let name = path
                    .file_name()
                    .ok_or_else(|| std::io::Error::other("invalid resource path"))?;
                Ok(target(parent)?.join(name))
            }
            Err(e) => Err(e),
        }
    }
    // The owner supplies the canonical workspace captured at launch. Do not
    // recanonicalize that authority: replacing the workspace itself with a
    // symlink must not transfer its trust to the new target.
    let resolved = target(path).map_err(|e| {
        RuntimeResourceLoadError::new(format!(
            "cannot authorize project resource {}: {e}",
            path.display()
        ))
    })?;
    if !resolved.starts_with(workspace) {
        return Err(RuntimeResourceLoadError::new(format!(
            "project resource {} is outside trusted workspace {}",
            path.display(),
            workspace.display()
        )));
    }
    Ok(())
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
        validate_project_resource_path(directory, &path)
            .map_err(|error| error.at(&path, "projectInstructions"))?;
        let bytes = crate::bounded_file::read_bounded(&path).map_err(|error| {
            RuntimeResourceLoadError::new(format!(
                "cannot read project context file {}: {error}",
                path.display()
            ))
            .at(&path, "projectInstructions")
        })?;
        let mut content = String::from_utf8(bytes).map_err(|error| {
            RuntimeResourceLoadError::new(format!(
                "project context file {} is not UTF-8: {error}",
                path.display()
            ))
            .at(&path, "projectInstructions")
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
    fn project_context_order_is_root_to_leaf_on_canonical_paths() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path().join("root");
        let middle = root.join("middle");
        let child = middle.join("leaf");
        std::fs::create_dir_all(&child).expect("workspace");
        std::fs::write(root.join("AGENTS.md"), "\u{feff}root agents").expect("root AGENTS");
        std::fs::write(middle.join("AGENTS.MD"), "middle agents").expect("middle AGENTS");
        std::fs::write(child.join("AGENTS.md"), "shadowed child agents").expect("child AGENTS");
        std::fs::write(child.join("AGENTS.override.md"), "child override").expect("child override");

        let files = load_project_context_files(&child).expect("project context");
        let canonical_root = std::fs::canonicalize(&root).expect("canonical root");
        assert_eq!(
            files
                .iter()
                .filter_map(|file| {
                    let parent = file.path.parent()?;
                    parent
                        .strip_prefix(&canonical_root)
                        .ok()
                        .map(|relative| (relative.to_path_buf(), file.content.clone()))
                })
                .collect::<Vec<_>>(),
            vec![(
                std::path::PathBuf::from("middle/leaf"),
                "child override".to_owned(),
            ),]
        );
        assert_eq!(
            concatenate_project_instructions(&files),
            Some("child override".to_owned())
        );
    }

    #[test]
    fn project_context_override_beats_agents_and_agents_beats_claude() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace = directory.path().join("workspace");
        let agents_directory = workspace.join("agents");
        std::fs::create_dir_all(&agents_directory).expect("workspace");

        std::fs::write(workspace.join("AGENTS.override.md"), "override wins").expect("override");
        std::fs::write(workspace.join("AGENTS.md"), "ordinary is shadowed").expect("AGENTS");
        std::fs::write(workspace.join("CLAUDE.md"), "claude is shadowed").expect("CLAUDE");
        std::fs::write(agents_directory.join("AGENTS.md"), "agents beats claude")
            .expect("nested AGENTS");
        std::fs::write(
            agents_directory.join("CLAUDE.md"),
            "nested claude is shadowed",
        )
        .expect("nested CLAUDE");

        let files = load_project_context_files(&agents_directory).expect("project context");
        let canonical_workspace = std::fs::canonicalize(&workspace).expect("canonical workspace");
        let selected = files
            .iter()
            .filter(|file| file.path.starts_with(&canonical_workspace))
            .map(|file| file.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(selected, vec!["agents beats claude"]);
    }

    #[test]
    fn project_context_filename_variants_are_recognized_without_case_only_files() {
        for (directory_name, filename, content) in [
            ("agents-lower", "AGENTS.md", "agents lower"),
            ("agents-upper", "AGENTS.MD", "agents upper"),
            ("claude-lower", "CLAUDE.md", "claude lower"),
            ("claude-upper", "CLAUDE.MD", "claude upper"),
        ] {
            let directory = tempfile::tempdir().expect("temporary directory");
            let workspace = directory.path().join(directory_name);
            std::fs::create_dir_all(&workspace).expect("workspace");
            std::fs::write(workspace.join(filename), content).expect("context file");

            let files = load_project_context_files(&workspace).expect("project context");
            let canonical_workspace =
                std::fs::canonicalize(&workspace).expect("canonical workspace");
            let selected = files
                .iter()
                .filter(|file| file.path.starts_with(&canonical_workspace))
                .collect::<Vec<_>>();
            assert_eq!(selected.len(), 1, "variant {filename} was selected");
            assert_eq!(selected[0].content, content);
            let selected_filename = selected[0]
                .path
                .file_name()
                .expect("filename")
                .to_string_lossy();
            assert!(
                selected_filename.eq_ignore_ascii_case(filename),
                "the selected spelling must be the requested variant modulo filesystem case: selected={selected_filename}, requested={filename}"
            );
        }
    }

    #[test]
    fn project_context_bom_removal_is_deterministic() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("CLAUDE.MD"), b"\xef\xbb\xbfwith bom").expect("context file");

        let files = load_project_context_files(&workspace).expect("project context");
        let canonical_workspace = std::fs::canonicalize(&workspace).expect("canonical workspace");
        let selected = files
            .iter()
            .find(|file| file.path.starts_with(&canonical_workspace))
            .expect("context file");
        assert_eq!(selected.content, "with bom");
    }
}
