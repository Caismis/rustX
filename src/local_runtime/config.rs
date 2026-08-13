//! The bounded explicit local session configuration (Issue #42).
//!
//! This is deliberately **not** M10 configuration discovery. There is no
//! global/project search path, no precedence chain, no named product
//! profile, no interactive editor, no credential store, and no manifest
//! migration layer: the local runtime is handed explicit file paths and
//! reads exactly those files.
//!
//! Unknown fields are rejected everywhere. A typo must fail startup loudly
//! rather than silently changing runtime semantics.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::context::SessionContextPolicy;
use crate::model::session::SessionModelConfig;
use crate::runtime::identity::{AgentId, ConversationId, McpServerId};
use crate::tools::environment::{ToolEnvironment, ToolEnvironmentError};
use crate::tools::mcp::{McpServerConfig, McpTransportConfig};
use crate::tools::native::NativeToolPolicies;
use crate::tools::types::{ToolConcurrencyPolicy, ToolExecutionPolicy, ToolInvocationPolicy};

/// The only local session schema version this runtime accepts.
pub const LOCAL_SESSION_SCHEMA_VERSION: u32 = 1;

/// The explicit local session configuration of one conversation runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalSessionConfig {
    /// The session schema version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The conversation identity this process owns.
    pub conversation_id: ConversationId,
    /// The agent executed by attempts of this conversation.
    pub agent_id: AgentId,
    /// The initial authoritative session model configuration.
    pub model: SessionModelConfig,
    /// The conversation IANA timezone used by the temporal Agent Status
    /// section. The process/system local timezone is never consulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<Tz>,
    /// The static session-owned context policy.
    pub context: ContextPolicyDocument,
    /// The configured MCP servers of the capability coordinator.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerDocument>,
    /// The per-tool execution policies of the native tool plane.
    #[serde(default)]
    pub native_tools: NativeToolPoliciesDocument,
    /// The base authorized tool environment of the conversation.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

const fn default_schema_version() -> u32 {
    LOCAL_SESSION_SCHEMA_VERSION
}

