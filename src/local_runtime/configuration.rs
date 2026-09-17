//! Reusable user-source ownership and fresh explicit-Session resolution.
//! No provider, Session runtime, or external source is prepared here.
//! Source content is reread per call; admitted configurations own their capture.

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
    pub(crate) credentials: crate::credentials::CredentialSnapshot,
    prospective: Box<ProspectiveSessionConfig>,
}

/// Shared authority, source overlay and path capture, before domain validation.
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

impl ProspectiveSessionConfig {
    /// Capture process credentials after coherent User and Workspace resolution.
    /// # Errors
    /// Rejects changed physical resource authority before credential capture.
    pub fn admit(
        self,
        credentials: impl FnOnce() -> crate::credentials::CredentialSnapshot,
    ) -> Result<AdmittedSessionConfig, String> {
        self.validate_resource_authority()?;
        Ok(AdmittedSessionConfig {
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
        for role in self
            .role_sources
            .values()
            .filter(|role| self.config.agent.agents.contains(&role.identity))
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

    /// Cold resolution and explicit reload share the same complete capture path.
    /// Process bindings are copied from admission; Session model intent is applied
    /// separately at the runtime publication boundary.
    pub(crate) fn reload_configuration(&self) -> Result<ProspectiveSessionConfig, String> {
        let mut sources = self.sources.clone();
        sources.runtime_root.clone_from(&self.runtime_root);
        UserConfigManager::new(sources)?
            .resolve_session(&SessionConfigInput {
                cwd: self.workspace.clone(),
                model: None,
            })
            .map_err(|error| error.to_string())
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

    pub(crate) fn resolve_model_configuration(
        &self,
        request: &SessionConfigInput,
    ) -> Result<SourceCapture, LaunchFailure> {
        #[cfg(test)]
        self.test_hooks.reach("validation_started");
        self.resolve_model_candidate(request, None)
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

        let mut merged = RuntimeLayer::default();
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
        let LayerCapture {
            mut revisions,
            locations,
            identity,
            merged,
            mut provenance,
            project_resources,
        } = self.capture_layers(request, candidate)?;
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
            &locations.workspace,
        );
        let mut diagnostics: Vec<_> = mcp
            .invalid_scopes
            .iter()
            .map(crate::runtime::capability_inspection::ResourceDiagnostic::from_error)
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
                        crate::runtime::capability_inspection::ResourceDiagnostic::from_error(
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
        // Reload reuses this physical identity; validators must not rebind it.
        let agent_root = normalize_missing(&host.home_directory.join("rustx/.agents/agents"))?;
        let project_context_files = {
            crate::runtime::load_project_context_files(&locations.workspace)
                .map_err(LaunchFailure::resource)?
        };
        let (subagents, role_sources) =
            super::agent_resources::load_authorized(&locations.workspace, &agent_root)
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
                &locations.workspace,
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
        let native_metadata =
            crate::tools::native::definitions(config.native_tools.to_policies(), &main_subagents);
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
            &locations.workspace,
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
                name: match id {
                    crate::capabilities::ToolSourceId::ManagedPython(name) => name.clone(),
                    crate::capabilities::ToolSourceId::Mcp(_) => {
                        unreachable!("Python catalog identity")
                    }
                },
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
        diagnostics.extend(
            subagents
                .invalid()
                .values()
                .chain(workflows.invalid().values())
                .chain(subagents.discovery_diagnostics.iter())
                .chain(workflows.discovery_diagnostics.iter())
                .chain(managed_python.discovery_diagnostics.iter())
                .map(crate::runtime::capability_inspection::ResourceDiagnostic::from_error),
        );
        diagnostics.extend(
            managed_python
                .packages()
                .iter()
                .filter_map(|(id, package)| {
                    package.as_ref().err().map(|_| {
                        crate::runtime::capability_inspection::ResourceDiagnostic {
                            file: None,
                            identity: id.to_string(),
                            reason: "Managed Python package is invalid or unreadable".into(),
                        }
                    })
                }),
        );
        for root in [
            host.home_directory.join("rustx/.agents"),
            locations.workspace.join(".agents"),
        ] {
            if revisions.get(&root) != Some(&super::resource_directory::revision(&root)) {
                return Err(LaunchFailure::at(
                    Some(root),
                    "resources",
                    "authored resources changed during candidate construction",
                    "retry reload after source edits settle",
                    "candidate discarded".into(),
                ));
            }
        }
        diagnostics.sort();
        diagnostics.dedup();
        diagnostics.truncate(256);
        inspection.resource_diagnostics = diagnostics;
        Ok(ProspectiveSessionConfig {
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
            "author app_server in the bound User rustx.toml and restart the process",
            "process policy is captured before Session composition and is not reloadable".into(),
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
