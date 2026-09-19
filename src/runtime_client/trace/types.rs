//! The Trace inspection vocabulary: bounded summaries and heavy detail.
//!
//! Trace is split into two levels because a ledger and an inspector want
//! opposite things. A ledger pages over thousands of records and needs each
//! one to stay small; an inspector opens one record and needs everything
//! rustX authoritatively knows about it. Putting both in one type forces a
//! choice between an unusable inspector and an unpageable ledger.
//!
//! ```text
//! TraceRecord   bounded, pageable, one ledger row, one timeline span
//! TraceDetail   heavy, fetched for one record, never carried by a page
//! ```
//!
//! Every type here is a projection. None of them is history, execution
//! authority, settlement authority, or recovery input, and constructing one
//! reads durable state without changing any.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::bounds::{TraceJson, TracePreview, TraceText};
use crate::model::error::ModelErrorKind;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    ArtifactId, AttemptId, MessageId, RequestId, ToolCallId, ToolId, TurnId,
};

/// Opaque Trace-only exclusive boundary, valid only in its conversation.
///
/// It is not a live cursor, a transcript cursor, an Event Journal sequence,
/// a Request ID, or an artifact ID. Clients order by it and page with it;
/// they never parse it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct TraceCursor(pub(super) String);

impl TraceCursor {
    pub(super) fn at(sequence: u64) -> Self {
        Self(format!("trace:{sequence}"))
    }

    pub(super) fn sequence(&self) -> Result<u64, crate::durable::ConversationStoreError> {
        self.0
            .strip_prefix("trace:")
            .and_then(|value| value.parse().ok())
            .filter(|value| i64::try_from(*value).is_ok())
            .ok_or_else(|| {
                crate::durable::ConversationStoreError::InvalidReference(
                    "invalid Trace cursor".into(),
                )
            })
    }
}

/// One finite page of bounded summary records, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracePage {
    pub records: Vec<TraceRecord>,
    pub next_cursor: Option<TraceCursor>,
}

/// Server-resolved native grouping of one record.
///
/// `TurnId` is the logical model step inside an Attempt. An actual request
/// retry never allocates a new step, so retries of one step group together.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceLocation {
    pub attempt_id: Option<AttemptId>,
    pub step_id: Option<TurnId>,
}

/// The closed record vocabulary of the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    Attempt,
    Step,
    /// Canonical inbound accepted as a turn this conversation owes an answer for.
    User,
    Request,
    Assistant,
    Tool,
    Compaction,
    Background,
    Subagent,
    Workflow,
    Interaction,
}

/// The closed lifecycle vocabulary.
///
/// `Incomplete` is the truthful answer whenever no terminal fact exists and
/// no current runtime projection positively proves activity. It is never
/// upgraded by the absence of evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceState {
    Incomplete,
    Running,
    Pending,
    Cancelling,
    Settling,
    Waiting,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Limited,
    Denied,
    OutcomeUnknown,
    Interrupted,
}

/// Two authoritative instants, or fewer.
///
/// `duration_ms` exists exactly when both endpoints do. Receipt time, render
/// time and reconnect time are never endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceTiming {
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
}

/// Derived generation metrics for one actual request.
///
/// Every field is `None` unless its authoritative endpoints exist. A missing
/// metric states that rustX did not observe what the metric measures.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceGeneration {
    /// Request-relative monotonic phase positions, only with a native clock bridge.
    pub timeline: Option<TraceGenerationTimeline>,
    /// Milliseconds from the request's dispatch frontier to its first output.
    pub ttft_ms: Option<u64>,
    /// Milliseconds from the first output to the provider terminal.
    pub generation_ms: Option<u64>,
    /// Milliseconds from the dispatch frontier to the provider terminal.
    pub terminal_ms: u64,
    /// Output tokens per second, present only when usage and both decode
    /// endpoints exist and the decode span is measurable.
    pub output_tokens_per_second: Option<f64>,
}

/// All offsets share the paired durable request-start origin. The browser must
/// not stretch them to fit the independently recorded Journal wall duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceGenerationTimeline {
    pub dispatch_ms: u64,
    pub first_output_ms: Option<u64>,
    pub last_output_ms: Option<u64>,
    pub terminal_ms: u64,
}

/// Safe reference to the existing native artifact carrier, never a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceArtifact {
    pub artifact_id: ArtifactId,
    pub image: bool,
    pub name: Option<String>,
    pub mime_type: Option<String>,
}

/// One canonical `ToolCall` the model proposed.
///
/// A proposal proves that the generation assembled a call. It never proves
/// that execution started: a started Tool record is separate evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolCall {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    pub name: String,
}

