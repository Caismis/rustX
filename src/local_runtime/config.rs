//! The bounded explicit current runtime/project configuration (Issue #96).
//!
//! This is deliberately a current-runtime input, never durable Session state.
//! It is read for every process start, including resume, so changing MCP,
//! Skill, Tool, environment, context, Agent Status, or agent settings takes
//! effect without rewriting the Session catalog. Resource reload does not
//! reread this launch-scoped document.
//!
//! Unknown fields are rejected everywhere. A typo must fail startup loudly
//! rather than silently changing runtime semantics.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::context::{AgentStatusConfig, SessionContextPolicy};
use crate::model::catalog::ModelRef;
use crate::model::deadline::{
    DEFAULT_RESPONSE_START_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT, ModelTimeoutPolicy,
};
use crate::model::session::SessionModelConfig;
use crate::runtime::ApprovalMode;
use crate::runtime::identity::{AgentId, McpServerId};
use crate::runtime::subagent::{
    MAX_SUBAGENT_DEFINITIONS, SubagentName, SubagentToolSelector, SubagentWorkspacePolicy,
};
use crate::runtime::workflow::{MAX_WORKFLOW_DEFINITIONS, WorkflowId};
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentError};
use crate::tools::mcp::{McpServerBinding, McpServerBindings, McpTransportConfig};
use crate::tools::native::NativeToolPolicies;
use crate::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationPolicy};

/// The only current runtime configuration schema version this runtime accepts.
pub const CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 5;

/// The explicit current runtime/project configuration.
///
/// No field in this type is persisted by [`SessionCatalog`](super::session::SessionCatalog).
/// The selected Session contributes its separate [`SessionModelConfig`] state
/// during composition; every other field here remains current launch-scoped
/// runtime state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CurrentRuntimeConfig {
    /// The runtime configuration schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The agent executed by attempts of this conversation.
    pub agent_id: AgentId,
    /// The default model used when a brand-new Session is created.
    pub model: SessionModelConfig,
    /// The current runtime-wide approval control mode. This is launch
    /// configuration, never Session history.
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    /// The launch-scoped Agent Status module configuration.
    #[serde(default)]
    pub agent_status: AgentStatusConfig,
    /// The current runtime context policy.
    pub context: ContextPolicyDocument,
    /// The finite runtime-owned deadline policy shared by primary and
    /// summarizer model requests. This is current launch state, never model
    /// input or historical request state.
    #[serde(default)]
    pub model_timeout_policy: ModelTimeoutPolicyDocument,
    /// The ecosystem-compatible named MCP server map, keyed by server
    /// identity exactly as mainstream MCP clients spell it.
    #[serde(default)]
    pub mcp_servers: BTreeMap<McpServerId, McpServerDocument>,
    /// The rustX-owned per-server tool invocation policy overlay.
    ///
    /// Deliberately not part of `mcpServers`: an `mcpServers` entry must stay
    /// copy-pasteable from an MCP server's own documentation.
    #[serde(default)]
    pub mcp_tool_policies: BTreeMap<McpServerId, InvocationPolicyDocument>,
    /// The per-tool execution, concurrency, and approval policies of the
    /// native tool plane.
    #[serde(default)]
    pub native_tools: NativeToolPoliciesDocument,
    /// The current base authorized tool environment.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Optional native/built-in tool names active by default. An empty list
    /// disables optional built-in activation while mandatory native Read
    /// remains active and available.
    #[serde(default = "default_tools")]
    pub default_tools: Vec<String>,
    /// Explicit Skill roots or package paths supplied by the project config.
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

