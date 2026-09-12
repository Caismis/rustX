//! The bounded explicit current runtime/project configuration (Issue #96).
//!
//! This is deliberately a current-runtime input, never durable Session state.
//! It is read for every process start, including resume, so changing MCP,
//! Skill, Tool, environment, context, native Agent Extension, or agent
//! settings takes effect without rewriting the Session catalog. Resource
//! reload does not reread this launch-scoped document, which is exactly why
//! the native Agent Extension composition it declares is launch-scoped: a
//! running `ConversationRuntime` executes against the composition frozen for
//! its launch (Issue #256).
//!
//! Unknown fields are rejected everywhere. A typo must fail startup loudly
//! rather than silently changing runtime semantics.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::capabilities::selection::ToolSelectionDocument;
use crate::context::SessionContextPolicy;
use crate::extensions::{NativeAgentExtensions, NativeAgentExtensionsDocument};
use crate::model::catalog::ModelRef;
use crate::model::deadline::{
    DEFAULT_RESPONSE_START_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT, ModelTimeoutPolicy,
};
use crate::model::session::SessionModelConfig;
use crate::runtime::ApprovalMode;
use crate::runtime::identity::{AgentId, McpServerId};
use crate::runtime::subagent::{MAX_SUBAGENT_DEFINITIONS, SubagentExecutionDeadline, SubagentName};
use crate::runtime::workflow::{MAX_WORKFLOW_DEFINITIONS, WorkflowId};
use crate::runtime::workspace::WorkspacePolicy;
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentError};
use crate::tools::mcp::{McpServerBinding, McpServerBindings, McpTransportConfig};
use crate::tools::native::NativeToolPolicies;
use crate::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationPolicy};

/// The only current runtime configuration schema version this runtime accepts.
pub const CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 8;

/// The explicit current runtime/project configuration.
///
/// No field in this type is persisted by [`SessionCatalog`](super::session::SessionCatalog).
/// The selected Session contributes its separate [`SessionModelConfig`] state
/// during composition; every other field here remains current launch-scoped
/// runtime state.
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
    /// The default model used when a brand-new Session is created.
    pub model: SessionModelConfig,
    /// The current runtime-wide approval control mode. This is launch
    /// host-only configuration, never project authority or Session history.
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    /// The closed launch-scoped **Native Agent Extension** composition of
    /// the root Agent (Issue #256).
    ///
    /// This is the single surface for optional Agent augmentation. It is
    /// read once, at composition, and deliberately never republished by
    /// resource reload: a running `ConversationRuntime` executes against the
    /// extension composition frozen for its launch.
    #[serde(default)]
    pub extensions: NativeAgentExtensionsDocument,
    /// The current runtime context policy.
    #[serde(default)]
    pub context: ContextPolicyDocument,
    /// The finite runtime-owned deadline policy shared by primary and
    /// summarizer model requests. This is current launch state, never model
    /// input or historical request state.
    #[serde(default)]
    pub model_timeout_policy: ModelTimeoutPolicyDocument,
    /// The finite runtime-owned execution-liveness deadline policy of
    /// foreground Tool executions (Issue #204): one generic hard deadline,
    /// plus an optional idle-liveness window refreshed by executor progress.
    /// This is current launch state, never model input, executor-visible
    /// data, or historical execution state.
    #[serde(default)]
    pub tool_deadline_policy: ToolDeadlinePolicyDocument,
    /// The ecosystem-compatible named MCP server map, keyed by server
    /// identity exactly as mainstream MCP clients spell it.
    #[serde(default)]
    pub mcp_servers: BTreeMap<McpServerId, McpServerDocument>,
    /// Explicit managed Python source decisions keyed by `python:<folder>`.
    #[serde(default)]
    pub python_sources: BTreeMap<McpServerId, crate::capabilities::activation::SourceEnablement>,
    /// The host-owned per-server tool invocation policy overlay; forbidden in project layers.
    ///
    /// Deliberately not part of `mcpServers`: an `mcpServers` entry must stay
    /// copy-pasteable from an MCP server's own documentation.
    #[serde(default)]
    pub mcp_tool_policies: BTreeMap<McpServerId, InvocationPolicyDocument>,
    /// The per-tool execution, concurrency, and approval policies of the
    /// native tool plane. This complete object is host-only, including execution/concurrency.
    #[serde(default)]
    pub native_tools: NativeToolPoliciesDocument,
    /// The current base authorized tool environment.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Ordinary native/built-in names active by default. An empty list
    /// selects no built-ins; Read has no activation exception.
    ///
    /// This is the *ordinary capability* plane only. A Tool contributed by a
    /// Native Agent Extension — today `todo` — may not appear here: it is
    /// composed by `extensions`, and naming it is a validation error rather
    /// than a silently ineffective entry (Issue #259).
    #[serde(default = "default_tools")]
    pub default_tools: Vec<String>,
    /// Explicit Skill roots/packages; launch provenance retains host/project/CLI authority.
    #[serde(default)]
    pub skills: Vec<PathBuf>,
    /// The named subagent definitions and their launch-scoped capacity
    /// (Issue #144).
    #[serde(default)]
    pub subagents: SubagentsDocument,
    /// The explicitly registered Workflow definitions and model-visible
    /// admission set (Issue #83).
    #[serde(default)]
    pub workflows: WorkflowsDocument,
}

