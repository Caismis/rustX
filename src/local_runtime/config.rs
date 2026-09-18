//! Resolved runtime policy and complete Agent profiles for one CFG3 generation.
//! Authored partial documents live in `authoring::RuntimeLayer`; this type is
//! neither a file binding nor durable Session authority. Cold composition and
//! reload resolve current User/Workspace sources before preparing this value.
//! A published generation is immutable and contains policy, model bindings,
//! Agent profiles, resource catalogs, and prepared finite source demand.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::capabilities::selection::ToolSelectionDocument;
use crate::context::SessionContextPolicy;
use crate::extensions::{NativeAgentExtensions, NativeAgentExtensionsDocument};
use crate::model::deadline::{
    DEFAULT_RESPONSE_START_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT, ModelTimeoutPolicy,
};
use crate::model::session::SessionModelConfig;
use crate::runtime::ApprovalMode;
use crate::runtime::identity::{AgentId, McpServerId};
use crate::runtime::subagent::{SubagentExecutionDeadline, SubagentName};
use crate::runtime::workflow::WorkflowId;
use crate::runtime::workspace::WorkspacePolicy;
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentError};
use crate::tools::mcp::{McpServerBinding, McpServerBindings, McpTransportConfig};
use crate::tools::native::NativeToolPolicies;
use crate::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationPolicy};

/// The only current runtime configuration schema version this runtime accepts.
pub const CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 9;

/// Resolved policies and Root profile within one published configuration.
///
/// No field in this type is persisted by [`SessionCatalog`](super::session::SessionCatalog).
/// The selected Session contributes its separate [`SessionModelConfig`] state
/// during composition. This is materialized current source/default content,
/// frozen per runtime generation, not mutable Session intent.
/// Source bindings and explicit input presence live in `configuration` owners;
/// no complete effective value is durable Session configuration authority.
/// See docs/configuration.md for the exhaustive semantic overlay table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct CurrentRuntimeConfig {
    /// The runtime configuration schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The agent executed by attempts of this conversation.
    #[serde(default = "default_agent_id")]
    pub agent_id: AgentId,
    /// Runtime-wide approval mode resolved from User < Workspace configuration.
    /// Frozen in the published generation, never deliberate Session intent.
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    /// The root Agent uses the same profile document as canonical named Agents.
    #[serde(default)]
    pub agent: AgentProfileDocument,
    /// Complete context policy resolved from User < Workspace; atomic replacement.
    #[serde(default)]
    pub context: ContextPolicyDocument,
    /// The finite runtime-owned deadline policy shared by primary and
    /// summarizer model requests. This is published generation policy resolved
    /// atomically from User < Workspace, frozen into admitted requests.
    #[serde(default)]
    pub model_timeout_policy: ModelTimeoutPolicyDocument,
    /// The finite runtime-owned execution-liveness deadline policy of
    /// foreground Tool executions (Issue #204): one generic hard deadline,
    /// plus an optional idle-liveness window refreshed by executor progress.
    /// User < Workspace replaces this complete policy atomically; admitted
    /// execution retains the frozen generation policy.
    #[serde(default)]
    pub tool_deadline_policy: ToolDeadlinePolicyDocument,
    /// The ecosystem-compatible named MCP server map, keyed by server
    /// identity exactly as mainstream MCP clients spell it.
    #[serde(default)]
    pub mcp_servers: BTreeMap<McpServerId, McpServerDocument>,
    /// Global invocation policy, atomically replaced per MCP source by User < Workspace.
    /// Kept separate from inert `.agents/mcp.toml` connection definitions.
    #[serde(default)]
    pub mcp_tool_policies: BTreeMap<McpServerId, InvocationPolicyDocument>,
    /// The per-tool execution, concurrency, and approval policies of the
    /// Native Tool plane, atomically replaced per Tool by User < Workspace.
    #[serde(default)]
    pub native_tools: NativeToolPoliciesDocument,
    /// Literal Tool environment; User < Workspace replaces one variable at a time.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Runtime-global child capacity for the published generation.
    #[serde(default)]
    pub subagents: SubagentsDocument,
}

/// The resolved native representation of the named-subagent plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct SubagentsDocument {
    /// Runtime-global child capacity resolved from User < Workspace as one object.
    /// Safe-boundary configuration publication updates the registry policy only
    /// when no admitted child owns it; existing child specs remain frozen.
    pub max_concurrent: usize,
}

impl Default for SubagentsDocument {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        }
    }
}

/// The generation-owned subagent capacity used when the document omits it.
pub const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 4;

/// The hard upper bound of the generation-owned subagent capacity.
pub const MAX_MAX_CONCURRENT_SUBAGENTS: usize = 64;

/// Strict Agent Profile authoring shared by root and named Agents.
///
/// Complete selected intent resolves against admitted resources. Execution
/// scope controls child lifecycle/worktree applicability. Dynamic child
/// invocations may replace only Tools, Skills and Extensions within their
/// frozen delegation ceiling; the other dimensions remain authored defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema, Default)]
pub struct AgentProfileDocument {
    #[serde(default)]
    pub agents: Vec<SubagentName>,
    #[serde(default)]
    pub workflows: Vec<WorkflowId>,
    /// The bounded model-facing routing description. Empty defaults stay omitted
    /// in native source projections and complete-resource serialization.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Explicit primary Agent instructions authored as TOML data.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    /// The explicit model this agent runs on. Omit to inherit the invoking
    /// attempt's frozen effective model configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(
        deserialize_with = "super::authoring::deserialize_profile_model",
        serialize_with = "super::authoring::serialize_profile_model"
    )]
    #[schemars(with = "Option<super::authoring::ModelLayer>")]
    pub model: Option<SessionModelConfig>,
    /// The optional maximum wall-clock duration of the complete child
    /// lifecycle, in milliseconds. The model cannot override or extend it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// The exact source-qualified capability selection.
    #[serde(default)]
    pub tools: ToolSelectionDocument,
    /// Skill descriptions advertised in the prompt: "all", exact names, or [].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<crate::runtime::agent_profile::AgentSkillSelection>,
    /// The project-instruction policy of this agent.
    #[serde(default)]
    pub agents_md: AgentProjectInstructionsDocument,
    /// The bounded project-workspace policy of this agent.
    #[serde(default)]
    pub worktree: AgentWorktreeDocument,
    /// Closed native Extension composition. Omission selects none. Root
    /// product defaults are an explicit lower-priority authoring layer,
    /// independent of this complete document's semantics.
    #[serde(default)]
    #[serde(rename = "plugins")]
    pub extensions: NativeAgentExtensionsDocument,
}

impl AgentProfileDocument {
    /// Converts the TOML millisecond field into the validated runtime type.
    ///
    /// # Errors
    ///
    /// Returns the explicit zero/maximum validation detail. Malformed authoring
    /// values are rejected by serde before this boundary is reached.
    pub fn execution_deadline(&self) -> Result<Option<SubagentExecutionDeadline>, String> {
        self.timeout_ms
            .map(SubagentExecutionDeadline::from_millis)
            .transpose()
            .map_err(|error| error.to_string())
    }
}

/// Project-instruction selection from admitted workspace resources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct AgentProjectInstructionsDocument {
    /// Whether the invoking generation's normal project instruction chain is
    /// prepended to the explicit files.
    pub inherit: bool,
    /// Explicit agent-owned project instruction files, in deterministic
    /// configured order. Relative paths resolve against the owning
    /// configuration document's directory at launch resolution.
    pub files: Vec<PathBuf>,
}

impl Default for AgentProjectInstructionsDocument {
    fn default() -> Self {
        Self {
            inherit: true,
            files: Vec::new(),
        }
    }
}

