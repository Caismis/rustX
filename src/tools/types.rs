//! Canonical tool contracts.
//!
//! These types describe what the runtime knows about tools: their
//! definitions, the calls the current agent issues, the normalized
//! execution results, and the runtime-owned invocation data delivered to
//! executors. The canonical [`ToolDefinition`] is tool-owned and carries the
//! three independent execution policy axes; the provider-neutral compiled
//! [`ModelToolDefinition`] is what actually reaches a model request.
//! Execution scheduling and executors are runtime-owned (M3+); native and
//! MCP executors reuse the same contract (managed Python tool packages are
//! MCP tools, Issue #174). No external SDK type appears here.

use serde::{Deserialize, Serialize};

use crate::runtime::identity::{McpServerId, ToolCallId, ToolId};
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
    /// A tool served by a bound MCP server. Managed Python tool packages
    /// (Issue #174) compile into this origin through their synthesized
    /// server identity (`python:<folder>`).
    Mcp {
        /// Identity of the MCP server exposing the tool.
        server_id: McpServerId,
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
    /// Tool-owned result content.
    ///
    /// This content is TOOL-OWNED: [`ToolResultContent::Json`] is arbitrary
    /// tool-owned structured data, and the runtime never infers semantics
    /// from its property names. rustX reserves no ordinary JSON field names;
    /// runtime-owned facts live in the typed fields of this struct. A
    /// provider-independent, bounded model-facing representation is produced
    /// by [`Self::model_facing_projection`]; producers do not append runtime
    /// status or managed-output continuation text here.
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

/// The provider-independent model-facing representation of one tool result.
///
/// The projection is text-oriented because the three current tool-result
/// protocol surfaces translate tool text and compact JSON into textual wire
/// content. Runtime-owned status feedback and managed-output continuation are
/// added here, rather than by an executor or provider adapter. The complete
/// joined representation is always bounded by
/// [`MAX_MODEL_TOOL_RESULT_BYTES`](crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES).
///
/// Tool-owned file/image references remain marked as non-text content so
/// protocol adapters can reject them when their wire format cannot represent
/// them. Their bounded textual mention is still available to text-only
/// consumers such as background terminal publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultProjection {
    parts: Vec<String>,
    contains_non_text_content: bool,
}

impl ToolResultProjection {
    /// Returns the already-bounded text parts in canonical order.
    #[must_use]
    pub fn parts(&self) -> &[String] {
        &self.parts
    }

    /// Returns the complete bounded representation with canonical separators.
    #[must_use]
    pub fn as_text(&self) -> String {
        self.parts.join("\n")
    }

    /// Returns the exact byte length of [`Self::as_text`].
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.parts.iter().fold(0usize, |length, part| {
            length
                .saturating_add(part.len())
                .saturating_add(usize::from(length > 0))
        })
    }

    /// Whether the canonical result contained a file/image block that a
    /// text-only provider protocol cannot represent natively.
    #[must_use]
    pub fn contains_non_text_content(&self) -> bool {
        self.contains_non_text_content
    }

    /// Whether this result has no model-facing text.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

