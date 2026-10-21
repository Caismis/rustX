//! The Runtime Client snapshot read model.
//!
//! [`RuntimeClientSnapshot`] is the one deterministic external read model
//! of authoritative runtime state. It is a projection, never a second
//! authority:
//!
//! - canonical Ledger headers and loaded body caches are finite display pages;
//! - the native model-context Surface remains a separate read-cut domain;
//! - the attempt view mirrors the current/latest attempt execution;
//! - in-flight output carries bounded headers and released publication frontiers;
//! - exact deferred bytes remain in their authoritative canonical/audit/live source;
//! - background executions mirror the authoritative conversation registry;
//! - the Agent Status window derives from the exact composed statuses;
//! - inbound diagnostics mirror the authoritative mailbox;
//! - the capability view mirrors the active capability snapshot.
//!
//! The snapshot carries no internal executors, no executor environment paths, no
//! provider objects, and no synchronization identities.
//!
//! A checkpoint at cursor `C` captures one consistent native display cut and
//! finite descriptors for its unloaded ranges. Continuous complete transitions
//! after `C` converge with the endpoint checkpoint; a gap requires explicit
//! recovery. Neither observation nor transport delivery owns execution.
use crate::observation::capability_source::CapabilityView;
use crate::observation::head::{RuntimeClientCompactionView, RuntimeClientProjectionHeadView};
use crate::observation::interaction::RuntimeClientInteractionHeader;
use crate::observation::resource_source::RuntimeClientResourcesView;
use crate::observation::status::AgentStatusView;
use crate::observation::tool::{ForegroundToolExecution, JournalFieldRef};
use crate::observation::transcript::{
    RuntimeClientMessageDescriptor, RuntimeClientPublicationSettlementRecord,
    RuntimeClientTranscriptCursor, RuntimeClientTranscriptPage,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::event::RuntimeClientOutcome;
use crate::message::types::{ContentBlockIndex, MessageBlock, UserMessageBlock};
use crate::model::session::{AttemptModelView, SessionModelView};
use crate::model::types::ModelUsage;
use crate::runtime::identity::{
    AttemptId, ConversationId, MessageId, ToolCallId, ToolExecutionId, ToolId,
};
use crate::runtime::inbound::InboundSequence;
use crate::runtime::types::ApprovalMode;
use crate::tools::background::BackgroundLifecycle;
use crate::tools::todo::TodoSnapshot;
use crate::tools::types::{ToolExecutionResult, ToolProgress};

/// The client-visible durable-authority failure state of a conversation
/// runtime (Issue #63).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeDurabilityFailure {
    /// The operation that failed persistently.
    pub operation: String,
    /// The human-readable failure diagnostic.
    pub diagnostic: String,
}