/// The resource representation of a named subagent's optional Git worktree
/// isolation. An omitted or disabled value is the existing shared-workspace
/// behavior; there is no model-facing per-call override.
///
/// Issue #188: when enabled, isolation runs the child from the committed
/// parent `HEAD` snapshot. `require_clean_parent` defaults to `true`
/// (strict): the parent source workspace must be clean, otherwise the child
/// is rejected rather than silently dropping parent-local changes. Only an
/// explicit `"require_clean_parent": false` permits a dirty parent while the
/// child still receives exactly the committed snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct AgentWorktreeDocument {
    /// Whether this named definition uses an isolated Git worktree.
    pub enabled: bool,
    /// Whether acquisition rejects a dirty parent workspace/index.
    ///
    /// This is the one authoritative strictness switch, normalized at the
    /// configuration boundary: it defaults to `true` whenever isolation is
    /// enabled, and an explicit `false` is the intentional opt-out that runs
    /// from the captured committed `HEAD` while excluding dirty parent bytes.
    pub require_clean_parent: bool,
}

impl Default for AgentWorktreeDocument {
    /// The derived default kept both booleans `false`, which made an omitted
    /// `"require_clean_parent"` silently mean “allow a dirty parent”. The two
    /// booleans now have independent defaults (Issue #188): isolation stays
    /// disabled, while the clean-parent requirement is strict whenever
    /// isolation is enabled.
    fn default() -> Self {
        Self {
            enabled: false,
            require_clean_parent: true,
        }
    }
}

impl AgentWorktreeDocument {
    /// Resolves the configuration document into the bounded runtime policy.
    ///
    /// The resolved policy is the single normalized value the runtime
    /// consumes; no runtime call site re-interprets an omitted field.
    #[must_use]
    pub const fn to_policy(self) -> WorkspacePolicy {
        if self.enabled {
            WorkspacePolicy::GitWorktree {
                require_clean_parent: self.require_clean_parent,
            }
        } else {
            WorkspacePolicy::SharedWorkspace
        }
    }
}

/// The resolved native model request timeout policy.
///
/// Milliseconds keep the configuration human-readable while the runtime
/// receives a typed [`ModelTimeoutPolicy`] containing only finite
/// [`Duration`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct ModelTimeoutPolicyDocument {
    /// Maximum time to observe the first generation progress.
    pub response_start_timeout_ms: u64,
    /// Maximum time between generation/liveness events after generation has
    /// begun.
    pub stream_idle_timeout_ms: u64,
}

impl Default for ModelTimeoutPolicyDocument {
    fn default() -> Self {
        Self {
            response_start_timeout_ms: u64::try_from(DEFAULT_RESPONSE_START_TIMEOUT.as_millis())
                .expect("the default response-start timeout fits in milliseconds"),
            stream_idle_timeout_ms: u64::try_from(DEFAULT_STREAM_IDLE_TIMEOUT.as_millis())
                .expect("the default stream-idle timeout fits in milliseconds"),
        }
    }
}

impl ModelTimeoutPolicyDocument {
    /// Converts the current-runtime document to the runtime policy.
    ///
    /// # Errors
    ///
    /// Returns an error when either configured deadline is zero.
    #[must_use = "the validated policy must be used by runtime composition"]
    pub fn to_policy(self) -> Result<ModelTimeoutPolicy, String> {
        if self.response_start_timeout_ms == 0 {
            return Err(
                "model_timeout_policy.response_start_timeout_ms must be positive".to_owned(),
            );
        }
        if self.stream_idle_timeout_ms == 0 {
            return Err("model_timeout_policy.stream_idle_timeout_ms must be positive".to_owned());
        }
        Ok(ModelTimeoutPolicy::new(
            Duration::from_millis(self.response_start_timeout_ms),
            Duration::from_millis(self.stream_idle_timeout_ms),
        ))
    }
}

/// The current-runtime document of the tool execution-liveness deadline
/// policy (Issue #204).
///
/// Milliseconds keep the configuration human-readable while the runtime
/// receives a typed
/// [`ToolExecutionDeadlinePolicy`](crate::tools::deadline::ToolExecutionDeadlinePolicy)
/// containing only finite [`Duration`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct ToolDeadlinePolicyDocument {
    /// The total maximum execution lifetime of one admitted foreground Tool
    /// call, measured from its executor-start frontier. Progress never
    /// extends it.
    pub hard_deadline_ms: u64,
    /// The optional idle-liveness window: the maximum time one started
    /// execution may go without meaningful executor progress evidence.
    /// Omitted means no idle watchdog. The window applies only to
    /// executions whose executor declares meaningful progress capability;
    /// all other executions run under the hard deadline only.
    pub idle_liveness_ms: Option<u64>,
}

impl Default for ToolDeadlinePolicyDocument {
    fn default() -> Self {
        Self {
            hard_deadline_ms: u64::try_from(
                crate::tools::deadline::DEFAULT_TOOL_HARD_DEADLINE.as_millis(),
            )
            .expect("the default tool hard deadline fits in milliseconds"),
            idle_liveness_ms: None,
        }
    }
}

impl ToolDeadlinePolicyDocument {
    /// Converts the current-runtime document to the runtime policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the hard deadline or a configured idle-liveness
    /// window is zero.
    #[must_use = "the validated policy must be used by runtime composition"]
    pub fn to_policy(self) -> Result<crate::tools::deadline::ToolExecutionDeadlinePolicy, String> {
        if self.hard_deadline_ms == 0 {
            return Err("tool_deadline_policy.hard_deadline_ms must be positive".to_owned());
        }
        if let Some(idle) = self.idle_liveness_ms
            && idle == 0
        {
            return Err("tool_deadline_policy.idle_liveness_ms must be positive".to_owned());
        }
        Ok(crate::tools::deadline::ToolExecutionDeadlinePolicy::new(
            Duration::from_millis(self.hard_deadline_ms),
            self.idle_liveness_ms.map(Duration::from_millis),
        ))
    }
}

const fn default_schema_version() -> u32 {
    CURRENT_RUNTIME_SCHEMA_VERSION
}

fn default_agent_id() -> AgentId {
    AgentId::new("rustx")
}

pub(crate) fn builtin_root_profile() -> AgentProfileDocument {
    AgentProfileDocument::default()
}

impl CurrentRuntimeConfig {
    /// Launch default used only when creating a new durable Session.
    #[must_use]
    /// # Panics
    /// Panics if called before root model validation.
    pub fn initial_model(&self) -> &SessionModelConfig {
        self.agent
            .model
            .as_ref()
            .expect("validated root Agent model")
    }

    pub(super) fn defaults(model: SessionModelConfig) -> Self {
        Self {
            schema_version: default_schema_version(),
            agent_id: default_agent_id(),
            approval_mode: ApprovalMode::default(),
            agent: AgentProfileDocument {
                model: Some(model),
                ..builtin_root_profile()
            },
            context: ContextPolicyDocument::default(),
            model_timeout_policy: ModelTimeoutPolicyDocument::default(),
            tool_deadline_policy: ToolDeadlinePolicyDocument::default(),
            mcp_servers: BTreeMap::default(),
            mcp_tool_policies: BTreeMap::default(),
            native_tools: NativeToolPoliciesDocument::default(),
            environment: BTreeMap::default(),
            subagents: SubagentsDocument::default(),
        }
    }

    /// Parses and validates current runtime configuration from TOML bytes.
    ///
    /// Strict `snake_case` authoring types resolve into native configuration.
    ///
    /// # Errors
    ///
    /// Returns [`CurrentRuntimeConfigError::Syntax`] for malformed TOML or
    /// unknown fields, and a specific validation error otherwise.
    pub fn from_toml_slice(bytes: &[u8]) -> Result<Self, CurrentRuntimeConfigError> {
        let layer: super::authoring::RuntimeLayer = crate::toml_authoring::parse(bytes)
            .map_err(|detail| CurrentRuntimeConfigError::Syntax { detail })?;
        let config = layer
            .resolve()
            .map_err(|detail| CurrentRuntimeConfigError::Syntax { detail })?;
        config.validate()?;
        Ok(config)
    }