impl ToolExecutionResult {
    /// Produces the one canonical model-facing projection of this result.
    ///
    /// The deterministic policy is:
    ///
    /// 1. render typed runtime status feedback first;
    /// 2. reserve the structural managed-output continuation (including its
    ///    locator and complete/partial truth) next;
    /// 3. use the remaining bytes for ordinary tool-owned content.
    ///
    /// Status feedback and continuation therefore survive pressure before
    /// ordinary content. Oversized status/content text is shortened at a
    /// UTF-8 boundary with an explicit marker. Continuation diagnostics are
    /// advisory; the bounded continuation renderer prioritizes its fixed
    /// complete/partial label, locator, and guidance. The typed status and
    /// managed-output fields remain unchanged and authoritative.
    #[must_use]
    pub fn model_facing_projection(&self) -> ToolResultProjection {
        const CONTENT_TRUNCATION_MARKER: &str = "\n...[tool-owned result content truncated]";
        const STATUS_TRUNCATION_MARKER: &str = "\n...[tool status truncated]";
        let bound = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES;

        let status = self.status.feedback_text();
        let (content, contains_non_text_content) = render_tool_owned_content(&self.content);
        // Reserve the minimum truthful continuation before allowing a large
        // status diagnostic to consume the result budget. This makes the
        // locator/guidance structural rather than something that can be
        // accidentally truncated away by an error string.
        let continuation_minimum = self
            .managed_output
            .as_ref()
            .map(ManagedOutputContinuation::minimum_render)
            .map_or(0, |text| text.len());
        let status_budget = bound.saturating_sub(
            continuation_minimum + usize::from(status.is_some() && self.managed_output.is_some()),
        );
        let status = status
            .map(|text| bounded_projection_text(&text, status_budget, STATUS_TRUNCATION_MARKER));

        // Continuation is allocated after status, before ordinary content.
        // Its structural minimum is guaranteed by the reservation above.
        let continuation_budget = bound
            .saturating_sub(status.as_ref().map_or(0, String::len) + usize::from(status.is_some()));
        let continuation = self
            .managed_output
            .as_ref()
            .map(|continuation| continuation.render_bounded(continuation_budget));

        let reserved = status.as_ref().map_or(0, String::len)
            + continuation.as_ref().map_or(0, String::len)
            + usize::from(status.is_some() && continuation.is_some());
        let content_budget = bound.saturating_sub(
            reserved
                + if content.is_empty() {
                    0
                } else {
                    usize::from(status.is_some()) + usize::from(continuation.is_some())
                },
        );
        let content = (!content.is_empty())
            .then(|| bounded_projection_text(&content, content_budget, CONTENT_TRUNCATION_MARKER));

        let parts = [status, content, continuation]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect();
        let projection = ToolResultProjection {
            parts,
            contains_non_text_content,
        };
        debug_assert!(
            projection.byte_len() <= bound,
            "the complete model-facing tool result projection is bounded"
        );
        projection
    }
}

fn render_tool_owned_content(content: &[ToolResultContent]) -> (String, bool) {
    let mut rendered = String::new();
    let mut contains_non_text_content = false;
    for (index, block) in content.iter().enumerate() {
        let (text, non_text) = match block {
            ToolResultContent::Text(text) => (text.text.clone(), false),
            ToolResultContent::Json { value } => (
                serde_json::to_string(value)
                    .unwrap_or_else(|_| "<unserializable JSON result>".to_owned()),
                false,
            ),
            ToolResultContent::File(reference) => (
                format!(
                    "[file artifact: {}]",
                    reference
                        .name
                        .clone()
                        .unwrap_or_else(|| reference.artifact_id.as_str().to_owned())
                ),
                true,
            ),
            ToolResultContent::Image(_) => ("[image content]".to_owned(), true),
        };
        if index > 0 {
            rendered.push('\n');
        }
        rendered.push_str(&text);
        contains_non_text_content |= non_text;
    }
    (rendered, contains_non_text_content)
}