/// The authoritative Runtime Client snapshot of one conversation runtime.
///
/// Every section is a deterministic projection of one authoritative
/// runtime owner. The shape belongs to the Runtime Client protocol: internal
/// snapshot types are projected into these external DTOs, never exposed
/// directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientSnapshot {
    pub settings_evidence: super::settings::SettingsEvidence,
    /// The frozen effective native Agent Extension composition of the Agent
    /// runtime this snapshot projects (Issue #256).
    ///
    /// This is the composition the attached runtime is *already executing
    /// against*, read from the extension owners it materialized — the value
    /// frozen at `LocalConversationCore::compose` for a root, and the value
    /// carried in `ResolvedSubagentSpec::extensions` for a Subagent child.
    /// Nothing on this path rereads `rustx.toml`, project or host
    /// configuration, a role document, a `ProspectiveSessionConfig`, or the latest
    /// `RuntimeResourceSnapshot`, and nothing infers it from Agent Status
    /// observations, context messages, or the Event Journal. It is therefore
    /// deliberately distinct from `rustx config show --sources`, which
    /// describes a *prospective next launch* and may legitimately disagree
    /// after a configuration edit.
    ///
    /// `None` means no authoritative Agent composition is available to
    /// project — historical-only durable inspection. It is never filled from
    /// current disk configuration or built-in defaults. Inside the value,
    /// `agent_status: None` is the separate fact that the extension is not
    /// part of this Agent's composition at all.
    pub effective_plugins: Option<super::settings::EffectivePlugins>,
    /// Bounded native Workflow state, never reconstructed from the journal.
    pub workflows: crate::runtime::workflow::read_model::WorkflowSnapshot,
    /// The conversation this snapshot belongs to.
    pub conversation_id: ConversationId,
    /// Whether runtime drain has begun and new inbound admission is closed.
    /// The correlated shutdown response resolves only after quiescence.
    pub shutting_down: bool,
    /// The authoritative mode used by the current attempt boundary.
    pub effective_approval_mode: ApprovalMode,
    /// Whether this live runtime could adopt a prepared Session configuration
    /// now. Runtime-domain advisory state published by the runtime with its
    /// adoption gate's idle rule over in-memory owners, and changed only by the
    /// `configuration_adoption_eligibility_changed` event; it describes no
    /// candidate and is never configuration application state. `unavailable` until the runtime runs, after it stops, and for a
    /// durable inspection projection. `session/adoptConfiguration` revalidates
    /// the real gate at its commit boundary.
    pub configuration_adoption_eligibility: crate::runtime::adoption::AdoptionEligibility,
    /// The runtime's durable-authority failure, when it has entered the
    /// explicit degraded state. While set, no new durable admission/execution
    /// work may begin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability_failure: Option<RuntimeDurabilityFailure>,
    /// The client projection of canonical Message Ledger observations.
    ///
    /// It is repaired from the native durable authority at bootstrap and
    /// from committed observations while live; it is never independently
    /// mutable or recovery input. A restarted projection contains the
    /// current Surface working set, while historical Ledger pages remain
    /// available through `ConversationStore` APIs.
    pub messages: Vec<MessageBlock>,
    pub message_descriptors: Vec<RuntimeClientMessageDescriptor>,
    /// Native projection-selected settlement head; retained audit bodies remain in storage.
    #[serde(default)]
    pub publication_settlements: Vec<RuntimeClientPublicationSettlementRecord>,
    /// The bounded newest page of the derived durable transcript. This is
    /// distinct from `messages`: the latter is the current Surface working
    /// set, while this page remains readable after compaction excludes messages
    /// from the model projection. Older pages are fetched through `transcript_page_get`.
    pub transcript: RuntimeClientTranscriptPage,
    /// Bounded native Trace read window; independent from transcript and live cursors.
    pub trace: crate::observation::trace::TracePage,
    /// Lifecycle repairs for explicitly requested loaded Trace identities.
    #[schemars(length(max = 512))]
    pub trace_updates: Vec<crate::observation::trace::TraceLifecycle>,
    /// The current/latest attempt view, when any attempt exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<RuntimeClientAttempt>,
    /// The inbound mailbox diagnostics (pending items and the latest
    /// finite drain observation).
    pub inbound: InboundDiagnostics,
    /// Live process-owned native interaction requests. This is projection
    /// state, never durable recovery input or client-owned truth.
    #[serde(default)]
    pub pending_interactions: Vec<RuntimeClientInteractionHeader>,
    /// All background executions in execution allocation order, including
    /// terminal records retained by the authoritative registry.
    #[serde(default)]
    pub jobs: Vec<RuntimeClientJob>,
    /// Durable child Agents in creation order. Repeated activations replace
    /// the same Agent row; activation history remains in the Event Journal.
    #[serde(default)]
    pub agents: Vec<RuntimeClientAgent>,
    /// The bounded newest window of composed Agent Status observations, in
    /// runtime composition order (oldest first).
    ///
    /// This is a **list, not a latest value**: a composed status is a
    /// historical fact of the conversation, and a later attempt neither
    /// retracts nor relocates an earlier one. A client that wants "the
    /// latest" takes the last element; it never needs a second field that
    /// could disagree with this order.
    ///
    /// The window is bounded by [`AGENT_STATUS_WINDOW`], and that bound is
    /// owned **here, in the projection, and nowhere else**. A client folding
    /// [`RuntimeClientEvent::AgentStatusComposed`](super::RuntimeClientEvent)
    /// incrementally never applies a retention rule of its own: each event
    /// carries the eviction this exact projection transition performed, so
    /// the fold reproduces the window rather than re-deciding it. Two
    /// retention owners would be two policies, and after the bound was
    /// crossed a snapshot repair would silently drop compositions a
    /// continuously subscribed client still believed in.
    ///
    /// The canonical Agent Status Context message is request-scoped model
    /// history and never enters the durable transcript, so unlike a
    /// transcript page this window cannot be paged backwards: a composition
    /// older than the window is simply no longer projected. Like `attempt`
    /// and `context`, it describes the live runtime and starts empty on a
    /// fresh runtime.
    #[serde(default)]
    pub statuses: Vec<AgentStatusView>,
    /// Runtime-owned context-compaction diagnostics. The values describe
    /// committed `RuntimeEvent::CompactionCompleted` facts by identity;
    /// they are never a second Conversation Surface authority and never
    /// carry summary content (the committed summary itself appears in
    /// `messages`, like every other canonical Ledger fact).
    #[serde(default)]
    pub context: RuntimeClientContextView,
    /// The active capability projection.
    pub capabilities: CapabilityView,
    /// The active runtime resource generation: the project context files
    /// the runtime actually loaded, and whether an agent profile is frozen
    /// into the generation.
    ///
    /// This is deliberately separate from [`CapabilityView`]: a
    /// resource publication advances the resource revision while the
    /// capability revision stays put. Nothing here is conversation content
    /// — a project context file is request input the runtime assembles into
    /// the Effective System Prompt, never a canonical Ledger message.
    #[serde(default)]
    pub resources: RuntimeClientResourcesView,
    /// The redacted session model state: the authoritative *desired*
    /// configuration and its resolution.
    ///
    /// This is deliberately distinct from
    /// [`RuntimeClientAttempt::model`], which is the immutable snapshot an
    /// already-admitted attempt froze. While an attempt on model A runs and
    /// the session has been switched to model B, this section truthfully
    /// shows B and the attempt section truthfully shows A.
    ///
    /// No credential, adapter object, provider HTTP client, or
    /// synchronization identity appears here.
    /// Absent when no live Session model authority exists (historical inspection).
    pub model: Option<SessionModelView>,
    /// Admission availability remains explicit when growing model details are deferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_status: Option<RuntimeClientModelStatus>,
    /// The conversation's task list, as of the newest committed `todo`
    /// result — present exactly when this runtime composes the **Todo**
    /// Agent Extension (Issue #259).
    ///
    /// This is a **projection of canonical history, not a second
    /// authority**: the runtime derives it from exactly the tool results the
    /// Ledger holds, the same fact the runtime's own
    /// [`ConversationTodoList`] is rebuilt from, so the two can never
    /// disagree.
    ///
    /// It is carried here rather than left for a client to scan out of the
    /// transcript because a client holds only a bounded newest page of that
    /// transcript. A conversation that committed a page or more of messages
    /// after its last `todo` result would otherwise attach with no list at
    /// all, and would appear to have none until the reader happened to page
    /// far enough back — while the runtime, reading the whole Ledger, still
    /// had one.
    ///
    /// A conversation that never called `todo` carries the empty list.
    ///
    /// `None` is a different fact and must render differently: this runtime
    /// composes no Todo extension, so there is no current task list to show
    /// at all. Canonical history may still contain `todo` calls and results
    /// from a launch that did compose it; those remain renderable as
    /// *transcript history* and must not be folded back into a current panel.
    ///
    /// [`ConversationTodoList`]: crate::tools::todo::ConversationTodoList
    #[serde(default)]
    pub todos: Option<TodoSnapshot>,
    /// Durable Goal read model at this projection cursor; absent when the
    /// extension is not composed. Journal facts never reconstruct it.
    pub goal: Option<crate::observation::goal::RuntimeClientGoalView>,
}