    /// Validates the semantic constraints of the configuration.
    ///
    /// # Errors
    ///
    /// Returns the first validation failure.
    pub fn validate(&self) -> Result<(), CurrentRuntimeConfigError> {
        if self.schema_version != CURRENT_RUNTIME_SCHEMA_VERSION {
            return Err(CurrentRuntimeConfigError::UnsupportedSchemaVersion {
                supported: CURRENT_RUNTIME_SCHEMA_VERSION,
                found: self.schema_version,
            });
        }
        if self.agent_id.as_str().is_empty() {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "agent_id must be non-empty".to_owned(),
            });
        }
        self.context.validate()?;
        self.timeout_policy()?;
        if self.agent.model.is_none() {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "agent.model.model is required for root".into(),
            });
        }
        crate::runtime::agent_profile::AgentProfile::from_document(
            &self.agent,
            crate::runtime::agent_profile::AgentProfileKind::Root,
            Vec::new(),
        )
        .map_err(|detail| CurrentRuntimeConfigError::Invalid { detail })?;
        if self.agent.worktree.enabled || self.agent.timeout_ms.is_some() {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "root scope cannot acquire a child worktree or child lifecycle deadline"
                    .into(),
            });
        }
        self.agent
            .tools
            .validate_spelling()
            .map_err(|detail| CurrentRuntimeConfigError::Invalid { detail })?;
        let native = crate::tools::native::definitions(
            crate::tools::native::NativeToolPolicies::default(),
            &crate::runtime::subagent::AgentCatalog::default(),
        );
        for name in &self.agent.tools.builtin {
            if !native
                .iter()
                .any(|(definition, _)| definition.name == *name)
            {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!("unknown Native Tool {name:?}"),
                });
            }
        }
        self.agent
            .execution_deadline()
            .map_err(|detail| CurrentRuntimeConfigError::Invalid { detail })?;
        // Duplicate MCP identity is structurally impossible: `mcpServers` is
        // a keyed map. Normalization is the remaining semantic gate, and it
        // runs here so a malformed entry fails at parse time rather than at
        // composition time.
        self.mcp_bindings()?;
        self.validate_subagents()?;
        self.validate_workflows()?;
        Ok(())
    }

    /// Validates the structural constraints of the named-subagent plane.
    ///
    /// Capability, Skill, and model *authority* is validated later, against
    /// the prepared resource generation that will admit the catalog: this
    /// gate covers only what the document can decide on its own.
    fn validate_subagents(&self) -> Result<(), CurrentRuntimeConfigError> {
        if self.subagents.max_concurrent == 0
            || self.subagents.max_concurrent > MAX_MAX_CONCURRENT_SUBAGENTS
        {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: format!(
                    "subagents.max_concurrent must be between 1 and \
                     {MAX_MAX_CONCURRENT_SUBAGENTS}, found {}",
                    self.subagents.max_concurrent
                ),
            });
        }
        Self::validate_subagent_admission("agent.agents", &self.agent.agents)?;
        Ok(())
    }

    /// Validates one independent profile admission domain.
    pub(super) fn validate_subagent_admission(
        label: &str,
        admission: &[SubagentName],
    ) -> Result<(), CurrentRuntimeConfigError> {
        let mut seen = std::collections::BTreeSet::new();
        for name in admission {
            if !seen.insert(name) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!("{label} contains duplicate profile {name:?}"),
                });
            }
        }
        Ok(())
    }

    /// Validates Workflow selection identity uniqueness.
    fn validate_workflows(&self) -> Result<(), CurrentRuntimeConfigError> {
        validate_unique_workflow_ids("agent.workflows", &self.agent.workflows)?;
        Ok(())
    }

    /// The context policy supplied to the current runtime composition.
    #[must_use]
    pub const fn context_policy(&self) -> SessionContextPolicy {
        self.context.to_policy()
    }

    /// Freezes the root Agent's native Agent Extension composition for this
    /// launch (Issue #256).
    ///
    /// Composition calls this exactly once. The returned value is the whole
    /// extension authority of the composed root `ConversationRuntime`: no
    /// later resource generation, reload, or configuration edit reaches it.
    #[must_use]
    pub fn extension_composition(&self) -> NativeAgentExtensions {
        self.agent.extensions.resolve()
    }

    /// The validated finite model request deadline policy for this runtime.
    ///
    /// The policy is copied into admitted execution state. It is not placed
    /// in a model request, request snapshot, canonical history, or provider
    /// continuation.
    ///
    /// # Errors
    ///
    /// Returns [`CurrentRuntimeConfigError::Invalid`] when either deadline is
    /// zero.
    pub fn timeout_policy(&self) -> Result<ModelTimeoutPolicy, CurrentRuntimeConfigError> {
        self.model_timeout_policy
            .to_policy()
            .map_err(|detail| CurrentRuntimeConfigError::Invalid { detail })
    }

    /// The validated finite tool execution-liveness deadline policy for this
    /// runtime (Issue #204).
    ///
    /// The policy is copied into the frozen execution policy of each admitted
    /// attempt. It is not placed in a model request, a tool executor context,
    /// canonical history, or any durable state.
    ///
    /// # Errors
    ///
    /// Returns [`CurrentRuntimeConfigError::Invalid`] when the hard deadline
    /// or a configured idle-liveness window is zero.
    pub fn tool_deadline_policy(
        &self,
    ) -> Result<crate::tools::deadline::ToolExecutionDeadlinePolicy, CurrentRuntimeConfigError>
    {
        self.tool_deadline_policy
            .to_policy()
            .map_err(|detail| CurrentRuntimeConfigError::Invalid { detail })
    }

    /// The base authorized tool environment this configuration expresses.
    ///
    /// # Errors
    ///
    /// Returns [`CurrentRuntimeConfigError::Environment`] when an entry is
    /// malformed or claims a runtime-owned key.
    pub fn tool_environment(&self) -> Result<ToolEnvironment, CurrentRuntimeConfigError> {
        ToolEnvironment::from_authorized(
            self.environment
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        )
        .map_err(CurrentRuntimeConfigError::Environment)
    }

    /// The typed MCP runtime bindings this configuration expresses.
    ///
    /// This is the one normalization boundary: ecosystem spellings (`url`,
    /// `command`, `env`, `type: "http"`) are resolved here and never reach
    /// the MCP adapter, the capability coordinator, the Agent Loop, or the
    /// TUI.
    ///
    /// # Errors
    ///
    /// Returns [`CurrentRuntimeConfigError::Invalid`] when an entry is
    /// ambiguous, contradictory, or incomplete, or when the policy overlay
    /// names a server that `mcpServers` does not declare.
    pub fn mcp_bindings(&self) -> Result<McpServerBindings, CurrentRuntimeConfigError> {
        resolve_mcp_bindings(&self.mcp_servers, &self.mcp_tool_policies)
    }
}

pub(super) fn resolve_mcp_bindings(
    servers: &BTreeMap<McpServerId, McpServerDocument>,
    policies: &BTreeMap<McpServerId, InvocationPolicyDocument>,
) -> Result<McpServerBindings, CurrentRuntimeConfigError> {
    if servers.len() > 128 {
        return Err(CurrentRuntimeConfigError::Invalid {
            detail: "configuration supports at most 128 MCP sources".into(),
        });
    }

    for server_id in policies.keys() {
        if !servers.contains_key(server_id) {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: format!(
                    "mcp_tool_policies names {server_id}, which mcp_servers does not declare"
                ),
            });
        }
    }
    servers
        .iter()
        .map(|(server_id, document)| {
            let transport = resolve_mcp_entry(server_id, document)?;
            Ok((
                server_id.clone(),
                McpServerBinding {
                    credentials: crate::credentials::SourceCredentials {
                        environment: document.sensitive_env.clone(),
                        headers: document.sensitive_headers.clone(),
                        ..Default::default()
                    },
                    transport,
                    policy: policies
                        .get(server_id)
                        .copied()
                        .unwrap_or_default()
                        .to_policy(),
                },
            ))
        })
        .collect()
}