/// The resolved native representation of the named-subagent plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct SubagentsDocument {
    /// The **launch-scoped** per-conversation concurrency bound.
    ///
    /// It is read once, at composition, and is deliberately not resized by
    /// resource reload: capacity is live-registry state, and shrinking it
    /// under already-committed children would either orphan ownership or
    /// silently lie about the bound.
    pub max_concurrent: usize,
    /// Explicit canonical role identities. Each resolves to one `{name}.md`
    /// resource; registration grants neither main nor Workflow admission.
    pub definitions: Vec<SubagentName>,
    /// Profiles admitted to the main Agent's existing `subagent` capability.
    pub main: Vec<SubagentName>,
    /// Profiles admitted to Workflow Agent and Parallel nodes.
    pub workflow: Vec<SubagentName>,
}

impl Default for SubagentsDocument {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            definitions: Vec::new(),
            main: Vec::new(),
            workflow: Vec::new(),
        }
    }
}

/// The resolved native Workflow definition and admission plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct WorkflowsDocument {
    /// Workflow ids whose YAML files are explicitly registered.
    pub definitions: Vec<WorkflowId>,
    /// Registered workflow ids exposed as concrete model-facing Tools.
    pub main: Vec<WorkflowId>,
}

/// The launch-scoped subagent capacity used when the document omits it.
pub const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 4;

/// The hard upper bound of the launch-scoped subagent capacity.
pub const MAX_MAX_CONCURRENT_SUBAGENTS: usize = 64;

/// Strict canonical role Markdown frontmatter. The body supplies primary instructions.
///
/// Everything here is *definition* state, and the definition is the child's
/// canonical **default** execution profile. One invocation may replace
/// `tools`, `skills`, and `extensions` for exactly that child through the
/// shared `SubagentInvocationOverride` (Issue #258), within an explicit
/// delegation ceiling. Every other field here — model, instructions,
/// `timeoutMs`, `agentsMd`, `worktree` — has no per-call form at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct SubagentDocument {
    /// The bounded model-facing routing description.
    pub description: String,
    /// The explicit model this agent runs on. Omit to inherit the invoking
    /// attempt's frozen effective model configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// The optional maximum wall-clock duration of the complete child
    /// lifecycle, in milliseconds. The model cannot override or extend it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// The exact source-qualified capability selection.
    #[serde(default)]
    pub tools: SubagentToolsDocument,
    /// The exact Skill allowlist over the admitted Skill catalog.
    #[serde(default)]
    pub skills: Vec<String>,
    /// The project-instruction policy of this agent.
    #[serde(default)]
    pub agents_md: SubagentAgentsMdDocument,
    /// The bounded project-workspace policy of this agent.
    #[serde(default)]
    pub worktree: SubagentWorktreeDocument,
    /// The closed **Native Agent Extension** composition of this named role
    /// (Issue #256).
    ///
    /// Independently authored: a role never inherits the root Agent's
    /// extension set, and an omitted value means this role's own built-in
    /// defaults, never the invoking runtime's configuration.
    #[serde(default)]
    pub extensions: NativeAgentExtensionsDocument,
}

