//! Typed semantic-unit authoring. Native writers canonicalize rustX-owned documents.
//! Save commits bytes only; runtime publication belongs exclusively to reload.
use super::super::authoring::{
    ContextLayer, McpAuthoring, ModelLayer, RuntimeLayer, SubagentsLayer, TimeoutLayer,
    ToolDeadlineLayer,
};
use super::{SessionConfigInput, UserConfigManager};
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
    /// Native current-file approval resolution. None when prospective configuration is invalid.
    pub prospective_approval_mode: Option<crate::runtime::ApprovalMode>,
    /// Current-file analysis, separate from the published runtime. Never prepares sources.
    pub prospective_resources: Option<crate::runtime::capability_inspection::CapabilityInspection>,
    pub prospective_diagnostic: Option<String>,
    /// Exact CAS token for a currently absent resource identity.
    pub absent_resource_revision: String,
    pub resource_revisions: BTreeMap<PathBuf, String>,
    pub loaded: Option<LoadedSources>,
    pub user: SourceView<RuntimeLayer<ProviderView>>,
    pub workspace: SourceView<RuntimeLayer<ProviderView>>,
    pub user_resource_root: PathBuf,
    pub workspace_resource_root: PathBuf,
    pub runtime_root: PathBuf,
    pub user_mcp: SourceView<BTreeMap<crate::runtime::identity::McpServerId, McpView>>,
    pub workspace_mcp: SourceView<BTreeMap<crate::runtime::identity::McpServerId, McpView>>,
    pub agents: Vec<AgentSourceView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LoadedSources {
    pub generation: crate::runtime::identity::RuntimeResourceRevision,
    pub pending_reload: bool,
    pub changed_sources: Vec<PathBuf>,
}
impl SourceSettings {
    pub(crate) fn with_loaded(mut self, loaded: &EffectiveConfiguration) -> Self {
        let mut current = BTreeMap::from([
            (self.user.path.clone(), self.user.revision.clone()),
            (self.workspace.path.clone(), self.workspace.revision.clone()),
            (self.user_mcp.path.clone(), self.user_mcp.revision.clone()),
            (
                self.workspace_mcp.path.clone(),
                self.workspace_mcp.revision.clone(),
            ),
        ]);
        current.extend(self.resource_revisions.clone());
        current.extend(
            self.agents
                .iter()
                .map(|agent| (agent.source.path.clone(), agent.source.revision.clone())),
        );
        let paths: std::collections::BTreeSet<_> = current
            .keys()
            .chain(loaded.source_revisions.keys())
            .cloned()
            .collect();
        let changed_sources: Vec<_> = paths
            .into_iter()
            .filter(|path| current.get(path) != loaded.source_revisions.get(path))
            .collect();
        self.loaded = Some(LoadedSources {
            generation: loaded.generation,
            pending_reload: !changed_sources.is_empty(),
            changed_sources,
        });
        self
    }
}

