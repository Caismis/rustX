//! Reusable user-source ownership and fresh explicit-Session resolution.
//! No provider, Session runtime, or external source is prepared here.
//! Source content is reread per call; admitted configurations own their capture.

pub mod application;
pub mod settings;

use crate::bounded_file::read_bounded;
use crate::runtime::capability_inspection::{ResourceDefinition, ResourceFamily};
use settings::SourceScope;
use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::authoring::RuntimeLayer;
use super::config::CurrentRuntimeConfig;
use super::diagnostics::LaunchFailure;
use crate::model::catalog::ModelCatalog;

/// Filesystem locations and controls produced by the launch resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLocations {
    /// The model-visible workspace root.
    pub workspace: PathBuf,
    /// The exact runtime-private root from which disjoint private
    /// subdirectories are derived. Child conversation stores live below its
    /// stable `subagents/<conversation-id>` semantic directory; child
    /// execution incarnations remain private and disposable.
    pub runtime_root: PathBuf,
}

impl SessionLocations {
    /// Prospective environment location; this does not grant storage authority.
    #[must_use]
    pub fn environment_store_root(&self) -> PathBuf {
        self.runtime_root.join("environments")
    }

    /// The capability environment store of one independent conversation
    /// lineage. Branches do not share mutable environment materialization.
    #[must_use]
    pub fn environment_store_root_for(
        &self,
        conversation_id: &crate::runtime::identity::ConversationId,
    ) -> PathBuf {
        self.environment_store_root().join(conversation_id.as_str())
    }
}

/// Stable user/process source bindings. No parsed effective configuration or credentials.
#[derive(Debug, Clone)]
pub struct UserConfigSources {
    pub home_directory: PathBuf,
    pub config_path: PathBuf,
    pub runtime_root: PathBuf,
}

/// Reusable source owner. Each resolution rereads current files; snapshots never
/// borrow mutable manager state. The manager has no process launch directory.
#[derive(Debug, Clone)]
pub struct UserConfigManager {
    sources: UserConfigSources,
    runtime_root_origin: Origin,
    #[cfg(test)]
    pub(crate) test_hooks: ConfigurationHooks,
}

#[cfg(test)]
type ConfigurationHookMap = BTreeMap<&'static str, Box<dyn FnOnce() + Send>>;
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct ConfigurationHooks(std::sync::Arc<std::sync::Mutex<ConfigurationHookMap>>);
#[cfg(test)]
impl std::fmt::Debug for ConfigurationHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfigurationHooks")
    }
}
#[cfg(test)]
impl ConfigurationHooks {
    pub(crate) fn insert(&self, point: &'static str, hook: impl FnOnce() + Send + 'static) {
        self.0.lock().unwrap().insert(point, Box::new(hook));
    }
    pub(crate) fn reach(&self, point: &'static str) {
        let hook = self.0.lock().unwrap().remove(point);
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// Intentional inputs for one prospective Session. Omission remains omission.
/// Paths are absolute; cwd is execution/configuration context, not a sandbox.
#[derive(Clone)]
pub struct SessionConfigInput {
    pub cwd: PathBuf,
    pub model: Option<crate::model::session::SessionModelConfig>,
}
impl SessionConfigInput {
    #[must_use]
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd, model: None }
    }
}
/// Canonical authoring identity determines the base of relative user bindings.
pub(super) fn canonical_settings_source(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("user settings source must be absolute".into());
    }
    normalize_missing(path)
}

impl UserConfigManager {
    /// Bind absolute user roots without reading configuration or credentials.
    /// # Errors
    /// Relative source paths are rejected rather than interpreted using process cwd.
    pub fn new(mut sources: UserConfigSources) -> Result<Self, String> {
        for path in [
            &sources.home_directory,
            &sources.config_path,
            &sources.runtime_root,
        ] {
            if !path.is_absolute() {
                return Err("user source bindings must be absolute".into());
            }
        }
        sources.home_directory = normalize_missing(&sources.home_directory)?;
        sources.config_path = canonical_settings_source(&sources.config_path)?;
        sources.runtime_root = normalize_missing(&sources.runtime_root)?;
        let explicit = Origin::Process {
            base: sources.home_directory.join("rustx"),
        };
        Ok(Self {
            sources,
            runtime_root_origin: explicit,
            #[cfg(test)]
            test_hooks: ConfigurationHooks::default(),
        })
    }

    /// Bootstrap process bindings once using the canonical user document and
    /// optional host overrides. Supplied sources provide the default locations;
    /// no Session cwd participates in this operation.
    /// # Errors
    /// Rejects invalid user authoring and nonabsolute source bindings.
    pub fn bootstrap(
        sources: UserConfigSources,
        runtime_root: Option<PathBuf>,
    ) -> Result<Self, LaunchFailure> {
        let mut sources = sources;
        sources.config_path = canonical_settings_source(&sources.config_path)?;
        if let Some(root) = runtime_root {
            if !root.is_absolute() {
                return Err("runtime root must be absolute".into());
            }
            sources.runtime_root = normalize_missing(&root)?;
        }
        let manager = Self::new(sources)?;
        Ok(manager)
    }
    /// Canonical native process bindings, without a wire projection.
    pub(crate) fn source_bindings(&self) -> &UserConfigSources {
        &self.sources
    }

    /// User-scoped product root, independent of any Session or process cwd.
    #[must_use]
    pub fn runtime_root(&self) -> &Path {
        &self.sources.runtime_root
    }

    /// Read process policy from the canonical user document only.
    /// # Errors
    /// Invalid user authoring or policy is rejected before traffic admission.
    pub fn app_server_policy(&self) -> Result<super::app_server_policy::AppServerPolicy, String> {
        let policy = read_layer(&self.sources.config_path, false, false)
            .map_err(|error| error.to_string())?
            .app_server
            .unwrap_or_default();
        policy.validate()?;
        Ok(policy)
    }

    /// Resolve only locations; no configuration, credentials, or runtime creation.
    /// # Errors
    /// Invalid explicit paths and Tool restrictions are rejected.
    pub fn resolve_locations(
        &self,
        request: &SessionConfigInput,
    ) -> Result<(SessionLocations, String), String> {
        if !request.cwd.is_absolute() {
            return Err("Session cwd must be absolute".into());
        }
        let workspace = canonical_directory(&request.cwd)?;
        let identity = workspace_identity(&workspace);
        let runtime_root = self.sources.runtime_root.clone();
        if normalize_missing(&runtime_root)? != runtime_root {
            return Err("bound runtime_root physical authority changed".into());
        }
        Ok((
            SessionLocations {
                workspace,
                runtime_root,
            },
            identity,
        ))
    }
}

/// Values are never included: provenance cannot expose credentials or environment values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Builtin,
    User { document: PathBuf, base: PathBuf },
    Workspace { document: PathBuf, base: PathBuf },
    Process { base: PathBuf },
}

