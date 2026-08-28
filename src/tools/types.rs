//! Canonical tool contracts.
//!
//! These types describe what the runtime knows about tools: their
//! definitions, the calls the current agent issues, the normalized
//! execution results, and the runtime-owned invocation data delivered to
//! executors. The canonical [`ToolDefinition`] is tool-owned and carries the
//! three independent execution policy axes; the provider-neutral compiled
//! [`ModelToolDefinition`] is what actually reaches a model request.
//! Execution scheduling and executors are runtime-owned (M3+); native, MCP,
//! and Python executors reuse the same contract. No external
//! SDK type appears here.

use serde::{Deserialize, Serialize};

use crate::runtime::identity::{McpServerId, ToolCallId, ToolId, ToolVersionId};
use crate::runtime::types::CancellationReason;

/// The canonical runtime/tool contract of one registered tool.
///
/// The definition is owned by the tool plane's registry, which pairs it with
/// an executor. The three policy axes are independent:
///
/// - [`ToolExecutionPolicy`] decides who owns the execution: foreground work
///   is attempt-owned and settles before the attempt continues, background
///   work is conversation-owned and detached after accepted dispatch.
/// - [`ToolConcurrencyPolicy`] decides how calls of one batch are scheduled
///   relative to each other.
/// - [`ToolApprovalPolicy`] decides whether an eligible invocation needs a
///   native human approval before the executor starts.
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
    /// Whether execution requires a native approval interaction.
    #[serde(default)]
    pub approval_policy: ToolApprovalPolicy,
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
///   background execution for each invocation through the required
///   `execution_mode` invocation field. That field is reserved by the
///   runtime only under this policy; registration rejects a `ModelSelectable`
///   tool whose canonical schema already defines it.
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

/// The three origin-independent policy axes attached to an external tool
/// configuration. Native tools use the concrete `NativeToolPolicies` table;
/// MCP servers and Python manifests carry this value directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocationPolicy {
    /// Foreground/background ownership policy.
    pub execution: ToolExecutionPolicy,
    /// In-batch scheduling policy.
    pub concurrency: ToolConcurrencyPolicy,
    /// Whether a human approval is required before execution.
    #[serde(default)]
    pub approval: ToolApprovalPolicy,
}

impl Default for ToolInvocationPolicy {
    fn default() -> Self {
        Self::new(
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
            ToolApprovalPolicy::Never,
        )
    }
}

impl ToolInvocationPolicy {
    /// Creates a policy from the three canonical axes.
    #[must_use]
    pub const fn new(
        execution: ToolExecutionPolicy,
        concurrency: ToolConcurrencyPolicy,
        approval: ToolApprovalPolicy,
    ) -> Self {
        Self {
            execution,
            concurrency,
            approval,
        }
    }
}

/// Whether a resolved tool invocation requires a native approval interaction.
///
/// Denial is intentionally not part of this policy. A tool that is not
/// eligible belongs to availability/authorization and is rejected before
/// approval; HITL only decides whether an otherwise eligible invocation needs
/// a human decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalPolicy {
    /// Execute without an approval interaction.
    #[default]
    Never,
    /// Publish an approval interaction before execution.
    Always,
}

/// The resolved execution ownership of one canonical invocation.
///
/// The runtime resolves the effective mode from the tool's declared
/// [`ToolExecutionPolicy`] and (for `ModelSelectable`) the required
/// `execution_mode` invocation field, and delivers it to the executor as
/// canonical invocation data. The field is consumed before business-argument
/// validation and never reaches an executor. No policy resolution happens
/// inside executors.
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
/// themselves idempotent. `Idempotent` is metadata for the future #12
/// recovery policy, not permission to invent replay behavior in M8.
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
    ///
    /// This content is TOOL-OWNED: [`ToolResultContent::Json`] is arbitrary
    /// tool-owned structured data, and the runtime never infers semantics
    /// from its property names. rustX reserves no ordinary JSON field names;
    /// runtime-owned facts live in the typed fields of this struct.
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
    /// Runtime-owned managed textual-output continuation metadata: where
    /// the complete — or honestly partial — textual output of this result
    /// lives in the conversation's managed tool-output store (Issue #86).
    /// Absent for results whose output fits the model-facing content.
    ///
    /// This is the one typed source of truth for complete-vs-partial
    /// managed output; producers never encode these facts as magic
    /// properties of tool-owned JSON, and generic runtime publication code
    /// consumes only this typed field, never arbitrary JSON keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_output: Option<ManagedOutputContinuation>,
}

