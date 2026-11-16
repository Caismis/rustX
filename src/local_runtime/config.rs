//! Resolved runtime policy and complete Agent profiles for one CFG3 generation.
//! Authored partial documents live in `authoring::RuntimeLayer`; this type is
//! neither a file binding nor durable Session authority. Cold composition and
//! configuration capture resolves current User/Workspace sources before preparing this value.
//! A published generation is immutable and contains policy, model bindings,
//! Agent profiles, resource catalogs, and prepared finite source demand.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capabilities::selection::ToolSelectionDocument;
use crate::extensions::{NativeAgentExtensions, NativeAgentExtensionsDocument};
use crate::model::deadline::{
    DEFAULT_RESPONSE_START_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT, ModelTimeoutPolicy,
};
use crate::model::session::SessionModelConfig;
use crate::runtime::ApprovalMode;
use crate::runtime::identity::{AgentId, McpServerId};
use crate::runtime::subagent::{SubagentExecutionDeadline, SubagentName};
use crate::runtime::workflow::WorkflowId;
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentError};
use crate::tools::mcp::{McpServerBinding, McpServerBindings, McpTransportConfig};
use crate::tools::types::ToolInvocationPolicy;

/// The only current runtime configuration schema version this runtime accepts.
/// Version 13 combines shared Auxiliary selection, resource permissions and automatic compaction.
pub const CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 13;

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
    /// The root Agent profile, including its named Agent allowlist.
    #[serde(default)]
    pub agent: AgentProfileDocument,
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
    /// Initial MCP readiness wait, measured from Native's first submission
    /// receipt. User < Workspace replaces this complete object atomically.
    /// Preparation before submission does not consume the budget.
    #[serde(default)]
    pub mcp_startup_policy: McpStartupPolicyDocument,
    /// The ecosystem-compatible named MCP server map, keyed by server
    /// identity exactly as mainstream MCP clients spell it.
    #[serde(default)]
    pub mcp_servers: BTreeMap<McpServerId, McpServerDocument>,
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

// Root and named Agent profiles share spelling and lowering for common fields.
macro_rules! agent_profile_document {
    ($(#[$doc:meta])* $name:ident, $default_skills:expr, { $($fields:tt)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $($fields)*
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
            /// Closed native Extension composition. Omission selects none. Root
            /// product defaults are an explicit lower-priority authoring layer,
            /// independent of this complete document's semantics.
            #[serde(default)]
            #[serde(rename = "plugins")]
            pub extensions: NativeAgentExtensionsDocument,
            /// Source-default three-mode permission object (Issue #544).
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
        }

        impl $name {
            pub(crate) fn lower(&self) -> Result<crate::runtime::agent_profile::AgentProfile, String> {
                use crate::runtime::subagent::catalog::{
                    MAX_SUBAGENT_DESCRIPTION_BYTES, MAX_SUBAGENT_INSTRUCTIONS_BYTES,
                };
                self.tools.validate_spelling()?;
                if self.description.len() > MAX_SUBAGENT_DESCRIPTION_BYTES
                    || self.instructions.len() > MAX_SUBAGENT_INSTRUCTIONS_BYTES
                {
                    return Err("Agent description or instructions exceeds its native bound".into());
                }
                Ok(crate::runtime::agent_profile::AgentProfile {
                    description: self.description.clone(),
                    instructions: self.instructions.clone(),
                    model: self.model.clone(),
                    execution_deadline: self.execution_deadline()?,
                    tools: self.tools.selectors(),
                    skills: self.skills.clone().unwrap_or_else($default_skills),
                    extensions: self.extensions.resolve(),
                    workflows: self.workflows.iter().cloned().collect(),
                    inherit_project_instructions: self.agents_md.inherit,
                    permissions: self.permissions,
                })
            }

            /// Converts the TOML millisecond field into the validated runtime type.
            ///
            /// # Errors
            /// Returns the explicit zero/maximum validation detail.
            pub fn execution_deadline(&self) -> Result<Option<SubagentExecutionDeadline>, String> {
                self.timeout_ms
                    .map(SubagentExecutionDeadline::from_millis)
                    .transpose()
                    .map_err(|error| error.to_string())
            }
        }
    };
}

agent_profile_document!(
    /// Root Agent profile. Only the root can delegate to named Agents.
    AgentProfileDocument, || crate::runtime::agent_profile::AgentSkillSelection::All, {
        #[serde(default)]
        pub agents: Vec<SubagentName>,
    }
);

agent_profile_document!(
    /// A complete named Agent resource profile.
    NamedAgentProfileDocument,
    crate::runtime::agent_profile::AgentSkillSelection::default,
    {}
);