/// How one actual request's frozen System Prompt relates to its predecessor.
///
/// The relationship is request-relative and resolved from native durable
/// authority, never from the records a client happens to have loaded. A page
/// that begins in the middle of history therefore reports exactly what a page
/// containing the predecessor would.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceSystemPromptState {
    /// Native authority proves no earlier actual request exists in this
    /// conversation. A `retry_number` of zero alone never proves this.
    Initial,
    /// The nearest preceding actual request froze a different prompt.
    Changed,
    /// The nearest preceding actual request froze the identical prompt.
    Unchanged,
    /// A predecessor exists, but the projection could not establish its
    /// frozen prompt at this read cut. It is never used to hide a durable
    /// read failure, which is propagated as an error instead.
    PreviousUnavailable,
}

/// Bounded System Prompt presentation for one actual request.
///
/// The complete prompt is deliberately absent: a page of 32 requests would
/// otherwise carry 32 complete system prompts. The exact historical value
/// stays in [`TraceRequestDetail::effective_system_prompt`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceSystemPromptPresentation {
    pub state: TraceSystemPromptState,
    /// One-line preview of the prompt this request introduced. Absent for
    /// `Unchanged`, where the preceding request's row already carries it.
    /// A present but empty preview records an empty historical prompt.
    pub preview: Option<TracePreview>,
}

/// The closed presentation family of one admitted model-visible context fact.
///
/// This is a presentation vocabulary, not the internal `ContextKind` payload:
/// a complete `GoalSnapshot` or Agent Status generation metadata never enters
/// a pageable summary through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceContextKind {
    GoalStatus,
    RuntimeToolObservation,
    ExtensionEnvironment,
    AgentStatus,
}

/// One canonical request Context fact introduced by one actual request.
///
/// Identity and order come from the immutable `RequestSnapshot`; content
/// comes from keyed Message Ledger reads. Neither the browser nor Trace
/// itself decides which request introduced a Context fact: the request that
/// committed the identity atomically with its own start did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceContextPresentation {
    pub message_id: MessageId,
    pub context_kind: TraceContextKind,
    /// Provenance namespace of the canonical inbound fact.
    pub source: String,
    pub preview: Option<TracePreview>,
    pub attachments: Vec<TraceArtifact>,
    pub truncated: bool,
}

/// Bounded request facts carried by a pageable summary row.
///
/// The historical request input — system prompt, reconstructed context, Tool
/// definitions — is deliberately absent. It lives in detail so a page of 32
/// requests does not carry 32 complete model contexts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequestSummary {
    pub request_id: RequestId,
    /// Native actual-request ordinal within the logical step. Zero is the
    /// initial request; retry and recovery requests continue the sequence.
    /// It is read from the frozen snapshot, never inferred from timestamps.
    pub retry_number: u32,
    pub assistant_message_id: MessageId,
    /// Historical model, from the request's own immutable snapshot.
    pub model: String,
    /// The exact preceding actual request's failure class, when recorded.
    pub previous_failure_kind: Option<ModelErrorKind>,
    pub failure_kind: Option<ModelErrorKind>,
    pub usage: Option<ModelUsage>,
    pub generation: Option<TraceGeneration>,
    /// Request-relative System Prompt presentation, resolved natively so the
    /// browser never compares request details to discover a prompt change.
    pub system_prompt: TraceSystemPromptPresentation,
    /// Canonical request Context this exact request introduced, in the order
    /// frozen by `RequestSnapshot.request_context_ids`. A retry or recovery
    /// request reuses admitted context and therefore introduces none.
    pub context_additions: Vec<TraceContextPresentation>,
    /// Whether the Context list was shortened by the Trace summary bound.
    pub context_truncated: bool,
}

/// Bounded Tool facts carried by a pageable summary row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolSummary {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    /// Recorded model-facing name, when the canonical proposal is loadable.
    pub name: Option<String>,
    /// Whether execution started, as proven by its own durable start fact.
    pub started: bool,
    /// Typed outcome class, present only once the execution settled.
    pub outcome: Option<TraceToolOutcome>,
    /// Bounded typed error or status detail from the canonical result.
    pub detail: Option<TracePreview>,
}

/// The closed canonical Tool outcome vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceToolOutcome {
    Success,
    Failed,
    Denied,
    Cancelled,
    TimedOut,
    OutcomeUnknown,
}