/// Validated Session configuration authority consumed by the existing native composition owner.
#[derive(Debug, Clone)]
pub struct AdmittedSessionConfig {
    pub(crate) binding_revision: u64,
    pub(crate) credentials: crate::credentials::CredentialSnapshot,
    prospective: Box<ProspectiveSessionConfig>,
}

/// Shared authority, source overlay and path capture, before domain validation.
#[derive(Clone)]
struct LayerCapture {
    revisions: BTreeMap<PathBuf, String>,
    locations: SessionLocations,
    identity: String,
    merged: RuntimeLayer,
    provenance: BTreeMap<String, Origin>,
    project_resources: Vec<PathBuf>,
}

/// Canonical source/configuration capture before any launch resource preparation.
pub(crate) struct SourceCapture {
    definitions: Vec<crate::runtime::capability_inspection::ResourceDefinition>,
    diagnostics: Vec<crate::runtime::capability_inspection::ResourceDiagnostic>,
    revisions: BTreeMap<PathBuf, String>,
    effective: RuntimeLayer,
    locations: SessionLocations,
    identity: String,
    pub(crate) config: CurrentRuntimeConfig,
    pub(crate) models: ModelCatalog,
    pub(crate) provenance: BTreeMap<String, Origin>,
    project_resources: Vec<PathBuf>,
}

/// Immutable prospective Session settings and statically resolved resources.
/// Admission adds credentials; native composition owns external preparation.
#[derive(Clone)]
pub struct ProspectiveSessionConfig {
    pub(crate) component_revisions: BTreeMap<application::ApplyUnit, String>,
    pub(crate) source_revisions: BTreeMap<PathBuf, String>,
    pub(crate) effective: RuntimeLayer,
    pub(crate) root_agent_project_files: Vec<crate::runtime::resources::ProjectContextFile>,
    pub(crate) skill_discovery: crate::skills::SkillDiscoveryOutcome,
    pub(crate) project_context_files: Vec<crate::runtime::resources::ProjectContextFile>,
    pub(crate) inspection: crate::runtime::capability_inspection::CapabilityInspection,
    pub(crate) input: SessionConfigInput,
    pub(crate) sources: UserConfigSources,
    pub(crate) managed_python: crate::runtime::resources::ManagedPythonCatalog,
    pub(crate) workflows: std::sync::Arc<crate::runtime::workflow::WorkflowCatalog>,
    pub(crate) subagents: crate::runtime::subagent::AgentCatalog,
    pub(crate) agent_root: PathBuf,
    pub(crate) role_sources:
        BTreeMap<crate::runtime::subagent::SubagentName, super::agent_resources::AgentSource>,
    pub(crate) locations: SessionLocations,
    pub(crate) config: std::sync::Arc<CurrentRuntimeConfig>,
    pub(crate) models: ModelCatalog,
    pub(crate) provenance: BTreeMap<String, Origin>,
    pub(crate) identity: String,
    pub(crate) skill_sources: Vec<crate::skills::AutomaticSkillRoot>,
    /// Project-origin paths retain their authority even after becoming absolute.
    project_resources: Vec<PathBuf>,
}