impl SubagentDocument {
    /// Converts the frontmatter millisecond field into the validated runtime type.
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

/// The source-qualified capability selection of one named definition.
///
/// Role frontmatter, a Workflow Agent node's invocation override, and the
/// model-facing `subagent` Tool's `override` all express a capability
/// selection with exactly this vocabulary, so the type is the shared
/// [`ToolSelectionDocument`] rather than a second structurally identical
/// authoring shape.
pub type SubagentToolsDocument = ToolSelectionDocument;

/// The project-instruction policy of one named definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct SubagentAgentsMdDocument {
    /// Whether the invoking generation's normal project instruction chain is
    /// prepended to the explicit files.
    pub inherit: bool,
    /// Explicit agent-owned project instruction files, in deterministic
    /// configured order. Relative paths resolve against the owning
    /// configuration document's directory at launch resolution.
    pub files: Vec<PathBuf>,
}

impl Default for SubagentAgentsMdDocument {
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
/// explicit `"requireCleanParent": false` permits a dirty parent while the
/// child still receives exactly the committed snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[derive(schemars::JsonSchema)]
pub struct SubagentWorktreeDocument {
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

impl Default for SubagentWorktreeDocument {
    /// The derived default kept both booleans `false`, which made an omitted
    /// `"requireCleanParent"` silently mean “allow a dirty parent”. The two
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

impl SubagentWorktreeDocument {
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

fn default_tools() -> Vec<String> {
    [
        "execution",
        "ask_user",
        "read",
        "write",
        "edit",
        "glob",
        "grep",
        "bash",
        "subagent",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn default_agent_id() -> AgentId {
    AgentId::new("rustx")
}

impl CurrentRuntimeConfig {
    pub(super) fn defaults(model: SessionModelConfig) -> Self {
        Self {
            schema_version: default_schema_version(),
            agent_id: default_agent_id(),
            model,
            approval_mode: ApprovalMode::default(),
            extensions: NativeAgentExtensionsDocument::default(),
            context: ContextPolicyDocument::default(),
            model_timeout_policy: ModelTimeoutPolicyDocument::default(),
            tool_deadline_policy: ToolDeadlinePolicyDocument::default(),
            mcp_servers: BTreeMap::default(),
            python_sources: BTreeMap::default(),
            mcp_tool_policies: BTreeMap::default(),
            native_tools: NativeToolPoliciesDocument::default(),
            environment: BTreeMap::default(),
            default_tools: default_tools(),
            skills: Vec::default(),
            subagents: SubagentsDocument::default(),
            workflows: WorkflowsDocument::default(),
        }
    }

    /// Normalize source intent after the launch resolver has accepted project
    /// trust and resource authority. No configuration can author host-only states.
    #[must_use]
    pub fn python_activations(
        &self,
    ) -> BTreeMap<McpServerId, crate::capabilities::activation::SourceActivation> {
        self.python_sources
            .iter()
            .map(|(id, intent)| {
                (
                    id.clone(),
                    crate::capabilities::activation::SourceActivation::evaluate(
                        Some(*intent),
                        true,
                    ),
                )
            })
            .collect()
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
        if self.mcp_servers.len() + self.python_sources.len() > 128
            || self.subagents.definitions.len() > 128
            || self.workflows.definitions.len() > 128
        {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "configuration supports at most 128 sources, 128 roles, and 128 Workflows"
                    .into(),
            });
        }
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
        if self.context.summary_output_cap == Some(0) {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "context.summary_output_cap must be positive when present".to_owned(),
            });
        }
        self.timeout_policy()?;
        if self.default_tools.iter().any(|name| name.trim().is_empty()) {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "default_tools entries must be non-empty names".to_owned(),
            });
        }
        // `defaultTools` addresses ordinary capabilities. An extension's Tool
        // is refused here, at authoring time, rather than tolerated as an
        // unknown name that quietly decides nothing (Issue #259).
        for name in &self.default_tools {
            if let Some(extension) = crate::capabilities::extension_provided_tool(name) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!(
                        "default_tools entry {name:?} is provided by the \
                         {extension:?} Agent Extension, not by ordinary Tool \
                         selection; compose it with extensions.{extension}.enabled instead"
                    ),
                });
            }
        }
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
        if self.subagents.definitions.len() > MAX_SUBAGENT_DEFINITIONS {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: format!(
                    "subagents.definitions declares {} agents; at most \
                     {MAX_SUBAGENT_DEFINITIONS} are admitted",
                    self.subagents.definitions.len()
                ),
            });
        }
        Self::validate_subagent_admission(
            "subagents.main",
            &self.subagents.main,
            &self.subagents.definitions,
        )?;
        Self::validate_subagent_admission(
            "subagents.workflow",
            &self.subagents.workflow,
            &self.subagents.definitions,
        )?;
        Self::validate_subagent_admission(
            "subagents.definitions",
            &self.subagents.definitions,
            &self.subagents.definitions,
        )?;
        Ok(())
    }

    /// Validates one independent profile admission domain.
    fn validate_subagent_admission(
        label: &str,
        admission: &[SubagentName],
        definitions: &[SubagentName],
    ) -> Result<(), CurrentRuntimeConfigError> {
        let mut seen = std::collections::BTreeSet::new();
        for name in admission {
            if !seen.insert(name) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!("{label} contains duplicate profile {name:?}"),
                });
            }
            if !definitions.contains(name) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!("{label} references undefined profile {name:?}"),
                });
            }
        }
        Ok(())
    }

    /// Validates registered Workflow ids and their model-visible subset.
    fn validate_workflows(&self) -> Result<(), CurrentRuntimeConfigError> {
        if self.workflows.definitions.len() > MAX_WORKFLOW_DEFINITIONS {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: format!(
                    "workflows.definitions declares too many workflows ({} > {})",
                    self.workflows.definitions.len(),
                    MAX_WORKFLOW_DEFINITIONS
                ),
            });
        }
        validate_unique_workflow_ids("workflows.definitions", &self.workflows.definitions)?;
        validate_unique_workflow_ids("workflows.main", &self.workflows.main)?;
        let registered = self
            .workflows
            .definitions
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        if let Some(unknown) = self
            .workflows
            .main
            .iter()
            .find(|workflow| !registered.contains(workflow))
        {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: format!("workflows.main references unregistered workflow {unknown:?}"),
            });
        }
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
        self.extensions.resolve()
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
        for id in self.python_sources.keys() {
            if !id.as_str().strip_prefix("python:").is_some_and(|name| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            }) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: "python_sources keys must be python:<folder> identities".into(),
                });
            }
        }
        for server_id in self.mcp_tool_policies.keys() {
            if !self.mcp_servers.contains_key(server_id) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!(
                        "mcp_tool_policies names {server_id}, which mcp_servers does not declare"
                    ),
                });
            }
        }
        self.mcp_servers
            .iter()
            .map(|(server_id, document)| {
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
                let transport = document.to_transport().map_err(|detail| {
                    CurrentRuntimeConfigError::Invalid {
                        detail: format!("mcp_servers.{server_id}: {detail}"),
                    }
                })?;
                Ok((
                    server_id.clone(),
                    McpServerBinding {
                        credentials: crate::credentials::SourceCredentials {
                            environment: document.sensitive_env.clone(),
                            headers: document.sensitive_headers.clone(),
                            ..Default::default()
                        },
                        activation: document.activation(),
                        resource_workspace: None,
                        transport,
                        policy: self
                            .mcp_tool_policies
                            .get(server_id)
                            .copied()
                            .unwrap_or_default()
                            .to_policy(),
                    },
                ))
            })
            .collect()
    }
}