impl LocalSessionConfig {
    /// Parses and validates a session configuration from JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns [`LocalSessionConfigError::Syntax`] for malformed JSON or
    /// unknown fields, and a specific validation error otherwise.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, LocalSessionConfigError> {
        let config: Self =
            serde_json::from_slice(bytes).map_err(|error| LocalSessionConfigError::Syntax {
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
    pub fn validate(&self) -> Result<(), LocalSessionConfigError> {
        if self.schema_version != LOCAL_SESSION_SCHEMA_VERSION {
            return Err(LocalSessionConfigError::UnsupportedSchemaVersion {
                supported: LOCAL_SESSION_SCHEMA_VERSION,
                found: self.schema_version,
            });
        }
        if self.conversation_id.as_str().is_empty() || self.agent_id.as_str().is_empty() {
            return Err(LocalSessionConfigError::Invalid {
                detail: "conversationId and agentId must be non-empty".to_owned(),
            });
        }
        if self.context.keep_recent_tokens > 0 && self.context.summary_output_cap == Some(0) {
            return Err(LocalSessionConfigError::Invalid {
                detail: "context.summaryOutputCap must be positive when present".to_owned(),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for server in &self.mcp_servers {
            if !seen.insert(server.server_id.clone()) {
                return Err(LocalSessionConfigError::Invalid {
                    detail: format!("duplicate MCP server identity {}", server.server_id),
                });
            }
        }
        Ok(())
    }

    /// The static session context policy this configuration expresses.
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
    /// Returns [`LocalSessionConfigError::Environment`] when an entry is
    /// malformed or claims a runtime-owned key.
    pub fn tool_environment(&self) -> Result<ToolEnvironment, LocalSessionConfigError> {
        ToolEnvironment::from_authorized(
            self.environment
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        )
        .map_err(LocalSessionConfigError::Environment)
    }

    /// The MCP server bindings this configuration expresses.
    #[must_use]
    pub fn mcp_server_configs(&self) -> Vec<McpServerConfig> {
        self.mcp_servers
            .iter()
            .map(McpServerDocument::to_config)
            .collect()
    }
}

/// The static session-owned context policy document.
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

/// The per-tool execution policies of the native tool plane.
///
/// The runtime intrinsic `background_task` is deliberately outside this set:
/// its foreground-only sequential policy is enforced by the registry itself.
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
}

impl InvocationPolicyDocument {
    /// The runtime policy this document expresses.
    #[must_use]
    pub const fn to_policy(self) -> ToolInvocationPolicy {
        ToolInvocationPolicy::new(self.execution.to_policy(), self.concurrency.to_policy())
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

/// One configured MCP server of the local session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerDocument {
    /// The stable server identity.
    pub server_id: McpServerId,
    /// The configured transport.
    pub transport: McpTransportDocument,
    /// One origin-independent policy for every tool of this server.
    #[serde(default)]
    pub policy: InvocationPolicyDocument,
}

impl McpServerDocument {
    /// The runtime binding this document expresses.
    #[must_use]
    pub fn to_config(&self) -> McpServerConfig {
        McpServerConfig {
            server_id: self.server_id.clone(),
            transport: self.transport.to_config(),
            policy: self.policy.to_policy(),
        }
    }
}

/// One configured MCP transport of the local session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpTransportDocument {
    /// A stdio server launched with an explicit environment and a
    /// workspace-relative working directory.
    Stdio {
        /// Executable path or explicit executable name.
        program: String,
        /// Program arguments.
        #[serde(default)]
        args: Vec<String>,
        /// Workspace-relative working directory; absent means workspace
        /// root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        /// The explicit child environment.
        #[serde(default)]
        environment: BTreeMap<String, String>,
    },
    /// A stateless Streamable HTTP endpoint with explicit headers.
    StreamableHttp {
        /// The endpoint URL.
        endpoint: String,
        /// Explicit static request headers.
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

impl McpTransportDocument {
    /// The runtime transport this document expresses.
    #[must_use]
    pub fn to_config(&self) -> McpTransportConfig {
        match self {
            Self::Stdio {
                program,
                args,
                cwd,
                environment,
            } => McpTransportConfig::Stdio {
                program: program.clone(),
                args: args.clone(),
                cwd: cwd.clone(),
                environment: environment.clone(),
            },
            Self::StreamableHttp { endpoint, headers } => McpTransportConfig::StreamableHttp {
                endpoint: endpoint.clone(),
                headers: headers.clone(),
            },
        }
    }
}

/// A local session configuration failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSessionConfigError {
    /// The document is not valid JSON for the session schema.
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

impl std::fmt::Display for LocalSessionConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax { detail } => write!(f, "malformed local session config: {detail}"),
            Self::UnsupportedSchemaVersion { supported, found } => write!(
                f,
                "unsupported local session schemaVersion {found}; this runtime speaks {supported}"
            ),
            Self::Invalid { detail } => write!(f, "invalid local session config: {detail}"),
            Self::Environment(error) => {
                write!(f, "invalid base tool environment: {error:?}")
            }
        }
    }
}

impl std::error::Error for LocalSessionConfigError {}

#[cfg(test)]
mod tests {
    use super::{LocalSessionConfig, LocalSessionConfigError};

    const MINIMAL: &str = r#"{
        "conversationId": "conv-1",
        "agentId": "agent-a",
        "model": {"model": "p/m"},
        "context": {"reserveTokens": 1024, "keepRecentTokens": 4096}
    }"#;

    /// The minimal configuration parses and derives its policy pieces.
    #[test]
    fn minimal_configuration_parses() {
        let config = LocalSessionConfig::from_json_slice(MINIMAL.as_bytes()).expect("valid");
        assert_eq!(config.conversation_id.as_str(), "conv-1");
        assert_eq!(config.context_policy().reserve_tokens, 1024);
        assert!(config.mcp_server_configs().is_empty());
        assert!(
            config
                .tool_environment()
                .expect("environment")
                .authorized_entries()
                .is_empty()
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
            LocalSessionConfig::from_json_slice(json.as_bytes()).expect_err("must fail"),
            LocalSessionConfigError::Syntax { .. }
        ));
    }

    /// An unsupported schema version fails.
    #[test]
    fn unsupported_schema_version_fails() {
        let json = r#"{
            "schemaVersion": 99,
            "conversationId": "c", "agentId": "a",
            "model": {"model": "p/m"},
            "context": {"reserveTokens": 0, "keepRecentTokens": 0}
        }"#;
        assert!(matches!(
            LocalSessionConfig::from_json_slice(json.as_bytes()).expect_err("must fail"),
            LocalSessionConfigError::UnsupportedSchemaVersion { .. }
        ));
    }

    /// Duplicate MCP server identities fail.
    #[test]
    fn duplicate_mcp_servers_fail() {
        let json = r#"{
            "conversationId": "c", "agentId": "a",
            "model": {"model": "p/m"},
            "context": {"reserveTokens": 0, "keepRecentTokens": 0},
            "mcpServers": [
              {"serverId": "s", "transport": {"type": "streamable_http", "endpoint": "https://x"}},
              {"serverId": "s", "transport": {"type": "streamable_http", "endpoint": "https://y"}}
            ]
        }"#;
        let error = LocalSessionConfig::from_json_slice(json.as_bytes()).expect_err("must fail");
        assert!(error.to_string().contains("duplicate MCP server"));
    }
}