/// Runtime-owned managed textual-output continuation metadata of one tool
/// result (Issue #86): where the complete — or honestly partial — textual
/// output of the execution lives in the conversation's managed tool-output
/// store.
///
/// This is rustX runtime metadata, explicitly typed and separate from
/// arbitrary tool-owned structured content (`ToolResultContent::Json`). It
/// is not a semantic artifact, not a `FileReference`, and not a File
/// modality: textual output stays textual. The locator is an advisory
/// model-facing absolute path inside the read-only managed tool-output
/// root — it is a locator, never filesystem authority.
///
/// The two managed-output lifecycles both use this type: a foreground
/// result references its lazy result spill (`results/result_N.txt`) only
/// when the complete representation crossed the shared preview threshold,
/// while a background result references its dispatch-allocated live-output
/// file (`tasks/exec_N.output`). A size-only cutoff is never a semantic tool
/// failure; `Partial`/`Unavailable` make output-storage failure explicit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManagedOutputContinuation {
    /// The complete textual output of the result is retained at the
    /// absolute managed-output locator; the bounded result content is a
    /// preview of it.
    Complete {
        /// The absolute locator inside the managed tool-output root.
        locator: std::path::PathBuf,
    },
    /// Output storage did not settle completely: the locator holds only
    /// partial output and must never be presented as the complete output.
    Partial {
        /// The absolute locator inside the managed tool-output root.
        locator: std::path::PathBuf,
        /// The output-storage failure diagnostic. Advisory only: it is
        /// bounded whenever the continuation is rendered.
        diagnostic: String,
    },
    /// No managed-output file exists for the result — output storage
    /// failed before or without allocation — so the bounded result content
    /// is the only record.
    Unavailable {
        /// The output-storage failure diagnostic. Advisory only: it is
        /// bounded whenever the continuation is rendered.
        diagnostic: String,
    },
}

impl ManagedOutputContinuation {
    /// The fixed guidance sentence of a complete-output continuation.
    const COMPLETE_GUIDANCE: &'static str =
        "Use Read or Grep with this absolute path to inspect the complete output.";

    /// The fixed guidance sentence of a partial-output continuation.
    const PARTIAL_GUIDANCE: &'static str =
        "The output storage did not complete; this file does NOT hold the complete output.";

    /// The model-facing textual rendering of this continuation.
    ///
    /// This is the ONE rendering of runtime-owned continuation metadata,
    /// used both by a producer that presents its own continuation inside
    /// its tool-owned result content (a foreground result the model
    /// receives directly) and by the generic background terminal
    /// publication (which appends it as the structurally retained
    /// continuation section). The absolute locator and the fixed guidance
    /// are essential continuation state and always survive; the advisory
    /// diagnostic is bounded to
    /// [`MAX_OUTPUT_CONTINUATION_DIAGNOSTIC_BYTES`](crate::tools::limits::MAX_OUTPUT_CONTINUATION_DIAGNOSTIC_BYTES)
    /// so an arbitrary diagnostic can never make canonical history
    /// unbounded.
    #[must_use]
    pub fn render(&self) -> String {
        let bound_diagnostic = |diagnostic: &String| {
            crate::tools::limits::bound_utf8_text(
                diagnostic.clone(),
                crate::tools::limits::MAX_OUTPUT_CONTINUATION_DIAGNOSTIC_BYTES,
            )
        };
        match self {
            Self::Complete { locator } => format!(
                "Complete output: {}\n{}",
                locator.display(),
                Self::COMPLETE_GUIDANCE
            ),
            Self::Partial {
                locator,
                diagnostic,
            } => format!(
                "Partial output only: {}\n{}\nDiagnostic: {}",
                locator.display(),
                Self::PARTIAL_GUIDANCE,
                bound_diagnostic(diagnostic)
            ),
            Self::Unavailable { diagnostic } => format!(
                "The output capture did not complete; the bounded result content is the only \
                 record and no complete output file is available.\nDiagnostic: {}",
                bound_diagnostic(diagnostic)
            ),
        }
    }
}

/// The semantic phase at which a tool call was cancelled.
///
/// This is a closed, provider-independent fact owned by the canonical tool
/// result contract and shared by foreground and background runtime-owned
/// results. The foreground Agent Loop selects it from its per-call executor
/// start frontier; the background registry selects it from its detached-runner
/// frontier. Executors and clients never infer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCancellationPhase {
    /// The accepted call had execution authority, but its owner's executor
    /// start frontier was never crossed.
    BeforeStart,
    /// The owner's executor start frontier was crossed and cancellation won
    /// before normal completion. This does not promise rollback or absence of
    /// side effects.
    DuringExecution,
}

