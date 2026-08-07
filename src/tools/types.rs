//! Canonical tool contracts.
//!
//! These types describe what the runtime knows about tools: their
//! definitions, the calls the current agent issues, and the normalized
//! execution results. Execution is declarative metadata only in M1; tool
//! scheduling, executors, MCP, and Python tool execution are later
//! milestones. No external SDK type appears here.

use serde::{Deserialize, Serialize};

use crate::runtime::identity::{McpServerId, ToolCallId, ToolId, ToolVersionId};
use crate::runtime::types::CancellationReason;

/// A runtime-owned tool definition.
///
/// The `input_schema` is an arbitrary JSON Schema document that the runtime
/// passes through to model adapters; the runtime does not interpret it in M1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Identity of the tool within the capability set.
    pub id: ToolId,
    /// Stable tool name used when emitting tool calls.
    pub name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// JSON Schema document describing the accepted `ToolCall` arguments.
    pub input_schema: serde_json::Value,
    /// Declarative execution mode; scheduling is not implemented in M1.
    /// Required: a missing mode is never silently interpreted as parallel.
    pub execution_mode: ToolExecutionMode,
    /// Replay policy; `Never` is the safe default.
    #[serde(default)]
    pub replay_policy: ToolReplayPolicy,
    /// Where the tool comes from.
    pub origin: ToolOrigin,
}

/// Whether a batch of tool calls may run in parallel.
///
/// This is declarative metadata only in M1; no scheduling is implemented.
/// The explicit `Default` is `Sequential`: when a mode is not stated, the
/// runtime must not assume parallel execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionMode {
    /// Calls execute one at a time in the order issued.
    #[default]
    Sequential,
    /// Multiple calls may execute concurrently.
    Parallel,
}

/// Whether the runtime may automatically re-execute a tool after a crash.
///
/// `Never` is the safe default. After a crash the runtime must never blindly
/// replay a tool whose external completion state is unknown: it records an
/// interrupted/unknown result and lets the model decide the next action.
/// Automatic replay is allowed only for tools that explicitly declare
/// themselves idempotent. M1 defines the contract; recovery is a later
/// milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolReplayPolicy {
    /// Never automatically re-execute after an unknown outcome.
    #[default]
    Never,
    /// Re-execution is safe because repeated invocation has no side effects.
    Idempotent,
}

/// Where a tool comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOrigin {
    /// A tool built into the runtime. Platform communication tools such as
    /// future Fleet messaging are represented as built-in tools as well.
    Builtin,
    /// A tool served by a bound MCP server.
    Mcp {
        /// Identity of the MCP server exposing the tool.
        server_id: McpServerId,
    },
    /// A custom Python tool at an immutable version.
    Python {
        /// Identity of the immutable tool version to execute.
        tool_version_id: ToolVersionId,
    },
}

/// One tool call issued by the current agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Identity of this specific call, referenced by the matching
    /// `ToolMessageBlock` and tool-result events.
    pub id: ToolCallId,
    /// Identity of the tool being called within the capability set.
    pub tool_id: ToolId,
    /// Name of the tool at call time, sufficient for resolution together with
    /// `tool_id`.
    pub name: String,
    /// Arbitrary JSON arguments for the tool call.
    pub arguments: serde_json::Value,
}

/// The data known when a tool call starts, before its arguments stream.
///
/// Streaming protocols expose the call identity, tool identity, and name
/// before any argument JSON is available. The fully assembled `ToolCall`
/// (including `arguments`) is emitted only when the call completes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallStart {
    /// Identity of this specific call.
    pub id: ToolCallId,
    /// Identity of the tool being called within the capability set.
    pub tool_id: ToolId,
    /// Name of the tool at call time.
    pub name: String,
}

/// The normalized outcome of one tool execution.
///
/// `ToolMessageBlock` composes this type instead of duplicating its fields,
/// keeping one source of truth for tool results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    /// Typed execution status, including interrupted/unknown outcomes.
    pub status: ToolExecutionStatus,
    /// Model-facing result content.
    #[serde(default)]
    pub content: Vec<ToolResultContent>,
    /// Execution duration in integer milliseconds (stable for persistence).
    pub duration_ms: u64,
    /// Process exit code where the tool executed a process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Durable artifact/file references produced by the execution.
    #[serde(default)]
    pub artifacts: Vec<crate::message::content::FileReference>,
    /// Truncation metadata where output was truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationState>,
}

/// Typed execution status of a tool call.
///
/// Success and failure are not the only states: cancellation, timeout, and
/// interrupted execution (the runtime restarted while the call was in flight,
/// so the actual external outcome is unknown) are distinct and must never be
/// silently collapsed into success or generic error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolExecutionStatus {
    /// The tool completed successfully.
    Success,
    /// The tool failed with an error message.
    Failed {
        /// Human-readable error message.
        error: String,
    },
    /// The execution was cancelled.
    Cancelled {
        /// Why the execution was cancelled.
        reason: CancellationReason,
    },
    /// The execution exceeded its time budget.
    TimedOut,
    /// The execution was interrupted (for example by a runtime restart) and
    /// the actual external outcome is unknown.
    Interrupted,
}