/// One bounded pageable ledger record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRecord {
    /// Stable native Trace identity. Selection and detail reads use it, and
    /// it survives paging, prepending and reconnect for the same record.
    pub id: String,
    /// Ordering boundary of this read model; not an event cursor.
    pub position: TraceCursor,
    pub location: TraceLocation,
    pub kind: TraceKind,
    pub state: TraceState,
    pub timing: TraceTiming,
    /// One-line content preview, so a ledger row carries meaning rather than
    /// an identity alone.
    pub preview: Option<TracePreview>,
    pub request: Option<TraceRequestSummary>,
    pub tool: Option<TraceToolSummary>,
    /// Canonical `ToolCall` proposals of an Assistant record, in block order.
    pub calls: Vec<TraceToolCall>,
    /// Exact native detached execution / Subagent / Workflow / interaction ID.
    pub native_id: Option<String>,
    /// The exact outer `ToolCall` this Tool-owned domain record belongs to,
    /// copied from the native start fact. It is presentation and navigation
    /// correlation only: it confers no lifecycle, ownership, settlement or
    /// cancellation authority, and it is never resolved from a Tool name, a
    /// timestamp, row adjacency or the loaded page.
    pub originating_tool_call_id: Option<ToolCallId>,
    /// Canonical accepted message; publication without acceptance is absent.
    pub message_id: Option<MessageId>,
    pub attachments: Vec<TraceArtifact>,
    /// Whether this record has heavy detail available for inspection.
    pub has_detail: bool,
    pub truncated: bool,
}

/// Refresh of an already loaded record, resolved at the server's snapshot cut.
///
/// Only mutable lifecycle facts travel here. Immutable historical input is
/// never repeated, because it cannot have changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceLifecycle {
    pub id: String,
    pub state: TraceState,
    pub timing: TraceTiming,
    pub request: Option<TraceRequestOutcome>,
    pub tool: Option<TraceToolOutcomeUpdate>,
    pub message_id: Option<MessageId>,
    pub attachments: Vec<TraceArtifact>,
    pub truncated: bool,
}

/// Mutable request outcome only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequestOutcome {
    pub failure_kind: Option<ModelErrorKind>,
    pub usage: Option<ModelUsage>,
    pub generation: Option<TraceGeneration>,
}

/// Mutable Tool outcome only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolOutcomeUpdate {
    pub started: bool,
    pub outcome: Option<TraceToolOutcome>,
    pub detail: Option<TracePreview>,
}

// ---------------------------------------------------------------------------
// Detail
// ---------------------------------------------------------------------------

/// Heavy inspection detail for one exact record identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceDetail {
    /// Echoes the requested identity so a late reply can be fenced.
    pub id: String,
    pub kind: TraceKind,
    pub request: Option<TraceRequestDetail>,
    pub tool: Option<TraceToolDetail>,
    /// Canonical messages in native order (an adoption can contain a batch).
    pub messages: Vec<TraceMessageDetail>,
    pub truncated: bool,
}

/// The exact historical request, reconstructed from frozen native authority.
///
/// Every value comes from this request's own immutable Request Snapshot and
/// the historical Conversation Surface revision it froze. Current model
/// configuration, the current tool catalog and the current system prompt are
/// never consulted, so a request reopened after the Session was reconfigured
/// still shows what was actually sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequestDetail {
    pub request_id: RequestId,
    pub attempt_id: AttemptId,
    pub step_id: TurnId,
    pub retry_number: u32,
    pub assistant_message_id: MessageId,
    pub model: String,
    pub protocol: String,
    pub max_output_tokens: u32,
    pub context_window_tokens: u64,
    pub reasoning_enabled: bool,
    pub reasoning_profile: Option<String>,
    /// Allowlisted provider-neutral sampling options; see the options
    /// allowlist for exactly which keys may appear.
    pub options: Vec<TraceRequestOption>,
    /// How many configured request parameters were outside the allowlist.
    /// Their names are not disclosed; the count keeps the omission visible.
    pub omitted_option_count: usize,
    pub effective_system_prompt: TraceText,
    /// The reconstructed provider-neutral request context, in wire order.
    pub messages: Vec<TraceRequestMessage>,
    pub messages_truncated: bool,
    /// The exact historical Tool definitions this request carried.
    pub tools: Vec<TraceToolDefinition>,
    pub tools_truncated: bool,
    pub usage: Option<ModelUsage>,
    pub failure: Option<TraceRequestFailure>,
    pub generation: Option<TraceGeneration>,
}

/// One allowlisted provider-neutral request option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequestOption {
    pub name: String,
    pub value: TraceJson,
}

/// The terminal failure of one actual request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequestFailure {
    pub kind: ModelErrorKind,
    pub message: TraceText,
}