fn bounded_projection_text(text: &str, bound: usize, marker: &str) -> String {
    if text.len() <= bound {
        return text.to_owned();
    }
    if bound > marker.len() {
        let prefix = crate::tools::limits::bound_utf8_text(
            text.to_owned(),
            bound.saturating_sub(marker.len()),
        );
        return format!("{prefix}{marker}");
    }
    crate::tools::limits::bound_utf8_text(marker.to_owned(), bound)
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

    /// The complete textual rendering of this continuation.
    ///
    /// The provider-independent bounded model-facing projection uses
    /// [`Self::render_bounded`] rather than this complete rendering. The
    /// diagnostic is bounded here, but a pathological locator may still be
    /// longer than the model-result budget until the canonical projection
    /// applies its structural bound.
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

    /// Renders this continuation within `bound` bytes while preserving its
    /// complete/partial truth and fixed Read/Grep guidance whenever the bound
    /// can hold the structural form. A locator is essential, so an oversized
    /// locator is shortened with an explicit marker; diagnostics are advisory
    /// and are omitted before structural continuation text is lost.
    #[must_use]
    pub(crate) fn render_bounded(&self, bound: usize) -> String {
        let complete_prefix = "Complete output: ";
        let partial_prefix = "Partial output only: ";
        let diagnostic_prefix = "\nDiagnostic: ";
        let locator_marker = "[locator truncated]";

        match self {
            Self::Complete { locator } => {
                let guidance = format!("\n{}", Self::COMPLETE_GUIDANCE);
                let structural = format!("{complete_prefix}{locator_marker}{guidance}");
                if bound < structural.len() {
                    return crate::tools::limits::bound_utf8_text(structural, bound);
                }
                let locator_budget = bound
                    .saturating_sub(complete_prefix.len())
                    .saturating_sub(guidance.len());
                format!(
                    "{complete_prefix}{}{}",
                    bound_locator(locator, locator_budget, locator_marker),
                    guidance
                )
            }
            Self::Partial {
                locator,
                diagnostic,
            } => {
                let guidance = format!("\n{}", Self::PARTIAL_GUIDANCE);
                let structural = format!("{partial_prefix}{locator_marker}{guidance}");
                if bound < structural.len() {
                    return crate::tools::limits::bound_utf8_text(structural, bound);
                }
                let locator_text = locator.to_string_lossy();
                let structural_with_locator = format!("{partial_prefix}{locator_text}{guidance}");
                if structural_with_locator.len() > bound {
                    let locator_budget = bound
                        .saturating_sub(partial_prefix.len())
                        .saturating_sub(guidance.len());
                    return format!(
                        "{partial_prefix}{}{}",
                        bound_locator(locator, locator_budget, locator_marker),
                        guidance
                    );
                }
                let remaining = bound.saturating_sub(structural_with_locator.len());
                if remaining < diagnostic_prefix.len() + 1 {
                    return structural_with_locator;
                }
                let diagnostic_budget = remaining
                    .saturating_sub(diagnostic_prefix.len())
                    .min(crate::tools::limits::MAX_OUTPUT_CONTINUATION_DIAGNOSTIC_BYTES);
                let bounded =
                    crate::tools::limits::bound_utf8_text(diagnostic.clone(), diagnostic_budget);
                format!("{structural_with_locator}{diagnostic_prefix}{bounded}")
            }
            Self::Unavailable { diagnostic } => {
                let prefix = "The output capture did not complete; the bounded result content is the only record and no complete output file is available.";
                if bound < prefix.len() {
                    return crate::tools::limits::bound_utf8_text(prefix.to_owned(), bound);
                }
                let remaining = bound.saturating_sub(prefix.len());
                if remaining < diagnostic_prefix.len() + 1 {
                    return prefix.to_owned();
                }
                let diagnostic_budget = remaining
                    .saturating_sub(diagnostic_prefix.len())
                    .min(crate::tools::limits::MAX_OUTPUT_CONTINUATION_DIAGNOSTIC_BYTES);
                let bounded =
                    crate::tools::limits::bound_utf8_text(diagnostic.clone(), diagnostic_budget);
                format!("{prefix}{diagnostic_prefix}{bounded}")
            }
        }
    }

    fn minimum_render(&self) -> String {
        match self {
            Self::Complete { locator } => format!(
                "Complete output: {}\n{}",
                locator.display(),
                Self::COMPLETE_GUIDANCE
            ),
            Self::Partial { locator, .. } => format!(
                "Partial output only: {}\n{}",
                locator.display(),
                Self::PARTIAL_GUIDANCE
            ),
            Self::Unavailable { .. } => "The output capture did not complete; the bounded result content is the only record and no complete output file is available.".to_owned(),
        }
    }
}

fn bound_locator(locator: &std::path::Path, bound: usize, marker: &str) -> String {
    let locator = locator.to_string_lossy();
    if locator.len() <= bound {
        return locator.into_owned();
    }
    if bound > marker.len() {
        let prefix = crate::tools::limits::bound_utf8_text(
            locator.into_owned(),
            bound.saturating_sub(marker.len()),
        );
        format!("{prefix}{marker}")
    } else {
        crate::tools::limits::bound_utf8_text(marker.to_owned(), bound)
    }
}

