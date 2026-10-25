//! Typed semantic-unit authoring. Native writers canonicalize rustX-owned documents.
//! Typed CAS persistence transfers application responsibility to the native coordinator.
use super::super::authoring::{
    ContextLayer, McpAuthoring, ModelLayer, RuntimeLayer, SubagentsLayer, TimeoutLayer,
    ToolDeadlineLayer,
};
use super::super::settings::{AdmittedSource, SourceDirectory, SourceReread};
use super::UserConfigManager;
use crate::model::authoring::{Model, Provider};
use crate::model::catalog::{CredentialSource, CredentialSourceView};
use crate::runtime::resources::SourceScope;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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

    /// Revalidate the target on every operation; do not follow a replaced
    /// directory to a different source owner. A source transaction then holds
    /// the physical directory it admits (see `publish_locked`).
    /// # Errors
    /// Rejects relative, removed, redirected, or noncanonical Workspace paths.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if let Self::Workspace { directory } = self
            && (!directory.is_absolute()
                || super::capture::canonical_directory(directory)
                    .map_err(|_| SettingsError::Invalid)?
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
    /// The latest automatic repair observation of the User source in this
    /// process. Absent when the finite ledger retains none — never needed,
    /// withdrawn, or forgotten — which never proves the source valid; the
    /// views' diagnostics report that.
    pub user_repair: Option<super::repair::ConfigRepairReceipt>,
    /// The latest automatic repair observation of the target Workspace source.
    pub workspace_repair: Option<super::repair::ConfigRepairReceipt>,
    /// The native repair-ledger sequence `user_repair` and `workspace_repair`
    /// are current at, read with them under one lock. It orders this read
    /// against the `sequence` of each `configuration/sourceRepairChanged`
    /// for the same source: the higher one is the newer observation.
    pub repair_sequence: u64,
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
        default_model: Box<crate::model::session::SessionModelConfig>,
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
    /// The source manifest the adopted context generation was resolved from.
    /// Independently published units — execution policy, shared capacity and
    /// the Model Catalog — may be newer; `document` and `provenance` carry
    /// their values, and `application` says which revision each applied.
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
    pub admitted_attempt: Option<crate::runtime::adoption::AdmittedConfiguration>,
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
#[allow(clippy::large_enum_variant)] // A bounded authored model owns its policies together at this mutation boundary.
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
    /// One whole-file edit of exactly `<scope>/.agents/workflows/<id>.yaml`
    /// (Issue #491). The expected revision is that file's.
    Workflow {
        id: crate::runtime::workflow::WorkflowId,
        edit: super::super::workflow_authoring::WorkflowEdit,
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
    /// A Workflow candidate the native parser or compiler rejected, with its
    /// bounded diagnostic. Nothing was staged.
    InvalidWorkflow(Box<super::super::workflow_authoring::WorkflowDiagnostic>),
    Io,
    Committed,
}
fn read(path: &Path) -> Result<Option<Vec<u8>>, SettingsError> {
    super::super::settings::read_document(path).map_err(|_| SettingsError::Io)
}

/// A source pathname that cannot be admitted is `Invalid` when the target is
/// redirected, exactly as at entry; otherwise `Io`.
fn refused(target: &SourceTarget) -> SettingsError {
    target.validate().err().unwrap_or(SettingsError::Io)
}

/// Admit one `rustx.toml` of a capture round ([`AdmittedSource`]).
fn admit(path: &Path, target: &SourceTarget) -> Result<AdmittedSource, SettingsError> {
    AdmittedSource::admit(path).map_err(|_| refused(target))
}

/// A capture round's reread of `document` ([`AdmittedSource::reread`]): a
/// pathname that no longer names the admitted directory is `Invalid`.
fn reread(
    document: &AdmittedSource,
    target: &SourceTarget,
) -> Result<Option<Vec<u8>>, SettingsError> {
    document.reread().map_err(|failure| match failure {
        SourceReread::Redirected => SettingsError::Invalid,
        SourceReread::Unadmitted(_) => refused(target),
        SourceReread::Unreadable(_) => SettingsError::Io,
    })
}
fn revision(bytes: Option<&[u8]>) -> String {
    super::super::settings::revision(bytes)
}
/// The prospective composition's native semantics, as Session resolution and
/// automatic repair decide them; safe text only.
fn source_resolution_diagnostic(document: &RuntimeLayer) -> Option<String> {
    match super::semantics::composition_semantics(document) {
        Ok(super::semantics::Composition::Complete) => None,
        Ok(super::semantics::Composition::NoDefaultModel) => Some(
            "no unambiguous default model selected; set agent.model.model in User or Workspace rustx.toml"
                .into(),
        ),
        Err(failure) => Some(failure.summary()),
    }
}
fn view(path: PathBuf, bytes: Option<&[u8]>, workspace: bool) -> SourceView<SourceDocumentView> {
    let (authored, diagnostic) =
        match crate::toml_authoring::parse_detailed::<RuntimeLayer>(bytes.unwrap_or(b"")) {
            // A document violating its source contract is not editable either:
            // ordinary writes refuse it and manual repair accepts it.
            Ok(document) => {
                match super::capture::source_restrictions(&path, &document, workspace) {
                    Ok(()) => (Some(redact(document)), None),
                    Err(failure) => (
                        None,
                        Some(format!(
                            "invalid rustx.toml; source was not loaded: {} at {}",
                            failure.diagnostic.reason, failure.diagnostic.path
                        )),
                    ),
                }
            }
            // A request_params failure names only its field path, category
            // and locations, so it can say why the source cannot be edited.
            // Other parser messages may quote authored text.
            Err(failure) if failure.path.is_some() => (
                None,
                Some(format!(
                    "invalid rustx.toml; source was not loaded: {}",
                    failure.into_detail()
                )),
            ),
            Err(_) => (
                None,
                Some("invalid rustx.toml; source was not loaded".into()),
            ),
        };
    SourceView {
        path,
        revision: revision(bytes),
        diagnostic,
        authored,
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
        // Ordinary configuration authoring has no credential-write authority.
        // Existing literals can only be retained from this exact CAS source.
        if !authored.definition.env.is_empty() || !authored.definition.headers.is_empty() {
            return Err(SettingsError::Invalid);
        }
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
    crate::toml_authoring::write(&document)
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
    crate::toml_authoring::write(&document)
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
    pub(super) fn config_path(&self, target: &SourceTarget) -> PathBuf {
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
    ///
    /// Read-only: capture creates, locks into existence and writes nothing.
    /// It excludes rustX writers through the documents' persistent writer
    /// locks held shared, but only through locks that already exist. A
    /// document without one has never had a rustX writer, and the capture is
    /// accepted only if it still has none afterwards, so no rustX writer ran
    /// during it either: each lock that appears — lock inodes are never
    /// unlinked — sends the capture round once more, now holding that lock,
    /// so there are at most one more rounds than documents. Within a round,
    /// every document and resource revision is reread after the facts are
    /// derived; a change by a writer outside rustX, which no advisory lock
    /// excludes, is an explicit conflict.
    pub(crate) fn capture_source_settings(
        &self,
        target: &SourceTarget,
    ) -> Result<(SourceSettings, String), SettingsError> {
        target.validate()?;
        let user = self.config_path(&SourceTarget::User);
        let workspace = target
            .workspace()
            .map(|directory| directory.join("rustx.toml"));
        let distinct = 1 + usize::from(workspace.as_ref().is_some_and(|path| *path != user));
        for _ in 0..=distinct {
            #[cfg(test)]
            self.test_hooks.reach("before_documents");
            // Each round admits every document's physical directory once;
            // its lock, bytes and lock appearance are all relative to it.
            let user = admit(&user, target)?;
            let workspace = workspace
                .as_ref()
                .map(|path| admit(path, target))
                .transpose()?;
            let documents: Vec<_> = std::iter::once(&user)
                .chain(
                    workspace
                        .as_ref()
                        .filter(|document| document.path() != user.path()),
                )
                .collect();
            let mut locks = Vec::new();
            let mut unlocked = Vec::new();
            for document in documents {
                match document.share_lock().map_err(|_| SettingsError::Io)? {
                    Some(lock) => locks.push(lock),
                    None => unlocked.push(document),
                }
            }
            let captured = self
                .capture_round(target, &user, workspace.as_ref())
                .map_err(|error| {
                    // Whatever failed, a round whose pathname no longer names
                    // an admitted directory is refused as redirected.
                    if user.redirected()
                        || workspace.as_ref().is_some_and(AdmittedSource::redirected)
                    {
                        SettingsError::Invalid
                    } else {
                        error
                    }
                });
            let mut appeared = false;
            for document in unlocked {
                match document.lock_exists().map_err(|_| refused(target)) {
                    Ok(exists) => appeared |= exists,
                    // A round that failed reports its own failure.
                    Err(error) => return captured.and(Err(error)),
                }
            }
            if !appeared {
                return captured;
            }
        }
        // Only an actor outside rustX unlinking writer locks gets here.
        Err(SettingsError::Io)
    }

    /// One capture round, revision-fenced by rereading every input.
    #[allow(clippy::too_many_lines)] // One revision-fenced source capture.
    fn capture_round(
        &self,
        target: &SourceTarget,
        user_document: &AdmittedSource,
        workspace_document: Option<&AdmittedSource>,
    ) -> Result<(SourceSettings, String), SettingsError> {
        let user = user_document.path().to_path_buf();
        let workspace = workspace_document.map(|document| document.path().to_path_buf());
        let read = |document: &AdmittedSource| document.read().map_err(|_| SettingsError::Io);
        let user_bytes = read(user_document)?;
        let workspace_bytes = workspace_document.map(read).transpose()?.flatten();
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
        let input_revision = super::capture::source_manifest_revision(&manifest);
        let mut provenance = BTreeMap::new();
        let resolved = (|| {
            // Composition reads each document under its own source contract,
            // exactly as its source view and Session resolution do.
            let mut merged = RuntimeLayer::default();
            let user_layer =
                super::capture::parse_layer(&user, user_bytes.as_deref().unwrap_or(b""), false)
                    .map_err(|_| SettingsError::Invalid)?;
            merged.overlay(
                user_layer,
                &super::Origin::User {
                    document: user.clone(),
                    base: user.parent().ok_or(SettingsError::Invalid)?.into(),
                },
                &mut provenance,
            );
            if let Some(path) = &workspace {
                let layer = super::capture::parse_layer(
                    path,
                    workspace_bytes.as_deref().unwrap_or(b""),
                    true,
                )
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
        let (repair_sequence, user_repair, workspace_repair) = self.repairs().snapshot(target);
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
            user: view(user.clone(), user_bytes.as_deref(), false),
            workspace: workspace
                .as_ref()
                .map(|path| view(path.clone(), workspace_bytes.as_deref(), true)),
            user_repair,
            workspace_repair,
            repair_sequence,
            user_resource_root: self.resource_root(&SourceTarget::User),
            workspace_resource_root: target
                .workspace()
                .map(|directory| directory.join(".agents")),
            runtime_root: self.sources.runtime_root.clone(),
        };
        #[cfg(test)]
        self.test_hooks.reach("before_capture_verification");
        for (scope, document, captured) in
            std::iter::once((SourceScope::User, user_document, user_bytes)).chain(
                workspace_document
                    .map(|document| (SourceScope::Workspace, document, workspace_bytes)),
            )
        {
            let expected = revision(captured.as_deref());
            let actual = revision(reread(document, target)?.as_deref());
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
    ) -> Result<String, SettingsError> {
        target.validate()?;
        let scope = target.scope();
        let authors_config = matches!(
            &mutation,
            SourceMutation::Config { .. } | SourceMutation::RepairConfig { .. }
        );
        let path = match &mutation {
            SourceMutation::Mcp { .. } => self.resource_root(target).join("mcp.toml"),
            SourceMutation::Config { .. } | SourceMutation::RepairConfig { .. } => {
                self.config_path(target)
            }
            SourceMutation::Agent { name, .. } => self
                .resource_root(target)
                .join("agents")
                .join(format!("{name}.toml")),
            SourceMutation::Workflow { id, .. } => {
                super::super::workflow_authoring::file_path(self, target, id)
            }
        };
        // The physical source of this transaction, created when missing and
        // confirmed durable by `publish_locked`. A target redirected since
        // entry is invalid, as at entry.
        let source = SourceDirectory::create(&path).map_err(|error| {
            super::super::settings::report_io_failure("admit_writer_directory", &error);
            target.validate().err().unwrap_or(SettingsError::Io)
        })?;
        let name = path.file_name().ok_or(SettingsError::Invalid)?;
        #[cfg(test)]
        self.test_hooks.reach("write_before_lock");
        // A Workflow addition is admitted against the discovery bounds of its
        // scopes under their capacity locks, taken before the writer lock.
        let capacity = match &mutation {
            SourceMutation::Workflow {
                edit:
                    super::super::workflow_authoring::WorkflowEdit::Create { .. }
                    | super::super::workflow_authoring::WorkflowEdit::Copy { .. },
                ..
            } => Some(super::super::workflow_authoring::Capacity::lock(
                self, target,
            )?),
            _ => None,
        };
        #[cfg(test)]
        if capacity.is_some() {
            self.test_hooks.reach("workflow_capacity_locked");
        }
        let committed_revision;
        {
            let _lock = source.lock(name).map_err(|error| {
                super::super::settings::report_io_failure("acquire_writer_lock", &error);
                SettingsError::Io
            })?;
            let original = source.read(name).map_err(|_| SettingsError::Io)?;
            let actual = revision(original.as_deref());
            if actual != expected {
                return Err(SettingsError::Conflict {
                    scope,
                    expected: expected.into(),
                    actual,
                });
            }
            if let (Some(capacity), SourceMutation::Workflow { id, .. }) = (&capacity, &mutation) {
                capacity.admit(id)?;
            }
            let candidate = match mutation {
                SourceMutation::RepairConfig { document } => {
                    // Manual repair exists for documents that fail their own
                    // source contract, including TOML-parsable ones; it never
                    // overwrites a valid source.
                    if super::capture::parse_layer(
                        &path,
                        original.as_deref().unwrap_or(b""),
                        scope == SourceScope::Workspace,
                    )
                    .is_ok()
                    {
                        return Err(SettingsError::Invalid);
                    }
                    let repaired = super::capture::parse_layer(
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
                    // Ordinary editing requires a source satisfying its own
                    // source contract; anything else is manual repair's.
                    let mut document = super::capture::parse_layer(
                        &path,
                        original.as_deref().unwrap_or(b""),
                        scope == SourceScope::Workspace,
                    )
                    .map_err(|_| SettingsError::Invalid)?;
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
                        crate::toml_authoring::write(&profile)
                            .map(String::into_bytes)
                            .map_err(|_| SettingsError::Invalid)
                    })
                    .transpose()?,
                SourceMutation::Workflow { id, edit } => {
                    super::super::workflow_authoring::candidate(
                        &id,
                        edit,
                        original.as_deref(),
                        &source,
                        &path,
                        scope,
                    )?
                }
            };
            committed_revision = revision(candidate.as_deref());
            self.publish_locked(
                &source,
                name,
                scope,
                expected,
                candidate.as_deref(),
                original.is_some(),
            )?;
        }
        if authors_config {
            // A committed edit may have resolved an unresolved repair.
            self.refresh_repair_observation(target);
        }
        #[cfg(test)]
        self.test_hooks.reach("after_source_commit");
        Ok(committed_revision)
    }

    /// The one native source publication primitive. Ordinary authoring and
    /// automatic repair both publish through it, so every source commit has
    /// the same physical authority, revision fence, staging, durability and
    /// uncertainty rules.
    ///
    /// The caller opened `source` (the document's physical directory) when
    /// its transaction began — observed, or created for a write; neither
    /// claims durability — holds the writer lock of `name` in it and has
    /// verified `expected`. `candidate` replaces the document; `None` removes
    /// a present document. A candidate larger than the authored file bound
    /// its readers enforce is `Invalid` before anything is staged: nothing is
    /// ever published that its own reader would refuse. Everything goes
    /// through the admitted directory handle: the candidate is staged in it
    /// and synced; immediately before the atomic replacement the directory's
    /// path is confirmed ([`SourceDirectory::confirm`]: every name leading to
    /// it synced in its parent by this transaction, whoever created it, and
    /// the pathname proven to still name that directory) and the document
    /// must still have exactly `expected` bytes; the replacement is a
    /// `renameat` inside it — the linearization point — and the directory is
    /// synced afterwards. A success therefore means the published entry and
    /// every name needed to reach it are durable.
    ///
    /// A pathname that no longer names the admitted directory before the
    /// replacement is `Invalid`, exactly as [`SourceTarget::validate`] rejects
    /// a redirected target; changed bytes are a `Conflict`; a confirmation
    /// sync that fails while the pathname still names the directory is `Io`.
    /// All leave every source unchanged, as does any other failure before
    /// replacement (`Io`).
    /// A failed replacement is classified by rereading the admitted document.
    /// After replacement the bytes are in the admitted physical directory —
    /// never in a directory the pathname was redirected to — but the outcome
    /// is `Committed` (published, unconfirmed) when the directory sync fails
    /// or the pathname no longer names that directory. Nothing here retries or
    /// replays.
    #[cfg_attr(
        not(test),
        allow(
            clippy::unused_self,
            reason = "the owner carries the test I/O fault hooks"
        )
    )]
    pub(super) fn publish_locked(
        &self,
        source: &SourceDirectory,
        name: &std::ffi::OsStr,
        scope: SourceScope,
        expected: &str,
        candidate: Option<&[u8]>,
        present: bool,
    ) -> Result<(), SettingsError> {
        let verify = || {
            // Durable reachability of the admitted directory, confirmed by
            // this transaction, and proof that the pathname still names it.
            source.confirm().map_err(|_| {
                if source.holds() {
                    SettingsError::Io
                } else {
                    SettingsError::Invalid
                }
            })?;
            let actual = revision(source.read(name).map_err(|_| SettingsError::Io)?.as_deref());
            if actual == expected {
                Ok(())
            } else {
                Err(SettingsError::Conflict {
                    scope,
                    expected: expected.into(),
                    actual,
                })
            }
        };
        if let Some(bytes) = candidate {
            if !crate::bounded_file::within_bound(bytes) {
                return Err(SettingsError::Invalid);
            }
            #[cfg(test)]
            if self.test_hooks.fault("stage_write") {
                return Err(SettingsError::Io);
            }
            #[cfg(test)]
            if self.test_hooks.fault("stage_sync") {
                return Err(SettingsError::Io);
            }
            let staged = source.stage(bytes).map_err(|_| SettingsError::Io)?;
            #[cfg(test)]
            self.test_hooks.reach("before_publication");
            verify()?;
            // Injected replacement failures: refused before the rename, or
            // reported after it already happened.
            #[cfg(test)]
            if self.test_hooks.fault("replace_refused") {
                return Err(Self::classify_failed_replacement(source, name, expected));
            }
            #[cfg(test)]
            if self.test_hooks.fault("replace") {
                source
                    .replace(staged, name)
                    .map_err(|_| SettingsError::Io)?;
                return Err(Self::classify_failed_replacement(source, name, expected));
            }
            source
                .replace(staged, name)
                .map_err(|_| Self::classify_failed_replacement(source, name, expected))?;
        } else if present {
            #[cfg(test)]
            self.test_hooks.reach("before_publication");
            verify()?;
            source.remove(name).map_err(|_| SettingsError::Io)?;
        }
        #[cfg(test)]
        self.test_hooks.reach("after_publication");
        #[cfg(test)]
        if self.test_hooks.fault("directory_sync") {
            return Err(SettingsError::Committed);
        }
        source.sync().map_err(|_| SettingsError::Committed)?;
        if source.holds() {
            Ok(())
        } else {
            Err(SettingsError::Committed)
        }
    }

    /// A replacement that reported failure is unchanged only while the
    /// admitted document still has exactly the expected bytes; otherwise its
    /// outcome is unknown.
    fn classify_failed_replacement(
        source: &SourceDirectory,
        name: &std::ffi::OsStr,
        expected: &str,
    ) -> SettingsError {
        match source.read(name) {
            Ok(bytes) if revision(bytes.as_deref()) == expected => SettingsError::Io,
            _ => SettingsError::Committed,
        }
    }
}

#[cfg(test)]
mod mcp_write_tests {
    use super::*;

    #[test]
    fn editing_existing_settings_preserves_the_authored_mcp_startup_policy() {
        let mut document: RuntimeLayer =
            crate::toml_authoring::parse(b"[mcp_startup_policy]\nwait_timeout_ms = 23500\n")
                .unwrap();
        apply(
            &mut document,
            ConfigMutation::Instructions {
                authored: Some("updated instructions".into()),
            },
        )
        .unwrap();
        let encoded = encode(document).unwrap();
        let reread: RuntimeLayer = crate::toml_authoring::parse(&encoded).unwrap();
        assert_eq!(
            reread.mcp_startup_policy.unwrap().wait_timeout_ms,
            Some(23_500)
        );
        assert_eq!(
            reread.agent.unwrap().instructions.as_deref(),
            Some("updated instructions")
        );
    }

    #[test]
    fn removed_settings_controls_are_not_protocol_methods() {
        for method in ["mcp/connect", "mcp/status", "mcp/disconnect"] {
            let mut params = serde_json::json!({"target":{"kind":"user"}});
            if method != "mcp/status" {
                params["id"] = serde_json::json!("docs");
            }
            if method == "mcp/connect" {
                params["expected_revision"] = serde_json::json!("revision");
                params["refresh"] = serde_json::json!(false);
            }
            let request = serde_json::json!({"method":method,"params":params});
            assert!(
                serde_json::from_value::<crate::app_server::protocol::Method>(request).is_err()
            );
        }
    }

    #[test]
    fn mcp_literals_are_retained_or_removed_but_never_written_by_generic_authoring() {
        let original = br#"[mcp_servers.docs]
url = "https://example.invalid/mcp"
headers = { Authorization = "private-original" }
"#;
        let id = crate::runtime::identity::McpServerId::new("docs");
        let retained: McpWrite = serde_json::from_value(serde_json::json!({
            "definition":{"type":"http","url":"https://example.invalid/mcp"},
            "retained_headers":["Authorization"]
        }))
        .unwrap();
        let kept = mcp_candidate(Some(original), id.clone(), Some(retained.clone())).unwrap();
        assert!(
            String::from_utf8(kept)
                .unwrap()
                .contains("private-original")
        );
        let mut removed = retained.clone();
        removed.retained_headers.clear();
        let bytes = mcp_candidate(Some(original), id.clone(), Some(removed)).unwrap();
        assert!(
            !String::from_utf8(bytes)
                .unwrap()
                .contains("private-original")
        );
        let mut duplicate = retained.clone();
        duplicate.retained_headers.push("Authorization".into());
        assert!(matches!(
            mcp_candidate(Some(original), id.clone(), Some(duplicate)),
            Err(SettingsError::Invalid)
        ));
        for retain in [true, false] {
            let mut replaced = retained.clone();
            if !retain {
                replaced.retained_headers.clear();
            }
            replaced
                .definition
                .headers
                .insert("Authorization".into(), "private-new".into());
            let error = mcp_candidate(Some(original), id.clone(), Some(replaced)).unwrap_err();
            assert!(matches!(error, SettingsError::Invalid));
            assert!(!format!("{error:?}").contains("private"));
        }
        // Retention never copies a credential out of another source.
        assert!(matches!(
            mcp_candidate(None, id, Some(retained)),
            Err(SettingsError::Invalid)
        ));
    }
}