/// Truncation metadata for tool output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruncationState {
    /// Whether the result content was truncated.
    pub truncated: bool,
    /// Size of the untruncated output in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_bytes: Option<u64>,
}

/// A content block inside a tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    /// Plain text output.
    Text(crate::message::content::TextBlock),
    /// Structured output preserved as arbitrary JSON.
    Json {
        /// The structured tool output value.
        value: serde_json::Value,
    },
    /// A file reference produced by the tool.
    File(crate::message::content::FileReference),
    /// An image reference produced by the tool.
    Image(crate::message::content::ImageReference),
}

#[cfg(test)]
mod tests {
    use super::{
        ToolCall, ToolCallStart, ToolDefinition, ToolExecutionMode, ToolExecutionResult,
        ToolExecutionStatus, ToolReplayPolicy, TruncationState,
    };
    use crate::runtime::identity::{McpServerId, ToolCallId, ToolId};
    use serde_json::json;

    /// The safe replay default is `Never`.
    #[test]
    fn replay_policy_default_is_never() {
        assert_eq!(ToolReplayPolicy::default(), ToolReplayPolicy::Never);
    }

    /// The conservative execution-mode default is sequential, never parallel.
    #[test]
    fn execution_mode_default_is_sequential() {
        assert_eq!(ToolExecutionMode::default(), ToolExecutionMode::Sequential);
    }

    /// A missing execution mode must not silently deserialize as parallel.
    #[test]
    fn tool_definition_requires_explicit_execution_mode() {
        let json = r#"{
            "id": "tool-bash",
            "name": "bash",
            "description": "Run a shell command",
            "input_schema": {"type": "object"},
            "replay_policy": "never",
            "origin": "builtin"
        }"#;
        let result = serde_json::from_str::<ToolDefinition>(json);
        assert!(
            result.is_err(),
            "missing execution_mode must fail deserialization"
        );
    }

    /// An explicitly declared execution mode round-trips.
    #[test]
    fn tool_definition_with_explicit_execution_mode_round_trips() {
        let definition = ToolDefinition {
            id: ToolId::new("tool-bash"),
            name: "bash".to_owned(),
            description: "Run a shell command".to_owned(),
            input_schema: json!({"type": "object"}),
            execution_mode: ToolExecutionMode::Parallel,
            replay_policy: ToolReplayPolicy::Never,
            origin: crate::tools::types::ToolOrigin::Mcp {
                server_id: McpServerId::new("mcp-fs"),
            },
        };
        let json = serde_json::to_string(&definition).expect("serialize definition");
        let decoded: ToolDefinition = serde_json::from_str(&json).expect("deserialize definition");
        assert_eq!(decoded, definition);
    }

    /// Tool call starts carry only the data known before arguments stream.
    #[test]
    fn tool_call_start_carries_only_known_identity() {
        let start = ToolCallStart {
            id: ToolCallId::new("call_01"),
            tool_id: ToolId::new("tool-bash"),
            name: "bash".to_owned(),
        };
        let json = serde_json::to_string(&start).expect("serialize start");
        let decoded: ToolCallStart = serde_json::from_str(&json).expect("deserialize start");
        assert_eq!(decoded, start);
        let value = serde_json::to_value(&start).expect("serialize start value");
        assert!(
            value.get("arguments").is_none(),
            "no arguments exist at start"
        );
    }

    /// Tool calls round-trip arbitrary JSON arguments untouched.
    #[test]
    fn tool_call_round_trips_arbitrary_json_arguments() {
        let call = ToolCall {
            id: ToolCallId::new("call_01"),
            tool_id: ToolId::new("tool-bash"),
            name: "bash".to_owned(),
            arguments: json!({
                "command": "ls -la",
                "options": { "timeout_seconds": 30, "flags": [1, 2, 3] },
            }),
        };
        let json = serde_json::to_string(&call).expect("serialize call");
        let decoded: ToolCall = serde_json::from_str(&json).expect("deserialize call");
        assert_eq!(decoded, call);
        assert_eq!(
            decoded.arguments,
            json!({"command": "ls -la", "options": {"timeout_seconds": 30, "flags": [1, 2, 3]}})
        );
    }

    /// Interrupted/unknown execution is a distinct, round-trippable status.
    #[test]
    fn interrupted_result_round_trip() {
        let result = ToolExecutionResult {
            status: ToolExecutionStatus::Interrupted,
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: Some(TruncationState {
                truncated: false,
                original_bytes: None,
            }),
        };
        let json = serde_json::to_string(&result).expect("serialize result");
        let decoded: ToolExecutionResult = serde_json::from_str(&json).expect("deserialize result");
        assert_eq!(decoded, result);
        assert_eq!(decoded.status, ToolExecutionStatus::Interrupted);
    }

    /// Execution statuses serialize with stable explicit discriminators.
    #[test]
    fn execution_status_discriminators_are_stable() {
        let status = ToolExecutionStatus::Cancelled {
            reason: crate::runtime::types::CancellationReason::UserRequested,
        };
        let value = serde_json::to_value(&status).expect("serialize status");
        assert_eq!(value["type"], "cancelled");
        assert_eq!(value["reason"], "user_requested");
    }
}
