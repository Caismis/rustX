//! The bounded explicit current runtime/project configuration (Issue #96).
//!
//! This is deliberately a current-runtime input, never durable Session state.
//! It is read for every process start, including resume, so changing MCP,
//! Skill, Tool, environment, context, timezone, or agent settings takes
//! effect without rewriting the Session catalog.
//!
//! Unknown fields are rejected everywhere. A typo must fail startup loudly
//! rather than silently changing runtime semantics.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::context::SessionContextPolicy;
use crate::model::session::SessionModelConfig;
use crate::runtime::ApprovalMode;
use crate::runtime::identity::{AgentId, McpServerId};
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentError};
use crate::tools::mcp::{McpServerBinding, McpServerBindings, McpTransportConfig};
use crate::tools::native::NativeToolPolicies;
use crate::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationPolicy};

/// The only current runtime configuration schema version this runtime accepts.
pub const CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 2;

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
    /// The current IANA timezone used by the temporal Agent Status
    /// section. The process/system local timezone is never consulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<Tz>,
    /// The current runtime context policy.
    pub context: ContextPolicyDocument,
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
}

const fn default_schema_version() -> u32 {
    CURRENT_RUNTIME_SCHEMA_VERSION
}

fn default_tools() -> Vec<String> {
    [
        "background_task",
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

impl CurrentRuntimeConfig {
    /// Parses and validates current runtime configuration from JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns [`CurrentRuntimeConfigError::Syntax`] for malformed JSON or
    /// unknown fields, and a specific validation error otherwise.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, CurrentRuntimeConfigError> {
        let config: Self =
            serde_json::from_slice(bytes).map_err(|error| CurrentRuntimeConfigError::Syntax {
                detail: error.to_string(),
            })?;
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
/// The runtime intrinsics `background_task` and `ask_user` are deliberately
/// outside this set: their fixed foreground-only, sequential, approval-never
/// policies are enforced by the registry itself.
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
    use super::{CurrentRuntimeConfig, CurrentRuntimeConfigError};

    const MINIMAL: &str = r#"{
        "agentId": "agent-a",
        "model": {"model": "p/m"},
        "context": {"reserveTokens": 1024, "keepRecentTokens": 4096}
    }"#;

    /// The minimal configuration parses and derives its policy pieces.
    #[test]
    fn minimal_configuration_parses() {
        let config = CurrentRuntimeConfig::from_json_slice(MINIMAL.as_bytes()).expect("valid");
        assert_eq!(config.approval_mode, crate::runtime::ApprovalMode::Policy);
        assert_eq!(config.context_policy().reserve_tokens, 1024);
        assert!(config.mcp_bindings().expect("bindings").is_empty());
        assert!(
            config
                .tool_environment()
                .expect("environment")
                .authorized_entries()
                .is_empty()
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
        let config = CurrentRuntimeConfig::from_json_slice(json.as_bytes()).expect("valid");
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
            CurrentRuntimeConfig::from_json_slice(json.as_bytes()).expect_err("must fail"),
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
            CurrentRuntimeConfig::from_json_slice(json.as_bytes()).expect_err("must fail"),
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
            CurrentRuntimeConfig::from_json_slice(json.as_bytes()).expect_err("must fail"),
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
        let error = CurrentRuntimeConfig::from_json_slice(json.as_bytes()).expect_err("must fail");
        assert!(matches!(error, CurrentRuntimeConfigError::Invalid { .. }));
        assert!(
            error
                .to_string()
                .contains("summaryOutputCap must be positive")
        );
    }
}