/// The context diagnostics carried by the Runtime Client snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientContextView {
    /// Independent current historical projection, including inherited activations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_head: Option<RuntimeClientProjectionHeadView>,
    /// Latest manual operation admitted in this runtime incarnation. Never inferred
    /// from a newer canonical checkpoint. Absence is insufficient evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_compaction: Option<RuntimeClientManualCompaction>,
    /// Latest pre-commit diagnostic; cleared when the next compaction starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_error: Option<String>,
    /// Latest native measured request or committed compacted-context estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupancy: Option<crate::observation::occupancy::RuntimeClientContextOccupancy>,
    /// Whether the runtime currently owns a context-compaction operation.
    /// This is live operation state, not inferred from token usage.
    pub compaction_in_progress: bool,
    /// Runtime Client projection statistic: the number of committed
    /// compaction completions folded into this read model. The compaction
    /// generation remains the conversation-owned identity.
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub compaction_count: u64,
    /// The latest committed compaction metadata, when compaction occurred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_compaction: Option<RuntimeClientCompactionView>,
}

/// The external attempt view of the Runtime Client projection.
///
/// The view folds attempt lifecycle, turn progress, in-flight agent
/// output, and foreground tool execution into one structured read model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientAttempt {
    /// Exact inline display identity, when bounded.
    pub attempt_id: Option<AttemptId>,
    /// Original native owner evidence when the inline alias is deferred.
    pub attempt_id_source: Option<crate::observation::turn::AttemptIdentitySource>,
    /// The externally meaningful attempt phase.
    pub phase: RuntimeClientAttemptPhase,
    /// The number of completed turns.
    pub turn: u32,
    /// The latest normalized usage of a completed model request, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<ModelUsage>,
    /// The in-flight Assistant output, when a message is streaming: enough
    /// accumulated state to repair every client-visible streaming effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlightAssistantMessage>,
    /// The foreground tool executions of the attempt in call-assembly
    /// order.
    #[serde(default)]
    pub foreground: Vec<ForegroundToolExecution>,
    /// The immutable model snapshot this attempt was admitted with.
    ///
    /// A client never has to infer "which model is this attempt actually
    /// using" from event ordering: the answer is here for the attempt's
    /// whole lifetime, even after the session moved on to another model.
    pub model: Option<Box<AttemptModelView>>,
    /// Unavailable for history lacking native admission evidence. Never derived from current resources.
    pub execution_settings: Option<super::settings::AdmittedSettings>,
}

