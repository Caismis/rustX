//! One host-owned launch boundary. No provider, Session, or runtime is composed here.

use crate::capabilities::activation::{SourceActivation, SourceEnablement};
use crate::toml_authoring::read_bounded;
use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::authoring::{ModelLayer, RuntimeLayer};
use super::composition::StartupSession;
use super::config::CurrentRuntimeConfig;
use super::diagnostics::LaunchFailure;
use crate::model::catalog::ModelCatalog;

pub(super) const USER_PATH_FIELDS: &[&str] = &["models", "runtime_root"];
pub(super) const HOST_POLICY_FIELDS: &[&str] =
    &["approval_mode", "native_tools", "mcp_tool_policies"];
pub(super) const MCP_SECRET_FIELDS: &[&str] = &["sensitive_env", "sensitive_headers"];

/// Filesystem locations and controls produced by the launch resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchLocations {
    /// Repeatable explicit Skill package/root paths from the command line.
    pub skill_paths: Vec<PathBuf>,
    /// Disable automatic/default Skill roots while retaining explicit paths.
    pub no_skills: bool,
    /// Remove built-ins, including Read and generated Tools, from default selection.
    pub no_builtin_tools: bool,
    /// Expose zero ordinary main-model Tools; source activation stays independent.
    pub no_tools: bool,
    /// The Session this launch binds. Startup never resumes on its own;
    /// `--continue` is the explicit request behind
    /// [`StartupSession::ContinueActive`] and `--session` the one behind
    /// [`StartupSession::Select`].
    pub startup_session: StartupSession,
    /// The display name to give the Session this launch binds, from
    /// `--name`. A Session is otherwise unnamed and `/resume` shows it by
    /// its first message; naming one at startup is the same metadata
    /// operation `/name` performs, moved to the command line.
    pub session_name: Option<String>,
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

impl LaunchLocations {
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

/// Raw user intent. Absence is preserved until resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchRequest {
    pub models: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub runtime_root: Option<PathBuf>,
    pub model: Option<String>,
    pub trust: Option<TrustAction>,
    pub skill_paths: Vec<PathBuf>,
    pub no_skills: bool,
    pub no_builtin_tools: bool,
    pub no_tools: bool,
    pub startup_session: StartupSession,
    pub session_name: Option<String>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustAction {
    Grant,
    Revoke,
}

/// Captured once; tests supply isolated snapshots without changing process globals.
#[derive(Debug, Clone)]
pub struct HostEnvironment {
    pub launch_directory: PathBuf,
    pub config_directory: PathBuf,
    pub state_directory: PathBuf,
}

impl HostEnvironment {
    /// Capture the supported Unix host paths. Relative XDG paths are errors.
    ///
    /// # Errors
    /// Fails if the launch directory or required absolute host paths are unavailable.
    pub fn capture() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is required to locate user configuration and trust")?;
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
        Self::from_paths(
            std::env::current_dir().map_err(|e| e.to_string())?,
            home,
            config,
            state,
        )
    }

    /// Build an isolated host snapshot from explicit paths.
    ///
    /// # Errors
    /// Rejects relative HOME or XDG roots.
    #[allow(clippy::needless_pass_by_value)] // captured path inputs transfer together
    pub fn from_paths(
        launch_directory: PathBuf,
        home: PathBuf,
        config: Option<PathBuf>,
        state: Option<PathBuf>,
    ) -> Result<Self, String> {
        if !home.is_absolute()
            || config.as_ref().is_some_and(|p| !p.is_absolute())
            || state.as_ref().is_some_and(|p| !p.is_absolute())
        {
            return Err("HOME, XDG_CONFIG_HOME and XDG_STATE_HOME must be absolute paths".into());
        }
        // Use the same XDG convention on Linux and macOS; no platform-dependent fallback chain.
        Ok(Self {
            launch_directory,
            config_directory: config.unwrap_or_else(|| home.join(".config")).join("rustx"),
            state_directory: state
                .unwrap_or_else(|| home.join(".local/state"))
                .join("rustx"),
        })
    }
}

/// Values are never included: provenance cannot expose credentials or environment values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Builtin,
    User { document: PathBuf, base: PathBuf },
    Project { document: PathBuf, base: PathBuf },
    Cli { base: PathBuf },
}