/// Captured authority for the two independent policy units, including when
/// resource resolution fails. Both source and runtime composition consume it.
#[derive(Clone)]
pub(crate) struct IndependentPolicy {
    pub(crate) config: CurrentRuntimeConfig,
    effective: RuntimeLayer,
    provenance: BTreeMap<String, Origin>,
    revision: String,
}
impl std::ops::Deref for IndependentPolicy {
    type Target = CurrentRuntimeConfig;
    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

/// Fixed unit ownership, shared by source and runtime projections. Replacing
/// the owned keys also removes provenance for fields no longer authored.
pub(crate) fn copy_unit_provenance(
    target: &mut BTreeMap<String, Origin>,
    source: &BTreeMap<String, Origin>,
    unit: application::ApplyUnit,
) {
    use application::ApplyUnit;
    let prefixes: &[&str] = match unit {
        ApplyUnit::ExecutionPolicy => &[
            "approval_mode",
            "model_timeout_policy",
            "tool_deadline_policy",
        ],
        ApplyUnit::SharedCapacity => &["subagents"],
        ApplyUnit::Instructions => &["agent.instructions", "agent.agents_md", "context"],
        ApplyUnit::Provider => &["agent.model", "models", "providers"],
        ApplyUnit::Capabilities | ApplyUnit::ProcessBindings => unreachable!("not a composed unit"),
    };
    let owned = |key: &str| {
        prefixes.iter().any(|prefix| {
            key == *prefix
                || key
                    .strip_prefix(prefix)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    };
    target.retain(|key, _| !owned(key));
    target.extend(
        source
            .iter()
            .filter(|(key, _)| owned(key))
            .map(|(key, value)| (key.clone(), value.clone())),
    );
}
impl IndependentPolicy {
    pub(crate) fn compose_execution_policy<P, E>(
        &self,
        config: &mut CurrentRuntimeConfig,
        effective: &mut RuntimeLayer<P, E>,
        provenance: &mut BTreeMap<String, Origin>,
        revisions: &mut BTreeMap<application::ApplyUnit, String>,
    ) {
        let unchanged = config.approval_mode == self.config.approval_mode
            && config.model_timeout_policy == self.config.model_timeout_policy
            && config.tool_deadline_policy == self.config.tool_deadline_policy
            && effective.approval_mode == self.effective.approval_mode
            && effective.model_timeout_policy == self.effective.model_timeout_policy
            && effective.tool_deadline_policy == self.effective.tool_deadline_policy;
        let old_provenance = provenance.clone();
        config.approval_mode = self.config.approval_mode;
        config.model_timeout_policy = self.config.model_timeout_policy;
        config.tool_deadline_policy = self.config.tool_deadline_policy;
        effective.approval_mode = self.effective.approval_mode;
        effective
            .model_timeout_policy
            .clone_from(&self.effective.model_timeout_policy);
        effective
            .tool_deadline_policy
            .clone_from(&self.effective.tool_deadline_policy);
        copy_unit_provenance(
            provenance,
            &self.provenance,
            application::ApplyUnit::ExecutionPolicy,
        );
        if !unchanged || *provenance != old_provenance {
            revisions.insert(
                application::ApplyUnit::ExecutionPolicy,
                self.revision.clone(),
            );
        }
    }
    pub(crate) fn compose_shared_capacity<P, E>(
        &self,
        config: &mut CurrentRuntimeConfig,
        effective: &mut RuntimeLayer<P, E>,
        provenance: &mut BTreeMap<String, Origin>,
        revisions: &mut BTreeMap<application::ApplyUnit, String>,
    ) {
        let unchanged = config.subagents == self.config.subagents
            && effective.subagents == self.effective.subagents;
        let old_provenance = provenance.clone();
        config.subagents = self.config.subagents.clone();
        effective.subagents.clone_from(&self.effective.subagents);
        copy_unit_provenance(
            provenance,
            &self.provenance,
            application::ApplyUnit::SharedCapacity,
        );
        if !unchanged || *provenance != old_provenance {
            revisions.insert(
                application::ApplyUnit::SharedCapacity,
                self.revision.clone(),
            );
        }
    }
}

impl ProspectiveSessionConfig {
    pub(crate) fn compose_execution_policy(&mut self, desired: &IndependentPolicy) {
        desired.compose_execution_policy(
            std::sync::Arc::make_mut(&mut self.config),
            &mut self.effective,
            &mut self.provenance,
            &mut self.component_revisions,
        );
    }

    pub(crate) fn compose_shared_capacity(&mut self, desired: &IndependentPolicy) {
        desired.compose_shared_capacity(
            std::sync::Arc::make_mut(&mut self.config),
            &mut self.effective,
            &mut self.provenance,
            &mut self.component_revisions,
        );
    }

    /// The physical/profile closure admitted by Root selection and Workflow
    /// Agent nodes (including nested nodes and invocation overrides). Unrelated
    /// catalog entries remain inert.
    fn admitted_agent_dependencies(
        &self,
    ) -> std::collections::BTreeSet<crate::runtime::subagent::SubagentName> {
        let mut names: std::collections::BTreeSet<_> =
            self.config.agent.agents.iter().cloned().collect();
        for id in &self.config.agent.workflows {
            if let Some(entry) = self.workflows.entries().get(id) {
                names.extend(
                    entry
                        .source
                        .agent_nodes()
                        .iter()
                        .map(|node| node.profile.clone()),
                );
            }
        }
        names
    }

    /// Compare independently prepared capability inputs, excluding context and
    /// provider units. Authored revisions and shadowed/unselected metadata are
    /// not effective resource identities.
    pub(crate) fn same_capabilities(&self, other: &Self) -> bool {
        let normalized = |source: &Self| {
            let mut config = source.config.as_ref().clone();
            config.approval_mode = crate::runtime::ApprovalMode::default();
            config.model_timeout_policy = super::config::ModelTimeoutPolicyDocument::default();
            config.tool_deadline_policy = super::config::ToolDeadlinePolicyDocument::default();
            config.subagents = super::config::SubagentsDocument::default();
            config.context = super::config::ContextPolicyDocument::default();
            config.agent.instructions.clear();
            config.agent.model = None;
            config.agent.agents_md = super::config::AgentProjectInstructionsDocument::default();
            // Inert definitions are not resource demand.
            let demand = super::composition::admitted_source_demand(
                &config,
                &source.subagents,
                &source.workflows,
                source.managed_python.clone(),
            );
            config.mcp_servers.retain(|id, _| {
                demand
                    .sources
                    .contains(&crate::capabilities::ToolSourceId::Mcp(id.clone()))
            });
            (config, demand.sources)
        };
        let (a, demand_a) = normalized(self);
        let (b, demand_b) = normalized(other);
        if a != b || demand_a != demand_b {
            return false;
        }
        let agents = self.admitted_agent_dependencies();
        if agents != other.admitted_agent_dependencies()
            || agents
                .iter()
                .any(|name| self.subagents.get(name) != other.subagents.get(name))
        {
            return false;
        }
        if a.agent.workflows.iter().any(|id| {
            let view = |source: &Self| {
                source
                    .workflows
                    .entries()
                    .get(id)
                    .map(|entry| entry.source.tool_identity())
            };
            view(self) != view(other)
        }) {
            return false;
        }
        if demand_a.iter().any(|id| {
            self.managed_python.packages().get(id) != other.managed_python.packages().get(id)
        }) {
            return false;
        }
        // Skill dependencies are materialized as a shared environment, so even
        // non-advertised discovered packages participate in this closure today.
        self.skill_discovery.packages == other.skill_discovery.packages
    }

    pub(crate) fn same_context(&self, other: &Self) -> bool {
        self.config.agent.instructions == other.config.agent.instructions
            && self.config.agent.agents_md == other.config.agent.agents_md
            && self.config.context == other.config.context
            && self.project_context_files == other.project_context_files
            && self.root_agent_project_files == other.root_agent_project_files
    }

    pub(crate) fn same_provider(&self, other: &Self) -> bool {
        let selection = self.session_model();
        selection == other.session_model()
            && self.models.same_binding(&other.models, &selection.model)
            && selection
                .summary_selection()
                .is_none_or(|summary| self.models.same_binding(&other.models, &summary.model))
            && self.admitted_agent_dependencies() == other.admitted_agent_dependencies()
            && self.admitted_agent_dependencies().iter().all(|name| {
                let model = |source: &Self| {
                    source
                        .subagents
                        .get(name)
                        .and_then(|definition| definition.profile().model.clone())
                };
                let selected = model(self);
                selected == model(other)
                    && selected.as_ref().is_none_or(|model| {
                        self.models.same_binding(&other.models, &model.model)
                            && model.summary_selection().is_none_or(|summary| {
                                self.models.same_binding(&other.models, &summary.model)
                            })
                    })
            })
    }

    pub(crate) fn retaining_context_from(mut self, adopted: &Self) -> Self {
        let config = std::sync::Arc::make_mut(&mut self.config);
        config
            .agent
            .instructions
            .clone_from(&adopted.config.agent.instructions);
        config
            .agent
            .agents_md
            .clone_from(&adopted.config.agent.agents_md);
        config.context = adopted.config.context;
        self.project_context_files
            .clone_from(&adopted.project_context_files);
        self.root_agent_project_files
            .clone_from(&adopted.root_agent_project_files);
        // These paths are exclusively the Workspace-authored agents_md inputs
        // retained for physical-authority validation after path rebasing.
        self.project_resources
            .clone_from(&adopted.project_resources);
        let context = self.effective.agent.get_or_insert_with(Default::default);
        context.instructions = adopted
            .effective
            .agent
            .as_ref()
            .and_then(|agent| agent.instructions.clone());
        context.agents_md = adopted
            .effective
            .agent
            .as_ref()
            .and_then(|agent| agent.agents_md.clone());
        self.effective
            .context
            .clone_from(&adopted.effective.context);
        copy_unit_provenance(
            &mut self.provenance,
            &adopted.provenance,
            application::ApplyUnit::Instructions,
        );
        self.component_revisions.insert(
            application::ApplyUnit::Instructions,
            adopted.component_revisions[&application::ApplyUnit::Instructions].clone(),
        );
        self
    }

    /// Capture process credentials after coherent User and Workspace resolution.
    /// # Errors
    /// Rejects changed physical resource authority before credential capture.
    pub fn admit(
        self,
        credentials: impl FnOnce() -> crate::credentials::CredentialSnapshot,
    ) -> Result<AdmittedSessionConfig, String> {
        self.validate_resource_authority()?;
        Ok(AdmittedSessionConfig {
            binding_revision: 1,
            credentials: credentials(),
            prospective: Box::new(self),
        })
    }
    /// Recheck physical targets immediately before resource preparation. This
    /// never rereads launch documents and is not an execution filesystem sandbox.
    pub(crate) fn validate_resource_authority(&self) -> Result<(), String> {
        if std::fs::canonicalize(&self.workspace).as_ref().ok() != Some(&self.workspace) {
            return Err("Workspace physical binding changed after capture".into());
        }
        let agents = self.admitted_agent_dependencies();
        for role in self
            .role_sources
            .values()
            .filter(|role| agents.contains(&role.identity))
        {
            let boundary = if role.layer == "workspace" {
                &self.workspace
            } else {
                &self.agent_root
            };
            crate::runtime::resources::validate_project_resource_path(boundary, &role.selected)
                .map_err(|e| e.to_string())?;
        }
        for path in &self.project_resources {
            crate::runtime::resources::validate_project_resource_path(&self.workspace, path)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// The immutable validated settings this launch resolved.
    #[must_use]
    pub fn config(&self) -> &CurrentRuntimeConfig {
        &self.config
    }

    /// Prospective Session model after validating deliberate Session intent.
    /// The Root default and its authored provenance remain separate facts.
    #[must_use]
    pub fn session_model(&self) -> &crate::model::session::SessionModelConfig {
        self.input
            .model
            .as_ref()
            .unwrap_or_else(|| self.config.initial_model())
    }

    /// Native current-file analysis. This is prospective and has no authority
    /// over an already-published runtime generation.
    #[must_use]
    pub const fn resource_inspection(
        &self,
    ) -> &crate::runtime::capability_inspection::CapabilityInspection {
        &self.inspection
    }

    /// The effective source provenance of every Skill identity this launch
    /// admitted, including what each one shadowed (Issue #280).
    #[must_use]
    pub fn skill_provenance(&self) -> &[crate::skills::SkillProvenance] {
        &self.inspection.skills
    }

    /// The typed, canonically ordered Skill discovery facts of this launch.
    ///
    /// Unused malformed packages warn. Selecting an unavailable identity in
    /// an Agent profile fails at that profile's resolution or admission boundary.
    #[must_use]
    pub fn skill_diagnostics(&self) -> &[crate::skills::SkillDiagnostic] {
        &self.inspection.skill_diagnostics
    }

    /// Safe field origins, without configuration values.
    #[must_use]
    pub const fn provenance(&self) -> &BTreeMap<String, Origin> {
        &self.provenance
    }

    /// Stable identity of the canonical workspace path.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

impl AdmittedSessionConfig {
    pub(crate) fn with_model(&self, model: crate::model::session::SessionModelConfig) -> Self {
        let mut retained = self.clone();
        retained.prospective.input.model = Some(model);
        retained.binding_revision = retained
            .binding_revision
            .checked_add(1)
            .expect("Session binding revision exhausted");
        retained
    }
    pub(crate) fn with_execution_policy(&self, desired: &IndependentPolicy) -> Self {
        let mut retained = self.clone();
        retained.prospective.compose_execution_policy(desired);
        retained
    }
    pub(crate) fn with_shared_capacity(&self, desired: &IndependentPolicy) -> Self {
        let mut retained = self.clone();
        retained.prospective.compose_shared_capacity(desired);
        retained
    }
}

impl std::ops::Deref for AdmittedSessionConfig {
    type Target = ProspectiveSessionConfig;
    fn deref(&self) -> &Self::Target {
        &self.prospective
    }
}

impl std::ops::Deref for ProspectiveSessionConfig {
    type Target = SessionLocations;
    fn deref(&self) -> &Self::Target {
        &self.locations
    }
}

#[allow(
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::unnecessary_debug_formatting
)] // derived paths always have parents; debug escapes diagnostic paths
impl UserConfigManager {
    /// Capture policies independently of fallible context/resource preparation.
    /// Both consume the same immutable source overlay. Policy publication never
    /// borrows a second read merely because another closure fails.
    pub(crate) fn capture_application(
        &self,
        request: &SessionConfigInput,
    ) -> Result<application::CapturedApplication, String> {
        self.capture_application_at_boundary(request, || {})
    }

    pub(super) fn capture_application_at_boundary(
        &self,
        request: &SessionConfigInput,
        after_layers: impl FnOnce(),
    ) -> Result<application::CapturedApplication, String> {
        let layers = self
            .capture_layers(request, None)
            .map_err(|error| error.to_string())?;
        after_layers();
        let policy = layers
            .merged
            .clone()
            .resolve()
            .map_err(|error| error.clone())?;
        policy.timeout_policy().map_err(|error| error.to_string())?;
        policy
            .tool_deadline_policy()
            .map_err(|error| error.to_string())?;
        let revisions = layers.revisions.clone();
        let mut policy_origins = layers.provenance.clone();
        RuntimeLayer::record_default_origins(&policy, &mut policy_origins);
        let policy_effective = layers.merged.clone();
        let process = layers.merged.app_server.clone().unwrap_or_default();
        process.validate()?;
        let context = self
            .resolve_captured_layers(layers)
            .and_then(Self::resolve_runtime_configuration)
            .and_then(|capture| self.resolve_resources(request, capture))
            .map_err(|error| error.diagnostic.reason);
        for (path, expected) in &revisions {
            let actual = read_layer_revision(path, false, path != &self.sources.config_path)
                .map_err(|error| error.to_string())?
                .1;
            if &actual != expected {
                return Err("configuration changed during capture; rescan to retry".into());
            }
        }
        let manifest = context
            .as_ref()
            .map_or(&revisions, |capture| &capture.source_revisions);
        let revision = source_manifest_revision(manifest);
        let policy = IndependentPolicy {
            config: policy,
            effective: policy_effective,
            provenance: policy_origins,
            revision: revision.clone(),
        };
        Ok(application::CapturedApplication {
            policy,
            process,
            revision,
            context,
        })
    }

    /// Resolve current canonical sources for exactly this Session context.
    /// No credentials or external preparation are acquired here.
    ///
    /// # Errors
    /// Rejects invalid paths, authoring, authority, and semantic configuration.
    pub fn resolve_session(
        &self,
        request: &SessionConfigInput,
    ) -> Result<ProspectiveSessionConfig, LaunchFailure> {
        let resolved = self.resolve_model_candidate(request, None)?;
        let resolved = Self::resolve_runtime_configuration(resolved)?;
        self.resolve_resources(request, resolved)
    }

    pub(crate) fn capture_session_model(
        &self,
        adopted: &AdmittedSessionConfig,
        selection: crate::model::session::SessionModelConfig,
    ) -> Result<ProspectiveSessionConfig, String> {
        let mut input = adopted.input.clone();
        input.model = Some(selection);
        let source = self
            .capture_layers(&input, None)
            .map_err(|error| error.to_string())?;
        for (path, expected) in &source.revisions {
            let actual = read_layer_revision(path, false, path != &self.sources.config_path)
                .map_err(|error| error.to_string())?
                .1;
            if &actual != expected {
                return Err("model inputs changed during capture; retry".into());
            }
        }
        let mut candidate = adopted.prospective.as_ref().clone();
        candidate.input = input;
        candidate.models = ModelCatalog::from_document(
            crate::model::authoring::Catalog {
                schema_version: crate::model::catalog::MODEL_CATALOG_SCHEMA_VERSION,
                providers: source.merged.providers.clone().unwrap_or_default(),
                models: source.merged.models.clone().unwrap_or_default(),
            }
            .into(),
        )
        .map_err(|error| error.to_string())?;
        candidate.effective.models = source.merged.models;
        candidate.effective.providers = source.merged.providers;
        candidate.source_revisions.extend(source.revisions);
        Ok(candidate)
    }

    fn resolve_model_candidate(
        &self,
        request: &SessionConfigInput,
        candidate: Option<(&Path, &[u8])>,
    ) -> Result<SourceCapture, LaunchFailure> {
        let capture = self.capture_sources(request, candidate)?;
        let config = &capture.config;
        config.context.validate().map_err(|e| e.to_string())?;
        let (primary, summary) = crate::model::session::analyze_session_model_config(
            &capture.models,
            request
                .model
                .as_ref()
                .unwrap_or_else(|| config.initial_model()),
        )
        .map_err(|e| e.to_string())?;
        let summary = summary.as_ref().unwrap_or(&primary);
        config
            .context_policy()
            .validate_budgets(
                (primary.context_window, primary.max_output_tokens),
                (summary.context_window, summary.max_output_tokens),
            )
            .map_err(|error| {
                LaunchFailure::at(
                    Some(self.sources.config_path.clone()),
                    "context",
                    "context budgets cannot fit the selected models",
                    "correct reserves, recent-history budget, or explicit model limits",
                    error.message,
                )
            })?;
        Ok(capture)
    }

    // Execution-domain validation belongs only to full Session resolution.
    fn resolve_runtime_configuration(
        capture: SourceCapture,
    ) -> Result<SourceCapture, LaunchFailure> {
        let config = &capture.config;
        config.validate().map_err(|e| e.to_string())?;
        config
            .tool_deadline_policy
            .to_policy()
            .map_err(|e| e.clone())?;
        config.tool_environment().map_err(|e| e.to_string())?;
        Ok(capture)
    }

    // One strict source parse, authority, overlay and provenance owner. Lowering
    // is structural; semantic validation belongs to the consuming domain.
    fn capture_layers(
        &self,
        request: &SessionConfigInput,
        candidate: Option<(&Path, &[u8])>,
    ) -> Result<LayerCapture, LaunchFailure> {
        let read = |path: &Path, required, project| match candidate {
            Some((target, bytes))
                if normalize_missing(path).is_ok_and(|canonical| canonical == target) =>
            {
                parse_layer(path, bytes, project)
                    .map(|layer| (layer, super::settings::revision(Some(bytes))))
            }
            _ => read_layer_revision(path, required, project),
        };
        let host = &self.sources;
        let (locations, identity) = self.resolve_locations(request)?;
        let user_path = host.config_path.clone();
        let project_path = locations.workspace.join("rustx.toml");
        let (user, user_revision) = read(&user_path, false, false)?;

        // Both ordinary documents may be absent. Typed resolution determines
        // whether their composed configuration is complete.
        let (project, workspace_revision) = read(&project_path, false, true)?;
        let revisions = BTreeMap::from([
            (user_path.clone(), user_revision),
            (project_path.clone(), workspace_revision),
        ]);
        // Binding authoring remains strictly validated, but cannot rebind this manager.

        if locations.runtime_root.starts_with(&locations.workspace)
            || locations.workspace.starts_with(&locations.runtime_root)
        {
            return Err("runtime_root must be disjoint from the workspace".into());
        }

        let mut merged = RuntimeLayer {
            app_server: user.app_server.clone(),
            ..RuntimeLayer::default()
        };
        let mut provenance = BTreeMap::new();
        let mut project_resources = Vec::new();
        for (mut layer, origin) in [
            (
                user,
                Origin::User {
                    document: user_path.clone(),
                    base: user_path.parent().expect("parent").into(),
                },
            ),
            (
                project,
                Origin::Workspace {
                    document: project_path.clone(),
                    base: project_path.parent().expect("parent").into(),
                },
            ),
        ] {
            project_resources.extend(rebase_paths(
                &mut layer,
                &origin,
                &locations.workspace,
                false,
            )?);
            merged.overlay(layer, &origin, &mut provenance);
        }
        Ok(LayerCapture {
            revisions,
            locations,
            identity,
            merged,
            provenance,
            project_resources,
        })
    }

    fn capture_sources(
        &self,
        request: &SessionConfigInput,
        candidate: Option<(&Path, &[u8])>,
    ) -> Result<SourceCapture, LaunchFailure> {
        self.resolve_captured_layers(self.capture_layers(request, candidate)?)
    }

    fn resolve_captured_layers(
        &self,
        capture: LayerCapture,
    ) -> Result<SourceCapture, LaunchFailure> {
        let LayerCapture {
            mut revisions,
            locations,
            identity,
            merged,
            mut provenance,
            project_resources,
        } = capture;
        for root in [
            self.sources.home_directory.join("rustx/.agents"),
            locations.workspace.join(".agents"),
        ] {
            revisions.insert(root.clone(), super::resource_directory::revision(&root));
        }
        let user_path = self.sources.config_path.clone();
        let model_result = ModelCatalog::from_document(
            crate::model::authoring::Catalog {
                schema_version: crate::model::catalog::MODEL_CATALOG_SCHEMA_VERSION,
                providers: merged.providers.clone().unwrap_or_default(),
                models: merged.models.clone().unwrap_or_default(),
            }
            .into(),
        );
        let model_failure = |e: crate::model::catalog::ModelCatalogError| {
            LaunchFailure::at(
                Some(user_path.clone()),
                "models",
                "invalid model catalog semantics",
                "define Providers and Models in rustx.toml",
                e.to_string(),
            )
        };
        if merged
            .agent
            .as_ref()
            .and_then(|agent| agent.model.as_ref())
            .is_none_or(|model| model.model.is_none())
        {
            if let Err(error) = &model_result
                && !matches!(
                    error,
                    crate::model::catalog::ModelCatalogError::EmptyCatalog
                )
            {
                return Err(LaunchFailure::at(
                    Some(user_path.clone()),
                    "models",
                    "invalid model catalog semantics",
                    "define Providers and Models in rustx.toml",
                    error.to_string(),
                ));
            }
            let mut error = LaunchFailure::at(
                Some(user_path.clone()),
                "agent.model.model",
                "no unambiguous default model selected",
                "set agent.model.model in User or Workspace rustx.toml",
                "no unambiguous default model selected".into(),
            );
            error.incomplete = true;
            error.partial = Some(Box::new(super::diagnostics::PartialProjection::new(
                &locations,
                serde_json::to_value(&merged).map_err(|e| e.to_string())?,
            )));
            return Err(error);
        }
        let models = model_result.map_err(model_failure)?;
        let effective = merged.clone();
        let mut config = merged.resolve()?;
        let mcp = super::mcp_resources::load(
            &self.sources.home_directory.join("rustx/.agents"),
            Some(&locations.workspace),
        );
        let mut diagnostics: Vec<_> = mcp
            .invalid_scopes
            .iter()
            .map(|error| {
                crate::runtime::capability_inspection::ResourceDiagnostic::collection(
                    crate::runtime::capability_inspection::ResourceFamily::Mcp,
                    error,
                )
            })
            .collect();
        let definitions = mcp
            .locations
            .iter()
            .map(
                |(id, location)| crate::runtime::capability_inspection::ResourceDefinition {
                    family: crate::runtime::capability_inspection::ResourceFamily::Mcp,
                    name: id.to_string(),
                    location: location.clone(),
                    valid: mcp.definitions.get(id).is_some_and(Result::is_ok),
                },
            )
            .collect();
        for (source, selection) in &config.agent.tools.sources {
            if let crate::capabilities::ToolSourceId::Mcp(id) = source {
                let demanded = match selection {
                    crate::capabilities::selection::SourceToolSelection::All => true,
                    crate::capabilities::selection::SourceToolSelection::Exact(names) => {
                        !names.is_empty()
                    }
                };
                if demanded
                    && !mcp.definitions.contains_key(id)
                    && let Some(error) = mcp.invalid_scopes.last()
                {
                    return Err(LaunchFailure::resource(error.clone()));
                }
            }
        }
        revisions.extend(mcp.revisions);
        for (id, definition) in mcp.definitions {
            match definition {
                Ok(definition) => {
                    config.mcp_servers.insert(id.clone(), definition.resolve());
                }
                Err(error) => {
                    diagnostics.push(
                        crate::runtime::capability_inspection::ResourceDiagnostic::resource(
                            crate::runtime::capability_inspection::ResourceFamily::Mcp,
                            &id,
                            &error,
                        ),
                    );
                    if config
                        .agent
                        .tools
                        .sources
                        .get(&crate::capabilities::ToolSourceId::Mcp(id.clone()))
                        .is_some_and(|selection| match selection {
                            crate::capabilities::selection::SourceToolSelection::All => true,
                            crate::capabilities::selection::SourceToolSelection::Exact(names) => {
                                !names.is_empty()
                            }
                        })
                    {
                        return Err(LaunchFailure::resource(error));
                    }
                }
            }
        }
        for (id, origin) in mcp.origins {
            provenance.insert(format!("mcp_servers.{id}"), origin);
        }

        RuntimeLayer::record_default_origins(&config, &mut provenance);
        provenance.insert("runtime_root".into(), self.runtime_root_origin.clone());
        Ok(SourceCapture {
            definitions,
            diagnostics,
            revisions,
            effective,
            locations,
            identity,
            config,
            models,
            provenance,
            project_resources,
        })
    }

    fn resolve_resources(
        &self,
        request: &SessionConfigInput,
        resolved: SourceCapture,
    ) -> Result<ProspectiveSessionConfig, LaunchFailure> {
        let SourceCapture {
            definitions: mut resource_definitions,
            mut diagnostics,
            mut revisions,
            effective,
            locations,
            identity,
            config,
            models,
            provenance,
            project_resources,
        } = resolved;
        let host = &self.sources;
        for path in &project_resources {
            crate::runtime::resources::validate_project_resource_path(&locations.workspace, path)
                .map_err(LaunchFailure::resource)?;
        }
        // Both resource roots are fixed process/Workspace bindings; profiles select visibility.
        let skill_sources = crate::skills::automatic_skill_roots(
            Some(host.home_directory.as_path()),
            &locations.workspace,
        );
        // Capture authority once, including host path aliases and missing leaves.
        // Configuration preparation retains this physical identity; validators must not rebind it.
        let agent_root = normalize_missing(&host.home_directory.join("rustx/.agents/agents"))?;
        let project_context_files = {
            crate::runtime::load_project_context_files(&locations.workspace)
                .map_err(LaunchFailure::resource)?
        };
        let (subagents, role_sources) =
            super::agent_resources::load_authorized(Some(&locations.workspace), &agent_root)
                .map_err(LaunchFailure::resource)?;
        revisions.extend(
            role_sources
                .values()
                .map(|source| (source.selected.clone(), source.revision.clone())),
        );
        for source in role_sources.values() {
            if let (Some(path), Some(revision)) = (&source.overridden, &source.overridden_revision)
            {
                revisions.insert(path.clone(), revision.clone());
            }
        }
        let mut workflows = {
            super::workflow_resources::load(
                Some(&locations.workspace),
                &host.home_directory.join("rustx/.agents"),
            )
            .map_err(LaunchFailure::resource)?
        };
        for id in &config.agent.workflows {
            if let Some(error) = workflows.invalid().get(id) {
                return Err(LaunchFailure::resource(error.clone()));
            }
        }
        let workspace = crate::tools::workspace::Workspace::new(&locations.workspace)
            .map_err(|e| e.to_string())?;
        let skill_discovery = {
            crate::skills::SkillDiscovery::with_config(
                &workspace,
                crate::skills::SkillDiscoveryConfig {
                    automatic: skill_sources.clone(),
                },
            )
            .discover()
        };
        if let Some(crate::runtime::agent_profile::AgentSkillSelection::Exact(names)) =
            &config.agent.skills
        {
            for name in names {
                if !skill_discovery
                    .packages
                    .iter()
                    .any(|package| package.name() == name && !package.disable_model_invocation())
                {
                    return Err(LaunchFailure::at(
                        None,
                        "agent.skills",
                        "selected Skill is unavailable",
                        "select a valid visible Skill package",
                        name.clone(),
                    ));
                }
            }
        }
        let skills = crate::skills::SkillSnapshot::from_discovery(skill_discovery.clone());
        for name in &config.agent.agents {
            if let Some(error) = subagents.invalid().get(name) {
                return Err(LaunchFailure::resource(error.clone()));
            }
        }
        let main_subagents =
            subagents.selected_definitions(&config.agent.agents.iter().cloned().collect());
        let native_metadata = crate::tools::native::definitions(
            config.native_tools.to_policies(),
            Some(&main_subagents),
        );
        let native_leaves: std::collections::BTreeSet<_> = native_metadata
            .iter()
            .filter(|(_, policy)| *policy == crate::tools::deadline::ForegroundPolicy::Leaf)
            .map(|(definition, _)| definition.id.clone())
            .collect();
        let mut definitions: Vec<_> = native_metadata
            .into_iter()
            .map(|(definition, _)| definition)
            .collect();
        definitions.extend(
            workflows
                .entries()
                .values()
                .map(|entry| &entry.source)
                .map(|program| crate::tools::native::workflow_definition(program)),
        );
        let managed_python = super::managed_python_resources::discover(
            Some(&locations.workspace),
            &host.home_directory.join("rustx/.agents"),
        )
        .map_err(LaunchFailure::resource)?;
        let availability = config
            .mcp_servers
            .keys()
            .cloned()
            .map(crate::capabilities::ToolSourceId::Mcp)
            .chain(managed_python.packages().keys().cloned())
            .map(|id| (id, crate::capabilities::CapabilitySourceState::Unprepared))
            .collect();
        let available_catalog =
            crate::capabilities::AvailableToolCatalog::metadata(definitions.clone());
        workflows.admit_metadata(
            &available_catalog,
            &availability,
            &skills,
            &subagents,
            &native_leaves,
        );
        let root_agent_project_files =
            super::agent_resources::load_profile_files(&config.agent.agents_md.files)
                .map_err(LaunchFailure::resource)?;
        let policy = crate::capabilities::AgentActivation {
            profile: config.agent.clone(),
            admitted_agents: subagents.names().into_iter().cloned().collect(),
            admitted_workflows: workflows.enabled_ids().clone(),
            project_files: root_agent_project_files.clone(),
        };
        let main = crate::capabilities::inspect_profile(
            &definitions.iter().collect::<Vec<_>>(),
            &policy,
            &skills,
            &availability,
        )?;
        let names = subagents.names().into_iter().cloned().collect();
        let admitted_workflows = workflows.enabled_ids();
        let authority = crate::runtime::agent_profile::AgentProfileAuthority {
            tools: &available_catalog,
            availability: &availability,
            skills: &skills,
            agents: &names,
            workflows: &admitted_workflows,
            scope: crate::runtime::agent_profile::AgentScope::OneShotChild,
        };
        let profiles: BTreeMap<_, _> = subagents
            .definitions()
            .map(|definition| {
                (
                    definition.name().clone(),
                    crate::runtime::agent_profile::resolve_agent_profile(
                        definition.profile(),
                        &authority,
                    ),
                )
            })
            .collect();
        let mut inspection = crate::runtime::capability_inspection::CapabilityInspection::collect(
            Some(&main),
            profiles.iter().map(|(name, profile)| {
                (
                    name,
                    profile,
                    subagents
                        .get(name)
                        .expect("resolved definition")
                        .instructions_source(),
                )
            }),
            &workflows,
            &availability,
            &skills,
        );
        resource_definitions.extend(
            role_sources
                .iter()
                .map(|(name, source)| ResourceDefinition {
                    family: ResourceFamily::Agent,
                    name: name.to_string(),
                    location: crate::runtime::resources::ResourceLocation {
                        scope: if source.layer == "workspace" {
                            SourceScope::Workspace
                        } else {
                            SourceScope::User
                        },
                        path: source.selected.clone(),
                        shadowed: source.overridden.clone(),
                    },
                    valid: !subagents.invalid().contains_key(name),
                }),
        );
        resource_definitions.extend(managed_python.locations.iter().map(|(id, location)| {
            ResourceDefinition {
                family: ResourceFamily::ManagedPython,
                name: id.managed_python().expect("Python catalog identity").into(),
                location: location.clone(),
                valid: managed_python.packages().get(id).is_some_and(Result::is_ok),
            }
        }));
        resource_definitions.extend(
            skills
                .provenance()
                .iter()
                .map(|skill| (skill, true))
                .chain(skill_discovery.invalid.iter().map(|skill| (skill, false)))
                .map(|(skill, valid)| ResourceDefinition {
                    family: ResourceFamily::Skill,
                    name: skill.name.clone(),
                    location: crate::runtime::resources::ResourceLocation {
                        scope: match skill.source {
                            crate::skills::SkillSource::User => SourceScope::User,
                            crate::skills::SkillSource::Workspace => SourceScope::Workspace,
                        },
                        path: skill.location.clone().into(),
                        shadowed: skill
                            .shadowed
                            .first()
                            .map(|lower| lower.location.clone().into()),
                    },
                    valid,
                }),
        );
        inspection.definitions.extend(resource_definitions);
        inspection
            .definitions
            .sort_by(|a, b| (&a.family, &a.name).cmp(&(&b.family, &b.name)));
        {
            use crate::runtime::capability_inspection::ResourceDiagnostic;
            diagnostics.extend(
                ResourceDiagnostic::of_agents(&subagents)
                    .chain(ResourceDiagnostic::of_workflows(&workflows))
                    .chain(ResourceDiagnostic::of_managed_python(&managed_python)),
            );
        }
        for root in [
            host.home_directory.join("rustx/.agents"),
            locations.workspace.join(".agents"),
        ] {
            if revisions.get(&root) != Some(&super::resource_directory::revision(&root)) {
                return Err(LaunchFailure::at(
                    Some(root),
                    "resources",
                    "authored resources changed during candidate construction",
                    "rescan after source edits settle",
                    "candidate discarded".into(),
                ));
            }
        }
        for file in project_context_files
            .iter()
            .chain(root_agent_project_files.iter())
            .chain(
                subagents
                    .definitions()
                    .flat_map(|definition| definition.profile().project_instructions.files.iter()),
            )
        {
            let bytes = read_bounded(&file.path)?;
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| LaunchFailure::from("instruction text is not UTF-8"))?;
            if text.trim_start_matches('\u{feff}') != file.content {
                return Err("instruction input changed during capture; rescan to retry".into());
            }
            revisions.insert(file.path.clone(), super::settings::revision(Some(&bytes)));
        }
        for (path, expected) in &revisions {
            if path == &host.home_directory.join("rustx/.agents")
                || path == &locations.workspace.join(".agents")
            {
                continue;
            }
            let bytes = match read_bounded(path) {
                Ok(bytes) => Some(bytes),
                Err(_) if !path.exists() => None,
                Err(error) => return Err(error.into()),
            };
            if super::settings::revision(bytes.as_deref()) != *expected {
                return Err("configuration inputs changed during capture; rescan to retry".into());
            }
        }
        if crate::runtime::load_project_context_files(&locations.workspace)
            .map_err(LaunchFailure::resource)?
            != project_context_files
        {
            return Err(
                "project instruction discovery changed during capture; rescan to retry".into(),
            );
        }
        diagnostics.sort();
        diagnostics.dedup();
        diagnostics.truncate(256);
        inspection.resource_diagnostics = diagnostics;
        let revision = source_manifest_revision(&revisions);
        Ok(ProspectiveSessionConfig {
            component_revisions: [
                application::ApplyUnit::ExecutionPolicy,
                application::ApplyUnit::SharedCapacity,
                application::ApplyUnit::Capabilities,
                application::ApplyUnit::Instructions,
                application::ApplyUnit::Provider,
            ]
            .into_iter()
            .map(|unit| (unit, revision.clone()))
            .collect(),
            source_revisions: revisions,
            effective,
            root_agent_project_files,
            skill_discovery,
            project_context_files,
            inspection,
            input: request.clone(),
            sources: host.clone(),
            managed_python,
            workflows: std::sync::Arc::new(workflows),
            subagents,
            agent_root,
            role_sources,
            locations,
            config: std::sync::Arc::new(config),
            models,
            provenance,
            identity,
            skill_sources,
            project_resources,
        })
    }
}

impl std::fmt::Debug for ProspectiveSessionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProspectiveSessionConfig")
            .field("configuration", &"<redacted>")
            .finish_non_exhaustive()
    }
}

pub(super) fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    }
}