/// Validate and normalize one definition independently of policy target closure.
/// Runtime binding and authored (including shadowed) entries share this owner.
pub(super) fn resolve_mcp_entry(
    server_id: &McpServerId,
    document: &McpServerDocument,
) -> Result<McpTransportConfig, CurrentRuntimeConfigError> {
    if server_id.as_str().is_empty() {
        return Err(CurrentRuntimeConfigError::Invalid {
            detail: "mcp_servers keys must be non-empty server identities".to_owned(),
        });
    }
    // The `python:` MCP server namespace is structurally
    // reserved for rustX-managed Python packages (Issue #174):
    // every discovered package synthesizes `python:<folder>`,
    // and one `McpServerId` can never have two owners. This is
    // validated here, at configuration normalization, before
    // any capability preparation — not arbitrated at runtime.
    if server_id
        .as_str()
        .starts_with(crate::tools::python::MANAGED_MCP_NAMESPACE)
    {
        return Err(CurrentRuntimeConfigError::Invalid {
            detail: format!(
                "mcp_servers.{server_id}: the \"{}\" MCP server namespace is reserved \
                             for automatically discovered managed Python tool packages \
                             (each `.agents/tools/<folder>/` synthesizes \
                             \"{0}<folder>\"); configure this server under a different id",
                crate::tools::python::MANAGED_MCP_NAMESPACE,
            ),
        });
    }
    document
        .to_transport()
        .map_err(|detail| CurrentRuntimeConfigError::Invalid {
            detail: format!("mcp_servers.{server_id}: {detail}"),
        })
}

pub(super) fn validate_unique_workflow_ids(
    label: &str,
    ids: &[WorkflowId],
) -> Result<(), CurrentRuntimeConfigError> {
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: format!("{label} contains duplicate workflow id {id:?}"),
            });
        }
    }
    Ok(())
}

/// The static current-runtime context policy document.
///
/// There is deliberately no context window here: the window belongs to the
/// selected model and is derived per attempt from that attempt's immutable
/// model snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ContextPolicyDocument {
    /// Tokens permanently reserved out of whichever model window is in
    /// force.
    pub reserve_tokens: u64,
    /// Tokens of recent conversation history kept uncompressed.
    pub keep_recent_tokens: u64,
    /// The summary/output safety cap applied to the summary invocation
    /// through the runtime-owned protected max-output field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_output_cap: Option<u32>,
}

impl ContextPolicyDocument {
    pub(crate) fn validate(&self) -> Result<(), CurrentRuntimeConfigError> {
        if self.summary_output_cap == Some(0) {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "context.summary_output_cap must be positive when present".to_owned(),
            });
        }
        Ok(())
    }

    /// The native context policy represented by this document.
    #[must_use]
    pub const fn to_policy(&self) -> SessionContextPolicy {
        SessionContextPolicy {
            reserve_tokens: self.reserve_tokens,
            keep_recent_tokens: self.keep_recent_tokens,
            summary_output_cap: self.summary_output_cap,
        }
    }
}

impl Default for ContextPolicyDocument {
    fn default() -> Self {
        Self {
            reserve_tokens: 1024,
            keep_recent_tokens: 4096,
            summary_output_cap: Some(1024),
        }
    }
}

/// The per-tool execution, concurrency, and approval policies of the native
/// tool plane.
///
/// `execution` and `ask_user` are deliberately outside this set: they own
/// fixed foreground-only, sequential, approval-never policies, and the
/// registry enforces the intrinsic ones itself. The extension-provided `todo`
/// Tool is outside it for a stronger reason — it is not an ordinary native
/// capability at all (Issue #259).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct NativeToolPoliciesDocument {
    /// The policy of the native Read tool.
    pub read: NativePolicyOverrideDocument,
    /// The policy of the native Write tool.
    pub write: NativePolicyOverrideDocument,
    /// The policy of the native Edit tool.
    pub edit: NativePolicyOverrideDocument,
    /// The policy of the native Glob tool.
    pub glob: NativePolicyOverrideDocument,
    /// The policy of the native Grep tool.
    pub grep: NativePolicyOverrideDocument,
    /// The policy of the native Bash tool.
    pub bash: NativePolicyOverrideDocument,
}

impl NativeToolPoliciesDocument {
    /// The native tool policy table this document expresses.
    #[must_use]
    pub fn to_policies(self) -> NativeToolPolicies {
        let base = NativeToolPolicies::default();
        NativeToolPolicies {
            read: self.read.resolve(base.read),
            write: self.write.resolve(base.write),
            edit: self.edit.resolve(base.edit),
            glob: self.glob.resolve(base.glob),
            grep: self.grep.resolve(base.grep),
            bash: self.bash.resolve(base.bash),
        }
    }
}

/// Explicit native policy axes, resolved over that tool's product default.
/// Absence never applies the generic external-tool policy to a native tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct NativePolicyOverrideDocument {
    /// Foreground/background ownership override.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ExecutionPolicyDocument")]
    pub execution: Option<ExecutionPolicyDocument>,
    /// In-batch scheduling override.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ConcurrencyPolicyDocument")]
    pub concurrency: Option<ConcurrencyPolicyDocument>,
    /// Tool approval override.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ApprovalPolicyDocument")]
    pub approval: Option<ApprovalPolicyDocument>,
}

// Shared with launch documents: omission preserves absence, while an explicit
// value (including null) must satisfy the concrete field's serde contract.
pub(super) fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl NativePolicyOverrideDocument {
    fn resolve(self, base: ToolInvocationPolicy) -> ToolInvocationPolicy {
        ToolInvocationPolicy::new(
            self.execution
                .map_or(base.execution, ExecutionPolicyDocument::to_policy),
            self.concurrency
                .map_or(base.concurrency, ConcurrencyPolicyDocument::to_policy),
            self.approval
                .map_or(base.approval, ApprovalPolicyDocument::to_policy),
        )
    }
}

#[cfg(test)]
mod native_policy_defaults_tests {
    use super::*;

    #[test]
    fn partial_native_documents_resolve_each_axis_over_its_own_product_default() {
        let base = NativeToolPolicies::default();
        let empty: NativeToolPoliciesDocument = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.to_policies(), base);
        for axis in ["execution", "concurrency", "approval"] {
            assert!(
                serde_json::from_value::<NativeToolPoliciesDocument>(
                    serde_json::json!({"read":{axis:null}})
                )
                .is_err(),
                "an explicit null must not silently select a default"
            );
        }
        for name in ["read", "write", "edit", "glob", "grep", "bash"] {
            for mask in 0..8 {
                let mut axes = serde_json::Map::new();
                if mask & 1 != 0 {
                    axes.insert("execution".into(), serde_json::json!("background_only"));
                }
                if mask & 2 != 0 {
                    axes.insert("concurrency".into(), serde_json::json!("sequential"));
                }
                if mask & 4 != 0 {
                    axes.insert("approval".into(), serde_json::json!("always"));
                }
                let document: NativeToolPoliciesDocument =
                    serde_json::from_value(serde_json::json!({name: axes})).unwrap();
                let resolved = document.to_policies();
                for (tool, original, actual) in [
                    ("read", base.read, resolved.read),
                    ("write", base.write, resolved.write),
                    ("edit", base.edit, resolved.edit),
                    ("glob", base.glob, resolved.glob),
                    ("grep", base.grep, resolved.grep),
                    ("bash", base.bash, resolved.bash),
                ] {
                    let mut expected = original;
                    if tool == name {
                        if mask & 1 != 0 {
                            expected.execution =
                                crate::tools::types::ToolExecutionPolicy::BackgroundOnly;
                        }
                        if mask & 2 != 0 {
                            expected.concurrency =
                                crate::tools::types::ToolConcurrencyPolicy::Sequential;
                        }
                        if mask & 4 != 0 {
                            expected.approval = crate::tools::types::ToolApprovalPolicy::Always;
                        }
                    }
                    assert_eq!(actual, expected, "{name} axes {mask}: {tool}");
                }
                let roundtrip: NativeToolPoliciesDocument =
                    serde_json::from_value(serde_json::to_value(document).unwrap()).unwrap();
                assert_eq!(roundtrip, document);
            }
        }
    }
}

/// One tool invocation policy document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct InvocationPolicyDocument {
    /// Foreground/background ownership policy.
    pub execution: ExecutionPolicyDocument,
    /// In-batch scheduling policy.
    pub concurrency: ConcurrencyPolicyDocument,
    /// Human approval behavior for otherwise eligible calls.
    pub approval: ApprovalPolicyDocument,
}