/// Finite metadata for one original model name at its native request owner.
/// The original model JSON remains readable from the enclosing domain/item/path[].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientModelName {
    pub request_id: Option<crate::runtime::identity::RequestId>,
    pub request_id_source: Option<JournalFieldRef>,
    pub source_sequence: Option<u64>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub encoded_bytes: u64,
    pub value: Option<String>,
}

/// The externally meaningful phase of one attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum RuntimeClientAttemptPhase {
    /// A historical start has no recorded terminal or current live owner.
    Incomplete,
    /// The coordinator admitted the attempt; the loop has not started yet.
    Admitted,
    /// The attempt is executing.
    Running,
    /// The attempt settled; the terminal outcome is final and absorbing.
    Settled {
        /// The platform-level terminal settlement.
        outcome: RuntimeClientOutcome,
    },
}

/// The accumulated in-flight output of one streaming Assistant message.
///
/// A checkpoint carries bounded previews and exact publication frontiers.
/// Clients recover deferred bytes from those native references, independently
/// of advancing their Applied Cursor through complete display transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct InFlightAssistantMessage {
    /// The provisional message identity.
    pub message_id: MessageId,
    /// The ordered content blocks assembled so far.
    #[serde(default)]
    pub blocks: Vec<InFlightBlock>,
}