/// Persistent Linux/macOS identity: full lowercase SHA-256 of Unix-native
/// path bytes. The caller supplies the canonical workspace, without further
/// case/Unicode normalization. Never use Rust's unspecified `OsStr` encoding.
pub(super) fn workspace_identity(canonical_workspace: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(canonical_workspace.as_os_str().as_bytes())
    )
}

pub(super) fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path)
        .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(path)
}

pub(super) fn normalize_missing(path: &Path) -> Result<PathBuf, String> {
    if present_on_disk(path)? {
        return std::fs::canonicalize(path).map_err(|e| e.to_string());
    }
    let parent = path.parent().ok_or("path has no existing ancestor")?;
    let name = path.file_name().ok_or("invalid path component")?;
    Ok(normalize_missing(parent)?.join(name))
}

fn read_layer(path: &Path, required: bool, project: bool) -> Result<RuntimeLayer, LaunchFailure> {
    read_layer_revision(path, required, project).map(|(layer, _)| layer)
}
fn read_layer_revision(
    path: &Path,
    required: bool,
    project: bool,
) -> Result<(RuntimeLayer, String), LaunchFailure> {
    if !required && !present_on_disk(path)? {
        return Ok((RuntimeLayer::default(), super::settings::revision(None)));
    }
    let bytes = read_bounded(path).map_err(|detail| {
        LaunchFailure::at(
            Some(path.into()),
            "$",
            "selected configuration file is unavailable",
            "correct the selected path or create the file",
            detail,
        )
    })?;
    let revision = super::settings::revision(Some(&bytes));
    Ok((parse_layer(path, &bytes, project)?, revision))
}