impl InvocationPolicyDocument {
    /// The runtime policy this document expresses.
    #[must_use]
    pub const fn to_policy(self) -> ToolInvocationPolicy {
        ToolInvocationPolicy::new(
            self.execution.to_policy(),
            self.concurrency.to_policy(),
            self.approval.to_policy(),
        )
    }
}

/// The configurable HITL approval policy of one Tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ApprovalPolicyDocument {
    /// Execute without an approval interaction.
    #[default]
    Never,
    /// Require an approval interaction before execution.
    Always,
}

impl ApprovalPolicyDocument {
    /// The runtime policy this document expresses.
    #[must_use]
    pub const fn to_policy(self) -> crate::tools::types::ToolApprovalPolicy {
        match self {
            Self::Never => crate::tools::types::ToolApprovalPolicy::Never,
            Self::Always => crate::tools::types::ToolApprovalPolicy::Always,
        }
    }
}

/// The configurable execution-ownership policy of one tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ExecutionPolicyDocument {
    /// Attempt-owned execution only.
    #[default]
    ForegroundOnly,
    /// Conversation-owned execution only.
    BackgroundOnly,
    /// The model selects the execution mode per invocation.
    ModelSelectable,
}

impl ExecutionPolicyDocument {
    /// The runtime policy this document expresses.
    #[must_use]
    pub const fn to_policy(self) -> ToolExecutionPolicy {
        match self {
            Self::ForegroundOnly => ToolExecutionPolicy::ForegroundOnly,
            Self::BackgroundOnly => ToolExecutionPolicy::BackgroundOnly,
            Self::ModelSelectable => ToolExecutionPolicy::ModelSelectable,
        }
    }
}

/// The configurable in-batch scheduling policy of one tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum ConcurrencyPolicyDocument {
    /// Calls within one batch run one at a time.
    #[default]
    Sequential,
    /// Calls within one batch may run concurrently.
    Parallel,
}

impl ConcurrencyPolicyDocument {
    /// The runtime policy this document expresses.
    #[must_use]
    pub const fn to_policy(self) -> ToolConcurrencyPolicy {
        match self {
            Self::Sequential => ToolConcurrencyPolicy::Sequential,
            Self::Parallel => ToolConcurrencyPolicy::Parallel,
        }
    }
}

/// One `mcpServers` entry, in the shape mainstream MCP clients use.
///
/// The document is deliberately flat and permissive at the *field* level and
/// strict at the *combination* level: every accepted field is declared here,
/// unknown fields fail, and [`Self::to_transport`] rejects every ambiguous or
/// contradictory combination rather than guessing.
///
/// Two entry shapes are accepted per transport — the canonical one with an
/// explicit `type`, and the shorthand the ecosystem's own READMEs use:
///
/// - `{"type": "http", "url": ..., "headers": {...}}` / `{"url": ...}`;
/// - `{"type": "stdio", "command": ..., "args": [...], "env": {...},
///   "cwd": ...}` / `{"command": ..., "args": [...]}`.
///
/// No other spellings are accepted. There is no `streamable-http` alias, no
/// `sse`, and no `ws`: rustX has exactly two runtime transports.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct McpServerDocument {
    /// Explicit references owned by the complete winning User/Workspace definition;
    /// ordinary `env` is literal. Resolution happens only for admitted demand.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sensitive_env: BTreeMap<String, crate::credentials::EnvironmentReference>,
    /// Explicit references owned by the complete winning definition; ordinary
    /// `headers` is literal. Workspace never borrows shadowed User credentials.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sensitive_headers: BTreeMap<String, crate::credentials::EnvironmentReference>,
    /// The explicit transport selector, when the entry declares one.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub transport_type: Option<McpTransportType>,
    /// The Streamable HTTP endpoint URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Static request headers sent with every HTTP request.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// The stdio server executable path or explicit executable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The stdio server arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// The explicit stdio child environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The stdio document-relative working directory; absent means the
    /// workspace root. The runtime keeps enforcing that it stays inside the
    /// workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// The transport an `mcpServers` entry selects explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(schemars::JsonSchema)]
pub enum McpTransportType {
    /// Streamable HTTP. This is the canonical spelling for a remote server.
    Http,
    /// A locally launched stdio server.
    Stdio,
}

impl McpServerDocument {
    fn validate_transport_values(&self) -> Result<(), String> {
        if self.cwd.as_ref().is_some_and(|path| {
            path.as_os_str().is_empty() || path.as_os_str().as_encoded_bytes().contains(&0)
        }) {
            return Err("cwd must be a non-empty path without NUL".into());
        }
        if self.env.iter().any(|(key, value)| {
            !crate::credentials::valid_environment_name(key) || value.contains('\0')
        }) || self.args.iter().any(|value| value.contains('\0'))
            || self
                .command
                .as_ref()
                .is_some_and(|value| value.contains('\0'))
        {
            return Err("invalid stdio environment, argument or command".into());
        }
        let mut ordinary_headers = std::collections::BTreeSet::new();
        for (key, value) in &self.headers {
            if http::HeaderName::try_from(key).is_err()
                || http::HeaderValue::try_from(value).is_err()
                || !ordinary_headers.insert(key.to_ascii_lowercase())
            {
                return Err("invalid or duplicate HTTP header".into());
            }
        }
        for key in self.sensitive_env.keys() {
            if !crate::credentials::valid_environment_name(key) || self.env.contains_key(key) {
                return Err(
                    "sensitive_env requires valid environment names disjoint from env".into(),
                );
            }
        }
        let mut header_names = std::collections::BTreeSet::new();
        for key in self.sensitive_headers.keys() {
            if http::HeaderName::try_from(key).is_err()
                || !header_names.insert(key.to_ascii_lowercase())
                || self
                    .headers
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case(key))
            {
                return Err(
                    "sensitive_headers requires unique valid header names disjoint from headers"
                        .into(),
                );
            }
        }
        Ok(())
    }
    /// The runtime transport this entry normalizes to.
    ///
    /// # Errors
    ///
    /// Returns a human-readable detail when the entry is ambiguous,
    /// contradictory, or incomplete.
    pub fn to_transport(&self) -> Result<McpTransportConfig, String> {
        self.validate_transport_values()?;
        let has_http_fields =
            self.url.is_some() || !self.headers.is_empty() || !self.sensitive_headers.is_empty();
        let has_stdio_fields = self.command.is_some()
            || !self.sensitive_env.is_empty()
            || !self.args.is_empty()
            || !self.env.is_empty()
            || self.cwd.is_some();
        let selected = match self.transport_type {
            Some(explicit) => explicit,
            None => match (self.url.is_some(), self.command.is_some()) {
                (true, true) => {
                    return Err(
                        "declares both url and command; declare exactly one transport".to_owned(),
                    );
                }
                (true, false) => McpTransportType::Http,
                (false, true) => McpTransportType::Stdio,
                (false, false) => {
                    return Err(
                        "declares neither url (http) nor command (stdio); one is required"
                            .to_owned(),
                    );
                }
            },
        };
        match selected {
            McpTransportType::Http => {
                if has_stdio_fields {
                    return Err(
                        "is an http entry but declares stdio fields (command/args/env/cwd)"
                            .to_owned(),
                    );
                }
                let endpoint = self
                    .url
                    .as_deref()
                    .ok_or_else(|| "is an http entry but declares no url".to_owned())?;
                let parsed = url::Url::parse(endpoint)
                    .map_err(|_| "url must be a non-empty valid HTTP(S) endpoint")?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(
                        "MCP URL requires HTTP(S), a host, and no embedded credentials or fragment"
                            .into(),
                    );
                }
                Ok(McpTransportConfig::StreamableHttp {
                    endpoint: endpoint.to_owned(),
                    headers: self.headers.clone(),
                })
            }
            McpTransportType::Stdio => {
                if has_http_fields {
                    return Err(
                        "is a stdio entry but declares http fields (url/headers)".to_owned()
                    );
                }
                let program = self
                    .command
                    .as_deref()
                    .ok_or_else(|| "is a stdio entry but declares no command".to_owned())?;
                if program.trim().is_empty() {
                    return Err("command must be a non-empty executable".to_owned());
                }
                Ok(McpTransportConfig::Stdio {
                    program: program.to_owned(),
                    args: self.args.clone(),
                    cwd: self.cwd.clone(),
                    environment: self.env.clone(),
                })
            }
        }
    }
}