/// Validated launch authority consumed by the existing native composition owner.
#[derive(Debug, Clone)]
pub struct ResolvedLaunch {
    pub(crate) credentials: crate::credentials::CredentialSnapshot,
    prospective: Box<ProspectiveLaunch>,
}

/// Static next-launch values. This type grants no execution authority and contains
/// no credential snapshot. Only runtime admission can produce a resolved launch.
#[derive(Clone)]
pub struct ProspectiveLaunch {
    pub(crate) request: LaunchRequest,
    pub(crate) host: HostEnvironment,
    pub(crate) python_local_status:
        BTreeMap<crate::runtime::identity::McpServerId, PythonLocalStatus>,
    pub(crate) trusted: bool,
    pub(crate) workflows: std::sync::Arc<crate::runtime::workflow::WorkflowCatalog>,
    pub(crate) workflow_dependencies: std::sync::Arc<
        BTreeMap<
            crate::runtime::workflow::WorkflowId,
            Vec<crate::runtime::workflow::inspection::ToolDependency>,
        >,
    >,
    pub(crate) subagents: crate::runtime::subagent::SubagentCatalog,
    pub(crate) skill_names: Vec<String>,
    pub(crate) role_root: PathBuf,
    pub(crate) role_sources:
        BTreeMap<crate::runtime::subagent::SubagentName, super::subagent_resources::RoleSource>,
    pub(crate) source_activations: BTreeMap<
        crate::runtime::identity::McpServerId,
        crate::capabilities::activation::SourceActivation,
    >,
    pub(crate) selected_tools: Option<Vec<String>>,
    pub(crate) locations: LaunchLocations,
    pub(crate) config: std::sync::Arc<CurrentRuntimeConfig>,
    pub(crate) models: ModelCatalog,
    pub(crate) provenance: BTreeMap<String, Origin>,
    pub(crate) identity: String,
    pub(crate) skill_roots: Vec<PathBuf>,
    /// Fixed document slots for explicit resource reload; never discovery.
    documents: Vec<(PathBuf, bool, Origin)>,
    /// Project-origin paths retain their authority even after becoming absolute.
    project_resources: Vec<PathBuf>,
}

/// Inert package validation facts, independent of activation and MCP readiness.
#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PythonLocalStatus {
    Valid,
    Missing,
    Invalid,
}