/// One ordered block of an in-flight Assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum InFlightBlock {
    /// Accumulated text of one output block.
    Text {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated text.
        text: String,
    },
    /// Accumulated reasoning of one output block.
    Reasoning {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated reasoning text.
        text: String,
    },
    /// Accumulated refusal of one output block.
    Refusal {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The accumulated refusal text.
        text: String,
    },
    /// One tool call being assembled.
    ToolCall {
        /// The canonical block index.
        block_index: ContentBlockIndex,
        /// The tool-call identity.
        call_id: Option<ToolCallId>,
        metadata_ref: Option<crate::observation::content::ContentRef>,
        /// The canonical tool identity.
        tool_id: Option<ToolId>,
        /// The model-facing tool name.
        name: String,
        /// The accumulated JSON argument fragments.
        arguments: String,
    },
}

/// Finite proposal metadata; exact provider identifiers and arguments can be deferred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<ToolCallId>,
    pub tool_id: Option<ToolId>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    pub metadata_ref: Option<crate::observation::content::ContentRef>,
}

/// The inbound mailbox diagnostics of the Runtime Client projection.
///
/// A read-only view of authoritative mailbox state: it validates the
/// Issue #22 ordering contract and supports reconnect, and it can never
/// drain or mutate the mailbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct InboundDiagnostics {
    /// The currently pending inbound items in runtime-assigned inbound
    /// sequence order.
    #[serde(default)]
    pub pending: Vec<InboundItemView>,
    /// The latest observed finite drain boundary, when any drain occurred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_drain: Option<InboundDrainView>,
}

/// One pending inbound item of the diagnostics view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct InboundItemView {
    /// Native compare-and-set revision.
    pub revision: u64,
    /// The mailbox-assigned inbound sequence.
    pub sequence: InboundSequence,
    /// The bounded native header defers part of its authoritative message.
    #[serde(default)]
    pub deferred: bool,
    /// The canonical inbound message.
    pub message: UserMessageBlock,
}

/// The latest observed finite drain boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct InboundDrainView {
    /// The highest selected inbound sequence.
    pub watermark: InboundSequence,
    /// The number of drained items.
    pub count: usize,
}

/// The external background execution read model.
///
/// Projected from the authoritative [`ConversationBackgroundRegistry`]
/// ([`crate::tools::background::ConversationBackgroundRegistry`]); the
/// container shape belongs to the Runtime Client protocol while the
/// lifecycle, progress, and result leaf types are stable runtime-owned
/// value contracts. No internal task handles or process ids ever appear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientJob {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_fields: Vec<Vec<String>>,
    /// Native Bash presentation, derived from the canonical invocation.
    pub bash: Option<crate::tools::types::BashPresentation>,
    /// The detached runtime execution identity.
    pub job_id: ToolExecutionId,
    /// The canonical tool identity.
    pub tool_id: Option<ToolId>,
    /// The model-facing tool name.
    pub tool_name: String,
    /// The authoritative lifecycle state.
    pub state: BackgroundLifecycle,
    /// The latest bounded progress, when any was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ToolProgress>,
    /// The bounded terminal result, when terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolExecutionResult>,
}