/// Whether to include the automatically discovered AGENTS.md guidance chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AgentProjectInstructionsDocument {
    pub inherit: bool,
}

impl Default for AgentProjectInstructionsDocument {
    fn default() -> Self {
        Self { inherit: true }
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

/// One finite budget for the selected MCP initialization batch, including
/// remaining queueing, negotiation, complete discovery and validation. This
/// does not govern model responses or Tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct McpStartupPolicyDocument {
    /// Milliseconds after the first native submission receipt.
    #[schemars(range(min = 1))]
    pub wait_timeout_ms: u64,
}

impl Default for McpStartupPolicyDocument {
    fn default() -> Self {
        Self {
            wait_timeout_ms: 15_000,
        }
    }
}

impl McpStartupPolicyDocument {
    /// Construct the absolute deadline without extending an earlier receipt.
    ///
    /// # Errors
    /// Rejects zero or an interval that the monotonic clock cannot represent.
    pub fn deadline_from(
        self,
        received_at: tokio::time::Instant,
    ) -> Result<tokio::time::Instant, String> {
        if self.wait_timeout_ms == 0 {
            return Err("mcp_startup_policy.wait_timeout_ms must be positive".to_owned());
        }
        received_at
            .checked_add(Duration::from_millis(self.wait_timeout_ms))
            .ok_or_else(|| {
                "mcp_startup_policy.wait_timeout_ms exceeds the monotonic clock range".to_owned()
            })
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

    pub(super) fn defaults(model: Option<SessionModelConfig>) -> Self {
        Self {
            schema_version: default_schema_version(),
            agent_id: default_agent_id(),
            approval_mode: ApprovalMode::default(),
            agent: AgentProfileDocument {
                model,
                ..builtin_root_profile()
            },
            model_timeout_policy: ModelTimeoutPolicyDocument::default(),
            tool_deadline_policy: ToolDeadlinePolicyDocument::default(),
            mcp_startup_policy: McpStartupPolicyDocument::default(),
            mcp_servers: BTreeMap::default(),
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
        let layer = super::authoring::parse_runtime_layer(bytes).map_err(|error| {
            CurrentRuntimeConfigError::Syntax {
                detail: error.into_detail(),
            }
        })?;
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
        self.validate_resources()?;
        if self.agent.model.is_none() {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "agent.model.model is required for root".into(),
            });
        }
        Ok(())
    }

    /// Validate the runtime's resource configuration even if its catalog
    /// has no model for a new Session yet.
    pub(crate) fn validate_resources(&self) -> Result<(), CurrentRuntimeConfigError> {
        self.validate_authored().map_err(|failure| failure.error)?;
        // Duplicate MCP identity is structurally impossible: `mcpServers` is
        // a keyed map. Normalization is the remaining semantic gate, and it
        // runs here so a malformed entry fails at parse time rather than at
        // composition time.
        self.mcp_bindings()?;
        Ok(())
    }