/// Redacted, immutable facts read at the runtime configuration publication lock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EffectiveConfiguration {
    pub source_revisions: BTreeMap<PathBuf, String>,
    pub generation: crate::runtime::identity::RuntimeResourceRevision,
    pub document: RuntimeLayer<ProviderView>,
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
    Mcp {
        scope: SourceScope,
        id: crate::runtime::identity::McpServerId,
        authored: Option<McpWrite>,
    },
    Config {
        scope: SourceScope,
        mutation: ConfigMutation,
    },
    Agent {
        scope: SourceScope,
        name: crate::runtime::subagent::SubagentName,
        authored: Option<super::super::config::AgentProfileDocument>,
    },
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
fn view(path: PathBuf, bytes: Option<&[u8]>) -> SourceView<RuntimeLayer<ProviderView>> {
    let parsed = parse(bytes);
    let diagnostic = parsed
        .as_ref()
        .err()
        .map(|_| "invalid rustx.toml; source was not loaded".into());
    SourceView {
        path,
        revision: revision(bytes),
        diagnostic,
        authored: parsed.ok().map(|value| {
            value.map_providers(|provider| ProviderView {
                base_url: provider.base_url,
                credential: provider.api_key.view(),
            })
        }),
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
    let document = document.map_providers(|provider| PrivateProvider {
        base_url: provider.base_url,
        api_key: match provider.api_key {
            CredentialSource::Literal(value) => value,
            CredentialSource::Environment(name) => format!("${name}"),
        },
    });
    toml::to_string_pretty(&document)
        .map(String::into_bytes)
        .map_err(|_| SettingsError::Invalid)
}
impl UserConfigManager {
    #[must_use]
    pub fn resource_root(&self, input: &SessionConfigInput, scope: SourceScope) -> PathBuf {
        match scope {
            SourceScope::User => self.sources.home_directory.join("rustx/.agents"),
            SourceScope::Workspace => input.cwd.join(".agents"),
        }
    }
    fn config_path(&self, input: &SessionConfigInput, scope: SourceScope) -> PathBuf {
        match scope {
            SourceScope::User => self.sources.config_path.clone(),
            SourceScope::Workspace => input.cwd.join("rustx.toml"),
        }
    }
    /// Read redacted authored documents and exact byte revisions.
    /// # Errors
    /// Reports malformed documents, read failures, and resources changed during capture.
    pub fn read_source_settings(
        &self,
        input: &SessionConfigInput,
    ) -> Result<SourceSettings, SettingsError> {
        let user = self.config_path(input, SourceScope::User);
        let workspace = self.config_path(input, SourceScope::Workspace);
        let mut paths = vec![user.clone(), workspace.clone()];
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
        let workspace_bytes = read(&workspace)?;
        let resource_revisions = [SourceScope::User, SourceScope::Workspace]
            .into_iter()
            .map(|scope| {
                let root = self.resource_root(input, scope);
                let revision = super::super::resource_directory::revision(&root);
                (root, revision)
            })
            .collect();
        let (prospective_resources, prospective_approval_mode, prospective_diagnostic) =
            match self.resolve_session(input) {
                Ok(prospective) => (
                    Some(prospective.inspection),
                    Some(prospective.config.approval_mode),
                    None,
                ),
                Err(failure) => (
                    None,
                    None,
                    Some(format!(
                        "{}: {}",
                        failure.diagnostic.path, failure.diagnostic.reason
                    )),
                ),
            };
        let result = SourceSettings {
            prospective_approval_mode,
            prospective_resources,
            prospective_diagnostic,
            absent_resource_revision: revision(None),
            resource_revisions,
            loaded: None,
            user_mcp: mcp_view(
                self.resource_root(input, SourceScope::User)
                    .join("mcp.toml"),
            )?,
            workspace_mcp: mcp_view(
                self.resource_root(input, SourceScope::Workspace)
                    .join("mcp.toml"),
            )?,
            agents: agent_views(
                &self.resource_root(input, SourceScope::User),
                SourceScope::User,
            )?
            .into_iter()
            .chain(agent_views(
                &self.resource_root(input, SourceScope::Workspace),
                SourceScope::Workspace,
            )?)
            .collect(),
            user: view(user.clone(), user_bytes.as_deref()),
            workspace: view(workspace.clone(), workspace_bytes.as_deref()),
            user_resource_root: self.resource_root(input, SourceScope::User),
            workspace_resource_root: self.resource_root(input, SourceScope::Workspace),
            runtime_root: self.sources.runtime_root.clone(),
        };
        for (scope, path, captured) in [
            (SourceScope::User, user, user_bytes),
            (SourceScope::Workspace, workspace, workspace_bytes),
        ] {
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
        for scope in [SourceScope::User, SourceScope::Workspace] {
            let root = self.resource_root(input, scope);
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
        Ok(result)
    }
    /// Commit one typed semantic-unit edit under the source revision fence.
    /// # Errors
    /// Reports stale revisions, invalid edits, I/O failure, or uncertain committed state.
    #[allow(clippy::too_many_lines)] // One revision-fenced source commit transaction.
    pub fn write_source_settings(
        &self,
        input: &SessionConfigInput,
        expected: &str,
        mutation: SourceMutation,
    ) -> Result<SourceSettings, SettingsError> {
        let (scope, path) = match &mutation {
            SourceMutation::Mcp { scope, .. } => {
                (*scope, self.resource_root(input, *scope).join("mcp.toml"))
            }
            SourceMutation::Config { scope, .. } => (*scope, self.config_path(input, *scope)),
            SourceMutation::Agent { scope, name, .. } => (
                *scope,
                self.resource_root(input, *scope)
                    .join("agents")
                    .join(format!("{name}.toml")),
            ),
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
        self.read_source_settings(input)
            .map_err(|_| SettingsError::Committed)
    }
}