fn validate_unique_workflow_ids(
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
    /// Host-only explicit secret references; ordinary `env` is literal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sensitive_env: BTreeMap<String, crate::credentials::EnvironmentReference>,
    /// Host-only explicit secret references; ordinary `headers` is literal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sensitive_headers: BTreeMap<String, crate::credentials::EnvironmentReference>,
    /// Omission is discovery only; true explicitly admits preparation under host trust.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
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
    /// Normalize explicit presence without treating discovery as enablement.
    #[must_use]
    pub fn activation(&self) -> crate::capabilities::activation::SourceActivation {
        use crate::capabilities::activation::{SourceActivation, SourceEnablement};
        SourceActivation::evaluate(
            self.enabled.map(|enabled| {
                if enabled {
                    SourceEnablement::Enabled
                } else {
                    SourceEnablement::Disabled
                }
            }),
            true,
        )
    }
    /// The runtime transport this entry normalizes to.
    ///
    /// # Errors
    ///
    /// Returns a human-readable detail when the entry is ambiguous,
    /// contradictory, or incomplete.
    pub fn to_transport(&self) -> Result<McpTransportConfig, String> {
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
                if endpoint.trim().is_empty() {
                    return Err("url must be a non-empty endpoint".to_owned());
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

    const MINIMAL: &str = r#"agent_id = "agent-a"

[model]
model = "p/m"

[context]
reserve_tokens = 1024
keep_recent_tokens = 4096
"#;

    /// The minimal configuration parses and derives its policy pieces.
    #[test]
    fn minimal_configuration_parses() {
        let config = CurrentRuntimeConfig::from_toml_slice(MINIMAL.as_bytes()).expect("valid");
        assert_eq!(config.approval_mode, crate::runtime::ApprovalMode::Policy);
        assert_eq!(config.context_policy().reserve_tokens, 1024);
        assert!(config.extensions.agent_status.time.enabled);
        assert!(config.extensions.agent_status.background.enabled);
        assert_eq!(config.extensions.agent_status.time.timezone, None);
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
    /// omitted `"requireCleanParent"` resolves to `true`, exactly like an
    /// explicit `true`; only an explicit `false` retains the committed-
    /// snapshot permissive path, and disabled/omitted isolation keeps the
    /// shared-workspace policy unchanged. The normalization lives at this
    /// configuration/domain boundary: `enabled` stays `false` by default
    /// while `require_clean_parent` becomes `true` by default.
    #[test]
    fn named_subagent_worktree_policy_is_bounded_and_definition_scoped() {
        use crate::runtime::workspace::WorkspacePolicy as Policy;

        fn policy(worktree: &str) -> Policy {
            let field = if worktree.is_empty() {
                String::new()
            } else {
                format!(", \"worktree\": {worktree}")
            };
            let text =
                format!("---\n{{\"description\": \"worker\"{field}}}\n---\nWorker instructions\n");
            crate::local_runtime::subagent_resources::parse(&text)
                .expect("valid role")
                .0
                .worktree
                .to_policy()
        }

        // `enabled: true` with an omitted `requireCleanParent` resolves to
        // the strict clean-parent policy.
        assert_eq!(
            policy(r#"{"enabled": true}"#),
            Policy::GitWorktree {
                require_clean_parent: true,
            }
        );
        // An explicit `requireCleanParent: true` is the same strict policy.
        assert_eq!(
            policy(r#"{"enabled": true, "requireCleanParent": true}"#),
            Policy::GitWorktree {
                require_clean_parent: true,
            }
        );
        // An explicit `requireCleanParent: false` is the committed-snapshot
        // opt-out.
        assert_eq!(
            policy(r#"{"enabled": true, "requireCleanParent": false}"#),
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
        let default_document = super::SubagentWorktreeDocument::default();
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

    fn worker_config(timeout: &str) -> Result<super::SubagentDocument, String> {
        crate::local_runtime::subagent_resources::parse(&format!(
            "---\ndescription: worker\ntimeoutMs: {timeout}\n---\nWorker\n"
        ))
        .map(|(role, _)| role)
    }

    #[test]
    fn named_subagent_execution_deadline_is_optional_and_typed_at_admission() {
        let (absent, _) = crate::local_runtime::subagent_resources::parse(
            "---\ndescription: worker\n---\nWorker\n",
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

[model]
model = "p/m"

[context]
reserve_tokens = 0
keep_recent_tokens = 0
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
                "[extensions.future_goal]\nenabled = true",
                "unknown field `future_goal`",
            ),
            (
                "[extensions.todo]\nenabled = true\nfuture = true",
                "unknown field `future`",
            ),
            ("[extensions.todo]\nenabled = 'true'", "expected a boolean"),
            (
                "[extensions.agent_status]\nfuture = true",
                "unknown field `future`",
            ),
            (
                "[extensions.agent_status.time]\nenabled = true\nfuture = true",
                "unknown field `future`",
            ),
            (
                "[extensions.agent_status.background]\nfuture = true",
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
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
extensions = { "agent_status" = { "time" = { "timezone" = "Asia/Shanghai" }, "background" = { "enabled" = false } } }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(enabled.as_bytes()).expect("valid");
        let composition = config.extension_composition();
        let agent_status = composition
            .agent_status()
            .expect("the declared Agent Status extension is composed");
        assert_eq!(agent_status.time.timezone, Some(chrono_tz::Asia::Shanghai));
        assert!(!agent_status.background.enabled);

        let disabled = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
extensions = { "agent_status" = { "enabled" = false } }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(disabled.as_bytes()).expect("valid");
        assert!(
            config.extension_composition().agent_status().is_none(),
            "a disabled extension leaves an empty composition, not a disabled one"
        );

        // Issue #259: the members are independent, and switching both off is
        // what empties the composition.
        let neither = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
extensions = { "agent_status" = { "enabled" = false }, "todo" = { "enabled" = false } }"#,
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

[model]
model = "p/m"

[context]
reserve_tokens = 0
keep_recent_tokens = 0
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

[model]
model = "p/m"

[context]
reserve_tokens = 0
keep_recent_tokens = 0

[[mcp_servers]]
server_id = "s"

[mcp_servers.transport]
type = "streamable_http"
endpoint = "https://x"
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
        let json = format!("{MINIMAL}\nsummary_output_cap = {{ mode = \"limit\", tokens = 0 }}\n");
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
subagents = { "definitions" = ["worker"], "main" = [], "workflow" = ["worker"] }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(json.as_bytes()).expect("valid");
        assert!(config.subagents.main.is_empty());
        assert_eq!(config.subagents.workflow.len(), 1);
        assert_eq!(config.subagents.definitions.len(), 1);

        let defined_but_unadmitted = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
subagents = { "definitions" = ["worker"], "main" = [], "workflow" = [] }"#,
        );
        assert!(CurrentRuntimeConfig::from_toml_slice(defined_but_unadmitted.as_bytes()).is_ok());
    }

    #[test]
    fn unknown_or_duplicate_admission_ids_are_rejected() {
        let unknown = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
subagents = { "definitions" = [], "main" = ["missing"], "workflow" = [] }"#,
        );
        let error = CurrentRuntimeConfig::from_toml_slice(unknown.as_bytes()).expect_err("unknown");
        assert!(error.to_string().contains("subagents.main"));

        let duplicate = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
subagents = { "definitions" = ["worker"], "main" = ["worker", "worker"], "workflow" = [] }"#,
        );
        let error =
            CurrentRuntimeConfig::from_toml_slice(duplicate.as_bytes()).expect_err("duplicate");
        assert!(error.to_string().contains("duplicate profile"));
    }

    #[test]
    fn workflow_registration_and_main_exposure_are_separate() {
        let valid = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
workflows = { "definitions" = ["review_pr"], "main" = [] }"#,
        );
        let config = CurrentRuntimeConfig::from_toml_slice(valid.as_bytes()).expect("valid");
        assert_eq!(config.workflows.definitions.len(), 1);
        assert!(config.workflows.main.is_empty());

        let unknown = MINIMAL.replace(
            r#"agent_id = "agent-a""#,
            r#"agent_id = "agent-a"
workflows = { "definitions" = ["review_pr"], "main" = ["investigate"] }"#,
        );
        let error = CurrentRuntimeConfig::from_toml_slice(unknown.as_bytes()).expect_err("unknown");
        assert!(error.to_string().contains("workflows.main"));
    }
}