    /// Validates every constraint the authored configuration decides on its
    /// own (Issue #464).
    ///
    /// This needs no root model and no `.agents` resource: a partial source,
    /// which leaves those to another scope, is still checked completely here.
    /// Capability, Skill, subagent and model *authority* is validated later,
    /// against the prepared resource generation that admits it.
    ///
    /// # Errors
    ///
    /// Returns the first failure with the authored field it concerns.
    pub fn validate_authored(&self) -> Result<(), ConstraintFailure> {
        let at = |field, reason| {
            move |error| ConstraintFailure {
                field,
                reason,
                error,
            }
        };
        let invalid = |detail| CurrentRuntimeConfigError::Invalid { detail };
        if self.schema_version != CURRENT_RUNTIME_SCHEMA_VERSION {
            return Err(at("schema_version", "unsupported runtime schema version")(
                CurrentRuntimeConfigError::UnsupportedSchemaVersion {
                    supported: CURRENT_RUNTIME_SCHEMA_VERSION,
                    found: self.schema_version,
                },
            ));
        }
        if self.agent_id.as_str().is_empty() {
            return Err(at("agent_id", "agent_id must be non-empty")(invalid(
                "agent_id must be non-empty".to_owned(),
            )));
        }
        self.timeout_policy()
            .map_err(at("model_timeout_policy", "invalid model timeout policy"))?;
        self.tool_deadline_policy()
            .map_err(at("tool_deadline_policy", "invalid tool deadline policy"))?;
        self.mcp_startup_policy
            .deadline_from(tokio::time::Instant::now())
            .map_err(|detail| {
                at("mcp_startup_policy", "invalid MCP startup policy")(invalid(detail))
            })?;
        self.tool_environment()
            .map_err(at("environment", "invalid Tool environment entry"))?;
        self.agent
            .lower()
            .map_err(|detail| at("agent", "invalid root Agent profile")(invalid(detail)))?;
        if self.agent.timeout_ms.is_some() {
            return Err(at(
                "agent.timeout_ms",
                "the root Agent cannot declare a child lifecycle deadline",
            )(invalid(
                "root scope cannot declare a child lifecycle deadline".into(),
            )));
        }
        self.agent
            .tools
            .validate_spelling()
            .map_err(|detail| at("agent.tools", "invalid Tool selection")(invalid(detail)))?;
        for name in &self.agent.tools.builtin {
            if crate::capabilities::selection::classify_builtin_selection(name)
                == crate::capabilities::selection::BuiltinSelectionClass::Unknown
            {
                return Err(at("agent.tools.builtin", "unknown Native Tool")(invalid(
                    format!("unknown Native Tool {name:?}"),
                )));
            }
        }
        self.agent
            .execution_deadline()
            .map_err(|detail| at("agent", "invalid Agent execution deadline")(invalid(detail)))?;
        if self.subagents.max_concurrent == 0
            || self.subagents.max_concurrent > MAX_MAX_CONCURRENT_SUBAGENTS
        {
            return Err(at(
                "subagents.max_concurrent",
                "subagents.max_concurrent is outside its native range",
            )(invalid(format!(
                "subagents.max_concurrent must be between 1 and \
                 {MAX_MAX_CONCURRENT_SUBAGENTS}, found {}",
                self.subagents.max_concurrent
            ))));
        }
        Self::validate_subagent_admission("agent.agents", &self.agent.agents)
            .map_err(at("agent.agents", "duplicate subagent profile"))?;
        validate_unique_workflow_ids("agent.workflows", &self.agent.workflows)
            .map_err(at("agent.workflows", "duplicate Workflow identity"))?;
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

    /// Freezes the root Agent's native Agent Extension composition for this
    /// launch (Issue #256).
    ///
    /// Composition calls this exactly once. The returned value is the whole
    /// extension authority of the composed root `ConversationRuntime`: no
    /// later resource generation or configuration edit reaches it.
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

    /// The base tool environment this configuration expresses.
    ///
    /// `PATH`, `HOME`, `LANG`, and `LC_ALL` are host-baseline overrides for
    /// native Agent Bash. Excluded executors still compose
    /// [`ToolEnvironment::child_environment`], which keeps its own baseline.
    ///
    /// # Errors
    ///
    /// Returns [`CurrentRuntimeConfigError::Environment`] when an entry is malformed.
    pub fn tool_environment(&self) -> Result<ToolEnvironment, CurrentRuntimeConfigError> {
        ToolEnvironment::from_configured(
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
    /// ambiguous, contradictory, or incomplete.
    pub fn mcp_bindings(&self) -> Result<McpServerBindings, CurrentRuntimeConfigError> {
        resolve_mcp_bindings(&self.mcp_servers)
    }
}

pub(super) fn resolve_mcp_bindings(
    servers: &BTreeMap<McpServerId, McpServerDocument>,
) -> Result<McpServerBindings, CurrentRuntimeConfigError> {
    if servers.len() > 128 {
        return Err(CurrentRuntimeConfigError::Invalid {
            detail: "configuration supports at most 128 MCP sources".into(),
        });
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
                    // Issue #544: MCP invocation policy is product-default
                    // ForegroundOnly; tool-level execution overlays are gone.
                    policy: ToolInvocationPolicy::default(),
                    permissions: document.permissions,
                    tool_permissions: document
                        .tools
                        .iter()
                        .filter_map(|(name, tool)| {
                            tool.permissions
                                .map(|permissions| (name.clone(), permissions))
                        })
                        .collect(),
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
    /// Source-default three-mode permission object (Issue #544).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
    /// Exact-tool permission exceptions keyed by remote tool name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, McpToolPermissionDocument>,
}

/// Exact-tool permission override authored under one MCP server.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct McpToolPermissionDocument {
    /// Complete three-mode permission object for this exact tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<crate::runtime::permission::ResourcePermissionSet>,
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

/// A source-intrinsic constraint failure of [`CurrentRuntimeConfig::validate_authored`].
///
/// `field` and `reason` are native constants, safe to show anywhere; `error`
/// is the native failure, whose text may quote authored values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintFailure {
    /// The authored field the constraint concerns.
    pub field: &'static str,
    /// The fixed category of the failure.
    pub reason: &'static str,
    /// The native failure.
    pub error: CurrentRuntimeConfigError,
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
mod profile_authoring_tests;
#[cfg(test)]
mod tests;