/// A current runtime configuration failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentRuntimeConfigError {
    /// The document is not valid JSON for the runtime schema.
    Syntax {
        /// The parser detail.
        detail: String,
    },
    /// The document declares a schema version this runtime does not speak.
    UnsupportedSchemaVersion {
        /// The version this runtime supports.
        supported: u32,
        /// The version found in the document.
        found: u32,
    },
    /// The document violates a semantic constraint.
    Invalid {
        /// The failure detail.
        detail: String,
    },
    /// The base authorized environment is invalid.
    Environment(ToolEnvironmentError),
}

impl std::fmt::Display for CurrentRuntimeConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax { detail } => {
                write!(f, "malformed current runtime config: {detail}")
            }
            Self::UnsupportedSchemaVersion { supported, found } => write!(
                f,
                "unsupported current runtime schema_version {found}; this runtime speaks {supported}"
            ),
            Self::Invalid { detail } => {
                write!(f, "invalid current runtime config: {detail}")
            }
            Self::Environment(error) => {
                write!(f, "invalid base tool environment: {error:?}")
            }
        }
    }
}

impl std::error::Error for CurrentRuntimeConfigError {}

#[cfg(test)]
mod tests {
    use super::{CurrentRuntimeConfig, CurrentRuntimeConfigError, ModelTimeoutPolicyDocument};
    use crate::model::deadline::{DEFAULT_RESPONSE_START_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT};

    pub(super) const MINIMAL: &str = r#"agent_id = "agent-a"

[context]
reserve_tokens = 1024
keep_recent_tokens = 4096


[agent]
[agent.model]
model = "p/m"
"#;

    /// The minimal configuration parses and derives its policy pieces.
    #[test]
    fn minimal_configuration_parses() {
        let config = CurrentRuntimeConfig::from_toml_slice(MINIMAL.as_bytes()).expect("valid");
        assert_eq!(config.approval_mode, crate::runtime::ApprovalMode::Policy);
        assert_eq!(config.context_policy().reserve_tokens, 1024);
        assert!(config.agent.extensions.agent_status.time.enabled);
        assert!(config.agent.extensions.agent_status.background.enabled);
        assert_eq!(config.agent.extensions.agent_status.time.timezone, None);
        assert_eq!(
            config.model_timeout_policy,
            ModelTimeoutPolicyDocument::default()
        );
        let timeout_policy = config.timeout_policy().expect("finite timeout policy");
        assert_eq!(
            timeout_policy.response_start_timeout,
            DEFAULT_RESPONSE_START_TIMEOUT
        );
        assert_eq!(
            timeout_policy.stream_idle_timeout,
            DEFAULT_STREAM_IDLE_TIMEOUT
        );
        assert!(config.mcp_bindings().expect("bindings").is_empty());
        assert!(
            config
                .tool_environment()
                .expect("environment")
                .authorized_entries()
                .is_empty()
        );
    }

    /// Issue #188: an enabled isolated worktree is strict by default. An
    /// omitted `"require_clean_parent"` resolves to `true`, exactly like an
    /// explicit `true`; only an explicit `false` retains the committed-
    /// snapshot permissive path, and disabled/omitted isolation keeps the
    /// shared-workspace policy unchanged. The normalization lives at this
    /// configuration/domain boundary: `enabled` stays `false` by default
    /// while `require_clean_parent` becomes `true` by default.
    #[test]
    fn named_subagent_worktree_policy_is_bounded_and_definition_scoped() {
        use crate::runtime::workspace::WorkspacePolicy as Policy;

        fn policy(worktree: &str) -> Policy {
            let mut document =
                serde_json::json!({"description":"worker", "instructions":"Worker instructions"});
            if !worktree.is_empty() {
                document["worktree"] = serde_json::from_str(worktree).unwrap();
            }
            crate::local_runtime::agent_resources::parse(&toml::to_string(&document).unwrap())
                .expect("valid Agent")
                .worktree
                .to_policy()
        }

        // `enabled: true` with an omitted `require_clean_parent` resolves to
        // the strict clean-parent policy.
        assert_eq!(
            policy(r#"{"enabled": true}"#),
            Policy::GitWorktree {
                require_clean_parent: true,
            }
        );
        // An explicit `require_clean_parent: true` is the same strict policy.
        assert_eq!(
            policy(r#"{"enabled": true, "require_clean_parent": true}"#),
            Policy::GitWorktree {
                require_clean_parent: true,
            }
        );
        // An explicit `require_clean_parent: false` is the committed-snapshot
        // opt-out.
        assert_eq!(
            policy(r#"{"enabled": true, "require_clean_parent": false}"#),
            Policy::GitWorktree {
                require_clean_parent: false,
            }
        );
        // Disabled or omitted worktree isolation keeps the shared-workspace
        // policy unchanged.
        assert_eq!(policy(""), Policy::SharedWorkspace);
        assert_eq!(policy(r#"{"enabled": false}"#), Policy::SharedWorkspace);
        // The two document booleans default independently: `enabled` stays
        // false while `require_clean_parent` is true.
        let default_document = super::AgentWorktreeDocument::default();
        assert!(!default_document.enabled);
        assert!(default_document.require_clean_parent);
        assert_eq!(default_document.to_policy(), Policy::SharedWorkspace);
    }

    /// The one shared timeout policy is current runtime state and accepts
    /// finite millisecond values without entering Session model state.
    #[test]
    fn model_timeout_policy_is_configurable() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
model_timeout_policy = { "response_start_timeout_ms" = 7, "stream_idle_timeout_ms" = 11 }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        let policy = config.timeout_policy().expect("finite timeout policy");
        assert_eq!(
            policy.response_start_timeout,
            std::time::Duration::from_millis(7)
        );
        assert_eq!(
            policy.stream_idle_timeout,
            std::time::Duration::from_millis(11)
        );
    }

    fn worker_config(timeout: &str) -> Result<super::AgentProfileDocument, String> {
        crate::local_runtime::agent_resources::parse(&format!(
            "description = \"worker\"\ninstructions = \"Worker\"\ntimeout_ms = {timeout}\n"
        ))
    }

    #[test]
    fn named_subagent_execution_deadline_is_optional_and_typed_at_admission() {
        let absent = crate::local_runtime::agent_resources::parse(
            r#"description = "worker"
instructions = "Worker""#,
        )
        .unwrap();
        assert_eq!(absent.execution_deadline().unwrap(), None);
        assert_eq!(
            worker_config("30000")
                .unwrap()
                .execution_deadline()
                .unwrap()
                .unwrap()
                .as_millis(),
            30_000
        );
    }

    #[test]
    fn named_subagent_execution_deadline_rejects_zero_and_values_above_maximum() {
        for (raw, expected) in [
            ("0", "must be positive"),
            ("86400001", "must not exceed 86400000 milliseconds"),
        ] {
            let error = worker_config(raw).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
        assert!(worker_config("\"30s\"").is_err());
    }

    /// A zero deadline is rejected at the current-runtime composition
    /// boundary rather than creating an ambiguous request.
    #[test]
    fn zero_model_timeout_is_rejected() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
model_timeout_policy = { "response_start_timeout_ms" = 0, "stream_idle_timeout_ms" = 11 }"#,
        );
        let error = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail");
        assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
        assert!(
            error
                .to_string()
                .contains("model_timeout_policy.response_start_timeout_ms")
        );
    }

    /// A policy read for one admission remains unchanged when current
    /// configuration is edited; a later admission reads the new values.
    #[test]
    fn timeout_policy_changes_apply_only_to_later_admissions() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
model_timeout_policy = { "response_start_timeout_ms" = 7, "stream_idle_timeout_ms" = 11 }"#,
        );
        let mut config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        let admitted = config.timeout_policy().expect("initial policy");
        config.model_timeout_policy.response_start_timeout_ms = 13;
        config.model_timeout_policy.stream_idle_timeout_ms = 17;
        let later = config.timeout_policy().expect("later policy");

        assert_eq!(
            admitted.response_start_timeout,
            std::time::Duration::from_millis(7)
        );
        assert_eq!(
            admitted.stream_idle_timeout,
            std::time::Duration::from_millis(11)
        );
        assert_eq!(
            later.response_start_timeout,
            std::time::Duration::from_millis(13)
        );
        assert_eq!(
            later.stream_idle_timeout,
            std::time::Duration::from_millis(17)
        );
    }

    /// The tool execution-liveness deadline policy (Issue #204) is current
    /// runtime state and accepts finite millisecond values without entering
    /// executor-visible or durable state.
    #[test]
    fn tool_deadline_policy_is_configurable() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
tool_deadline_policy = { "hard_deadline_ms" = 7000, "idle_liveness_ms" = { "mode" = "window", "milliseconds" = 1500 } }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        let policy = config
            .tool_deadline_policy()
            .expect("finite deadline policy");
        assert_eq!(policy.hard_deadline, std::time::Duration::from_secs(7));
        assert_eq!(
            policy.idle_liveness,
            Some(std::time::Duration::from_millis(1500))
        );
    }