/// Typed execution status of a tool call.
///
/// Success and failure are not the only states: policy denial, cancellation,
/// timeout, and interrupted execution (the runtime restarted while the call
/// was in flight, so the actual external outcome is unknown) are distinct and
/// must never be silently collapsed into success or generic error.
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
    /// The pre-tool policy denied the already-resolved invocation before an
    /// executor future was created.
    Denied {
        /// The policy or human-readable approval reason.
        reason: String,
    },
    /// The execution was cancelled.
    Cancelled {
        /// Why the execution was cancelled.
        reason: CancellationReason,
        /// Whether cancellation won before executor start or while execution
        /// was already in flight.
        phase: ToolCancellationPhase,
    },
    /// The execution exceeded its time budget.
    TimedOut,
    /// The execution was interrupted (for example by a runtime restart) and
    /// the actual external outcome is unknown.
    Interrupted,
}

impl ToolExecutionStatus {
    /// Renders the cancellation fact for inclusion in a model-facing tool
    /// result. The typed status remains the source of truth; this text is
    /// presentation only and is never parsed back into a phase or reason.
    #[must_use]
    pub fn model_facing_text(&self) -> Option<String> {
        let Self::Cancelled { reason, phase } = self else {
            return None;
        };

        let reason = match reason {
            CancellationReason::UserRequested => "user_requested",
            CancellationReason::RuntimeShutdown => "runtime_shutdown",
            CancellationReason::ParentCancelled => "parent_cancelled",
        };
        let phase = match phase {
            ToolCancellationPhase::BeforeStart => {
                "rustX did not start execution of this tool call."
            }
            ToolCancellationPhase::DuringExecution => {
                "Execution had already started, but cancellation occurred before normal completion. Partial side effects may have occurred."
            }
        };
        Some(format!(
            "Tool call was cancelled (reason: {reason}). {phase}"
        ))
    }
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolProgress {
    /// A short human-readable progress message, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Completed units, when a total is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<f64>,
    /// Total units, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
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
/// required runtime-owned `execution_mode` invocation field and the compiled
/// `description` carries the runtime-owned reminder that the field is
/// mandatory; the stored canonical definition remains untouched.
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
        ToolApprovalPolicy, ToolCall, ToolCallStart, ToolCancellationPhase, ToolDefinition,
        ToolExecutionPolicy, ToolExecutionResult, ToolExecutionStatus, ToolReplayPolicy,
        TruncationState,
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
            approval_policy: ToolApprovalPolicy::Always,
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
            managed_output: None,
        };
        let json = serde_json::to_string(&result).expect("serialize result");
        let decoded: ToolExecutionResult = serde_json::from_str(&json).expect("deserialize result");
        assert_eq!(decoded, result);
        assert_eq!(decoded.status, ToolExecutionStatus::Interrupted);
    }

    /// Execution statuses serialize with stable explicit discriminators.
    #[test]
    fn issue136_execution_status_discriminators_are_stable() {
        let status = ToolExecutionStatus::Cancelled {
            reason: crate::runtime::types::CancellationReason::UserRequested,
            phase: ToolCancellationPhase::DuringExecution,
        };
        let value = serde_json::to_value(&status).expect("serialize status");
        assert_eq!(value["type"], "cancelled");
        assert_eq!(value["reason"], "user_requested");
        assert_eq!(value["phase"], "during_execution");
    }

    /// Both cancellation axes are closed, independent, and useful to the
    /// model without relying on prose as a serialization mechanism.
    #[test]
    fn issue136_cancellation_reason_and_phase_render_independently() {
        use crate::runtime::types::CancellationReason;

        for (reason, reason_label) in [
            (CancellationReason::UserRequested, "user_requested"),
            (CancellationReason::RuntimeShutdown, "runtime_shutdown"),
            (CancellationReason::ParentCancelled, "parent_cancelled"),
        ] {
            for phase in [
                ToolCancellationPhase::BeforeStart,
                ToolCancellationPhase::DuringExecution,
            ] {
                let status = ToolExecutionStatus::Cancelled { reason, phase };
                let text = status.model_facing_text().expect("cancelled text");
                assert!(text.contains(reason_label));
                match phase {
                    ToolCancellationPhase::BeforeStart => {
                        assert!(text.contains("did not start execution"));
                        assert!(!text.contains("Partial side effects"));
                    }
                    ToolCancellationPhase::DuringExecution => {
                        assert!(text.contains("already started"));
                        assert!(text.contains("Partial side effects may have occurred"));
                    }
                }
                if matches!(
                    reason,
                    CancellationReason::RuntimeShutdown | CancellationReason::ParentCancelled
                ) {
                    assert!(!text.contains("user requested"));
                }
            }
        }
    }
}
