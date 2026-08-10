//! Canonical tool contracts.
//!
//! These types describe what the runtime knows about tools: their
//! definitions, the calls the current agent issues, the normalized
//! execution results, and the runtime-owned invocation data delivered to
//! executors. The canonical [`ToolDefinition`] is tool-owned and carries the
//! two independent execution policy axes; the provider-neutral compiled
//! [`ModelToolDefinition`] is what actually reaches a model request.
//! Execution scheduling and executors are runtime-owned (M3+); MCP and
//! Python executors reuse the same contract in later milestones. No external
//! SDK type appears here.

use serde::{Deserialize, Serialize};

use crate::runtime::identity::{McpServerId, ToolCallId, ToolId, ToolVersionId};
use crate::runtime::types::CancellationReason;

/// The canonical runtime/tool contract of one registered tool.
///
/// The definition is owned by the tool plane's registry, which pairs it with
/// an executor. The two policy axes are independent:
///
/// - [`ToolExecutionPolicy`] decides who owns the execution: foreground work
///   is attempt-owned and settles before the attempt continues, background
///   work is conversation-owned and detached after accepted dispatch.
/// - [`ToolConcurrencyPolicy`] decides how calls of one batch are scheduled
///   relative to each other.
///
/// The `input_schema` is the original canonical JSON Schema document owned by
/// the tool. The runtime validates it at registration and never mutates it:
/// model-selectable invocation metadata is added only to the compiled
/// model-facing definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Identity of the tool within the capability set.
    pub id: ToolId,
    /// Stable model-facing tool name used when emitting tool calls.
    pub name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// The original canonical JSON Schema document describing the accepted
    /// tool-call arguments. Tool-owned and never mutated by the runtime.
    pub input_schema: serde_json::Value,
    /// Who owns an invocation of this tool: the attempt (foreground) or the
    /// conversation (background). Required: a missing policy is never
    /// silently interpreted.
    pub execution_policy: ToolExecutionPolicy,
    /// How calls of this tool within one batch are scheduled relative to
    /// each other.
    pub concurrency_policy: ToolConcurrencyPolicy,
    /// Replay policy; `Never` is the safe default.
    #[serde(default)]
    pub replay_policy: ToolReplayPolicy,
    /// Where the tool comes from.
    pub origin: ToolOrigin,
}

/// Who owns one execution of a tool.
///
/// This axis decides ownership and settlement, never scheduling:
///
/// - `ForegroundOnly` — every invocation settles before the current agent
///   attempt may continue past that tool result. The execution is
///   attempt-owned and observable attempt cancellation physically reaches
///   it.
/// - `BackgroundOnly` — every invocation is detached from the current
///   attempt after successful background dispatch; the conversation owns the
///   execution.
/// - `ModelSelectable` — the model must explicitly choose foreground or
///   background execution for each invocation through the reserved
///   `__rustx_execution` invocation field.
///
/// Foreground/background is orthogonal to sequential/parallel scheduling
/// ([`ToolConcurrencyPolicy`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionPolicy {
    /// Every invocation settles before the attempt continues; attempt-owned.
    ForegroundOnly,
    /// Every invocation is detached after accepted dispatch; conversation-owned.
    BackgroundOnly,
    /// The model selects foreground or background per invocation.
    ModelSelectable,
}

/// How calls of one tool within one batch are scheduled.
///
/// A `Sequential` invocation is an exclusive scheduling barrier; adjacent
/// `Parallel` invocations may execute concurrently as one group. This axis
/// describes concurrency only, never foreground/background ownership
/// ([`ToolExecutionPolicy`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolConcurrencyPolicy {
    /// Calls execute one at a time in the order issued.
    #[default]
    Sequential,
    /// Multiple adjacent calls may execute concurrently.
    Parallel,
}

/// The resolved execution ownership of one canonical invocation.
///
/// The runtime resolves the effective mode from the tool's declared
/// [`ToolExecutionPolicy`] and (for `ModelSelectable`) the reserved
/// `__rustx_execution` invocation field, and delivers it to the executor as
/// canonical invocation data. No policy resolution happens inside executors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolInvocationMode {
    /// Attempt-owned execution: settles before the attempt continues.
    Foreground,
    /// Conversation-owned execution: detached after accepted dispatch.
    Background,
}

/// Whether the runtime may automatically re-execute a tool after a crash.
///
/// `Never` is the safe default. After a crash the runtime must never blindly
/// replay a tool whose external completion state is unknown: it records an
/// interrupted/unknown result and lets the model decide the next action.
/// Automatic replay is allowed only for tools that explicitly declare
/// themselves idempotent. `Idempotent` is metadata for future recovery
/// policy (M8), not permission to invent replay behavior in this milestone.
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