/// The semantic phase at which a tool call was cancelled.
///
/// This is a closed, provider-independent fact owned by the canonical tool
/// result contract and shared by foreground and background runtime-owned
/// results. The foreground Agent Loop selects it from its per-call executor
/// start frontier; the background registry selects it from its detached-runner
/// frontier. Executors may report a provisional physical cancellation status,
/// but they do not own the canonical phase classification. Clients only
/// consume or project the canonical typed fact; provider adapters likewise
/// translate that fact without inferring its phase.
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
    /// Renders the status detail consumed by the canonical result projection.
    /// The typed status remains authoritative; this helper does not enforce
    /// the model-result byte budget and is never called by a provider adapter.
    #[must_use]
    pub(crate) fn feedback_text(&self) -> Option<String> {
        match self {
            Self::Failed { error } => Some(format!("Tool call failed: {error}")),
            Self::Denied { reason } => Some(format!("Denied: {reason}")),
            Self::Cancelled { reason, phase } => {
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
            Self::Success | Self::TimedOut | Self::Interrupted => None,
        }
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
                let text = status.feedback_text().expect("cancelled text");
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

    #[test]
    fn failed_tool_status_is_visible_to_the_model() {
        let status = ToolExecutionStatus::Failed {
            error: "input schema validation failed: query is required".to_owned(),
        };
        let result = ToolExecutionResult {
            status: status.clone(),
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        };
        let projection = result.model_facing_projection();
        assert_eq!(
            projection.as_text(),
            "Tool call failed: input schema validation failed: query is required"
        );
        assert_eq!(result.status, status);
        assert!(ToolExecutionStatus::Success.feedback_text().is_none());
    }

    #[test]
    fn model_facing_projection_bounds_content_status_and_continuation_together() {
        let bound = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES;
        let status = ToolExecutionStatus::Failed {
            error: format!(
                "input schema validation failed: query is required; {}",
                "e".repeat(4 * 1024)
            ),
        };
        let result = ToolExecutionResult {
            status: status.clone(),
            content: vec![super::ToolResultContent::Text(
                crate::message::content::TextBlock {
                    text: "o".repeat(bound.saturating_sub(128)),
                },
            )],
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: Some(super::ManagedOutputContinuation::Complete {
                locator: std::path::PathBuf::from("/tmp/rustx/results/result_7.txt"),
            }),
        };

        let first = result.model_facing_projection();
        let second = result.model_facing_projection();
        assert_eq!(first, second, "projection is deterministic");
        assert!(first.byte_len() <= bound);
        let text = first.as_text();
        assert!(
            text.contains("Tool call failed: input schema validation failed: query is required")
        );
        assert!(text.contains("Complete output: /tmp/rustx/results/result_7.txt"));
        assert!(text.contains("Read or Grep"));
        assert!(text.contains("tool-owned result content truncated"));
        assert_eq!(result.status, status, "typed status remains authoritative");
    }

    #[test]
    fn model_facing_projection_truncates_only_at_utf8_boundaries() {
        let result = ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![super::ToolResultContent::Text(
                crate::message::content::TextBlock {
                    text: "😀".repeat(crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES),
                },
            )],
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        };
        let projection = result.model_facing_projection();
        assert!(projection.byte_len() <= crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES);
        let text = projection.as_text();
        assert!(text.is_char_boundary(text.len()));
        assert!(text.contains("tool-owned result content truncated"));
        let marker = "\n...[tool-owned result content truncated]";
        let prefix = text.strip_suffix(marker).expect("truncation marker");
        assert_eq!(prefix.len() % "😀".len(), 0);
    }

    #[test]
    fn model_facing_projection_keeps_continuation_structure_before_status_tail() {
        let bound = crate::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES;
        let status = ToolExecutionStatus::Failed {
            error: "e".repeat(bound.saturating_mul(2)),
        };
        let result = ToolExecutionResult {
            status: status.clone(),
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: Some(super::ManagedOutputContinuation::Complete {
                locator: std::path::PathBuf::from("/tmp/rustx/results/result_8.txt"),
            }),
        };

        let projection = result.model_facing_projection();
        assert!(projection.byte_len() <= bound);
        let text = projection.as_text();
        assert!(text.contains("Tool call failed: e"));
        assert!(text.contains("...[tool status truncated]"));
        assert!(text.contains("Complete output: /tmp/rustx/results/result_8.txt"));
        assert!(text.contains("Read or Grep"));
        assert_eq!(result.status, status, "typed status remains authoritative");
    }
}