/// The role of one reconstructed request item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceMessageRole {
    User,
    Assistant,
    Tool,
    /// A request-only runtime context item with no canonical identity.
    RequestOnly,
}

/// One reconstructed provider-neutral request item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRequestMessage {
    pub role: TraceMessageRole,
    /// Canonical Ledger identity; absent exactly for a request-only item.
    pub message_id: Option<MessageId>,
    /// Provenance of a canonical User item.
    pub source: Option<String>,
    pub blocks: Vec<TraceContentBlock>,
    pub truncated: bool,
}

/// One canonical accepted message, projected for inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceMessageDetail {
    pub message_id: MessageId,
    pub role: TraceMessageRole,
    pub source: Option<String>,
    pub blocks: Vec<TraceContentBlock>,
    pub truncated: bool,
}

/// The closed projected content vocabulary.
///
/// Each variant is a semantic the browser can render with the matching
/// presentation primitive: prose as Markdown, structure as a JSON reader,
/// source as code, artifacts through the existing durable carrier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceContentBlock {
    Text {
        text: TraceText,
    },
    Reasoning {
        text: TraceText,
    },
    Refusal {
        text: TraceText,
    },
    Json {
        value: TraceJson,
    },
    ToolCall {
        call_id: ToolCallId,
        tool_id: ToolId,
        name: String,
        arguments: TraceJson,
    },
    /// A Tool result item inside a reconstructed request context.
    ToolResult {
        call_id: ToolCallId,
        tool_id: ToolId,
        outcome: TraceToolOutcome,
        blocks: Vec<TraceContentBlock>,
        truncated: bool,
    },
    Image {
        artifact: TraceArtifact,
        alt: Option<String>,
    },
    File {
        artifact: TraceArtifact,
    },
    /// A Session-owned workspace upload reference.
    Upload {
        name: String,
    },
}

/// The historical model-facing Tool definition of one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolDefinition {
    pub tool_id: ToolId,
    pub name: String,
    pub description: TraceText,
    pub input_schema: TraceJson,
}

/// How far one Tool call progressed, by its own native evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceToolLifecycle {
    /// The model assembled the call. Execution is not implied.
    Proposed,
    /// A durable start fact exists for this exact call and Tool.
    Started,
    /// A canonical Tool message was accepted for this exact call.
    Settled,
}

/// Heavy Tool inspection detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolDetail {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    pub name: Option<String>,
    pub lifecycle: TraceToolLifecycle,
    /// Exact structured arguments from the canonical `ToolCall` proposal.
    pub arguments: Option<TraceJson>,
    /// Program source identified by a native Tool contract, when there is one.
    pub source: Option<TraceToolSource>,
    /// The historical definition this call's own request carried.
    pub definition: Option<TraceToolDefinition>,
    pub result: Option<TraceToolResult>,
}

/// Program source carried by an argument field of a native Tool contract.
///
/// This exists only where a rustX native Tool identity states unambiguously
/// that one named argument is a program. It is never inferred from a value's
/// shape, a filename, or a file extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolSource {
    /// The argument field the native contract identifies as program source.
    pub field: String,
    pub text: TraceText,
    /// Highlighting language, present only when the native contract fixes
    /// it. A tool whose contract does not fix a language leaves this absent
    /// rather than guessing from the source.
    pub language: Option<String>,
}

/// The canonical execution result of one Tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolResult {
    pub outcome: TraceToolOutcome,
    /// Typed status detail: the error, denial reason, or cancellation reason.
    pub detail: Option<TraceText>,
    pub blocks: Vec<TraceContentBlock>,
    pub blocks_truncated: bool,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub attachments: Vec<TraceArtifact>,
    /// Tool-owned output truncation recorded by the execution itself.
    pub truncation: Option<TraceToolTruncation>,
    /// Runtime-owned managed-output continuation metadata, when present.
    pub managed_output: Option<TraceManagedOutput>,
}

/// Tool-recorded output truncation, distinct from Trace's own bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolTruncation {
    pub truncated: bool,
    pub original_bytes: Option<u64>,
}

/// Managed textual-output continuation metadata.
///
/// The recorded locator is an inspectable execution fact. It is presentation
/// data only and confers no filesystem, execution, or recovery authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceManagedOutput {
    /// Whether the store holds this result's complete textual output, so the
    /// bounded result content is a preview rather than the whole record.
    pub complete: bool,
    /// Whether any managed output file exists at all.
    pub available: bool,
    /// Exact native locator, when output storage owns one.
    pub locator: Option<std::path::PathBuf>,
    /// The bounded advisory output-storage diagnostic, when one was recorded.
    pub diagnostic: Option<TraceText>,
}