    /// An omitted tool deadline policy (Issue #204) resolves to the default
    /// finite hard deadline without an idle watchdog.
    #[test]
    fn tool_deadline_policy_defaults_when_omitted() {
        let config = CurrentRuntimeConfig::from_toml_slice(MINIMAL.as_bytes()).expect("valid");
        let policy = config
            .tool_deadline_policy()
            .expect("default deadline policy");
        assert_eq!(
            policy,
            crate::tools::deadline::ToolExecutionDeadlinePolicy::default()
        );
        assert_eq!(
            policy.hard_deadline,
            crate::tools::deadline::DEFAULT_TOOL_HARD_DEADLINE
        );
        assert_eq!(policy.idle_liveness, None);
    }

    /// A zero hard deadline is rejected at the policy boundary (Issue #204)
    /// rather than admitting an execution that can never make progress.
    #[test]
    fn zero_tool_hard_deadline_is_rejected() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
tool_deadline_policy = { "hard_deadline_ms" = 0, "idle_liveness_ms" = { "mode" = "window", "milliseconds" = 1500 } }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        let error = config
            .tool_deadline_policy()
            .expect_err("zero hard deadline must fail");
        assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
        assert!(
            error
                .to_string()
                .contains("tool_deadline_policy.hard_deadline_ms")
        );
    }

    /// A zero idle-liveness window is rejected at the policy boundary
    /// (Issue #204): it would fire at every observation.
    #[test]
    fn zero_tool_idle_liveness_is_rejected() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
tool_deadline_policy = { "hard_deadline_ms" = 7000, "idle_liveness_ms" = { "mode" = "window", "milliseconds" = 0 } }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        let error = config
            .tool_deadline_policy()
            .expect_err("zero idle liveness must fail");
        assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
        assert!(
            error
                .to_string()
                .contains("tool_deadline_policy.idle_liveness_ms")
        );
    }

    /// A policy read for one admission stays frozen when current
    /// configuration is edited (Issue #204); a later admission reads the new
    /// values.
    #[test]
    fn tool_deadline_policy_changes_apply_only_to_later_admissions() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
tool_deadline_policy = { "hard_deadline_ms" = 7000, "idle_liveness_ms" = { "mode" = "window", "milliseconds" = 1500 } }"#,
        );
        let mut config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        let admitted = config.tool_deadline_policy().expect("initial policy");
        config.tool_deadline_policy.hard_deadline_ms = 13000;
        config.tool_deadline_policy.idle_liveness_ms = Some(2500);
        let later = config.tool_deadline_policy().expect("later policy");

        assert_eq!(admitted.hard_deadline, std::time::Duration::from_secs(7));
        assert_eq!(
            admitted.idle_liveness,
            Some(std::time::Duration::from_millis(1500))
        );
        assert_eq!(later.hard_deadline, std::time::Duration::from_secs(13));
        assert_eq!(
            later.idle_liveness,
            Some(std::time::Duration::from_millis(2500))
        );
    }

    /// `ApprovalMode` is current runtime configuration and accepts the
    /// explicit `FullAccess` spelling without becoming Session state.
    #[test]
    fn approval_mode_is_current_configuration_with_policy_default() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
approval_mode = "full_access""#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        assert_eq!(
            config.approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
    }

    /// Unknown fields fail rather than silently changing semantics.
    #[test]
    fn unknown_fields_are_rejected() {
        let json = r#"agent_id = "a"
future_knob = true

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[agent]
[agent.model]
model = "p/m"
"#;
        json.parse::<toml_edit::DocumentMut>().expect("valid TOML");
        let error = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail");
        assert!(
            error.to_string().contains("unknown field `future_knob`"),
            "{error}"
        );
    }

    /// Schema v3 owns timezone under the Time status module; the obsolete
    /// top-level field is rejected rather than silently ignored.
    #[test]
    fn top_level_timezone_is_rejected() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
