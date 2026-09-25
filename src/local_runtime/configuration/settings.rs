//! Typed semantic-unit authoring. Native writers canonicalize rustX-owned documents.
//! Typed CAS persistence transfers application responsibility to the native coordinator.
use super::super::authoring::{
    ContextLayer, McpAuthoring, ModelLayer, RuntimeLayer, SubagentsLayer, TimeoutLayer,
    ToolDeadlineLayer,
};
use super::UserConfigManager;
use crate::model::authoring::{Model, Provider};
use crate::model::catalog::{CredentialSource, CredentialSourceView};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    User,
    Workspace,
}

/// Native source authority. A Session is never an authoring target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceTarget {
    User,
    Workspace { directory: PathBuf },
}

impl SourceTarget {
    #[must_use]
    pub const fn scope(&self) -> SourceScope {
        match self {
            Self::User => SourceScope::User,
            Self::Workspace { .. } => SourceScope::Workspace,
        }
    }

    /// Revalidate the physical authority on every operation; do not follow a
    /// replaced directory to a different source owner.
    /// # Errors
    /// Rejects relative, removed, redirected, or noncanonical Workspace paths.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if let Self::Workspace { directory } = self
            && (!directory.is_absolute()
                || super::canonical_directory(directory).map_err(|_| SettingsError::Invalid)?
                    != *directory)
        {
            return Err(SettingsError::Invalid);
        }
        Ok(())
    }

    #[must_use]
    pub fn workspace(&self) -> Option<&Path> {
        match self {
            Self::User => None,
            Self::Workspace { directory } => Some(directory),
        }
    }

    #[must_use]
    pub fn application_scope(&self) -> String {
        match self {
            Self::User => "source:user".into(),
            Self::Workspace { directory } => format!("source:workspace:{}", directory.display()),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderView {
    pub base_url: String,
    pub credential: CredentialSourceView,
}
impl From<Provider> for ProviderView {
    fn from(provider: Provider) -> Self {
        Self {
            base_url: provider.base_url,
            credential: provider.api_key.view(),
        }
    }
}

/// The one redacted projection of an authored or resolved configuration
/// document that ever leaves native authority.
///
/// Both secret-bearing members are replaced by identity-only facts: a Provider
/// becomes its [`ProviderView`], and the environment map becomes the list of
/// identities the document authors. Nothing outside this module may construct
/// the projection from the authoring type except through [`redact`], so an
/// added secret-bearing member has exactly one place to be redacted.
pub type SourceDocumentView =
    RuntimeLayer<ProviderView, crate::local_runtime::authoring::EnvironmentIdentities>;
/// Project one authored document into its redacted wire view.
pub fn redact<P: Into<ProviderView>>(
    document: RuntimeLayer<P, crate::local_runtime::authoring::AuthoredEnvironment>,
) -> SourceDocumentView {
    document.project(Into::into, |environment| environment.into_keys().collect())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialEdit {
    Retain,
    Environment { variable: String },
    Literal { value: String },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderWrite {
    pub base_url: String,
    pub credential: CredentialEdit,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SourceView<T> {
    pub path: PathBuf,
    pub revision: String,
    pub authored: Option<T>,
    pub diagnostic: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentSourceView {
    pub scope: SourceScope,
    pub name: crate::runtime::subagent::SubagentName,
    pub source: SourceView<super::super::config::AgentProfileDocument>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpView {
    pub definition: McpAuthoring,
    pub retained_env: Vec<String>,
    pub retained_headers: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpWrite {
    pub definition: McpAuthoring,
    #[serde(default)]
    pub retained_env: Vec<String>,
    #[serde(default)]
    pub retained_headers: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SourceSettings {
    pub target: SourceTarget,
    pub process_policy_impacts: BTreeMap<String, ProcessPolicyImpact>,
    pub process_bindings: Option<super::super::app_server_policy::AppServerPolicy>,
    pub application: Option<super::application::ConfigurationApplication>,
    /// Native current-file approval resolution. None when prospective configuration is invalid.
    pub prospective_approval_mode: Option<crate::runtime::ApprovalMode>,
    /// Current-file analysis, separate from the published runtime. Never prepares sources.
    pub prospective_resources: Option<crate::runtime::capability_inspection::CapabilityInspection>,
    pub prospective_diagnostic: Option<String>,
    /// Exact CAS token for a currently absent resource identity.
    pub absent_resource_revision: String,
    pub resource_revisions: BTreeMap<PathBuf, String>,
    /// Read-only native source resolution; never a Session adopted binding.
    pub resolved: Option<SourceDocumentView>,
    /// The catalog a Session created in this Workspace binds, exactly as its
    /// `session/models` then serves it. Absent for the User target. Clients
    /// select pre-Session models only from here, never from `resolved`.
    pub session_models: Option<SessionModelsView>,
    pub provenance: BTreeMap<String, super::Origin>,
    pub user: SourceView<SourceDocumentView>,
    pub workspace: Option<SourceView<SourceDocumentView>>,
    pub user_resource_root: PathBuf,
    pub workspace_resource_root: Option<PathBuf>,
    pub runtime_root: PathBuf,
    pub user_mcp: SourceView<BTreeMap<crate::runtime::identity::McpServerId, McpView>>,
    pub workspace_mcp: Option<SourceView<BTreeMap<crate::runtime::identity::McpServerId, McpView>>>,
    pub agents: Vec<AgentSourceView>,
}

/// Whether native can bind a Session in a Workspace, and its model catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionModelsView {
    /// The validated credential-free catalog of the Session creation binding.
    Available {
        catalog: crate::model::catalog::ModelCatalogView,
    },
    /// Session creation would fail here; no catalog exists to select from.
    Unavailable { diagnostic: String },
}

/// Redacted, immutable facts read at the runtime configuration publication lock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EffectiveConfiguration {
    pub process_bindings: Option<super::super::app_server_policy::AppServerPolicy>,
    pub application: Option<super::application::ConfigurationApplication>,
    pub adopted_binding: u64,
    pub source_revisions: BTreeMap<PathBuf, String>,
    pub generation: crate::runtime::identity::RuntimeResourceRevision,
    pub document: SourceDocumentView,
    pub root_agent: super::super::config::AgentProfileDocument,
    pub context: super::super::config::ContextPolicyDocument,
    pub model_timeout: super::super::config::ModelTimeoutPolicyDocument,
    pub tool_deadline: super::super::config::ToolDeadlinePolicyDocument,
    pub child_capacity: super::super::config::SubagentsDocument,
    pub approval_mode: crate::runtime::ApprovalMode,
    pub provenance: BTreeMap<String, super::Origin>,
    pub resources: crate::runtime::capability_inspection::CapabilityInspection,
    pub available_tools: Vec<crate::tools::types::ToolDefinition>,
    pub session_model: Option<crate::model::session::SessionModelConfig>,
    pub effective_model: crate::model::session::SessionModelView,
    pub admitted_attempt: Option<AdmittedConfiguration>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AdmittedConfiguration {
    pub approval_mode: crate::runtime::ApprovalMode,
    pub model_timeout: Option<super::super::config::ModelTimeoutPolicyDocument>,
    pub tool_deadline: Option<super::super::config::ToolDeadlinePolicyDocument>,
    pub attempt: crate::runtime::identity::AttemptId,
    pub generation: crate::runtime::identity::RuntimeResourceRevision,
    pub model: crate::model::session::SessionModelView,
    pub resources: crate::runtime::capability_inspection::CapabilityInspection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeTool {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Bash,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "unit", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigMutation {
    Provider {
        id: String,
        authored: Option<ProviderWrite>,
    },
    Model {
        id: String,
        authored: Option<Model>,
    },
    RootModel {
        authored: Option<ModelLayer>,
    },
    NativeTools {
        authored: Option<Vec<String>>,
    },
    SourceTools {
        id: crate::capabilities::ToolSourceId,
        authored: Option<crate::capabilities::selection::SourceToolSelection>,
    },
    Skills {
        authored: Option<crate::runtime::agent_profile::AgentSkillSelection>,
    },
    Todo {
        authored: Option<crate::extensions::TodoExtensionDocument>,
    },
    Goal {
        authored: Option<crate::extensions::GoalExtensionDocument>,
    },
    AgentStatus {
        authored: Option<crate::extensions::AgentStatusExtensionDocument>,
    },
    Agents {
        authored: Option<Vec<crate::runtime::subagent::SubagentName>>,
    },
    Workflows {
        authored: Option<Vec<crate::runtime::workflow::WorkflowId>>,
    },
    AgentIdentity {
        authored: Option<crate::runtime::identity::AgentId>,
    },
    Description {
        authored: Option<String>,
    },
    Instructions {
        authored: Option<String>,
    },
    ProjectGuidance {
        authored: Option<super::super::config::AgentProjectInstructionsDocument>,
    },
    Context {
        authored: Option<ContextLayer>,
    },
    ModelTimeout {
        authored: Option<TimeoutLayer>,
    },
    ToolDeadline {
        authored: Option<ToolDeadlineLayer>,
    },
    Capacity {
        authored: Option<SubagentsLayer>,
    },
    Approval {
        authored: Option<crate::runtime::ApprovalMode>,
    },
    NativePolicy {
        id: NativeTool,
        authored: Option<super::super::config::NativePolicyOverrideDocument>,
    },
    McpPolicy {
        id: crate::runtime::identity::McpServerId,
        authored: Option<super::super::config::InvocationPolicyDocument>,
    },
    Environment {
        name: String,
        authored: Option<String>,
    },
    AppServer {
        authored: Option<super::super::app_server_policy::AppServerPolicy>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[allow(clippy::large_enum_variant)] // Finite wire command; parsed and consumed once per CAS operation.
pub enum SourceMutation {
    /// Explicit repair of an unparseable configuration document. Never used
    /// for ordinary semantic-unit editing and never accepts an invalid source.
    RepairConfig {
        document: String,
    },
    Mcp {
        id: crate::runtime::identity::McpServerId,
        authored: Option<McpWrite>,
    },
    Config {
        mutation: ConfigMutation,
    },
    Agent {
        name: crate::runtime::subagent::SubagentName,
        authored: Option<super::super::config::AgentProfileDocument>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPolicyImpact {
    Hot,
    Restart,
}
#[derive(Debug)]
pub enum SettingsError {
    Conflict {
        scope: SourceScope,
        expected: String,
        actual: String,
    },
    Invalid,
    Io,
    Committed,
}
fn read(path: &Path) -> Result<Option<Vec<u8>>, SettingsError> {
    super::super::settings::read_document(path).map_err(|_| SettingsError::Io)
}
fn revision(bytes: Option<&[u8]>) -> String {
    super::super::settings::revision(bytes)
}
fn parse(bytes: Option<&[u8]>) -> Result<RuntimeLayer, SettingsError> {
    crate::toml_authoring::parse(bytes.unwrap_or(b"")).map_err(|_| SettingsError::Invalid)
}
fn source_resolution_diagnostic(document: &RuntimeLayer) -> Option<String> {
    let validate = || -> Result<(), String> {
        let config = document.clone().resolve()?;
        let catalog = crate::model::catalog::ModelCatalog::from_document(
            crate::model::authoring::Catalog {
                schema_version: crate::model::catalog::MODEL_CATALOG_SCHEMA_VERSION,
                providers: document.providers.clone().unwrap_or_default(),
                models: document.models.clone().unwrap_or_default(),
            }
            .into(),
        )
        .map_err(|error| error.to_string())?;
        catalog
            .model(&config.initial_model().model)
            .map_err(|error| error.to_string())?;
        Ok(())
    };
    validate().err()
}
fn view(path: PathBuf, bytes: Option<&[u8]>) -> SourceView<SourceDocumentView> {
    let parsed = parse(bytes);
    let diagnostic = parsed
        .as_ref()
        .err()
        .map(|_| "invalid rustx.toml; source was not loaded".into());
    SourceView {
        path,
        revision: revision(bytes),
        diagnostic,
        authored: parsed.ok().map(redact),
    }
}
fn mcp_view(
    path: PathBuf,
) -> Result<SourceView<BTreeMap<crate::runtime::identity::McpServerId, McpView>>, SettingsError> {
    let bytes = read(&path)?;
    let parsed: Result<super::super::mcp_resources::McpDocument, _> =
        crate::toml_authoring::parse(bytes.as_deref().unwrap_or(b""));
    let diagnostic = parsed.as_ref().err().map(|_| "invalid MCP document".into());
    let authored = parsed.ok().map(|document| {
        document
            .mcp_servers
            .into_iter()
            .map(|(id, mut definition)| {
                let retained_env = definition.env.keys().cloned().collect();
                let retained_headers = definition.headers.keys().cloned().collect();
                definition.env.clear();
                definition.headers.clear();
                (
                    id,
                    McpView {
                        definition,
                        retained_env,
                        retained_headers,
                    },
                )
            })
            .collect()
    });
    Ok(SourceView {
        path,
        revision: revision(bytes.as_deref()),
        authored,
        diagnostic,
    })
}
fn agent_views(root: &Path, scope: SourceScope) -> Result<Vec<AgentSourceView>, SettingsError> {
    let mut result = Vec::new();
    for path in super::super::resource_directory::files(root, &root.join("agents"), "toml")
        .map_err(|_| SettingsError::Io)?
    {
        let Some(name) = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| crate::runtime::subagent::SubagentName::parse(name).ok())
        else {
            continue;
        };
        let bytes = read(&path)?;
        let parsed = bytes
            .as_ref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| super::super::agent_resources::parse(text).ok());
        let diagnostic = parsed
            .is_none()
            .then(|| "invalid named Agent definition".into());
        result.push(AgentSourceView {
            scope,
            name,
            source: SourceView {
                path,
                revision: revision(bytes.as_deref()),
                authored: parsed,
                diagnostic,
            },
        });
    }
    Ok(result)
}
fn mcp_candidate(
    original: Option<&[u8]>,
    id: crate::runtime::identity::McpServerId,
    authored: Option<McpWrite>,
) -> Result<Vec<u8>, SettingsError> {
    let mut document: super::super::mcp_resources::McpDocument =
        crate::toml_authoring::parse(original.unwrap_or(b""))
            .map_err(|_| SettingsError::Invalid)?;
    if let Some(mut authored) = authored {
        let old = document.mcp_servers.get(&id);
        for (keys, target, previous) in [
            (
                authored.retained_env,
                &mut authored.definition.env,
                old.map(|d| &d.env),
            ),
            (
                authored.retained_headers,
                &mut authored.definition.headers,
                old.map(|d| &d.headers),
            ),
        ] {
            for key in keys {
                let value = previous
                    .and_then(|map| map.get(&key))
                    .ok_or(SettingsError::Invalid)?;
                if target.insert(key, value.clone()).is_some() {
                    return Err(SettingsError::Invalid);
                }
            }
        }
        super::super::config::resolve_mcp_entry(&id, &authored.definition.clone().resolve())
            .map_err(|_| SettingsError::Invalid)?;
        document.mcp_servers.insert(id, authored.definition);
    } else {
        document.mcp_servers.remove(&id);
    }
    toml::to_string_pretty(&document)
        .map(String::into_bytes)
        .map_err(|_| SettingsError::Invalid)
}
fn identity_unit<K: Ord, V>(map: &mut Option<BTreeMap<K, V>>, key: K, value: Option<V>) {
    if let Some(value) = value {
        map.get_or_insert_default().insert(key, value);
    } else if let Some(map) = map {
        map.remove(&key);
    }
}
#[allow(clippy::too_many_lines)] // Exhaustive semantic-unit writer, with no recursive merge.
fn apply(document: &mut RuntimeLayer, mutation: ConfigMutation) -> Result<(), SettingsError> {
    use ConfigMutation as M;
    match mutation {
        M::Provider { id, authored } => {
            let identity = crate::model::catalog::ProviderId::parse(&id)
                .map_err(|_| SettingsError::Invalid)?;
            let provider = authored
                .map(|authored| {
                    let credential = match authored.credential {
                        CredentialEdit::Retain => document
                            .providers
                            .as_ref()
                            .and_then(|map| map.get(&id))
                            .map(|provider| provider.api_key.clone())
                            .ok_or(SettingsError::Invalid)?,
                        CredentialEdit::Environment { variable } => {
                            CredentialSource::parse(&format!("${variable}"), &identity)
                                .map_err(|_| SettingsError::Invalid)?
                        }
                        CredentialEdit::Literal { value } => {
                            // The authored string syntax reserves a leading `$` for
                            // environment references. Never reinterpret a literal edit.
                            if value.is_empty() || value.starts_with('$') {
                                return Err(SettingsError::Invalid);
                            }
                            CredentialSource::Literal(value)
                        }
                    };
                    let endpoint =
                        url::Url::parse(&authored.base_url).map_err(|_| SettingsError::Invalid)?;
                    if !matches!(endpoint.scheme(), "http" | "https")
                        || endpoint.host().is_none()
                        || !endpoint.username().is_empty()
                        || endpoint.password().is_some()
                        || endpoint.query().is_some()
                        || endpoint.fragment().is_some()
                    {
                        return Err(SettingsError::Invalid);
                    }
                    Ok(Provider {
                        base_url: authored.base_url,
                        api_key: credential,
                    })
                })
                .transpose()?;
            identity_unit(&mut document.providers, id, provider);
        }
        M::Model { id, authored } => {
            crate::model::catalog::ModelRef::parse(&id).map_err(|_| SettingsError::Invalid)?;
            identity_unit(&mut document.models, id, authored);
        }
        M::RootModel { authored } => {
            if let Some(value) = &authored {
                value
                    .clone()
                    .resolve()
                    .map_err(|_| SettingsError::Invalid)?;
            }
            document.agent.get_or_insert_default().model = authored;
        }
        M::NativeTools { authored } => {
            document
                .agent
                .get_or_insert_default()
                .tools
                .get_or_insert_default()
                .builtin = authored;
        }
        M::SourceTools { id, authored } => identity_unit(
            &mut document
                .agent
                .get_or_insert_default()
                .tools
                .get_or_insert_default()
                .sources,
            id,
            authored,
        ),
        M::Skills { authored } => {
            if let Some(value) = &authored {
                for name in value.names() {
                    crate::skills::package::validate_skill_name(name)
                        .map_err(|_| SettingsError::Invalid)?;
                }
            }
            document.agent.get_or_insert_default().skills = authored;
        }
        M::Todo { authored } => {
            document
                .agent
                .get_or_insert_default()
                .plugins
                .get_or_insert_default()
                .todo = authored;
        }
        M::Goal { authored } => {
            document
                .agent
                .get_or_insert_default()
                .plugins
                .get_or_insert_default()
                .goal = authored;
        }
        M::AgentStatus { authored } => {
            document
                .agent
                .get_or_insert_default()
                .plugins
                .get_or_insert_default()
                .agent_status = authored;
        }
        M::Agents { authored } => document.agent.get_or_insert_default().agents = authored,
        M::Workflows { authored } => document.agent.get_or_insert_default().workflows = authored,
        M::AgentIdentity { authored } => document.agent_id = authored,
        M::Description { authored } => {
            document.agent.get_or_insert_default().description = authored;
        }
        M::Instructions { authored } => {
            document.agent.get_or_insert_default().instructions = authored;
        }
        M::ProjectGuidance { authored } => {
            document.agent.get_or_insert_default().agents_md = authored;
        }
        M::Context { authored } => document.context = authored,
        M::ModelTimeout { authored } => document.model_timeout_policy = authored,
        M::ToolDeadline { authored } => document.tool_deadline_policy = authored,
        M::Capacity { authored } => document.subagents = authored,
        M::Approval { authored } => document.approval_mode = authored,
        M::NativePolicy { id, authored } => {
            let policies = document.native_tools.get_or_insert_default();
            match id {
                NativeTool::Read => policies.read = authored,
                NativeTool::Write => policies.write = authored,
                NativeTool::Edit => policies.edit = authored,
                NativeTool::Glob => policies.glob = authored,
                NativeTool::Grep => policies.grep = authored,
                NativeTool::Bash => policies.bash = authored,
            }
        }
        M::McpPolicy { id, authored } => {
            identity_unit(&mut document.mcp_tool_policies, id, authored);
        }
        M::Environment { name, authored } => {
            if !crate::credentials::valid_environment_name(&name) {
                return Err(SettingsError::Invalid);
            }
            identity_unit(&mut document.environment, name, authored);
        }
        M::AppServer { authored } => {
            if let Some(value) = &authored {
                value.validate().map_err(|_| SettingsError::Invalid)?;
            }
            document.app_server = authored;
        }
    }
    Ok(())
}
/// Private serialization is the only place authored literal credentials are emitted.
fn encode(document: RuntimeLayer) -> Result<Vec<u8>, SettingsError> {
    #[derive(Serialize)]
    struct PrivateProvider {
        base_url: String,
        api_key: String,
    }
    // Encoding stays inside native authority, so the authored environment keeps
    // its literal values here; only the wire projection redacts them.
    let document = document.project(
        |provider| PrivateProvider {
            base_url: provider.base_url,
            api_key: match provider.api_key {
                CredentialSource::Literal(value) => value,
                CredentialSource::Environment(name) => format!("${name}"),
            },
        },
        |environment| environment,
    );
    toml::to_string_pretty(&document)
        .map(String::into_bytes)
        .map_err(|_| SettingsError::Invalid)
}
impl UserConfigManager {
    #[allow(clippy::too_many_lines)] // Finite inert resource-family projections; no preparation or scheduling.
    fn authoring_inventory(
        &self,
        target: &SourceTarget,
    ) -> crate::runtime::capability_inspection::CapabilityInspection {
        use crate::runtime::capability_inspection::{
            CapabilityInspection, ResourceDefinition, ResourceDiagnostic, ResourceFamily,
        };
        use crate::runtime::resources::ResourceLocation;
        let user = self.resource_root(&SourceTarget::User);
        let workspace = target.workspace();
        let mut inventory = CapabilityInspection::default();
        let mcp = super::super::mcp_resources::load(&user, workspace);
        inventory
            .definitions
            .extend(
                mcp.locations
                    .iter()
                    .map(|(id, location)| ResourceDefinition {
                        family: ResourceFamily::Mcp,
                        name: id.to_string(),
                        location: location.clone(),
                        valid: mcp.definitions.get(id).is_some_and(Result::is_ok),
                    }),
            );
        // A failed MCP document belongs to the document; a failed definition
        // belongs to exactly its own identity, never to its siblings in the
        // same document.
        inventory.resource_diagnostics.extend(
            mcp.invalid_scopes
                .iter()
                .map(|error| ResourceDiagnostic::collection(ResourceFamily::Mcp, error))
                .chain(mcp.definitions.iter().filter_map(|(id, definition)| {
                    definition
                        .as_ref()
                        .err()
                        .map(|error| ResourceDiagnostic::resource(ResourceFamily::Mcp, id, error))
                })),
        );
        match super::super::agent_resources::load_authorized(workspace, &user.join("agents")) {
            Ok((catalog, sources)) => {
                inventory
                    .definitions
                    .extend(sources.iter().map(|(name, source)| ResourceDefinition {
                        family: ResourceFamily::Agent,
                        name: name.to_string(),
                        location: ResourceLocation {
                            scope: if source.layer == "user" {
                                SourceScope::User
                            } else {
                                SourceScope::Workspace
                            },
                            path: source.selected.clone(),
                            shadowed: source.overridden.clone(),
                        },
                        valid: !catalog.invalid().contains_key(name),
                    }));
                inventory
                    .resource_diagnostics
                    .extend(ResourceDiagnostic::of_agents(&catalog));
            }
            Err(error) => inventory
                .resource_diagnostics
                .push(ResourceDiagnostic::collection(
                    ResourceFamily::Agent,
                    &error,
                )),
        }
        match super::super::workflow_resources::load(workspace, &user) {
            Ok(catalog) => {
                inventory
                    .definitions
                    .extend(
                        catalog
                            .locations
                            .iter()
                            .map(|(id, location)| ResourceDefinition {
                                family: ResourceFamily::Workflow,
                                name: id.to_string(),
                                location: location.clone(),
                                valid: !catalog.invalid().contains_key(id),
                            }),
                    );
                inventory
                    .resource_diagnostics
                    .extend(ResourceDiagnostic::of_workflows(&catalog));
            }
            Err(error) => inventory
                .resource_diagnostics
                .push(ResourceDiagnostic::collection(
                    ResourceFamily::Workflow,
                    &error,
                )),
        }
        match super::super::managed_python_resources::discover(workspace, &user) {
            Ok(catalog) => {
                inventory
                    .definitions
                    .extend(
                        catalog
                            .locations
                            .iter()
                            .map(|(id, location)| ResourceDefinition {
                                family: ResourceFamily::ManagedPython,
                                name: id.managed_python().expect("Python catalog identity").into(),
                                location: location.clone(),
                                valid: catalog.packages().get(id).is_some_and(Result::is_ok),
                            }),
                    );
                inventory
                    .resource_diagnostics
                    .extend(ResourceDiagnostic::of_managed_python(&catalog));
            }
            Err(error) => inventory
                .resource_diagnostics
                .push(ResourceDiagnostic::collection(
                    ResourceFamily::ManagedPython,
                    &error,
                )),
        }
        let skills = if let Some(workspace) = workspace {
            crate::tools::workspace::Workspace::new(workspace)
                .ok()
                .map(|workspace| {
                    crate::skills::SkillDiscovery::with_config(
                        &workspace,
                        crate::skills::SkillDiscoveryConfig {
                            automatic: crate::skills::automatic_skill_roots(
                                Some(&self.sources.home_directory),
                                workspace.root(),
                            ),
                        },
                    )
                    .discover()
                })
        } else {
            Some(crate::skills::SkillDiscovery::user_root(user.join("skills")).discover())
        };
        if let Some(skills) = skills {
            inventory.definitions.extend(
                skills
                    .provenance
                    .iter()
                    .map(|skill| (skill, true))
                    .chain(skills.invalid.iter().map(|skill| (skill, false)))
                    .map(|(skill, valid)| ResourceDefinition {
                        family: ResourceFamily::Skill,
                        name: skill.name.clone(),
                        valid,
                        location: ResourceLocation {
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
                    }),
            );
            inventory.skills = skills.provenance;
            inventory.skill_diagnostics = skills.diagnostics;
        }
        inventory
            .definitions
            .sort_by(|a, b| (&a.family, &a.name).cmp(&(&b.family, &b.name)));
        inventory
    }

    #[must_use]
    pub fn resource_root(&self, target: &SourceTarget) -> PathBuf {
        match target {
            SourceTarget::User => self.sources.home_directory.join("rustx/.agents"),
            SourceTarget::Workspace { directory } => directory.join(".agents"),
        }
    }
    fn config_path(&self, target: &SourceTarget) -> PathBuf {
        match target {
            SourceTarget::User => self.sources.config_path.clone(),
            SourceTarget::Workspace { directory } => directory.join("rustx.toml"),
        }
    }
    /// Read redacted authored documents and exact byte revisions.
    /// # Errors
    /// Reports malformed documents, read failures, and resources changed during capture.
    pub fn read_source_settings(
        &self,
        target: &SourceTarget,
    ) -> Result<SourceSettings, SettingsError> {
        self.capture_source_settings(target)
            .map(|(settings, _)| settings)
    }

    /// Capture source facts and their native identity in the same revision fence.
    /// Presentation, redaction, and protocol fields never participate in identity.
    #[allow(clippy::too_many_lines)] // One revision-fenced source capture.
    pub(crate) fn capture_source_settings(
        &self,
        target: &SourceTarget,
    ) -> Result<(SourceSettings, String), SettingsError> {
        target.validate()?;
        let user = self.config_path(&SourceTarget::User);
        let workspace = target
            .workspace()
            .map(|directory| directory.join("rustx.toml"));
        let mut paths: Vec<_> = std::iter::once(user.clone())
            .chain(workspace.clone())
            .collect();
        paths.sort();
        paths.dedup();
        #[cfg(test)]
        self.test_hooks.reach("before_documents");
        let mut locks = Vec::new();
        for path in &paths {
            std::fs::create_dir_all(path.parent().ok_or(SettingsError::Io)?)
                .map_err(|_| SettingsError::Io)?;
            locks.push(super::super::settings::lock_document(path).map_err(|_| SettingsError::Io)?);
        }
        let user_bytes = read(&user)?;
        let workspace_bytes = workspace
            .as_ref()
            .map(|path| read(path))
            .transpose()?
            .flatten();
        let roots: Vec<_> =
            std::iter::once((SourceScope::User, self.resource_root(&SourceTarget::User)))
                .chain(
                    target
                        .workspace()
                        .map(|directory| (SourceScope::Workspace, directory.join(".agents"))),
                )
                .collect();
        let resource_revisions: BTreeMap<_, _> = roots
            .iter()
            .map(|(_, root)| {
                let revision = super::super::resource_directory::revision(root);
                (root.clone(), revision)
            })
            .collect();
        // Documents (including absent/malformed bytes) and authored resource
        // trees are the inputs of this source transaction. Bound process roots
        // and derived/presentation fields are not mutable source inputs.
        let mut manifest = resource_revisions.clone();
        manifest.insert(user.clone(), revision(user_bytes.as_deref()));
        if let Some(path) = &workspace {
            manifest.insert(path.clone(), revision(workspace_bytes.as_deref()));
        }
        let input_revision = super::source_manifest_revision(&manifest);
        let mut provenance = BTreeMap::new();
        let resolved = (|| {
            let mut merged = RuntimeLayer::default();
            let user_layer = parse(user_bytes.as_deref())?;
            merged.overlay(
                user_layer,
                &super::Origin::User {
                    document: user.clone(),
                    base: user.parent().ok_or(SettingsError::Invalid)?.into(),
                },
                &mut provenance,
            );
            if let Some(path) = &workspace {
                let layer =
                    super::parse_layer(path, workspace_bytes.as_deref().unwrap_or(b""), true)
                        .map_err(|_| SettingsError::Invalid)?;
                merged.overlay(
                    layer,
                    &super::Origin::Workspace {
                        document: path.clone(),
                        base: path.parent().ok_or(SettingsError::Invalid)?.into(),
                    },
                    &mut provenance,
                );
            }
            Ok::<_, SettingsError>(merged)
        })();
        let prospective_approval_mode = resolved
            .as_ref()
            .ok()
            .and_then(|document| document.clone().resolve().ok())
            .map(|config| config.approval_mode);
        let prospective_diagnostic = match &resolved {
            Err(_) => {
                Some("Source cannot be resolved; repair the diagnosed authored document.".into())
            }
            Ok(document) => source_resolution_diagnostic(document),
        };
        let prospective_resources = Some(self.authoring_inventory(target));
        let result = SourceSettings {
            target: target.clone(),
            process_policy_impacts: [
                ("max_resident_runtimes", ProcessPolicyImpact::Hot),
                ("max_connections", ProcessPolicyImpact::Hot),
                ("max_external_attachments", ProcessPolicyImpact::Hot),
                ("idle_grace_ms", ProcessPolicyImpact::Hot),
                ("shutdown_deadline_ms", ProcessPolicyImpact::Restart),
            ]
            .into_iter()
            .map(|(key, impact)| (key.into(), impact))
            .collect(),
            process_bindings: None,
            application: None,
            prospective_approval_mode,
            prospective_resources,
            prospective_diagnostic,
            absent_resource_revision: revision(None),
            resource_revisions,
            resolved: resolved.ok().map(redact),
            session_models: None,
            provenance,
            user_mcp: mcp_view(self.resource_root(&SourceTarget::User).join("mcp.toml"))?,
            workspace_mcp: target
                .workspace()
                .map(|directory| mcp_view(directory.join(".agents/mcp.toml")))
                .transpose()?,
            agents: agent_views(&self.resource_root(&SourceTarget::User), SourceScope::User)?
                .into_iter()
                .chain(
                    target
                        .workspace()
                        .map(|directory| {
                            agent_views(&directory.join(".agents"), SourceScope::Workspace)
                        })
                        .transpose()?
                        .unwrap_or_default(),
                )
                .collect(),
            user: view(user.clone(), user_bytes.as_deref()),
            workspace: workspace
                .as_ref()
                .map(|path| view(path.clone(), workspace_bytes.as_deref())),
            user_resource_root: self.resource_root(&SourceTarget::User),
            workspace_resource_root: target
                .workspace()
                .map(|directory| directory.join(".agents")),
            runtime_root: self.sources.runtime_root.clone(),
        };
        for (scope, path, captured) in std::iter::once((SourceScope::User, user, user_bytes))
            .chain(workspace.map(|path| (SourceScope::Workspace, path, workspace_bytes)))
        {
            let expected = revision(captured.as_deref());
            let actual = revision(read(&path)?.as_deref());
            if actual != expected {
                return Err(SettingsError::Conflict {
                    scope,
                    expected,
                    actual,
                });
            }
        }
        for (scope, root) in roots {
            let expected = &result.resource_revisions[&root];
            let actual = super::super::resource_directory::revision(&root);
            if actual != *expected {
                return Err(SettingsError::Conflict {
                    scope,
                    expected: expected.clone(),
                    actual,
                });
            }
        }
        Ok((result, input_revision))
    }
    /// Commit one typed semantic-unit edit under the source revision fence.
    /// # Errors
    /// Reports stale revisions, invalid edits, I/O failure, or uncertain committed state.
    #[allow(clippy::too_many_lines)] // One revision-fenced source commit transaction.
    pub fn write_source_settings(
        &self,
        target: &SourceTarget,
        expected: &str,
        mutation: SourceMutation,
    ) -> Result<SourceSettings, SettingsError> {
        target.validate()?;
        let scope = target.scope();
        let path = match &mutation {
            SourceMutation::Mcp { .. } => self.resource_root(target).join("mcp.toml"),
            SourceMutation::Config { .. } | SourceMutation::RepairConfig { .. } => {
                self.config_path(target)
            }
            SourceMutation::Agent { name, .. } => self
                .resource_root(target)
                .join("agents")
                .join(format!("{name}.toml")),
        };
        std::fs::create_dir_all(path.parent().ok_or(SettingsError::Io)?)
            .map_err(|_| SettingsError::Io)?;
        {
            let _lock =
                super::super::settings::lock_document(&path).map_err(|_| SettingsError::Io)?;
            let original = read(&path)?;
            let actual = revision(original.as_deref());
            if actual != expected {
                return Err(SettingsError::Conflict {
                    scope,
                    expected: expected.into(),
                    actual,
                });
            }
            let candidate = match mutation {
                SourceMutation::RepairConfig { document } => {
                    if parse(original.as_deref()).is_ok() {
                        return Err(SettingsError::Invalid);
                    }
                    let repaired = super::parse_layer(
                        &path,
                        document.as_bytes(),
                        scope == SourceScope::Workspace,
                    )
                    .map_err(|_| SettingsError::Invalid)?;
                    Some(encode(repaired)?)
                }
                SourceMutation::Mcp { id, authored, .. } => {
                    Some(mcp_candidate(original.as_deref(), id, authored)?)
                }
                SourceMutation::Config { mutation, .. } => {
                    if scope == SourceScope::Workspace
                        && matches!(mutation, ConfigMutation::AppServer { .. })
                    {
                        return Err(SettingsError::Invalid);
                    }
                    let mut document = parse(original.as_deref())?;
                    apply(&mut document, mutation)?;
                    Some(encode(document)?)
                }
                SourceMutation::Agent { name, authored, .. } => authored
                    .map(|profile| {
                        let resolved = crate::runtime::agent_profile::AgentProfile::from_document(
                            &profile,
                            crate::runtime::agent_profile::AgentProfileKind::Named,
                            vec![],
                        )
                        .map_err(|_| SettingsError::Invalid)?;
                        crate::runtime::subagent::NamedAgentDefinition::new(
                            name,
                            resolved,
                            path.clone(),
                        )
                        .map_err(|_| SettingsError::Invalid)?;
                        toml::to_string_pretty(&profile)
                            .map(String::into_bytes)
                            .map_err(|_| SettingsError::Invalid)
                    })
                    .transpose()?,
            };
            let actual = revision(read(&path)?.as_deref());
            if actual != expected {
                return Err(SettingsError::Conflict {
                    scope,
                    expected: expected.into(),
                    actual,
                });
            }
            if let Some(bytes) = candidate {
                let mut staged =
                    tempfile::NamedTempFile::new_in(path.parent().ok_or(SettingsError::Io)?)
                        .map_err(|_| SettingsError::Io)?;
                staged.write_all(&bytes).map_err(|_| SettingsError::Io)?;
                staged.as_file().sync_all().map_err(|_| SettingsError::Io)?;
                #[cfg(test)]
                self.test_hooks.reach("before_publication");
                let actual = revision(read(&path)?.as_deref());
                if actual != expected {
                    return Err(SettingsError::Conflict {
                        scope,
                        expected: expected.into(),
                        actual,
                    });
                }
                staged.persist(&path).map_err(|_| SettingsError::Io)?;
            } else if original.is_some() {
                #[cfg(test)]
                self.test_hooks.reach("before_publication");
                let actual = revision(read(&path)?.as_deref());
                if actual != expected {
                    return Err(SettingsError::Conflict {
                        scope,
                        expected: expected.into(),
                        actual,
                    });
                }
                std::fs::remove_file(&path).map_err(|_| SettingsError::Io)?;
            }
            std::fs::File::open(path.parent().ok_or(SettingsError::Committed)?)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| SettingsError::Committed)?;
        }
        self.read_source_settings(target)
            .map_err(|_| SettingsError::Committed)
    }
}