/// One durable child Agent, keyed by `agent_id` across every activation.
///
/// The registry owns lifecycle arbitration. Journal ownership facts rebuild
/// identity and activation correlation; canonical child history owns content.
/// `activation_id` identifies the latest activation, while `current_activation`
/// is absent when inactive or reserving the next activation. Neither replaces
/// the stable Agent identity. Activity is bounded observation, never authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientAgent {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_fields: Vec<Vec<String>>,
    pub title: String,
    pub parent_agent_id: crate::runtime::identity::AgentId,
    /// The most recently admitted finite activation identity.
    pub activation_id: crate::runtime::identity::SubagentId,
    /// The stable child Agent identity and canonical message provenance.
    pub agent_id: crate::runtime::identity::AgentId,
    /// The child's own durable conversation identity.
    pub child_conversation_id: ConversationId,
    /// The canonical named-agent identity frozen at start (Issue #144).
    pub agent: String,
    /// The deterministic definition digest frozen at start (Issue #144).
    ///
    /// A client observing an already-running child sees the definition it
    /// actually started with, so a later configuration adoption that redefines the
    /// same agent name can never be mistaken for a change to that child.
    pub definition_digest: String,
    /// The deterministic **effective execution profile** digest frozen at
    /// child start (Issue #258).
    ///
    /// It distinguishes two children of one named agent that an authorized
    /// invocation override specialized differently — the same
    /// `definition_digest`, different effective tools, Skills, or extensions.
    /// It is committed with durable ownership, so it survives a restart
    /// unchanged and a recovery-projected child reports the same value a live
    /// one does.
    ///
    /// It is a bounded correlation identity and nothing more: no effective
    /// selection, prompt, Skill body, or materialization detail is projected
    /// with it, and no authority decision reads it.
    pub profile_digest: String,
    /// The authoritative lifecycle state.
    pub state: crate::runtime::subagent::AgentState,
    pub current_activation: Option<crate::runtime::identity::SubagentId>,
    pub activation_state: crate::runtime::subagent::SubagentState,
    /// The bounded terminal failure/cancellation diagnostic, once known.
    ///
    /// A successful child's answer content never appears here (Issue
    /// #178): the durable terminal inbound publication is the one result
    /// channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The latest live activity projection reported by the child (Issue
    /// #178). Latest-value coalesced; never a lifecycle authority.
    pub observation: crate::runtime::subagent::SubagentObservation,
    /// The redacted execution profile frozen at child start (Issue #178);
    /// `None` for recovery-projected records. Named `execution_profile`
    /// on the wire: the bare `profile` key was the obsolete pre-#144
    /// profile-name field and stays retired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_profile: Option<crate::runtime::subagent::SubagentExecutionProfile>,
    /// When the ownership committed; clients derive elapsed time from it.
    pub started_at: DateTime<Utc>,
    /// The Session execution binding this Agent froze at creation.
    pub workspace: std::path::PathBuf,
}

/// The bounded number of composed Agent Status observations one snapshot
/// projects.
///
/// Agent Status is composed at most once per logical primary step, so this
/// window covers far more history than any client renders at once while
/// keeping the snapshot's size independent of conversation length.
///
/// This constant is the single authoritative retention policy. It is
/// deliberately not part of the wire contract and deliberately not mirrored
/// by any client: a client learns that a composition left the window only
/// from the eviction its own
/// [`RuntimeClientEvent::AgentStatusComposed`](super::RuntimeClientEvent)
/// fold carries. Changing this value therefore changes one behaviour in one
/// place, and no already-connected client can disagree with the change.
pub const AGENT_STATUS_WINDOW: usize = 64;

impl RuntimeClientSnapshot {
    /// The conversation identity the snapshot belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationWindow {
    pub cut: crate::durable::reading::ConversationReadCut,
    pub page: RuntimeClientTranscriptPage,
    pub newer_cursor: Option<RuntimeClientTranscriptCursor>,
    pub target: Option<crate::observation::turn::ObservedTurnIdentity>,
    pub target_cursor: Option<RuntimeClientTranscriptCursor>,
}

/// Bounded read evidence for the most recently admitted manual compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientManualCompaction {
    /// Caller correlation; absent for an uncorrelated native caller.
    pub request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    /// False until coordinator state is restored and maintenance released.
    pub released: bool,
    /// Exact pre-commit failure diagnostic, if released unsuccessfully.
    pub error: Option<String>,
}

/// Derived availability of the existing native Session model authority.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientModelStatus {
    pub available: bool,
    pub body_deferred: bool,
    /// Existing native Session binding revision; absent for an unbound read source.
    pub binding_revision: Option<u64>,
}