pub(super) fn parse_layer(
    path: &Path,
    bytes: &[u8],
    workspace: bool,
) -> Result<RuntimeLayer, LaunchFailure> {
    let layer: RuntimeLayer =
        crate::toml_authoring::parse_detailed(bytes).map_err(|e| LaunchFailure::parse(path, e))?;
    if workspace && layer.app_server.is_some() {
        return Err(LaunchFailure::at(
            Some(path.into()),
            "app_server",
            "App Server process policy cannot be authored by a Workspace",
            "author app_server in the bound User rustx.toml",
            "process policy belongs to the User scope; native application distinguishes live limits from restart bindings".into(),
        ));
    }
    if let Some(version) = layer.schema_version
        && version != super::config::CURRENT_RUNTIME_SCHEMA_VERSION
    {
        return Err(LaunchFailure::at(
            Some(path.into()),
            "schema_version",
            "unsupported runtime schema version",
            "use the current canonical authoring schema",
            format!(
                "unsupported runtime schema_version {version}; this runtime speaks {}",
                super::config::CURRENT_RUNTIME_SCHEMA_VERSION
            ),
        ));
    }
    Ok(layer)
}

fn rebase_paths(
    layer: &mut RuntimeLayer,
    origin: &Origin,
    workspace: &Path,
    validate_resources: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut resources = Vec::new();
    let base = match origin {
        Origin::User { base, .. } | Origin::Workspace { base, .. } | Origin::Process { base } => {
            base
        }
        Origin::Builtin => return Ok(resources),
    };
    let mut path = |value: &mut PathBuf| -> Result<(), String> {
        if value.as_os_str().is_empty() {
            return Err("path must be non-empty".into());
        }
        let resolved = absolute(base, value);
        if matches!(origin, Origin::Workspace { .. }) {
            if validate_resources {
                crate::runtime::resources::validate_project_resource_path(workspace, &resolved)
                    .map_err(|e| e.to_string())?;
            }
            resources.push(resolved.clone());
        }
        *value = resolved;
        Ok(())
    };
    if let Some(agent) = &mut layer.agent
        && let Some(policy) = &mut agent.agents_md
    {
        for file in &mut policy.files {
            path(file)?;
        }
    }
    Ok(resources)
}

