//! Reusable user-source ownership and fresh explicit-Session resolution.
//! No provider, Session runtime, or external source is prepared here.
//! Source content is reread per call; admitted configurations own their capture.

pub mod integrations;
pub mod settings;

use crate::bounded_file::read_bounded;
use crate::capabilities::activation::{SourceActivation, SourceEnablement};
use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::authoring::{ModelLayer, RuntimeLayer};
use super::config::CurrentRuntimeConfig;
use super::diagnostics::LaunchFailure;
use crate::model::catalog::ModelCatalog;

pub(super) const USER_PATH_FIELDS: &[&str] = &["models", "runtime_root"];
pub(super) const HOST_POLICY_FIELDS: &[&str] = &[
    "app_server",
    "approval_mode",
    "native_tools",
    "mcp_tool_policies",
];
pub(super) const MCP_SECRET_FIELDS: &[&str] = &["sensitive_env", "sensitive_headers"];

/// Filesystem locations and controls produced by the launch resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLocations {
    /// Repeatable explicit Skill package/root paths from the command line.
    pub skill_paths: Vec<PathBuf>,
    /// Disable automatic/default Skill roots while retaining explicit paths.
    pub no_automatic_skills: bool,
    /// Remove ordinary builtin Tools from direct selection; delegation and Workflow dispatch stay independent.
    pub no_builtin_tools: bool,
    /// Remove ordinary direct Tool exposure; source activation and dispatch stay independent.
    pub no_direct_tools: bool,
    /// Strict startup Tool allowlist, when supplied.
    pub tools: Option<Vec<String>>,
    /// Final startup Tool exclusions.
    pub exclude_tools: Vec<String>,
    /// The model-visible workspace root.
    pub workspace: PathBuf,
    /// The exact runtime-private root from which disjoint private
    /// subdirectories are derived. Child conversation stores live below its
    /// stable `subagents/<conversation-id>` semantic directory; child
    /// execution incarnations remain private and disposable.
    pub runtime_root: PathBuf,
}

impl SessionLocations {
    /// Prospective artifact location for diagnostics; native allocation must
    /// derive from admitted `ProductRoot`, not from these launch spellings.
    #[must_use]
    pub fn artifacts_root(&self) -> PathBuf {
        self.runtime_root.join("artifacts")
    }

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
    pub config_directory: PathBuf,
    pub state_directory: PathBuf,
    pub settings: PathBuf,
    pub models: PathBuf,
    pub runtime_root: PathBuf,
}

/// Reusable source owner. Each resolution rereads current files; snapshots never
/// borrow mutable manager state. The manager has no process launch directory.
#[derive(Debug, Clone)]
pub struct UserConfigManager {
    sources: UserConfigSources,
    models_origin: Origin,
    runtime_root_origin: Origin,
    catalog_required: bool,
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
    pub config: Option<PathBuf>,
    pub model: Option<crate::model::session::SessionModelConfig>,
    pub skill_paths: Vec<PathBuf>,
    pub no_automatic_skills: bool,
    pub no_builtin_tools: bool,
    pub no_direct_tools: bool,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
}
impl SessionConfigInput {
    #[must_use]
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            config: None,
            model: None,
            skill_paths: Vec::new(),
            no_automatic_skills: false,
            no_builtin_tools: false,
            no_direct_tools: false,
            tools: None,
            exclude_tools: None,
        }
    }
}
/// Canonical authoring identity determines the base of relative user bindings.
pub(super) fn canonical_settings_source(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("user settings source must be absolute".into());
    }
    normalize_missing(path)
}

/// Shared location semantics, independent of full settings deserialization.
/// The caller supplies a canonical settings identity and absolute host/default paths.
pub(super) fn bind_user_source_path(
    settings: &Path,
    authored: Option<&Path>,
    explicit: Option<&Path>,
    default: &Path,
) -> Result<PathBuf, String> {
    let selected = if let Some(path) = explicit {
        if !path.is_absolute() {
            return Err("host source bindings must be absolute".into());
        }
        path.to_path_buf()
    } else if let Some(path) = authored {
        if path.as_os_str().is_empty() {
            return Err("user source binding must be a non-empty path".into());
        }
        absolute(
            settings.parent().ok_or("settings source has no parent")?,
            path,
        )
    } else {
        if !default.is_absolute() {
            return Err("default source bindings must be absolute".into());
        }
        default.to_path_buf()
    };
    normalize_missing(&selected)
}