impl ProspectiveLaunch {
    /// Apply real trust admission before calling the credential owner.
    ///
    /// # Errors
    /// An untrusted workspace never reaches the credential capture callback.
    #[allow(clippy::unnecessary_debug_formatting)] // escape the actionable shell path
    pub fn admit(
        self,
        credentials: impl FnOnce() -> crate::credentials::CredentialSnapshot,
    ) -> Result<ResolvedLaunch, String> {
        if !self.trusted {
            return Err(format!(
                "project {} is not trusted; run rustx --workspace {:?} --trust grant (revoke with --trust revoke)",
                self.workspace.display(),
                self.workspace
            ));
        }
        Ok(ResolvedLaunch {
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
                &self.role_root
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

    pub(crate) fn validate_workspace_resource_roots(&self) -> Result<(), String> {
        for relative in [".agents/tools", ".agents/skills"] {
            if relative == ".agents/skills" && self.no_skills {
                continue;
            }
            crate::runtime::resources::validate_project_resource_path(
                &self.workspace,
                &self.workspace.join(relative),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub(crate) fn settings_view(&self) -> crate::runtime_client::settings::LaunchSettings {
        use crate::runtime_client::settings::{LaunchSettings, ModelDefault, SettingOrigin};
        let origin = |field: &str| match self.provenance.get(field) {
            Some(Origin::User { document, .. }) => SettingOrigin::User {
                document: document.display().to_string(),
            },
            Some(Origin::Project { document, .. }) => SettingOrigin::Project {
                document: document.display().to_string(),
            },
            Some(Origin::Cli { .. }) => SettingOrigin::Cli,
            _ => SettingOrigin::Builtin,
        };
        LaunchSettings {
            model: ModelDefault {
                model: self.config.model.model.clone(),
                reasoning_profile: self.config.model.reasoning_profile.clone(),
            },
            model_origin: origin("model.model"),
            reasoning_origin: origin("model.reasoning_profile"),
            approval_mode: self.config.approval_mode,
            approval_origin: origin("approval_mode"),
            runtime_root_origin: origin("runtime_root"),
            tool_selection_origin: if self.no_tools
                || self.no_builtin_tools
                || self.tools.is_some()
                || self.request.exclude_tools.is_some()
            {
                SettingOrigin::Cli
            } else {
                origin("default_tools")
            },
        }
    }

    /// The immutable validated settings this launch resolved.
    #[must_use]
    pub fn config(&self) -> &CurrentRuntimeConfig {
        &self.config
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
            rebase_paths(&mut layer, origin, &self.workspace)?;
            merged.overlay(layer, origin, &mut provenance);
        }
        if !self.skill_paths.is_empty() {
            merged.skills = Some(self.skill_paths.clone());
        }
        merged.model = Some(ModelLayer {
            model: Some(self.config.model.model.clone()),
            ..Default::default()
        });
        let resources = merged.resolve()?;
        let mut config = self.config.as_ref().clone();
        RuntimeLayer::copy_resources(&mut config, resources);
        config.validate().map_err(|e| e.to_string())?;
        Ok((config, provenance))
    }
}

impl std::ops::Deref for ResolvedLaunch {
    type Target = ProspectiveLaunch;
    fn deref(&self) -> &Self::Target {
        &self.prospective
    }
}

impl std::ops::Deref for ProspectiveLaunch {
    type Target = LaunchLocations;
    fn deref(&self) -> &Self::Target {
        &self.locations
    }
}

/// Read-only location resolution is also used by inspection and trust commands.
/// It neither reads model/settings documents nor creates runtime state.
///
/// # Errors
/// Rejects invalid directories or an exhausted discovery bound.
pub fn resolve_locations(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<(LaunchLocations, String), String> {
    if let Some(names) = &request.exclude_tools {
        crate::capabilities::validate_tool_names(names, "exclusion")?;
    }
    crate::capabilities::ToolActivationPolicy {
        no_tools: request.no_tools,
        no_builtin_tools: request.no_builtin_tools,
        tools: request.tools.clone(),
        exclude_tools: request.exclude_tools.clone().unwrap_or_default(),
        ..Default::default()
    }
    .validate()?;
    let launch = canonical_directory(&host.launch_directory)?;
    let workspace = match &request.workspace {
        Some(path) => canonical_directory(&absolute(&launch, path))?,
        None => discover_workspace(&launch)?,
    };
    let identity = workspace_identity(&workspace);
    let runtime_root = absolute(
        &launch,
        &request
            .runtime_root
            .clone()
            .unwrap_or_else(|| host.state_directory.join("workspaces").join(&identity)),
    );
    Ok((
        LaunchLocations {
            workspace,
            runtime_root,
            skill_paths: request
                .skill_paths
                .iter()
                .map(|p| absolute(&launch, p))
                .collect(),
            no_skills: request.no_skills,
            no_builtin_tools: request.no_builtin_tools,
            no_tools: request.no_tools,
            startup_session: request.startup_session.clone(),
            session_name: request.session_name.clone(),
            tools: request.tools.clone(),
            exclude_tools: request.exclude_tools.clone().unwrap_or_default(),
        },
        identity,
    ))
}

/// User-owned membership directories make grant/revoke atomic and independent per identity.
///
/// # Errors
/// Rejects invalid workspace/store ownership and filesystem failures.
pub fn change_trust(
    request: &LaunchRequest,
    host: &HostEnvironment,
    action: TrustAction,
) -> Result<(), String> {
    #[cfg(test)]
    super::static_effects::observe(super::static_effects::Effect::Trust);
    let (locations, identity) = resolve_locations(request, host)?;
    let root = trust_root(host, &locations.workspace)?;
    let record = root.join(identity);
    match action {
        TrustAction::Grant => {
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            match std::fs::create_dir(&record) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && record.is_dir() => {
                    Ok(())
                }
                Err(e) => Err(e.to_string()),
            }
        }
        TrustAction::Revoke => match std::fs::remove_dir(&record) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        },
    }
}

fn trust_root(host: &HostEnvironment, workspace: &Path) -> Result<PathBuf, String> {
    let root = normalize_missing(&host.state_directory.join("trust"))?;
    if root.starts_with(workspace) || workspace.starts_with(&root) {
        return Err("host trust store must be outside the project workspace".into());
    }
    Ok(root)
}

fn present_on_disk(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

/// Resolve inspection state without loading models or project configuration.
/// Only the host state-location member is relevant; runtime settings stay inert.
///
/// # Errors
/// Rejects invalid filesystem locations or an unreadable/malformed host state reference.
pub fn resolve_inspection_locations(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<LaunchLocations, String> {
    let (mut locations, _) = resolve_locations(request, host)?;
    let settings = host.config_directory.join("settings.toml");
    if request.runtime_root.is_none() && present_on_disk(&settings)? {
        // Location-only inspection must not validate unrelated model/resource
        // fields. This bounded projection grants no configuration authority.
        #[derive(serde::Deserialize)]
        struct StateLocation {
            runtime_root: Option<PathBuf>,
        }
        let document: StateLocation = crate::toml_authoring::parse(&read_bounded(&settings)?)?;
        if let Some(path) = document.runtime_root {
            if path.as_os_str().is_empty() {
                return Err("user runtime_root must be a non-empty path".into());
            }
            locations.runtime_root = normalize_missing(&absolute(&host.config_directory, &path))?;
        }
    }
    Ok(locations)
}

/// Resolve once, before any native composition or Session publication.
///
/// # Errors
/// Rejects invalid documents, unauthorized fields, absent trust, invalid paths
/// and semantic configuration failures, without publishing runtime state.
#[allow(
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::unnecessary_debug_formatting
)] // derived paths always have parents; debug escapes diagnostic paths
pub fn analyze(
    request: &LaunchRequest,
    host: &HostEnvironment,
) -> Result<ProspectiveLaunch, LaunchFailure> {
    let (mut locations, identity) = resolve_locations(request, host)?;
    let launch = canonical_directory(&host.launch_directory)?;
    let user_path = host.config_directory.join("settings.toml");
    let project_path = request.config.as_ref().map_or_else(
        || locations.workspace.join("rustx.toml"),
        |p| absolute(&launch, p),
    );
    let mut user = read_layer(&user_path, false, false)?;
    let project = read_layer(&project_path, request.config.is_some(), true)?;
    // Even an empty workspace requires trust: files added before composition or reload
    // must never turn a previously inert launch into project activation.
    let trusted = trust_root(host, &locations.workspace)?
        .join(&identity)
        .is_dir();
    let user_models = user.models.take();
    let user_selected_catalog = user_models.is_some();
    let models_path = request
        .models
        .as_ref()
        .map(|p| absolute(&launch, p))
        .or_else(|| user_models.map(|p| absolute(user_path.parent().expect("parent"), &p)))
        .unwrap_or_else(|| host.config_directory.join("models.toml"));
    let state = user.runtime_root.take();
    let state_origin = if state.is_some() {
        Origin::User {
            document: user_path.clone(),
            base: user_path.parent().expect("parent").into(),
        }
    } else {
        Origin::Builtin
    };
    if request.runtime_root.is_none()
        && let Some(value) = state
    {
        locations.runtime_root = absolute(user_path.parent().expect("parent"), &value);
    }
    locations.runtime_root = normalize_missing(&locations.runtime_root)?;
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
    let model_bytes = read_bounded(&models_path).map_err(|detail| {
        let mut error = LaunchFailure::at(
            Some(models_path.clone()),
            "$",
            "model catalog is unavailable",
            "run rustx init or correct the explicit catalog path",
            detail,
        );
        error.incomplete =
            request.models.is_none() && !user_selected_catalog && !models_path.exists();
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
    let documents = vec![
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
        project_resources.extend(rebase_paths(&mut layer, &origin, &locations.workspace)?);
        merged.overlay(layer, &origin, &mut provenance);
    }
    if !request.skill_paths.is_empty() {
        merged.skills = Some(locations.skill_paths.clone());
        provenance.insert(
            "skills".into(),
            Origin::Cli {
                base: launch.clone(),
            },
        );
    }
    if let Some(model) = &request.model {
        merged.model = Some(ModelLayer {
            model: Some(crate::model::catalog::ModelRef::parse(model).map_err(|e| e.to_string())?),
            ..Default::default()
        });
        provenance.retain(|key, _| !key.starts_with("model."));
        provenance.insert(
            "model.model".into(),
            Origin::Cli {
                base: launch.clone(),
            },
        );
    }
    if merged
        .model
        .as_ref()
        .is_none_or(|model| model.model.is_none())
    {
        let mut error = LaunchFailure::at(
            Some(user_path.clone()),
            "model.model",
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
    let config = merged.resolve()?;
    config.validate().map_err(|e| e.to_string())?;
    config
        .tool_deadline_policy
        .to_policy()
        .map_err(|e| e.clone())?;
    config.tool_environment().map_err(|e| e.to_string())?;
    let (primary, summary) =
        crate::model::session::analyze_session_model_config(&models, &config.model)
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
                Some(user_path.clone()),
                "context",
                "context budgets cannot fit the selected models",
                "correct reserves, recent-history budget, or explicit model limits",
                error.message,
            )
        })?;
    RuntimeLayer::record_default_origins(&config, &mut provenance);
    for (key, present) in [
        ("skills.cli", !request.skill_paths.is_empty()),
        ("no_skills", request.no_skills),
        ("no_tools", request.no_tools),
        ("no_builtin_tools", request.no_builtin_tools),
        ("tools", request.tools.is_some()),
        ("exclude_tools", request.exclude_tools.is_some()),
    ] {
        provenance.insert(
            key.into(),
            if present {
                Origin::Cli {
                    base: launch.clone(),
                }
            } else {
                Origin::Builtin
            },
        );
    }
    for (key, explicit, fallback) in [
        ("workspace", request.workspace.is_some(), Origin::Builtin),
        ("runtime_root", request.runtime_root.is_some(), state_origin),
        (
            "models",
            request.models.is_some(),
            Origin::User {
                document: if user_selected_catalog {
                    user_path.clone()
                } else {
                    models_path.clone()
                },
                base: if user_selected_catalog {
                    user_path.parent().expect("parent").into()
                } else {
                    models_path.parent().expect("parent").into()
                },
            },
        ),
    ] {
        provenance.insert(
            key.into(),
            if explicit {
                Origin::Cli {
                    base: launch.clone(),
                }
            } else {
                fallback
            },
        );
    }
    let skill_roots = if request.no_skills {
        Vec::new()
    } else {
        vec![
            host.config_directory.join("skills"),
            locations.workspace.join(".agents/skills"),
        ]
    };
    let workflows = if trusted {
        super::workflow_resources::load(&locations.workspace, &config.workflows, &config.subagents)
            .map_err(LaunchFailure::resource)?
    } else {
        crate::runtime::workflow::WorkflowCatalog::empty()
    };
    // Capture authority once, including host path aliases and missing leaves.
    // Reload reuses this physical identity; validators must not rebind it.
    let role_root = normalize_missing(&host.config_directory.join("subagents"))?;
    if trusted {
        crate::runtime::load_project_context_files(&locations.workspace)
            .map_err(LaunchFailure::resource)?;
    }
    let (subagents, role_sources) = if trusted {
        super::subagent_resources::load(&locations.workspace, &role_root, &config.subagents)
            .map_err(LaunchFailure::resource)?
    } else {
        (
            crate::runtime::subagent::SubagentCatalog::empty(),
            BTreeMap::new(),
        )
    };
    let workspace =
        crate::tools::workspace::Workspace::new(&locations.workspace).map_err(|e| e.to_string())?;
    let skill_packages = if trusted {
        if !request.no_skills {
            crate::runtime::resources::validate_project_resource_path(
                &locations.workspace,
                &locations.workspace.join(".agents/skills"),
            )
            .map_err(LaunchFailure::resource)?;
        }
        crate::skills::SkillDiscovery::with_config(
            &workspace,
            crate::skills::SkillDiscoveryConfig {
                automatic_roots: skill_roots.clone(),
                explicit_paths: config.skills.clone(),
            },
        )
        .discover()
        .map_err(|e| {
            LaunchFailure::at(
                None,
                "skills",
                "invalid local Skill package or path",
                "correct the explicit Skill path and package metadata",
                e.to_string(),
            )
        })?
    } else {
        Vec::new()
    };
    let skill_names = skill_packages
        .iter()
        .map(|package| package.name().to_owned())
        .collect();
    let skills = crate::skills::SkillSnapshot::new(
        skill_packages
            .into_iter()
            .map(std::sync::Arc::new)
            .collect(),
    );
    crate::runtime::subagent::SubagentResolver::validate_local_references(
        &subagents,
        &skills,
        |reference| {
            crate::model::invocation::analyze_selection(
                models.model(reference).map_err(|e| e.to_string())?,
                &crate::model::invocation::ModelSelection::of(reference.clone()),
                crate::model::invocation::RequestParamsLayer::SessionOverrides,
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        },
    )
    .map_err(|(name, error)| {
        LaunchFailure::at(
            None,
            &format!("subagents.definitions.{name}"),
            "invalid local model or Skill reference",
            "select a declared model and an available local Skill",
            error.to_string(),
        )
    })?;
    let main_subagents = if trusted {
        subagents
            .admitted(&config.subagents.main.iter().cloned().collect())
            .map_err(|e| e.to_string())?
    } else {
        crate::runtime::subagent::SubagentCatalog::empty()
    };
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
            .main()
            .iter()
            .filter_map(|id| workflows.get(id))
            .map(|program| crate::tools::native::workflow_definition(program)),
    );
    let mut source_activations = BTreeMap::new();
    for (id, source) in &config.mcp_servers {
        source_activations.insert(
            id.clone(),
            SourceActivation::evaluate(
                source.enabled.map(|enabled| {
                    if enabled {
                        SourceEnablement::Enabled
                    } else {
                        SourceEnablement::Disabled
                    }
                }),
                trusted,
            ),
        );
    }
    for (id, intent) in &config.python_sources {
        source_activations.insert(
            id.clone(),
            SourceActivation::evaluate(Some(*intent), trusted),
        );
    }
    let mut python_local_status = BTreeMap::new();
    for (id, activation) in &source_activations {
        if config.python_sources.contains_key(id) && *activation == SourceActivation::Enabled {
            python_local_status.insert(id.clone(), PythonLocalStatus::Missing);
        }
    }
    if trusted {
        crate::runtime::resources::validate_project_resource_path(
            &locations.workspace,
            &locations.workspace.join(".agents/tools"),
        )
        .map_err(LaunchFailure::resource)?;
        let packages = crate::tools::python::discover_admitted_python_packages(&workspace, |id| {
            *source_activations
                .entry(id.clone())
                .or_insert(SourceActivation::Unconfigured)
                == SourceActivation::Enabled
        })
        .map_err(|e| e.to_string())?;
        for package in packages {
            python_local_status.insert(
                package.server_id,
                if package.outcome.is_ok() {
                    PythonLocalStatus::Valid
                } else {
                    PythonLocalStatus::Invalid
                },
            );
        }
    }
    let availability = source_activations
        .iter()
        .map(|(id, activation)| {
            (
                crate::capabilities::CapabilitySourceId::Mcp(id.clone()),
                crate::capabilities::CapabilitySourceState::before_preparation(*activation),
            )
        })
        .collect();
    for definition in subagents.definitions() {
        crate::runtime::subagent::resolver::validate_metadata_selectors(
            definition,
            &definitions,
            &availability,
        )
        .map_err(|e| {
            LaunchFailure::at(
                None,
                &format!("subagents.definitions.{}.tools", definition.name()),
                "invalid local Tool reference",
                "use a known source-qualified Tool selector",
                e.to_string(),
            )
        })?;
    }
    // Every Agent node's trusted static invocation override is validated
    // against the same prospective metadata, offline and side-effect free.
    // An unavailable source is tolerated per selector rather than ending the
    // walk, so it cannot hide a statically invalid selection listed later.
    workflows
        .validate_agent_overrides(&definitions, &availability, &skills)
        .map_err(|e| {
            let reason: String = e.reason.chars().take(1024).collect();
            LaunchFailure::at(
                Some(
                    locations
                        .workspace
                        .join(".agents/workflows")
                        .join(format!("{}.yaml", e.workflow)),
                ),
                &e.path,
                &reason,
                "select a capability and Skill this generation admits, or remove the \
                 invocation override",
                e.to_string(),
            )
        })?;
    let workflow_dependencies = workflows
        .inspect_metadata(&definitions, &availability, |definition| {
            Ok(native_leaves.contains(&definition.id))
        })
        .map_err(|e| {
            let reason: String = e.reason.chars().take(1024).collect();
            LaunchFailure::at(
                Some(
                    locations
                        .workspace
                        .join(".agents/workflows")
                        .join(format!("{}.yaml", e.workflow)),
                ),
                &e.path,
                &reason,
                "select a known eligible native leaf or a declared external source",
                e.to_string(),
            )
        })?;
    let mut defaults = config.default_tools.clone();
    defaults.extend(workflows.main().iter().map(ToString::to_string));
    let online = config
        .mcp_servers
        .values()
        .any(|source| source.enabled == Some(true))
        || config
            .python_sources
            .values()
            .any(|intent| *intent == crate::capabilities::activation::SourceEnablement::Enabled);
    let policy = crate::capabilities::ToolActivationPolicy {
        default_tools: Some(defaults),
        no_tools: locations.no_tools,
        no_builtin_tools: locations.no_builtin_tools,
        tools: locations.tools.clone(),
        exclude_tools: locations.exclude_tools.clone(),
    };
    let selected_tools = if (online || !trusted) && !locations.no_tools {
        None
    } else {
        Some(
            crate::capabilities::select_definitions(
                &definitions.iter().collect::<Vec<_>>(),
                &policy,
            )?
            .into_iter()
            .map(|definition| definition.name.clone())
            .collect(),
        )
    };
    Ok(ProspectiveLaunch {
        request: request.clone(),
        host: host.clone(),
        workflow_dependencies: std::sync::Arc::new(workflow_dependencies),
        python_local_status,
        trusted,
        workflows: std::sync::Arc::new(workflows),
        subagents,
        role_root,
        role_sources,
        skill_names,
        source_activations,
        selected_tools,
        locations,
        config: std::sync::Arc::new(config),
        models,
        provenance,
        identity,
        skill_roots,
        documents,
        project_resources,
    })
}

/// Resolve static launch semantics, then require real host trust before admission.
///
/// # Errors
/// Rejects invalid configuration or an untrusted workspace, without activation.
pub fn resolve(request: &LaunchRequest, host: &HostEnvironment) -> Result<ResolvedLaunch, String> {
    analyze(request, host)?.admit(crate::credentials::CredentialSnapshot::capture)
}

impl std::fmt::Debug for ProspectiveLaunch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProspectiveLaunch")
            .field("trusted", &self.trusted)
            .field("configuration", &"<redacted>")
            .finish_non_exhaustive()
    }
}

fn absolute(base: &Path, path: &Path) -> PathBuf {
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

fn canonical_directory(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path)
        .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(path)
}

fn normalize_missing(path: &Path) -> Result<PathBuf, String> {
    if present_on_disk(path)? {
        return std::fs::canonicalize(path).map_err(|e| e.to_string());
    }
    let parent = path.parent().ok_or("path has no existing ancestor")?;
    let name = path.file_name().ok_or("invalid path component")?;
    Ok(normalize_missing(parent)?.join(name))
}

fn discover_workspace(launch: &Path) -> Result<PathBuf, String> {
    for (depth, directory) in launch.ancestors().enumerate() {
        if depth >= 128 {
            return Err("workspace discovery exceeded 128 directories; pass --workspace".into());
        }
        if present_on_disk(&directory.join(".git"))?
            || present_on_disk(&directory.join("rustx.toml"))?
        {
            return Ok(directory.into());
        }
    }
    Ok(launch.into())
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
    if let Some(subagents) = &layer.subagents
        && let Some(names) = &subagents.definitions
    {
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        if unique.len() != names.len() {
            return Err(LaunchFailure::at(
                Some(path.into()),
                "subagents.definitions",
                "duplicate role registration",
                "register each canonical identity once per layer",
                "duplicate role registration".into(),
            ));
        }
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
) -> Result<Vec<PathBuf>, String> {
    let mut resources = Vec::new();
    let base = match origin {
        Origin::User { base, .. } | Origin::Project { base, .. } | Origin::Cli { base } => base,
        Origin::Builtin => return Ok(resources),
    };
    let mut path = |value: &mut PathBuf| -> Result<(), String> {
        if value.as_os_str().is_empty() {
            return Err("path must be non-empty".into());
        }
        let resolved = absolute(base, value);
        if matches!(origin, Origin::Project { .. }) {
            crate::runtime::resources::validate_project_resource_path(workspace, &resolved)
                .map_err(|e| e.to_string())?;
            resources.push(resolved.clone());
        }
        *value = resolved;
        Ok(())
    };
    if let Some(skills) = &mut layer.skills {
        for skill in skills {
            path(skill)?;
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