/// The JSONC representation of the named-subagent plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct SubagentsDocument {
    /// The **launch-scoped** per-conversation concurrency bound.
    ///
    /// It is read once, at composition, and is deliberately not resized by
    /// resource reload: capacity is live-registry state, and shrinking it
    /// under already-committed children would either orphan ownership or
    /// silently lie about the bound.
    pub max_concurrent: usize,
    /// The one authoritative named definitions map, keyed by canonical
    /// [`SubagentName`]. Admission is separate in `main` and `workflow`.
    ///
    /// The key *is* the name: a definition never repeats it as a field.
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub definitions: BTreeMap<SubagentName, SubagentDocument>,
    /// Profiles admitted to the main Agent's existing `subagent` capability.
    pub main: Vec<SubagentName>,
    /// Profiles admitted to Workflow Agent and Parallel nodes.
    pub workflow: Vec<SubagentName>,
}

impl Default for SubagentsDocument {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            definitions: BTreeMap::new(),
            main: Vec::new(),
            workflow: Vec::new(),
        }
    }
}

/// The JSONC representation of the Workflow definition and admission plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
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

/// One named subagent definition, as configured.
///
/// Everything here is *definition* state. None of it is exposed as a
/// per-call model argument: the model chooses which named agent runs and
/// nothing else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentDocument {
    /// The bounded model-facing routing description.
    pub description: String,
    /// The child instruction document. Relative paths resolve against the
    /// canonical workspace root.
    pub instructions_file: PathBuf,
    /// The explicit model this agent runs on. Omit to inherit the invoking
    /// attempt's frozen effective model configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
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
}

/// The source-qualified capability selection of one named definition.
///
/// Origins are named explicitly rather than collapsed into bare strings, so
/// a Builtin `read` and an MCP server's `read` are never interchangeable and
/// resolution keeps exact source identity. Wildcards are deliberately
/// absent: a selection is an exact list. Managed Python tool packages are
/// selected through the `mcp` map under their synthesized server identity
/// (`python:<folder>`, Issue #174).
///
/// The `python:` namespace is reserved for those synthesized identities:
/// `mcpServers` configuration may never declare a server under it (rejected
/// during validation), so a subagent selection naming `python:<folder>`
/// always resolves to the managed package, never to a configured server.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct SubagentToolsDocument {
    /// Built-in/native capabilities, by canonical model-facing name.
    pub builtin: Vec<String>,
    /// MCP capabilities, keyed by server identity.
    pub mcp: BTreeMap<McpServerId, Vec<String>>,
}

impl SubagentToolsDocument {
    /// The typed selectors this document expresses.
    #[must_use]
    pub fn selectors(&self) -> Vec<SubagentToolSelector> {
        let mut selectors: Vec<SubagentToolSelector> = self
            .builtin
            .iter()
            .map(|name| SubagentToolSelector::Builtin { name: name.clone() })
            .collect();
        for (server_id, names) in &self.mcp {
            selectors.extend(names.iter().map(|name| SubagentToolSelector::Mcp {
                server_id: server_id.clone(),
                name: name.clone(),
            }));
        }
        selectors
    }
}

/// The project-instruction policy of one named definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct SubagentAgentsMdDocument {
    /// Whether the invoking generation's normal project instruction chain is
    /// prepended to the explicit files.
    pub inherit: bool,
    /// Explicit agent-owned project instruction files, in deterministic
    /// configured order. Relative paths resolve against the canonical
    /// workspace root.
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

/// The JSONC representation of a named subagent's optional Git worktree
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
    pub const fn to_policy(self) -> SubagentWorkspacePolicy {
        if self.enabled {
            SubagentWorkspacePolicy::GitWorktree {
                require_clean_parent: self.require_clean_parent,
            }
        } else {
            SubagentWorkspacePolicy::SharedWorkspace
        }
    }
}