impl UserConfigManager {
    /// Bind absolute user roots without reading configuration or credentials.
    /// # Errors
    /// Relative source paths are rejected rather than interpreted using process cwd.
    pub fn new(mut sources: UserConfigSources) -> Result<Self, String> {
        for path in [
            &sources.home_directory,
            &sources.config_directory,
            &sources.state_directory,
            &sources.settings,
            &sources.models,
            &sources.runtime_root,
        ] {
            if !path.is_absolute() {
                return Err("user source bindings must be absolute".into());
            }
        }
        sources.home_directory = normalize_missing(&sources.home_directory)?;
        sources.config_directory = normalize_missing(&sources.config_directory)?;
        sources.state_directory = normalize_missing(&sources.state_directory)?;
        sources.settings = canonical_settings_source(&sources.settings)?;
        sources.models = normalize_missing(&sources.models)?;
        sources.runtime_root = normalize_missing(&sources.runtime_root)?;
        let explicit = Origin::Explicit {
            base: sources.config_directory.clone(),
        };
        Ok(Self {
            sources,
            models_origin: explicit.clone(),
            runtime_root_origin: explicit,
            catalog_required: true,
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
        models: Option<PathBuf>,
        runtime_root: Option<PathBuf>,
    ) -> Result<Self, LaunchFailure> {
        let mut sources = sources;
        sources.settings = canonical_settings_source(&sources.settings)?;
        let user = read_layer(&sources.settings, false, false)?;
        let base = sources
            .settings
            .parent()
            .ok_or("settings source has no parent")?
            .to_path_buf();
        let user_origin = Origin::User {
            document: sources.settings.clone(),
            base: base.clone(),
        };
        let explicit = Origin::Explicit {
            base: sources.config_directory.clone(),
        };
        let catalog_required = models.is_some() || user.models.is_some();
        let mut models_origin = Origin::Builtin;
        let mut runtime_root_origin = Origin::Builtin;
        for (target, origin, host, authored) in [
            (&mut sources.models, &mut models_origin, models, user.models),
            (
                &mut sources.runtime_root,
                &mut runtime_root_origin,
                runtime_root,
                user.runtime_root,
            ),
        ] {
            *target = bind_user_source_path(
                &sources.settings,
                authored.as_deref(),
                host.as_deref(),
                target,
            )?;
            if host.is_some() {
                *origin = explicit.clone();
            } else if authored.is_some() {
                *origin = user_origin.clone();
            }
        }
        // Only selected paths are canonicalized; unused default files are inert.
        let mut manager = Self::new(sources)?;
        manager.models_origin = models_origin;
        manager.runtime_root_origin = runtime_root_origin;
        manager.catalog_required = catalog_required;
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
        let policy = read_layer(&self.sources.settings, false, false)
            .map_err(|error| error.to_string())?
            .app_server
            .unwrap_or_default();
        policy.validate()?;
        Ok(policy)
    }

    /// Validate the selected model catalog before publishing a server listener.
    /// Uses the canonical model authoring parser, without Session resolution.
    /// # Errors
    /// Returns an error for unreadable or invalid model authoring.
    pub fn validate_catalog(&self) -> Result<(), String> {
        let bytes = read_bounded(&self.sources.models)?;
        ModelCatalog::from_toml_slice(&bytes).map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Resolve only locations; no configuration, credentials, or runtime creation.
    /// # Errors
    /// Invalid explicit paths and Tool restrictions are rejected.
    pub fn resolve_locations(
        &self,
        request: &SessionConfigInput,
    ) -> Result<(SessionLocations, String), String> {
        for path in std::iter::once(&request.cwd)
            .chain(request.config.iter())
            .chain(request.skill_paths.iter())
        {
            if !path.is_absolute() {
                return Err("Session paths must be absolute".into());
            }
        }
        if let Some(names) = &request.exclude_tools {
            crate::capabilities::validate_tool_names(names, "exclusion")?;
        }
        crate::capabilities::AgentActivation {
            no_direct_tools: request.no_direct_tools,
            no_builtin_tools: request.no_builtin_tools,
            tools: request.tools.clone(),
            exclude_tools: request.exclude_tools.clone().unwrap_or_default(),
            ..Default::default()
        }
        .validate()?;
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
                skill_paths: request.skill_paths.clone(),
                no_automatic_skills: request.no_automatic_skills,
                no_builtin_tools: request.no_builtin_tools,
                no_direct_tools: request.no_direct_tools,
                tools: request.tools.clone(),
                exclude_tools: request.exclude_tools.clone().unwrap_or_default(),
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
    Project { document: PathBuf, base: PathBuf },
    Explicit { base: PathBuf },
}

/// Validated Session configuration authority consumed by the existing native composition owner.
#[derive(Debug, Clone)]
pub struct AdmittedSessionConfig {
    pub(crate) credentials: crate::credentials::CredentialSnapshot,
    prospective: Box<ProspectiveSessionConfig>,
}

/// Shared authority, source overlay and path capture, before domain validation.
struct LayerCapture {
    locations: SessionLocations,
    identity: String,
    merged: RuntimeLayer,
    provenance: BTreeMap<String, Origin>,
    documents: Vec<(PathBuf, bool, Origin)>,
    project_resources: Vec<PathBuf>,
}

/// Canonical source/configuration capture before any launch resource preparation.
pub(crate) struct SourceCapture {
    locations: SessionLocations,
    identity: String,
    trusted: bool,
    pub(crate) config: CurrentRuntimeConfig,
    pub(crate) models: ModelCatalog,
    pub(crate) provenance: BTreeMap<String, Origin>,
    documents: Vec<(PathBuf, bool, Origin)>,
    project_resources: Vec<PathBuf>,
}

/// Immutable prospective Session settings and statically resolved resources.
/// Admission adds credentials; native composition owns external preparation.
#[derive(Clone)]
pub struct ProspectiveSessionConfig {
    pub(crate) root_agent_project_files: Vec<crate::runtime::resources::ProjectContextFile>,
    pub(crate) skill_discovery: crate::skills::SkillDiscoveryOutcome,
    pub(crate) project_context_files: Vec<crate::runtime::resources::ProjectContextFile>,
    pub(crate) inspection: crate::runtime::capability_inspection::CapabilityInspection,
    pub(crate) input: SessionConfigInput,
    pub(crate) sources: UserConfigSources,
    pub(crate) managed_python: crate::runtime::resources::ManagedPythonCatalog,
    pub(crate) trusted: bool,
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
    /// Fixed document slots for explicit resource reload; never discovery.
    documents: Vec<(PathBuf, bool, Origin)>,
    /// Project-origin paths retain their authority even after becoming absolute.
    project_resources: Vec<PathBuf>,
}

impl ProspectiveSessionConfig {
    /// Capture User-owned credentials after source resolution. Project trust
    /// gates project sources at resolution; an untrusted cwd remains usable.
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
        for role in self.role_sources.values() {
            let boundary = if role.layer == "project" {
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
        self.validate_workspace_resource_roots()
    }

    /// Recheck the workspace-owned resource collection roots.
    ///
    /// The Skill roots are deliberately **not** in the generic list below:
    /// this layer has no access to the session Skill source policy, and
    /// `[skills].sources` may legitimately omit `workspace` entirely. Their
    /// owner ([`crate::skills::source::validate_selected_roots`]) is
    /// policy-aware and validates exactly the roots this launch selected and
    /// froze, so an unselected Skill source cannot fail startup or reload.
    pub(crate) fn validate_workspace_resource_roots(&self) -> Result<(), String> {
        if self.trusted {
            crate::runtime::resources::validate_project_resource_path(
                &self.workspace,
                &self.workspace.join(".agents/tools"),
            )
            .map_err(|e| e.to_string())?;
        }
        crate::skills::source::validate_selected_roots(&self.workspace, &self.skill_sources)
            .map_err(|e| e.to_string())
    }
    /// Local CLI projection of the shared configuration origins.
    pub(crate) fn settings_view(&self) -> crate::runtime_client::settings::LaunchSettings {
        use crate::runtime_client::settings::{LaunchSettings, ModelDefault, SettingOrigin};
        let origin = |field: &str| match self.provenance.get(field) {
            Some(Origin::User { document, .. }) => SettingOrigin::User {
                document: document.display().to_string(),
            },
            Some(Origin::Project { document, .. }) => SettingOrigin::Project {
                document: document.display().to_string(),
            },
            Some(Origin::Explicit { .. }) => SettingOrigin::Cli,
            _ => SettingOrigin::Builtin,
        };
        LaunchSettings {
            model: ModelDefault {
                model: self.config.initial_model().model.clone(),
                reasoning_profile: self.config.initial_model().reasoning_profile.clone(),
            },
            model_origin: origin("agent.model.model"),
            reasoning_origin: origin("agent.model.reasoning_profile"),
            approval_mode: self.config.approval_mode,
            approval_origin: origin("approval_mode"),
            runtime_root_origin: origin("runtime_root"),
            tool_selection_origin: if self.no_direct_tools
                || self.no_builtin_tools
                || self.tools.is_some()
                || self.input.exclude_tools.is_some()
            {
                SettingOrigin::Cli
            } else {
                origin("agent.tools")
            },
        }
    }

    /// The immutable validated settings this launch resolved.
    #[must_use]
    pub fn config(&self) -> &CurrentRuntimeConfig {
        &self.config
    }

    /// The effective source provenance of every Skill identity this launch
    /// admitted, including what each one shadowed (Issue #280).
    #[must_use]
    pub fn skill_provenance(&self) -> &[crate::skills::SkillProvenance] {
        &self.inspection.skills
    }

    /// The typed, canonically ordered Skill discovery facts of this launch.
    ///
    /// An excluded malformed package appears here rather than failing the
    /// launch; only the explicit `--skill` authority itself can fail one.
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

    /// The existing resource-generation owner may reread only the slots this
    /// launch authorized. Catalog, workspace, state and trust remain frozen.
    pub(crate) fn reload_resource_config(
        &self,
    ) -> Result<(CurrentRuntimeConfig, BTreeMap<String, Origin>), String> {
        self.validate_workspace_resource_roots()?;
        let mut merged = RuntimeLayer::default();
        let mut provenance = BTreeMap::new();
        for (path, required, origin) in &self.documents {
            let mut layer = read_layer(path, *required, matches!(origin, Origin::Project { .. }))?;
            // Reload owns only resource inputs. Startup/Session defaults remain
            // the launch capture, even when disk defaults have since changed.
            layer.resources_only();
            rebase_paths(&mut layer, origin, &self.workspace, true)?;
            merged.overlay(layer, origin, &mut provenance);
        }

        merged.agent.get_or_insert_default().model = Some(ModelLayer {
            model: Some(self.config.initial_model().model.clone()),
            ..Default::default()
        });
        let resources = merged.resolve()?;
        let mut config = self.config.as_ref().clone();
        RuntimeLayer::copy_resources(&mut config, resources);
        config.validate().map_err(|e| e.to_string())?;
        Ok((config, provenance))
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
    /// Read current project trust without parsing project configuration, acquiring
    /// credentials, composing a runtime, or writing trust. This is source trust;
    /// an already loaded runtime retains its admitted resource generation.
    /// # Errors
    /// Invalid location or trust-store ownership is rejected.
    pub fn project_trusted(&self, request: &SessionConfigInput) -> Result<bool, String> {
        let (locations, identity) = self.resolve_locations(request)?;
        // One atomic membership observation. Static analysis must not allocate
        // state/coordination files; its captured decision is reused throughout.
        Ok(trust_root(&self.sources, &locations.workspace)?
            .join(identity)
            .is_dir())
    }

    fn trust_epoch(&self, request: &SessionConfigInput) -> Result<TrustEpoch, String> {
        let (locations, identity) = self.resolve_locations(request)?;
        TrustEpoch::acquire(&self.sources, &locations.workspace, &identity)
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
        let trusted = self.project_trusted(request)?;
        let resolved = self.resolve_model_candidate(request, None, trusted)?;
        let resolved = Self::resolve_runtime_configuration(resolved)?;
        self.resolve_resources(request, resolved)
    }

    pub(crate) fn resolve_model_configuration(
        &self,
        request: &SessionConfigInput,
    ) -> Result<SourceCapture, LaunchFailure> {
        #[cfg(test)]
        self.test_hooks.reach("validation_started");
        let trusted = self.project_trusted(request)?;
        self.resolve_model_candidate(request, None, trusted)
    }

    fn resolve_model_candidate(
        &self,
        request: &SessionConfigInput,
        candidate: Option<(&Path, &[u8])>,
        trusted: bool,
    ) -> Result<SourceCapture, LaunchFailure> {
        let capture = self.capture_sources(request, candidate, trusted)?;
        let config = &capture.config;
        config.context.validate().map_err(|e| e.to_string())?;
        let (primary, summary) = crate::model::session::analyze_session_model_config(
            &capture.models,
            config.initial_model(),
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
                    Some(self.sources.settings.clone()),
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
        trusted: bool,
    ) -> Result<LayerCapture, LaunchFailure> {
        let read = |path: &Path, required, project| match candidate {
            Some((target, bytes))
                if normalize_missing(path).is_ok_and(|canonical| canonical == target) =>
            {
                parse_layer(path, bytes, project)
            }
            _ => read_layer(path, required, project),
        };
        let host = &self.sources;
        let (locations, identity) = self.resolve_locations(request)?;
        let launch = locations.workspace.clone();
        let user_path = host.settings.clone();
        let project_path = request.config.as_ref().map_or_else(
            || locations.workspace.join("rustx.toml"),
            |p| absolute(&launch, p),
        );
        let mut user = read(&user_path, false, false)?;

        // Even an empty project needs trust for project activation. Later file
        // creation or reload cannot widen the captured source authority.
        let project = if trusted {
            read(&project_path, request.config.is_some(), true)?
        } else {
            RuntimeLayer::default()
        };
        // Binding authoring remains strictly validated, but cannot rebind this manager.
        user.models = None;
        user.runtime_root = None;
        let trust_directory = trust_root(host, &locations.workspace)?;
        if locations.runtime_root.starts_with(&trust_directory)
            || trust_directory.starts_with(&locations.runtime_root)
        {
            return Err("runtime_root must be disjoint from host trust authority".into());
        }
        if locations.runtime_root.starts_with(&locations.workspace)
            || locations.workspace.starts_with(&locations.runtime_root)
        {
            return Err("runtime_root must be disjoint from the workspace".into());
        }
        let mut documents = vec![
            (
                user_path.clone(),
                false,
                Origin::User {
                    document: user_path.clone(),
                    base: user_path.parent().expect("parent").into(),
                },
            ),
            (
                project_path.clone(),
                request.config.is_some(),
                Origin::Project {
                    document: project_path.clone(),
                    base: project_path.parent().expect("parent").into(),
                },
            ),
        ];
        if !trusted {
            documents.retain(|(_, _, origin)| !matches!(origin, Origin::Project { .. }));
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
                Origin::Project {
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
            locations,
            identity,
            merged,
            provenance,
            documents,
            project_resources,
        })
    }

    fn capture_sources(
        &self,
        request: &SessionConfigInput,
        candidate: Option<(&Path, &[u8])>,
        trusted: bool,
    ) -> Result<SourceCapture, LaunchFailure> {
        let LayerCapture {
            locations,
            identity,
            mut merged,
            mut provenance,
            documents,
            project_resources,
        } = self.capture_layers(request, candidate, trusted)?;
        let launch = locations.workspace.clone();
        let user_path = self.sources.settings.clone();
        let models_path = self.sources.models.clone();
        let model_bytes = read_bounded(&models_path).map_err(|detail| {
            let mut error = LaunchFailure::at(
                Some(models_path.clone()),
                "$",
                "model catalog is unavailable",
                "run rustx init or correct the explicit catalog path",
                detail,
            );
            error.incomplete = !self.catalog_required && !models_path.exists();
            error.partial = Some(Box::new(super::diagnostics::PartialProjection::new(
                &locations,
                trusted,
                Value::Null,
            )));
            error
        })?;
        let model_document: crate::model::authoring::Catalog =
            crate::toml_authoring::parse_detailed(&model_bytes)
                .map_err(|error| LaunchFailure::parse(&models_path, error))?;
        let models = ModelCatalog::from_document(model_document.into()).map_err(|e| {
            LaunchFailure::at(
                Some(models_path.clone()),
                "providers",
                "invalid model catalog semantics",
                "correct explicit model limits, protocol, compatibility, and capabilities",
                e.to_string(),
            )
        })?;
        if !request.skill_paths.is_empty() {
            provenance.insert(
                "skills".into(),
                Origin::Explicit {
                    base: launch.clone(),
                },
            );
        }
        if let Some(model) = &request.model {
            record_session_model_origins(
                model,
                &mut provenance,
                &Origin::Explicit {
                    base: launch.clone(),
                },
            );
        }
        if request.model.is_none()
            && merged
                .agent
                .as_ref()
                .and_then(|agent| agent.model.as_ref())
                .is_none_or(|model| model.model.is_none())
        {
            let mut error = LaunchFailure::at(
                Some(user_path.clone()),
                "agent.model.model",
                "no unambiguous default model selected",
                "set model.model in user settings.toml or pass --model provider/model",
                "no unambiguous default model selected".into(),
            );
            error.incomplete = true;
            error.partial = Some(Box::new(super::diagnostics::PartialProjection::new(
                &locations,
                trusted,
                serde_json::to_value(&merged).map_err(|e| e.to_string())?,
            )));
            return Err(error);
        }
        if let Some(model) = &request.model {
            merged.agent.get_or_insert_default().model = Some(ModelLayer {
                model: Some(model.model.clone()),
                ..Default::default()
            });
        }
        let mut config = merged.resolve()?;
        if let Some(model) = &request.model {
            config.agent.model = Some(model.clone());
        }
        RuntimeLayer::record_default_origins(&config, &mut provenance);
        for (key, present) in [
            ("skills.cli", !request.skill_paths.is_empty()),
            ("no_automatic_skills", request.no_automatic_skills),
            ("no_direct_tools", request.no_direct_tools),
            ("no_builtin_tools", request.no_builtin_tools),
            ("tools", request.tools.is_some()),
            ("exclude_tools", request.exclude_tools.is_some()),
        ] {
            provenance.insert(
                key.into(),
                if present {
                    Origin::Explicit {
                        base: launch.clone(),
                    }
                } else {
                    Origin::Builtin
                },
            );
        }
        provenance.insert(
            "workspace".into(),
            Origin::Explicit {
                base: launch.clone(),
            },
        );
        provenance.insert("runtime_root".into(), self.runtime_root_origin.clone());
        provenance.insert("models".into(), self.models_origin.clone());
        Ok(SourceCapture {
            locations,
            identity,
            trusted,
            config,
            models,
            provenance,
            documents,
            project_resources,
        })
    }

    fn resolve_resources(
        &self,
        request: &SessionConfigInput,
        resolved: SourceCapture,
    ) -> Result<ProspectiveSessionConfig, LaunchFailure> {
        let SourceCapture {
            locations,
            identity,
            trusted,
            config,
            models,
            provenance,
            documents,
            project_resources,
        } = resolved;
        let host = &self.sources;
        for path in &project_resources {
            crate::runtime::resources::validate_project_resource_path(&locations.workspace, path)
                .map_err(LaunchFailure::resource)?;
        }
        // The session Skill source policy decides *where* packages may be
        // discovered. `--no-automatic-skills` is the launch-level off switch for automatic
        // discovery; the policy itself never names a rustX configuration
        // directory, and the global root is resolved from the captured host home.
        let mut skill_sources = if request.no_automatic_skills {
            Vec::new()
        } else {
            crate::skills::automatic_skill_roots(
                Some(host.home_directory.as_path()),
                &locations.workspace,
                &config.skills.selected().map_err(|detail| {
                    LaunchFailure::at(
                        None,
                        "skills.sources",
                        "invalid Skill source policy",
                        "declare each of \"global\" and \"workspace\" at most once",
                        detail,
                    )
                })?,
            )
        };
        if !trusted {
            skill_sources.retain(|root| root.source != crate::skills::SkillSource::Workspace);
        }
        // Capture authority once, including host path aliases and missing leaves.
        // Reload reuses this physical identity; validators must not rebind it.
        let agent_root = normalize_missing(&host.config_directory.join("agents"))?;
        let project_context_files = if trusted {
            crate::runtime::load_project_context_files(&locations.workspace)
                .map_err(LaunchFailure::resource)?
        } else {
            Vec::new()
        };
        let (subagents, role_sources) =
            super::agent_resources::load_authorized(&locations.workspace, &agent_root, trusted)
                .map_err(LaunchFailure::resource)?;
        let mut workflows = if trusted {
            super::workflow_resources::load(&locations.workspace)
                .map_err(LaunchFailure::resource)?
        } else {
            crate::runtime::workflow::WorkflowCatalog::empty()
        };
        let workspace = crate::tools::workspace::Workspace::new(&locations.workspace)
            .map_err(|e| e.to_string())?;
        let skill_discovery = {
            // Only the roots this launch actually selected are validated: a Skill
            // source the policy did not select is inert, so an invalid or
            // redirected `<workspace>/.agents/skills` cannot fail a launch that
            // never scans it.
            crate::skills::source::validate_selected_roots(&locations.workspace, &skill_sources)
                .map_err(LaunchFailure::resource)?;
            crate::skills::SkillDiscovery::with_config(
                &workspace,
                crate::skills::SkillDiscoveryConfig {
                    automatic: skill_sources.clone(),
                    explicit_paths: locations.skill_paths.clone(),
                },
            )
            .discover()
            .map_err(|e| {
                LaunchFailure::at(
                    None,
                    "skills",
                    "unusable explicit Skill path",
                    "correct the explicit --skill path",
                    e.to_string(),
                )
            })?
        };
        let skills = crate::skills::SkillSnapshot::from_discovery(skill_discovery.clone());
        crate::runtime::subagent::SubagentResolver::validate_local_references(
            &subagents,
            &skills,
            |reference| {
                crate::model::invocation::analyze_selection(
                    models.model(&reference.model).map_err(|e| e.to_string())?,
                    &reference.selection(),
                    crate::model::invocation::RequestParamsLayer::SessionOverrides,
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
            },
        )
        .map_err(|(name, error)| {
            LaunchFailure::at(
                None,
                &format!("agents.{name}"),
                "invalid local model or Skill reference",
                "select a declared model and an available local Skill",
                error.to_string(),
            )
        })?;
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
        let mut source_activations = BTreeMap::new();
        for (id, source) in &config.mcp_servers {
            source_activations.insert(
                crate::capabilities::ToolSourceId::Mcp(id.clone()),
                SourceActivation::evaluate(
                    source.enabled.map(|enabled| {
                        if enabled {
                            SourceEnablement::Enabled
                        } else {
                            SourceEnablement::Disabled
                        }
                    }),
                    true, // Only User or admitted trusted-project entries reach this map.
                ),
            );
        }
        let managed_python = if trusted {
            super::managed_python_resources::discover(&locations.workspace)
                .map_err(LaunchFailure::resource)?
        } else {
            crate::runtime::resources::ManagedPythonCatalog::default()
        };
        {
            for id in managed_python.packages().keys() {
                source_activations.insert(id.clone(), SourceActivation::Enabled);
            }
        }
        let availability = source_activations
            .iter()
            .map(|(id, activation)| {
                (
                    id.clone(),
                    crate::capabilities::CapabilitySourceState::before_preparation(*activation),
                )
            })
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
            no_direct_tools: locations.no_direct_tools,
            no_builtin_tools: locations.no_builtin_tools,
            tools: locations.tools.clone(),
            exclude_tools: locations.exclude_tools.clone(),
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
        let inspection = crate::runtime::capability_inspection::CapabilityInspection::collect(
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
        Ok(ProspectiveSessionConfig {
            root_agent_project_files,
            skill_discovery,
            project_context_files,
            inspection,
            input: request.clone(),
            sources: host.clone(),
            managed_python,
            trusted,
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
            documents,
            project_resources,
        })
    }
}

impl std::fmt::Debug for ProspectiveSessionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProspectiveSessionConfig")
            .field("trusted", &self.trusted)
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

// The finite reload ownership vocabulary. Startup fields are captured separately.
/// Materialize only owned model-dependent settings. No project or resource I/O.
pub(super) fn user_model_sections(
    layer: RuntimeLayer,
) -> Result<
    (
        crate::model::session::SessionModelConfig,
        super::config::ContextPolicyDocument,
    ),
    LaunchFailure,
> {
    layer.model_sections().map_err(Into::into)
}

fn read_layer(path: &Path, required: bool, project: bool) -> Result<RuntimeLayer, LaunchFailure> {
    if !required && !present_on_disk(path)? {
        return Ok(RuntimeLayer::default());
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
    parse_layer(path, &bytes, project)
}

pub(super) fn parse_layer(
    path: &Path,
    bytes: &[u8],
    project: bool,
) -> Result<RuntimeLayer, LaunchFailure> {
    let layer: RuntimeLayer =
        crate::toml_authoring::parse_detailed(bytes).map_err(|e| LaunchFailure::parse(path, e))?;
    for (field, value) in [
        ("models", &layer.models),
        ("runtime_root", &layer.runtime_root),
    ] {
        if let Some(value) = value {
            if project {
                return Err(authority_failure(path, field));
            }
            if value.as_os_str().is_empty() {
                return Err(LaunchFailure::at(
                    Some(path.into()),
                    field,
                    "path must be non-empty",
                    "supply a non-empty local path",
                    format!("{}: {field} must be a non-empty path", path.display()),
                ));
            }
        }
    }
    if project {
        for (field, present) in [
            ("app_server", layer.app_server.is_some()),
            ("approval_mode", layer.approval_mode.is_some()),
            ("native_tools", layer.native_tools.is_some()),
            ("mcp_tool_policies", layer.mcp_tool_policies.is_some()),
        ] {
            if present {
                return Err(authority_failure(path, field));
            }
        }
        if let Some(servers) = &layer.mcp_servers {
            for (name, server) in servers {
                if server.sensitive_env.is_some() || server.sensitive_headers.is_some() {
                    return Err(authority_failure(path, &format!("mcp_servers.{name}")));
                }
            }
        }
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
fn authority_failure(path: &Path, field: &str) -> LaunchFailure {
    LaunchFailure::at(
        Some(path.into()),
        field,
        "forbidden host-owned project override",
        "move the complete host-owned field to user settings",
        format!(
            "{}: field {field} is forbidden in project settings (host-owned authority)",
            path.display()
        ),
    )
}

fn rebase_paths(
    layer: &mut RuntimeLayer,
    origin: &Origin,
    workspace: &Path,
    validate_resources: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut resources = Vec::new();
    let base = match origin {
        Origin::User { base, .. } | Origin::Project { base, .. } | Origin::Explicit { base } => {
            base
        }
        Origin::Builtin => return Ok(resources),
    };
    let mut path = |value: &mut PathBuf| -> Result<(), String> {
        if value.as_os_str().is_empty() {
            return Err("path must be non-empty".into());
        }
        let resolved = absolute(base, value);
        if matches!(origin, Origin::Project { .. }) {
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
    if let Some(servers) = &mut layer.mcp_servers {
        for server in servers.values_mut() {
            if let Some(cwd) = &mut server.cwd {
                path(cwd)?;
            }
            if let Some(command) = &mut server.command
                && command.contains('/')
            {
                let mut value = PathBuf::from(&*command);
                path(&mut value)?;
                *command = value.to_str().ok_or("command path must be UTF-8")?.into();
            }
        }
    }
    Ok(resources)
}

pub(super) fn authoring_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(RuntimeLayer)).expect("schema serializes")
}

/// Per-workspace authority epoch. Lock order: trust, sorted source documents;
/// never acquire a Session catalog mutex while holding either filesystem lock.
/// Membership create/remove and Workspace rename linearize under this guard.
pub(super) struct TrustEpoch {
    _lock: std::fs::File,
    record: PathBuf,
}
impl TrustEpoch {
    pub(super) fn acquire(
        sources: &UserConfigSources,
        workspace: &Path,
        identity: &str,
    ) -> Result<Self, String> {
        let root = trust_root(sources, workspace)?;
        // Coordination is a persistent sibling of the state directory, not a
        // trust member. Read-only analysis must not initialize durable state.
        let target = Self::lock_target(sources, identity)?;
        std::fs::create_dir_all(target.parent().ok_or("trust lock has no parent")?)
            .map_err(|e| e.to_string())?;
        let record = root.join(identity);
        let lock = super::settings::lock_document(&target).map_err(|e| e.to_string())?;
        Ok(Self {
            _lock: lock,
            record,
        })
    }
    fn lock_target(sources: &UserConfigSources, identity: &str) -> Result<PathBuf, String> {
        let name = sources
            .state_directory
            .file_name()
            .ok_or("state directory has no name")?;
        Ok(sources
            .state_directory
            .with_file_name(format!("{}.trust-{identity}", name.to_string_lossy())))
    }
    fn trusted(&self) -> bool {
        self.record.is_dir()
    }
    pub(super) fn change(&self, action: super::launch::TrustAction) -> Result<(), String> {
        match action {
            super::launch::TrustAction::Grant => {
                std::fs::create_dir_all(self.record.parent().ok_or("trust record has no parent")?)
                    .map_err(|e| e.to_string())?;
                match std::fs::create_dir(&self.record) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && self.trusted() => {
                        Ok(())
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            super::launch::TrustAction::Revoke => match std::fs::remove_dir(&self.record) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            },
        }
    }
}

pub(super) fn trust_root(host: &UserConfigSources, workspace: &Path) -> Result<PathBuf, String> {
    let root = normalize_missing(&host.state_directory.join("trust"))?;
    if root.starts_with(workspace) || workspace.starts_with(&root) {
        return Err("host trust store must be outside the project workspace".into());
    }
    Ok(root)
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

/// Whole-state ownership includes intentional catalog-default selections and
/// empty parameters. Exhaustive destructuring makes new model fields a compile
/// error here rather than silently giving them builtin provenance.
fn record_session_model_origins(
    selection: &crate::model::session::SessionModelConfig,
    origins: &mut BTreeMap<String, Origin>,
    origin: &Origin,
) {
    let crate::model::session::SessionModelConfig {
        model: _,
        reasoning_profile: _,
        request_params: _,
        max_output_tokens: _,
        summary_model: _,
    } = selection;
    origins.retain(|key, _| !key.starts_with("agent.model."));
    for field in [
        "model",
        "reasoning_profile",
        "request_params",
        "max_output_tokens",
        "summary_model",
    ] {
        origins.insert(format!("agent.model.{field}"), origin.clone());
    }
}