/// The canonical runtime-owned invocation delivered to a [`ToolExecutor`].
///
/// An invocation contains only runtime-owned execution data: the logical
/// call identity, the tool identity, the model-facing tool name, the
/// resolved execution mode, and the stripped, already-validated business
/// arguments. No provider types and no executor-specific fields (Bash
/// commands, MCP SDK types, Python runtime objects) ever appear here.
///
/// [`ToolExecutor`]: crate::tools::executor::ToolExecutor
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// The logical model-issued tool call identity.
    pub call_id: ToolCallId,
    /// The canonical tool identity.
    pub tool_id: ToolId,
    /// The model-facing tool name at call time.
    pub tool_name: String,
    /// The runtime-resolved execution ownership of this invocation.
    pub mode: ToolInvocationMode,
    /// The stripped business arguments, validated against the canonical
    /// schema. The reserved `__rustx_*` invocation metadata is never present.
    pub arguments: serde_json::Value,
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

/// A bounded structured progress notification of one tool execution.
///
/// Progress is an execution fact, never canonical message history. All
/// fields are optional; an empty `ToolProgress` is a bare tick. The progress
/// message text is bounded by [`MAX_PROGRESS_MESSAGE_BYTES`].
///
/// [`MAX_PROGRESS_MESSAGE_BYTES`]: crate::tools::limits::MAX_PROGRESS_MESSAGE_BYTES
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ToolProgress {
    /// A short human-readable progress message, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Completed units, when a total is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<u64>,
    /// Total units, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
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

/// The provider-neutral compiled model-facing tool definition.
///
/// One model request receives these compiled definitions, never the
/// canonical [`ToolDefinition`]: runtime execution, replay, and origin
/// policy never reach provider adapters. For a `ModelSelectable` tool the
/// compiled `input_schema` is the canonical schema decorated with the
/// reserved runtime-owned `__rustx_execution` invocation field; the stored
/// canonical schema remains untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelToolDefinition {
    /// Identity of the tool within the capability set.
    pub id: ToolId,
    /// Stable model-facing tool name.
    pub name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// The compiled model-facing JSON Schema, including the reserved
    /// runtime invocation metadata for `ModelSelectable` tools.
    pub input_schema: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::{
        ToolCall, ToolCallStart, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
        ToolExecutionStatus, ToolReplayPolicy, TruncationState,
    };
    use crate::runtime::identity::{McpServerId, ToolCallId, ToolId};
    use serde_json::json;

    /// The safe replay default is `Never`.
    #[test]
    fn replay_policy_default_is_never() {
        assert_eq!(ToolReplayPolicy::default(), ToolReplayPolicy::Never);
    }

    /// The conservative concurrency default is sequential, never parallel.
    #[test]
    fn concurrency_default_is_sequential() {
        use super::ToolConcurrencyPolicy;
        assert_eq!(
            ToolConcurrencyPolicy::default(),
            ToolConcurrencyPolicy::Sequential
        );
    }

    /// A missing execution policy must not silently deserialize.
    #[test]
    fn tool_definition_requires_explicit_execution_policy() {
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
            "missing execution_policy must fail deserialization"
        );
    }

    /// A definition with explicit policies round-trips.
    #[test]
    fn tool_definition_with_explicit_policies_round_trips() {
        use super::{ToolConcurrencyPolicy, ToolInvocationMode};
        let definition = ToolDefinition {
            id: ToolId::new("tool-bash"),
            name: "bash".to_owned(),
            description: "Run a shell command".to_owned(),
            input_schema: json!({"type": "object"}),
            execution_policy: ToolExecutionPolicy::ModelSelectable,
            concurrency_policy: ToolConcurrencyPolicy::Parallel,
            replay_policy: ToolReplayPolicy::Never,
            origin: crate::tools::types::ToolOrigin::Mcp {
                server_id: McpServerId::new("mcp-fs"),
            },
        };
        let json = serde_json::to_string(&definition).expect("serialize definition");
        let decoded: ToolDefinition = serde_json::from_str(&json).expect("deserialize definition");
        assert_eq!(decoded, definition);
        let invocation_mode =
            serde_json::from_str::<ToolInvocationMode>("\"foreground\"").expect("mode");
        assert_eq!(invocation_mode, ToolInvocationMode::Foreground);
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