/// The JSONC representation of the one shared model request timeout policy.
///
/// Milliseconds keep the configuration human-readable while the runtime
/// receives a typed [`ModelTimeoutPolicy`] containing only finite
/// [`Duration`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
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
            return Err("modelTimeoutPolicy.responseStartTimeoutMs must be positive".to_owned());
        }
        if self.stream_idle_timeout_ms == 0 {
            return Err("modelTimeoutPolicy.streamIdleTimeoutMs must be positive".to_owned());
        }
        Ok(ModelTimeoutPolicy::new(
            Duration::from_millis(self.response_start_timeout_ms),
            Duration::from_millis(self.stream_idle_timeout_ms),
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
        "todo",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

impl CurrentRuntimeConfig {
    /// Parses and validates current runtime configuration from JSONC bytes.
    ///
    /// The document is [JSONC](crate::config_format): JSON plus comments and
    /// trailing commas, so a `rustx.jsonc` can explain its own values.
    ///
    /// # Errors
    ///
    /// Returns [`CurrentRuntimeConfigError::Syntax`] for malformed JSONC or
    /// unknown fields, and a specific validation error otherwise.
    pub fn from_jsonc_slice(bytes: &[u8]) -> Result<Self, CurrentRuntimeConfigError> {
        let config: Self = crate::config_format::parse(bytes)
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
                detail: "agentId must be non-empty".to_owned(),
            });
        }
        if self.context.summary_output_cap == Some(0) {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "context.summaryOutputCap must be positive when present".to_owned(),
            });
        }
        self.timeout_policy()?;
        if self.default_tools.iter().any(|name| name.trim().is_empty()) {
            return Err(CurrentRuntimeConfigError::Invalid {
                detail: "defaultTools entries must be non-empty names".to_owned(),
            });
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
                    "subagents.maxConcurrent must be between 1 and \
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
        for (name, document) in &self.subagents.definitions {
            if document.instructions_file.as_os_str().is_empty() {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!(
                        "subagents.definitions.{name}.instructionsFile must be non-empty"
                    ),
                });
            }
            for selector in document.tools.selectors() {
                let empty = match &selector {
                    SubagentToolSelector::Builtin { name } => name.trim().is_empty(),
                    SubagentToolSelector::Mcp { server_id, name } => {
                        server_id.as_str().is_empty() || name.trim().is_empty()
                    }
                };
                if empty {
                    return Err(CurrentRuntimeConfigError::Invalid {
                        detail: format!(
                            "subagents.definitions.{name}.tools names an empty capability identity"
                        ),
                    });
                }
            }
            if document.skills.iter().any(|skill| skill.trim().is_empty()) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!(
                        "subagents.definitions.{name}.skills entries must be non-empty"
                    ),
                });
            }
        }
        Ok(())
    }

    /// Validates one independent profile admission domain.
    fn validate_subagent_admission(
        label: &str,
        admission: &[SubagentName],
        definitions: &BTreeMap<SubagentName, SubagentDocument>,
    ) -> Result<(), CurrentRuntimeConfigError> {
        let mut seen = std::collections::BTreeSet::new();
        for name in admission {
            if !seen.insert(name) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!("{label} contains duplicate profile {name:?}"),
                });
            }
            if !definitions.contains_key(name) {
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
        SessionContextPolicy {
            reserve_tokens: self.context.reserve_tokens,
            keep_recent_tokens: self.context.keep_recent_tokens,
            summary_output_cap: self.context.summary_output_cap,
        }
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
        for server_id in self.mcp_tool_policies.keys() {
            if !self.mcp_servers.contains_key(server_id) {
                return Err(CurrentRuntimeConfigError::Invalid {
                    detail: format!(
                        "mcpToolPolicies names {server_id}, which mcpServers does not declare"
                    ),
                });
            }
        }
        self.mcp_servers
            .iter()
            .map(|(server_id, document)| {
                if server_id.as_str().is_empty() {
                    return Err(CurrentRuntimeConfigError::Invalid {
                        detail: "mcpServers keys must be non-empty server identities".to_owned(),
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
                            "mcpServers.{server_id}: the \"{}\" MCP server namespace is reserved \
                             for automatically discovered managed Python tool packages \
                             (each `.agents/tools/<folder>/` synthesizes \
                             \"{0}<folder>\"); configure this server under a different id",
                            crate::tools::python::MANAGED_MCP_NAMESPACE,
                        ),
                    });
                }
                let transport = document.to_transport().map_err(|detail| {
                    CurrentRuntimeConfigError::Invalid {
                        detail: format!("mcpServers.{server_id}: {detail}"),
                    }
                })?;
                Ok((
                    server_id.clone(),
                    McpServerBinding {
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

/// Deserializes the authoritative definitions map without accepting a
/// duplicate profile key through a parser-specific last-write-wins rule.
fn deserialize_unique_map<'de, D, V>(deserializer: D) -> Result<BTreeMap<SubagentName, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct UniqueMapVisitor<V>(std::marker::PhantomData<V>);

    impl<'de, V> Visitor<'de> for UniqueMapVisitor<V>
    where
        V: Deserialize<'de>,
    {
        type Value = BTreeMap<SubagentName, V>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a map with unique subagent definition names")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
            let mut values = BTreeMap::new();
            while let Some(key) = access.next_key::<SubagentName>()? {
                if values.contains_key(&key) {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate subagent definition {key:?}"
                    )));
                }
                let value = access.next_value::<V>()?;
                values.insert(key, value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(UniqueMapVisitor(std::marker::PhantomData))
}

/// The static current-runtime context policy document.
///
/// There is deliberately no context window here: the window belongs to the
/// selected model and is derived per attempt from that attempt's immutable
/// model snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// The per-tool execution, concurrency, and approval policies of the native
/// tool plane.
///
/// `execution`, `ask_user`, and `todo` are deliberately outside this
/// set: they own fixed foreground-only, sequential, approval-never policies,
/// and the registry enforces the intrinsic ones itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct NativeToolPoliciesDocument {
    /// The policy of the native Read tool.
    pub read: InvocationPolicyDocument,
    /// The policy of the native Write tool.
    pub write: InvocationPolicyDocument,
    /// The policy of the native Edit tool.
    pub edit: InvocationPolicyDocument,
    /// The policy of the native Glob tool.
    pub glob: InvocationPolicyDocument,
    /// The policy of the native Grep tool.
    pub grep: InvocationPolicyDocument,
    /// The policy of the native Bash tool.
    pub bash: InvocationPolicyDocument,
}

impl NativeToolPoliciesDocument {
    /// The native tool policy table this document expresses.
    #[must_use]
    pub const fn to_policies(self) -> NativeToolPolicies {
        NativeToolPolicies {
            read: self.read.to_policy(),
            write: self.write.to_policy(),
            edit: self.edit.to_policy(),
            glob: self.glob.to_policy(),
            grep: self.grep.to_policy(),
            bash: self.bash.to_policy(),
        }
    }
}

/// One tool invocation policy document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
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
pub struct McpServerDocument {
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
    /// The stdio workspace-relative working directory; absent means the
    /// workspace root. The runtime keeps enforcing that it stays inside the
    /// workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// The transport an `mcpServers` entry selects explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransportType {
    /// Streamable HTTP. This is the canonical spelling for a remote server.
    Http,
    /// A locally launched stdio server.
    Stdio,
}

impl McpServerDocument {
    /// The runtime transport this entry normalizes to.
    ///
    /// # Errors
    ///
    /// Returns a human-readable detail when the entry is ambiguous,
    /// contradictory, or incomplete.
    pub fn to_transport(&self) -> Result<McpTransportConfig, String> {
        let has_http_fields = self.url.is_some() || !self.headers.is_empty();
        let has_stdio_fields = self.command.is_some()
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
                "unsupported current runtime schemaVersion {found}; this runtime speaks {supported}"
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

    const MINIMAL: &str = r#"{
        "agentId": "agent-a",
        "model": {"model": "p/m"},
        "context": {"reserveTokens": 1024, "keepRecentTokens": 4096}
    }"#;

    /// The minimal configuration parses and derives its policy pieces.
    #[test]
    fn minimal_configuration_parses() {
        let config = CurrentRuntimeConfig::from_jsonc_slice(MINIMAL.as_bytes()).expect("valid");
        assert_eq!(config.approval_mode, crate::runtime::ApprovalMode::Policy);
        assert_eq!(config.context_policy().reserve_tokens, 1024);
        assert!(config.agent_status.time.enabled);
        assert!(config.agent_status.background.enabled);
        assert_eq!(config.agent_status.time.timezone, None);
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
        use crate::runtime::subagent::SubagentWorkspacePolicy as Policy;

        /// Builds a minimal config whose single `worker` definition carries
        /// exactly the given `worktree` JSONC document (empty when omitted).
        fn worker_with_worktree(worktree: &str) -> String {
            let worktree = if worktree.is_empty() {
                String::new()
            } else {
                format!(r#", "worktree": {worktree}"#)
            };
            MINIMAL.replace(
                r#""agentId": "agent-a""#,
                &format!(
                    r#""agentId": "agent-a", "subagents": {{"definitions": {{"worker": {{"description": "worker", "instructionsFile": "worker.md"{worktree}}}}}, "main": ["worker"], "workflow": []}}"#
                ),
            )
        }

        fn policy(worktree: &str) -> Policy {
            let config =
                CurrentRuntimeConfig::from_jsonc_slice(worker_with_worktree(worktree).as_bytes())
                    .expect("valid");
            config
                .subagents
                .definitions
                .get(&crate::runtime::subagent::SubagentName::parse("worker").expect("name"))
                .expect("worker definition")
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
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "modelTimeoutPolicy": {"responseStartTimeoutMs": 7, "streamIdleTimeoutMs": 11}"#,
        );
        let config = CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect("valid");
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

    /// A zero deadline is rejected at the current-runtime composition
    /// boundary rather than creating an ambiguous request.
    #[test]
    fn zero_model_timeout_is_rejected() {
        let json = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "modelTimeoutPolicy": {"responseStartTimeoutMs": 0, "streamIdleTimeoutMs": 11}"#,
        );
        let error = CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect_err("must fail");
        assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
        assert!(error.to_string().contains("responseStartTimeoutMs"));
    }

    /// A policy read for one admission remains unchanged when current
    /// configuration is edited; a later admission reads the new values.
    #[test]
    fn timeout_policy_changes_apply_only_to_later_admissions() {
        let json = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "modelTimeoutPolicy": {"responseStartTimeoutMs": 7, "streamIdleTimeoutMs": 11}"#,
        );
        let mut config = CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect("valid");
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

    /// `ApprovalMode` is current runtime configuration and accepts the
    /// explicit `FullAccess` spelling without becoming Session state.
    #[test]
    fn approval_mode_is_current_configuration_with_policy_default() {
        let json = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "approvalMode": "full_access""#,
        );
        let config = CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect("valid");
        assert_eq!(
            config.approval_mode,
            crate::runtime::ApprovalMode::FullAccess
        );
    }

    /// Unknown fields fail rather than silently changing semantics.
    #[test]
    fn unknown_fields_are_rejected() {
        let json = r#"{
            "conversationId": "c", "agentId": "a",
            "model": {"model": "p/m"},
            "context": {"reserveTokens": 0, "keepRecentTokens": 0},
            "futureKnob": true
        }"#;
        assert!(matches!(
            CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect_err("must fail"),
            CurrentRuntimeConfigError::Syntax { .. }
        ));
    }

    /// Schema v3 owns timezone under the Time status module; the obsolete
    /// top-level field is rejected rather than silently ignored.
    #[test]
    fn top_level_timezone_is_rejected() {
        let json = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "timezone": "UTC""#,
        );
        assert!(matches!(
            CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect_err("must fail"),
            CurrentRuntimeConfigError::Syntax { .. }
        ));
    }

    /// The strongly typed Agent Status subtree keeps the global strict-field
    /// contract: unknown module knobs fail at launch rather than being
    /// ignored.
    #[test]
    fn unknown_agent_status_fields_are_rejected() {
        let json = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "agentStatus": {"time": {"future": true}}""#,
        );
        assert!(matches!(
            CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect_err("must fail"),
            CurrentRuntimeConfigError::Syntax { .. }
        ));
    }

    /// An unsupported schema version fails.
    #[test]
    fn unsupported_schema_version_fails() {
        let json = r#"{
            "schemaVersion": 99,
            "agentId": "a",
            "model": {"model": "p/m"},
            "context": {"reserveTokens": 0, "keepRecentTokens": 0}
        }"#;
        assert!(matches!(
            CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect_err("must fail"),
            CurrentRuntimeConfigError::UnsupportedSchemaVersion { .. }
        ));
    }

    /// The obsolete array-based `mcpServers` schema is not a valid document.
    #[test]
    fn array_based_mcp_servers_are_rejected() {
        let json = r#"{
            "agentId": "a",
            "model": {"model": "p/m"},
            "context": {"reserveTokens": 0, "keepRecentTokens": 0},
            "mcpServers": [
              {"serverId": "s", "transport": {"type": "streamable_http", "endpoint": "https://x"}}
            ]
        }"#;
        assert!(matches!(
            CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect_err("must fail"),
            CurrentRuntimeConfigError::Syntax { .. }
        ));
    }

    /// A present zero summary cap is a configuration error, even when the
    /// context would otherwise never need compaction.
    #[test]
    fn zero_summary_output_cap_is_rejected() {
        let json = MINIMAL.replace(
            r#""keepRecentTokens": 4096"#,
            r#""keepRecentTokens": 0, "summaryOutputCap": 0"#,
        );
        let error = CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect_err("must fail");
        assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
        assert!(
            error
                .to_string()
                .contains("summaryOutputCap must be positive")
        );
    }

    #[test]
    fn subagent_definition_and_admission_domains_are_independent() {
        let json = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "subagents": {"definitions": {"worker": {"description": "worker", "instructionsFile": "worker.md"}}, "main": [], "workflow": ["worker"]}"#,
        );
        let config = CurrentRuntimeConfig::from_jsonc_slice(json.as_bytes()).expect("valid");
        assert!(config.subagents.main.is_empty());
        assert_eq!(config.subagents.workflow.len(), 1);
        assert_eq!(config.subagents.definitions.len(), 1);

        let defined_but_unadmitted = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "subagents": {"definitions": {"worker": {"description": "worker", "instructionsFile": "worker.md"}}, "main": [], "workflow": []}"#,
        );
        assert!(CurrentRuntimeConfig::from_jsonc_slice(defined_but_unadmitted.as_bytes()).is_ok());
    }

    #[test]
    fn unknown_or_duplicate_admission_ids_are_rejected() {
        let unknown = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "subagents": {"definitions": {}, "main": ["missing"], "workflow": []}"#,
        );
        let error =
            CurrentRuntimeConfig::from_jsonc_slice(unknown.as_bytes()).expect_err("unknown");
        assert!(error.to_string().contains("subagents.main"));

        let duplicate = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "subagents": {"definitions": {"worker": {"description": "worker", "instructionsFile": "worker.md"}}, "main": ["worker", "worker"], "workflow": []}"#,
        );
        let error =
            CurrentRuntimeConfig::from_jsonc_slice(duplicate.as_bytes()).expect_err("duplicate");
        assert!(error.to_string().contains("duplicate profile"));
    }

    #[test]
    fn workflow_registration_and_main_exposure_are_separate() {
        let valid = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "workflows": {"definitions": ["review_pr"], "main": []}"#,
        );
        let config = CurrentRuntimeConfig::from_jsonc_slice(valid.as_bytes()).expect("valid");
        assert_eq!(config.workflows.definitions.len(), 1);
        assert!(config.workflows.main.is_empty());

        let unknown = MINIMAL.replace(
            r#""agentId": "agent-a""#,
            r#""agentId": "agent-a", "workflows": {"definitions": ["review_pr"], "main": ["investigate"]}"#,
        );
        let error =
            CurrentRuntimeConfig::from_jsonc_slice(unknown.as_bytes()).expect_err("unknown");
        assert!(error.to_string().contains("workflows.main"));
    }
}