timezone = "UTC""#,
        );
        assert!(matches!(
            CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail"),
            CurrentRuntimeConfigError::Syntax { .. }
        ));
    }

    /// Issue #256 regression 3: the obsolete top-level `agentStatus`
    /// contract is *rejected*, not silently accepted, quietly relocated, or
    /// warned about. There is no alias, no fallback parse, and no
    /// compatibility mode: the strict-field boundary is the whole mechanism.
    #[test]
    fn ext256_the_obsolete_top_level_agent_status_contract_is_rejected() {
        for obsolete in [
            "agent_status = {}",
            "agent_status = { time = { enabled = false } }",
            "agent_status = { time = { timezone = 'Asia/Shanghai' }, background = { enabled = true } }",
        ] {
            let text = format!("{obsolete}\n{MINIMAL}");
            let error =
                CurrentRuntimeConfig::from_toml_slice(text.as_bytes()).expect_err("must fail");
            assert!(
                matches!(error, CurrentRuntimeConfigError::Syntax { .. }),
                "the obsolete contract must fail loudly: {error}"
            );
            assert!(
                error.to_string().contains("agent_status"),
                "the failure names the offending obsolete field: {error}"
            );
        }
    }

    /// The closed extension surface keeps the global strict-field contract:
    /// an unknown *extension name* and an unknown knob inside a known
    /// extension both fail at launch rather than being ignored.
    #[test]
    fn ext256_unknown_extension_names_and_fields_are_rejected() {
        for (fragment, expected) in [
            ("future = true", "unknown field `future`"),
            (
                "[agent.plugins]\n[agent.plugins.future_goal]\nenabled = true\n",
                "unknown field `future_goal`",
            ),
            (
                "[agent.plugins]\n[agent.plugins.todo]\nenabled = true\nfuture = true\n",
                "unknown field `future`",
            ),
            (
                "[agent.plugins]\n[agent.plugins.todo]\nenabled = \"true\"\n",
                "expected a boolean",
            ),
            (
                "[agent.plugins]\n[agent.plugins.agent_status]\nfuture = true\n",
                "unknown field `future`",
            ),
            (
                "[agent.plugins]\n[agent.plugins.agent_status]\n[agent.plugins.agent_status.time]\nenabled = true\nfuture = true\n",
                "unknown field `future`",
            ),
            (
                "[agent.plugins]\n[agent.plugins.agent_status]\n[agent.plugins.agent_status.background]\nfuture = true\n",
                "unknown field `future`",
            ),
        ] {
            let text = if fragment.starts_with('[') {
                format!("{MINIMAL}\n{fragment}\n")
            } else {
                format!("{fragment}\n{MINIMAL}")
            };
            text.parse::<toml_edit::DocumentMut>()
                .expect("valid TOML must reach the typed schema");
            let error = CurrentRuntimeConfig::from_toml_slice(text.as_bytes())
                .expect_err("strict schema rejects the field or type");
            assert!(matches!(error, CurrentRuntimeConfigError::Syntax { .. }));
            assert!(error.to_string().contains(expected), "{fragment}: {error}");
        }
    }

    /// The closed extension surface composes exactly what it declares, and
    /// `enabled: false` removes the extension from the frozen composition
    /// rather than leaving a disabled one behind.
    #[test]
    fn ext256_the_extension_surface_freezes_the_declared_composition() {
        let enabled = MINIMAL.replace(
            r"[agent]",
            r#"[agent]
plugins = { "agent_status" = { "enabled" = true, "time" = { "timezone" = "Asia/Shanghai" }, "background" = { "enabled" = false } } }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(enabled.as_bytes()).expect("valid");
        let composition = config.extension_composition();
        let agent_status = composition
            .agent_status()
            .expect("the declared Agent Status extension is composed");
        assert_eq!(agent_status.time.timezone, Some(chrono_tz::Asia::Shanghai));
        assert!(!agent_status.background.enabled);

        let disabled = MINIMAL.replace(
            r"[agent]",
            r#"[agent]
plugins = { "agent_status" = { "enabled" = false } }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(disabled.as_bytes()).expect("valid");
        assert!(
            config.extension_composition().agent_status().is_none(),
            "a disabled extension leaves an empty composition, not a disabled one"
        );

        // Issue #259: the members are independent, and switching both off is
        // what empties the composition.
        let neither = MINIMAL.replace(
            r"[agent]",
            r#"[agent]
plugins = { "agent_status" = { "enabled" = false }, "todo" = { "enabled" = false } }"#,
        );
        assert!(
            CurrentRuntimeConfig::from_toml_slice(neither.as_bytes())
                .expect("valid")
                .extension_composition()
                .is_empty()
        );
    }

    /// An unsupported schema version fails.
    #[test]
    fn unsupported_schema_version_fails() {
        let json = r#"schema_version = 99
agent_id = "a"

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[agent]
[agent.model]
model = "p/m"
"#;
        assert!(matches!(
            CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail"),
            CurrentRuntimeConfigError::UnsupportedSchemaVersion { .. }
        ));
    }

    /// The obsolete array-based `mcpServers` schema is not a valid document.
    #[test]
    fn array_based_mcp_servers_are_rejected() {
        let json = r#"agent_id = "a"
mcp_servers = [{ server_id = "s", transport = { type = "streamable_http", endpoint = "https://x" } }]

[context]
reserve_tokens = 0
keep_recent_tokens = 0


[agent]
[agent.model]
model = "p/m"
"#;
        assert!(matches!(
            CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail"),
            CurrentRuntimeConfigError::Syntax { .. }
        ));
    }

    /// A present zero summary cap is a configuration error, even when the
    /// context would otherwise never need compaction.
    #[test]
    fn zero_summary_output_cap_is_rejected() {
        let json = MINIMAL.replace(
            "[context]",
            "[context]\nsummary_output_cap = { mode = 'limit', tokens = 0 }",
        );
        let error = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect_err("must fail");
        assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
        assert!(
            error
                .to_string()
                .contains("summary_output_cap must be positive")
        );
    }

    #[test]
    fn subagent_definition_and_admission_domains_are_independent() {
        let json = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
subagents = {  "max_concurrent" = 4 }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        assert!(config.agent.agents.is_empty());
        assert!(config.agent.agents.is_empty());

        let defined_but_unadmitted = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
subagents = {  "max_concurrent" = 4 }"#,
        );
        assert!(CurrentRuntimeConfig::from_toml_slice(defined_but_unadmitted.as_bytes()).is_ok());
    }

    #[test]
    fn selection_identity_resolution_is_deferred_but_duplicates_are_rejected() {
        let unknown = MINIMAL.replace(
            r"[agent]",
            r#"[agent]
agents = ["missing"]"#,
        );
        assert!(CurrentRuntimeConfig::from_toml_slice(unknown.as_bytes()).is_ok());

        let duplicate = MINIMAL.replace(
            r"[agent]",
            r#"[agent]
agents = ["worker", "worker"]"#,
        );
        let error =
            CurrentRuntimeConfig::from_toml_slice(duplicate.as_bytes()).expect_err("duplicate");
        assert!(error.to_string().contains("duplicate profile"));
    }

    #[test]
    fn workflow_selection_resolves_after_discovery() {
        let valid = MINIMAL.replace(
            r"[agent]",
            r"[agent]
workflows = []",
        );
        let config = CurrentRuntimeConfig::from_toml_slice(valid.as_bytes()).expect("valid");
        assert!(config.agent.workflows.is_empty());
        assert!(config.agent.workflows.is_empty());

        let unknown = MINIMAL.replace(
            r"[agent]",
            r#"[agent]
workflows = ["investigate"]"#,
        );
        assert!(CurrentRuntimeConfig::from_toml_slice(unknown.as_bytes()).is_ok());
    }
}

#[cfg(test)]
mod profile_authoring_tests {
    use super::tests::MINIMAL;
    use super::*;
    #[test]
    fn cfg273_root_and_named_share_the_complete_profile_document() {
        // Root and named Agents share complete profile dimensions.
        let named = r"description = 'review'
instructions = 'Review carefully'
agents = ['helper']
workflows = ['check']
[tools]
builtin = ['read']
[plugins]
[model]
model = 'provider/model'
";
        let root = format!(
            "[agent]\n{}",
            named
                .replace("[tools]", "[agent.tools]")
                .replace("[plugins]", "[agent.plugins]")
                .replace("[model]", "[agent.model]")
        );
        let root = CurrentRuntimeConfig::from_toml_slice(root.as_bytes()).unwrap();
        let named = crate::local_runtime::agent_resources::parse(named).unwrap();
        assert_eq!(root.agent, named);
        assert_eq!(named.extensions.resolve(), NativeAgentExtensions::none());
    }
    #[test]
    fn cfg332_root_and_named_skill_visibility_share_all_exact_none_semantics() {
        use crate::runtime::agent_profile::AgentSkillSelection;
        for (authored, expected) in [
            ("skills = 'all'", AgentSkillSelection::All),
            (
                "skills = ['guide']",
                AgentSkillSelection::Exact(vec!["guide".into()]),
            ),
            ("skills = []", AgentSkillSelection::Exact(vec![])),
        ] {
            let root = CurrentRuntimeConfig::from_toml_slice(
                MINIMAL
                    .replace("[agent]", &format!("[agent]\n{authored}"))
                    .as_bytes(),
            )
            .unwrap();
            let named = crate::local_runtime::agent_resources::parse(&format!(
                "description = 'r'\ninstructions = 'i'\n{authored}"
            ))
            .unwrap();
            assert_eq!(root.agent.skills, Some(expected.clone()));
            assert_eq!(named.skills, Some(expected));
        }
        let omitted = CurrentRuntimeConfig::from_toml_slice(MINIMAL.as_bytes()).unwrap();
        assert_eq!(omitted.agent.skills, None);
        let named = crate::local_runtime::agent_resources::parse("description = 'r'").unwrap();
        assert_eq!(named.skills, None);
        for authored in [
            "disabled_skills = []",
            "skills = ['NOT VALID']",
            "skills = 'none'",
        ] {
            assert!(
                CurrentRuntimeConfig::from_toml_slice(
                    MINIMAL
                        .replace("[agent]", &format!("[agent]\n{authored}"))
                        .as_bytes(),
                )
                .is_err(),
                "{authored}"
            );
            assert!(
                crate::local_runtime::agent_resources::parse(&format!(
                    "description = 'r'\n{authored}"
                ),)
                .is_err(),
                "{authored}"
            );
        }
    }

    #[test]
    fn cfg332_skill_roots_are_process_bindings_not_authored_source_policy() {
        for sources in ["[]", "['workspace']", "['user', 'workspace']"] {
            assert!(
                CurrentRuntimeConfig::from_toml_slice(
                    format!("{MINIMAL}\n[skills]\nsources = {sources}").as_bytes(),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn cfg273_closed_profile_authoring_rejects_unknown_and_duplicate_tools() {
        for text in [
            "unknown = true",
            "[extensions.unknown]",
            "[tools]\nbuiltin = ['read', 'read']",
            "agents = ['INVALID']",
            "[tools]\nbuiltin = false",
        ] {
            assert!(
                crate::local_runtime::agent_resources::parse(text).is_err(),
                "{text}"
            );
        }
    }
}