pub(super) fn authoring_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(RuntimeLayer)).expect("schema serializes")
}

pub(super) fn present_on_disk(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

impl std::fmt::Debug for SessionConfigInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfigInput")
            .field("cwd", &self.cwd)
            .field("selections", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Internal source identity: version tag, u64 big-endian entry count, then
/// length-prefixed Unix path bytes and revision bytes in `PathBuf` `BTreeMap` order.
/// Framing distinguishes both field and entry boundaries without requiring
/// filesystem paths to be Unicode or allocating a serialized manifest.
fn source_manifest_revision(revisions: &BTreeMap<PathBuf, String>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"rustx-source-manifest-v1");
    hash.update((revisions.len() as u64).to_be_bytes());
    for (path, revision) in revisions {
        for bytes in [path.as_os_str().as_bytes(), revision.as_bytes()] {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        }
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod source_manifest_tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn exact_path_bytes_and_revisions_determine_identity() {
        let a = PathBuf::from(OsString::from_vec(b"/config-\xfe".to_vec()));
        let b = PathBuf::from(OsString::from_vec(b"/config-\xff".to_vec()));
        assert_eq!(a.to_string_lossy(), b.to_string_lossy());
        let manifest = BTreeMap::from([(a.clone(), "revision-x".into())]);
        let identity = source_manifest_revision(&manifest);
        assert_eq!(identity, source_manifest_revision(&manifest));
        assert_ne!(
            identity,
            source_manifest_revision(&BTreeMap::from([(b, "revision-x".into())]))
        );
        assert_ne!(
            identity,
            source_manifest_revision(&BTreeMap::from([(a, "revision-y".into())]))
        );
    }

    #[test]
    fn field_and_entry_boundaries_are_unambiguous() {
        let manifest = |entries: &[(&str, &str)]| {
            entries
                .iter()
                .map(|(path, revision)| (PathBuf::from(path), (*revision).to_owned()))
                .collect::<BTreeMap<_, _>>()
        };
        // Same concatenated payload, different path/revision split.
        assert_ne!(
            source_manifest_revision(&manifest(&[("ab", "c")])),
            source_manifest_revision(&manifest(&[("a", "bc")]))
        );
        // Same concatenated payload, different entry boundaries/count.
        assert_ne!(
            source_manifest_revision(&manifest(&[("a", "bcde")])),
            source_manifest_revision(&manifest(&[("a", "b"), ("c", "de")]))
        );
        // Same count and concatenated payload, different revision boundaries.
        assert_ne!(
            source_manifest_revision(&manifest(&[("a", "bc"), ("d", "e")])),
            source_manifest_revision(&manifest(&[("a", "b"), ("cd", "e")]))
        );
        let ordered = manifest(&[("a", "b"), ("c", "de")]);
        let reversed = manifest(&[("c", "de"), ("a", "b")]);
        assert_eq!(
            source_manifest_revision(&ordered),
            source_manifest_revision(&reversed)
        );
    }
}
