//! The closed, bounded Agent Status engine (Issues #129 and #130).
//!
//! Agent Status is optional, provider-independent runtime context for an
//! already-established primary model turn. A settled tool batch can add
//! [`PostToolBatchStatusOpportunity`] to the next already-existing primary
//! step; it never schedules that step. The engine does not schedule work,
//! create a turn, or prolong an attempt:
//!
//! ```text
//! FreshInbound and/or PostToolBatch
//!     -> capture authoritative state once
//!     -> evaluate frozen snapshots once
//!     -> validate the code-owned payload mapping
//!     -> apply module-local semantic bounds
//!     -> whole-section UTF-8-byte admission
//!     -> optional AgentStatus User context message
//! ```
//!
//! The known modules are deliberately represented by a closed Rust enum. This
//! is not a provider registry or an extension SDK: adding a module requires an
//! intentional source change to the enum and its semantic source order.
//!
//! Module failures are optional-context failures. A failed module is
//! quarantined in the attempt-local engine and the surviving modules continue;
//! the failure never becomes a Context Assembly or model-turn failure.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::conversation::ConversationSurfaceSnapshot;
use crate::durable::{AgentStatusEmissionLookup, AgentStatusEmissionRecord};
use crate::message::types::{
    AgentStatusEmission, AgentStatusGenerationMetadata, AgentStatusModuleId, MessageBlock,
};
use crate::runtime::identity::MessageId;
use crate::tools::background::{BackgroundExecutionSnapshot, ConversationBackgroundRegistry};
use crate::tools::todo::{ConversationTodoList, TodoSnapshot, TodoStatus};
use crate::tools::types::ToolProgress;

/// The maximum number of active executions the Background module presents.
pub const MAX_BACKGROUND_STATUS_EXECUTIONS: usize = 8;

/// The maximum byte length of one dynamic Background status field.
///
/// This limit is applied to source fields before rendering. It is not a
/// substitute for the final Agent Status cap.
pub const MAX_BACKGROUND_STATUS_TEXT_BYTES: usize = 256;

/// The maximum number of Todo tasks included in one bounded status
/// contribution. The current in-progress task, when any, uses one slot.
pub const MAX_TODO_STATUS_TASKS: usize = 6;

/// The maximum byte length of one Todo task label included in Agent Status.
pub const MAX_TODO_STATUS_TEXT_BYTES: usize = 256;

/// The stable semantic identity of the active Todo reminder.
pub const TODO_STATUS_EMISSION_KEY: &str = "active_actionable";

/// The number of newly committed first requests of later logical primary model
/// steps that must follow an identical Todo reminder before it can be shown
/// again. The boundary is inclusive: a candidate at exactly `last + 4` is
/// eligible.
///
/// This is a Todo-specific semantic progress window, not a timer and not a
/// scheduler. The durable conversation store owns the bounded sequence. A
/// model-turn start advances it once; request-scoped context, Agent Status,
/// compaction, and provider-overflow retries do not add units of their own.
pub const TODO_STATUS_REMINDER_PROGRESS_INTERVAL: u64 = 4;

/// The final defensive Agent Status rendering bound, measured in UTF-8 bytes.
pub const GLOBAL_AGENT_STATUS_BYTE_CAP: usize = 4096;

/// Time is refreshed when the latest visible Time contribution reaches this
/// age. The threshold is inclusive.
pub const TIME_REFRESH_INTERVAL: ChronoDuration = ChronoDuration::minutes(30);

/// Background reminders are eligible after this many visible non-AgentStatus
/// canonical messages follow the latest visible Background contribution.
pub const BACKGROUND_REMINDER_MESSAGE_INTERVAL: usize = 8;

const DEFAULT_ENABLED: bool = true;

/// Launch-scoped Agent Status configuration.
///
/// Omitting `agentStatus` or either nested module enables both built-in
/// modules. Unknown fields remain rejected by the surrounding strict serde
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct AgentStatusConfig {
    /// The Time module configuration.
    #[serde(default)]
    pub time: TimeStatusConfig,
    /// The Background module configuration.
    #[serde(default)]
    pub background: BackgroundStatusConfig,
}

/// Launch-scoped configuration for the Time status module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct TimeStatusConfig {
    /// Whether Time participates in an available Agent Status opportunity.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The optional IANA timezone used only by Time presentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<Tz>,
}

impl Default for TimeStatusConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
            timezone: None,
        }
    }
}

/// Launch-scoped configuration for the Background status module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct BackgroundStatusConfig {
    /// Whether Background participates in an available Agent Status opportunity.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_enabled() -> bool {
    DEFAULT_ENABLED
}

impl Default for BackgroundStatusConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
        }
    }
}

impl AgentStatusModuleId {
    /// The stable section identity carried by the internal status value.
    #[must_use]
    pub const fn section_id(self) -> &'static str {
        match self {
            Self::Time => AgentStatusSectionId::TEMPORAL,
            Self::Background => AgentStatusSectionId::BACKGROUND_EXECUTION,
            Self::Todo => AgentStatusSectionId::TODO,
        }
    }
}

/// The inbound Agent Status delivery opportunity. Its inbound identity is
/// retained separately from the status message identity, which does not exist
/// until Context Assembly stages it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshInboundStatusOpportunity {
    /// The final canonical inbound message that made this opportunity
    /// eligible.
    pub target_message_id: MessageId,
}

/// The batch-level `PostToolBatch` opportunity.
///
/// The marker intentionally carries no durable identity or payload. Its
/// existence means only that one complete canonical `ToolResult` batch settled
/// before this primary step; it cannot be reconstructed after an attempt
/// dies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PostToolBatchStatusOpportunity;

/// The opportunities available to one logical primary step.
///
/// `FreshInbound` and `PostToolBatch` are independent members rather than
/// mutually exclusive alternatives. A module receives this whole set once,
/// so matching multiple present opportunities still produces one capture and
/// one evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentStatusOpportunitySet {
    /// The `FreshInbound` opportunity, when one is present.
    pub fresh_inbound: Option<FreshInboundStatusOpportunity>,
    /// The complete settled tool-batch opportunity, when one is pending for
    /// this attempt's next primary step.
    pub post_tool_batch: Option<PostToolBatchStatusOpportunity>,
}

/// One bounded Todo task shown by Agent Status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoStatusTask {
    /// The conversation-owned task id.
    pub id: u64,
    /// The bounded task subject.
    pub subject: String,
    /// The bounded in-progress label, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// The authoritative committed lifecycle status.
    pub status: TodoStatus,
    /// Whether an active dependency still blocks this task.
    pub blocked: bool,
}

/// The bounded semantic Todo presentation and fingerprint input.
///
/// Tasks are in conversation creation order. `current` is the first committed
/// `InProgress` task, when any; `tasks` contains the remaining committed active
/// tasks up to the explicit module bound. Counts cover the complete committed
/// snapshot, while `omitted_count` is the number of active tasks not shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TodoStatusPresentation {
    /// The first committed `InProgress` task, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<TodoStatusTask>,
    /// Remaining active tasks in deterministic creation order.
    #[serde(default)]
    pub tasks: Vec<TodoStatusTask>,
    /// Number of committed Pending or `InProgress` tasks.
    pub active_count: usize,
    /// Number of active tasks with an unresolved active dependency.
    pub blocked_count: usize,
    /// Number of committed Completed tasks.
    pub completed_count: usize,
    /// Number of committed Deleted tasks.
    pub deleted_count: usize,
    /// Number of active tasks omitted by the bounded presentation.
    pub omitted_count: usize,
}

/// The structured data of one accepted Agent Status section.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatusSectionData {
    /// The Time module's typed presentation payload.
    Temporal {
        /// The UTC clock instant captured for this generation.
        current_time: DateTime<Utc>,
        /// The configured IANA timezone, when known.
        timezone: Option<Tz>,
    },
    /// The Background module's typed presentation payload.
    BackgroundExecution {
        /// The bounded active execution entries in registry allocation order.
        executions: Vec<BackgroundExecutionSnapshot>,
        /// Active executions omitted by the module-local entry bound.
        omitted_count: usize,
    },
    /// The bounded conversation-owned Todo payload.
    Todo {
        /// The bounded semantic Todo presentation.
        presentation: TodoStatusPresentation,
    },
}

/// The stable identity of one Agent Status section.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentStatusSectionId(String);

impl AgentStatusSectionId {
    /// The stable Time section id.
    pub const TEMPORAL: &'static str = "temporal";
    /// The stable Background section id.
    pub const BACKGROUND_EXECUTION: &'static str = "background_execution";
    /// The stable Todo section id.
    pub const TODO: &'static str = "todo";

    /// Creates a section id for internal projection construction.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the raw section id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for AgentStatusSectionId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One accepted structured Agent Status section.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatusSection {
    /// The stable section identity.
    pub id: AgentStatusSectionId,
    /// The typed section payload.
    pub data: AgentStatusSectionData,
}

/// One accepted Agent Status generation.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatus {
    /// The one Agent Status clock instant used for this generation.
    pub generated_at: DateTime<Utc>,
    /// Sections in rustX semantic source order.
    pub sections: Vec<AgentStatusSection>,
}

impl AgentStatus {
    /// Returns the durable descriptor attached to the canonical Agent Status
    /// message for this generation.
    ///
    /// # Panics
    ///
    /// Panics if called on a hand-constructed `AgentStatus` that contains no
    /// valid closed-engine sections. Production generations are assembled by
    /// the closed engine and always satisfy this invariant.
    #[must_use]
    pub fn generation_metadata(&self) -> AgentStatusGenerationMetadata {
        let modules: Vec<AgentStatusModuleId> = self
            .sections
            .iter()
            .map(|section| match section.id.as_str() {
                AgentStatusSectionId::TEMPORAL => AgentStatusModuleId::Time,
                AgentStatusSectionId::BACKGROUND_EXECUTION => AgentStatusModuleId::Background,
                AgentStatusSectionId::TODO => AgentStatusModuleId::Todo,
                _ => unreachable!("the closed Agent Status engine emitted an unknown section"),
            })
            .collect();
        AgentStatusGenerationMetadata::new(self.generated_at, modules)
            .expect("the closed Agent Status engine emits a valid generation")
    }
}

impl AgentStatusOpportunitySet {
    /// Whether this logical step has no Agent Status delivery opportunity.
    ///
    /// Delivery opportunity is deliberately independent from module trigger
    /// policy. Both members may be present in one set, and the set is consumed
    /// once by the closed engine.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fresh_inbound.is_none() && self.post_tool_batch.is_none()
    }
}

/// One canonical message body resolved for an active Surface identity.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMessageView {
    /// The active Surface identity.
    pub id: MessageId,
    /// The immutable canonical body resolved from the Message Ledger.
    pub message: MessageBlock,
}

/// An invalid identity/body projection supplied to an Agent Status Surface
/// view boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatusSurfaceViewError {
    /// A frozen snapshot must contain one canonical body for every active
    /// identity.
    IdentityBodyCountMismatch {
        /// Number of active identities in the snapshot.
        active_message_ids: usize,
        /// Number of canonical bodies in the snapshot.
        messages: usize,
    },
    /// A keyed canonical body does not have the identity assigned to its
    /// active Surface position.
    IdentityBodyMismatch {
        /// Identity from the Surface projection.
        expected: MessageId,
        /// Identity found in the hydrated canonical body.
        actual: MessageId,
    },
}

impl core::fmt::Display for AgentStatusSurfaceViewError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IdentityBodyCountMismatch {
                active_message_ids,
                messages,
            } => write!(
                f,
                "Agent Status Surface snapshot has {active_message_ids} identities and {messages} bodies"
            ),
            Self::IdentityBodyMismatch { expected, actual } => write!(
                f,
                "Agent Status Surface identity {expected} contains canonical body {actual}"
            ),
        }
    }
}

impl std::error::Error for AgentStatusSurfaceViewError {}

/// The finite immutable model-visible Surface projection used by Agent Status.
///
/// This value is built once at the status preparation boundary from one
/// Surface head and keyed Ledger hydration. It contains no authoritative
/// domain state and no durable emission history. The small latest-status index
/// is derived from the same private immutable message slice; it is not a
/// second mutable Surface authority. There is no public constructor that can
/// bypass the identity/body validation.
///
/// The view's representation is intentionally private: callers can inspect a
/// frozen snapshot but cannot replace one of its identities, bodies, or
/// derived indexes after construction.
///
/// ```compile_fail
/// use rustx::context::AgentStatusSurfaceView;
///
/// fn mutate(view: &mut AgentStatusSurfaceView) {
///     view.revision = rustx::conversation::SurfaceRevision::INITIAL;
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatusSurfaceView {
    revision: crate::conversation::SurfaceRevision,
    compaction_generation: u64,
    active_message_ids: Arc<[MessageId]>,
    messages: Arc<[SurfaceMessageView]>,
    latest_by_module: [Option<VisibleAgentStatus>; 3],
}

/// The latest visible Agent Status generation containing one module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleAgentStatus {
    /// The canonical Agent Status message identity.
    pub message_id: MessageId,
    /// The structured generation timestamp.
    pub generated_at: DateTime<Utc>,
}

impl AgentStatusSurfaceView {
    /// Builds a Surface view from one already-frozen conversation snapshot.
    ///
    /// The snapshot has already established the Surface/Message Ledger read
    /// boundary. This constructor only derives the bounded semantic index.
    ///
    /// # Errors
    ///
    /// Returns an error if the snapshot has a missing body or an identity/body
    /// mismatch. Durable decoding and the Conversation Surface hydration path
    /// therefore fail closed instead of materializing an inconsistent view.
    pub fn from_snapshot(
        snapshot: ConversationSurfaceSnapshot,
    ) -> Result<Self, AgentStatusSurfaceViewError> {
        if snapshot.active_message_ids.len() != snapshot.messages.len() {
            return Err(AgentStatusSurfaceViewError::IdentityBodyCountMismatch {
                active_message_ids: snapshot.active_message_ids.len(),
                messages: snapshot.messages.len(),
            });
        }
        let active_message_ids: Arc<[MessageId]> =
            Arc::from(snapshot.active_message_ids.into_boxed_slice());
        let messages = active_message_ids
            .iter()
            .cloned()
            .zip(snapshot.messages)
            .map(|(id, message)| SurfaceMessageView { id, message })
            .collect::<Vec<_>>();
        Self::from_parts(
            snapshot.revision,
            snapshot.compaction_generation,
            active_message_ids,
            Arc::from(messages.into_boxed_slice()),
        )
    }

    /// Builds a view from one captured ordered identity/body set.
    ///
    /// This internal constructor is also used by focused unit tests. It is
    /// fallible for the same reason as [`Self::from_snapshot`], so tests cannot
    /// construct a view whose derived indexes disagree with its identity/body
    /// sequence.
    #[cfg(test)]
    pub(crate) fn for_test(
        revision: crate::conversation::SurfaceRevision,
        compaction_generation: u64,
        active_message_ids: Arc<[MessageId]>,
        messages: Arc<[SurfaceMessageView]>,
    ) -> Result<Self, AgentStatusSurfaceViewError> {
        Self::from_parts(
            revision,
            compaction_generation,
            active_message_ids,
            messages,
        )
    }

    fn from_parts(
        revision: crate::conversation::SurfaceRevision,
        compaction_generation: u64,
        active_message_ids: Arc<[MessageId]>,
        messages: Arc<[SurfaceMessageView]>,
    ) -> Result<Self, AgentStatusSurfaceViewError> {
        if active_message_ids.len() != messages.len() {
            return Err(AgentStatusSurfaceViewError::IdentityBodyCountMismatch {
                active_message_ids: active_message_ids.len(),
                messages: messages.len(),
            });
        }
        let mut latest_by_module = std::array::from_fn(|_| None);
        for (index, message) in messages.iter().enumerate() {
            if active_message_ids[index] != message.id {
                return Err(AgentStatusSurfaceViewError::IdentityBodyMismatch {
                    expected: active_message_ids[index].clone(),
                    actual: message.id.clone(),
                });
            }
            if message.id != *message.message.id() {
                return Err(AgentStatusSurfaceViewError::IdentityBodyMismatch {
                    expected: message.id.clone(),
                    actual: message.message.id().clone(),
                });
            }
            let Some(metadata) = message.message.agent_status_metadata() else {
                continue;
            };
            for module in metadata.modules() {
                latest_by_module[module_index(*module)] = Some(VisibleAgentStatus {
                    message_id: message.id.clone(),
                    generated_at: metadata.generated_at(),
                });
            }
        }
        Ok(Self {
            revision,
            compaction_generation,
            active_message_ids,
            messages,
            latest_by_module,
        })
    }

    /// The exact Surface revision represented by this view.
    #[must_use]
    pub fn revision(&self) -> crate::conversation::SurfaceRevision {
        self.revision
    }

    /// The compaction generation represented by this view.
    #[must_use]
    pub fn compaction_generation(&self) -> u64 {
        self.compaction_generation
    }

    /// Active identities in model-visible order.
    #[must_use]
    pub fn active_message_ids(&self) -> &[MessageId] {
        &self.active_message_ids
    }

    /// Keyed canonical bodies in the same order as [`Self::active_message_ids`].
    #[must_use]
    pub fn messages(&self) -> &[SurfaceMessageView] {
        &self.messages
    }

    /// The latest visible Agent Status generation containing `module`.
    #[must_use]
    pub fn latest_status(&self, module: AgentStatusModuleId) -> Option<&VisibleAgentStatus> {
        self.latest_by_module[module_index(module)].as_ref()
    }

    /// Whether a visible Agent Status generation contains `module`.
    #[must_use]
    pub fn contains_status(&self, module: AgentStatusModuleId) -> bool {
        self.latest_status(module).is_some()
    }

    /// Counts active model-visible non-AgentStatus messages after `message_id`.
    ///
    /// Agent Status messages are deliberately excluded so reminders cannot
    /// self-excite. `None` means the supplied identity is not active in this
    /// frozen Surface.
    #[must_use]
    pub fn non_status_messages_since(&self, message_id: &MessageId) -> Option<usize> {
        let position = self
            .active_message_ids
            .iter()
            .position(|active| active == message_id)?;
        Some(
            self.messages
                .iter()
                .skip(position.saturating_add(1))
                .filter(|message| !message.message.is_agent_status())
                .count(),
        )
    }
}

const fn module_index(module: AgentStatusModuleId) -> usize {
    match module {
        AgentStatusModuleId::Time => 0,
        AgentStatusModuleId::Background => 1,
        AgentStatusModuleId::Todo => 2,
    }
}

/// The clock boundary of the Time module.
pub trait AgentStatusClock: Send + Sync {
    /// Returns the current UTC instant.
    fn now(&self) -> DateTime<Utc>;
}

/// The production UTC clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl AgentStatusClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// One closed internal Agent Status module.
///
/// The array in [`AgentStatusEngine`] is the semantic source order. No map,
/// registration sequence, or lexical sort participates in composition.
enum AgentStatusModule {
    /// The built-in Time module.
    Time(TimeStatusModule),
    /// The built-in Background module.
    Background(BackgroundStatusModule),
    /// The built-in conversation-owned Todo module.
    Todo(TodoStatusModule),
}

impl AgentStatusModule {
    fn id(&self) -> AgentStatusModuleId {
        match self {
            Self::Time(_) => AgentStatusModuleId::Time,
            Self::Background(_) => AgentStatusModuleId::Background,
            Self::Todo(_) => AgentStatusModuleId::Todo,
        }
    }

    fn enabled(&self) -> bool {
        match self {
            Self::Time(module) => module.config.enabled,
            Self::Background(module) => module.config.enabled,
            Self::Todo(_) => true,
        }
    }

    fn interested_in(&self, opportunities: &AgentStatusOpportunitySet) -> bool {
        // The production modules all inspect the same finite opportunity set
        // once. Time and Background retain #131's FreshInbound-only policy;
        // Todo is the first module whose reminder policy uses both delivery
        // opportunities. This is a set intersection, never one invocation per
        // opportunity member.
        match self {
            Self::Time(_) | Self::Background(_) => opportunities.fresh_inbound.is_some(),
            Self::Todo(_) => !opportunities.is_empty(),
        }
    }

    fn capture(
        &self,
        frozen: &AgentStatusEvaluationSnapshot<'_>,
        seam: Option<&AgentStatusTestSeam>,
    ) -> Result<AgentStatusModuleSnapshot, ModuleFailurePhase> {
        let id = self.id();
        if let Some(seam) = seam {
            seam.record_capture(id);
            if seam.take_capture_failure(id) {
                return Err(ModuleFailurePhase::Capture);
            }
        }
        match self {
            Self::Time(_) => Ok(AgentStatusModuleSnapshot::Time(TimeStatusModule::capture(
                frozen,
            ))),
            Self::Background(_) => Ok(AgentStatusModuleSnapshot::Background(
                BackgroundStatusModule::capture(frozen),
            )),
            Self::Todo(_) => Ok(AgentStatusModuleSnapshot::Todo(TodoStatusModule::capture(
                frozen,
            )?)),
        }
    }

    fn evaluate(
        &self,
        snapshot: &AgentStatusModuleSnapshot,
        now: DateTime<Utc>,
        seam: Option<&AgentStatusTestSeam>,
    ) -> Result<Option<AgentStatusPayload>, ModuleFailurePhase> {
        let id = self.id();
        if let Some(seam) = seam {
            seam.record_evaluate(id);
            if seam.take_evaluate_failure(id) {
                return Err(ModuleFailurePhase::Evaluate);
            }
        }
        let payload = match (self, snapshot) {
            (Self::Time(module), AgentStatusModuleSnapshot::Time(snapshot)) => {
                module.evaluate(snapshot)
            }
            (Self::Background(_), AgentStatusModuleSnapshot::Background(snapshot)) => {
                BackgroundStatusModule::evaluate(snapshot)
            }
            (Self::Todo(_), AgentStatusModuleSnapshot::Todo(snapshot)) => {
                TodoStatusModule::evaluate(snapshot)
            }
            _ => return Err(ModuleFailurePhase::Evaluate),
        };
        if seam.is_some_and(|value| value.take_payload_mismatch(id)) {
            return Ok(Some(match id {
                AgentStatusModuleId::Time => AgentStatusPayload::BackgroundExecution {
                    executions: Vec::new(),
                    omitted_count: 0,
                },
                AgentStatusModuleId::Background | AgentStatusModuleId::Todo => {
                    AgentStatusPayload::Temporal {
                        current_time: now,
                        timezone: None,
                    }
                }
            }));
        }
        Ok(payload)
    }
}

/// The code-owned Time module.
struct TimeStatusModule {
    config: TimeStatusConfig,
}

impl TimeStatusModule {
    fn capture(frozen: &AgentStatusEvaluationSnapshot<'_>) -> TimeStatusSnapshot {
        TimeStatusSnapshot {
            current_time: frozen.now,
            latest_visible: frozen
                .surface
                .latest_status(AgentStatusModuleId::Time)
                .cloned(),
        }
    }

    fn evaluate(&self, snapshot: &TimeStatusSnapshot) -> Option<AgentStatusPayload> {
        let eligible = snapshot.latest_visible.as_ref().is_none_or(|latest| {
            snapshot
                .current_time
                .signed_duration_since(latest.generated_at)
                >= TIME_REFRESH_INTERVAL
        });
        eligible.then_some(AgentStatusPayload::Temporal {
            current_time: snapshot.current_time,
            timezone: self.config.timezone,
        })
    }
}

/// The code-owned Background module.
struct BackgroundStatusModule {
    config: BackgroundStatusConfig,
}

impl BackgroundStatusModule {
    fn capture(frozen: &AgentStatusEvaluationSnapshot<'_>) -> BackgroundStatusSnapshot {
        let latest_visible = frozen
            .surface
            .latest_status(AgentStatusModuleId::Background)
            .cloned();
        let non_status_messages_since = latest_visible
            .as_ref()
            .and_then(|latest| frozen.surface.non_status_messages_since(&latest.message_id));
        BackgroundStatusSnapshot {
            executions: Arc::clone(&frozen.active_background),
            latest_visible,
            non_status_messages_since,
        }
    }

    fn evaluate(snapshot: &BackgroundStatusSnapshot) -> Option<AgentStatusPayload> {
        let eligible = !snapshot.executions.is_empty()
            && snapshot.latest_visible.as_ref().is_none_or(|_| {
                snapshot
                    .non_status_messages_since
                    .is_some_and(|count| count >= BACKGROUND_REMINDER_MESSAGE_INTERVAL)
            });
        if !eligible {
            return None;
        }
        let retained = snapshot
            .executions
            .iter()
            .take(MAX_BACKGROUND_STATUS_EXECUTIONS)
            .cloned()
            .map(bound_background_snapshot)
            .collect::<Vec<_>>();
        Some(AgentStatusPayload::BackgroundExecution {
            omitted_count: snapshot.executions.len().saturating_sub(retained.len()),
            executions: retained,
        })
    }
}

/// The code-owned Todo status module.
///
/// Todo state remains owned by [`ConversationTodoList`]. This module only
/// builds a bounded view of its committed snapshot and applies the one
/// concrete reminder policy documented on [`Self::evaluate`].
struct TodoStatusModule;

impl TodoStatusModule {
    fn capture(
        frozen: &AgentStatusEvaluationSnapshot<'_>,
    ) -> Result<TodoStatusSnapshot, ModuleFailurePhase> {
        let latest_emission = frozen
            .emission_lookup
            .latest_agent_status_emission(AgentStatusModuleId::Todo, TODO_STATUS_EMISSION_KEY)
            .map_err(|_| ModuleFailurePhase::SuppressionLookup)?;
        let todo_progress = frozen
            .emission_lookup
            .current_todo_progress()
            .map_err(|_| ModuleFailurePhase::SuppressionLookup)?;
        Ok(TodoStatusSnapshot {
            committed: frozen.committed_todos.clone(),
            latest_emission,
            todo_progress,
        })
    }

    /// Emits exactly one bounded reminder when committed actionable work
    /// exists and either its semantic fingerprint changed or the explicit
    /// Todo model-progress reminder window elapsed since the last durable
    /// reminder for the stable Todo key. `FreshInbound` and `PostToolBatch`
    /// use this same policy; the opportunity set is eligibility, not a second
    /// trigger state machine.
    fn evaluate(snapshot: &TodoStatusSnapshot) -> Option<AgentStatusPayload> {
        let presentation = todo_presentation(&snapshot.committed);
        if presentation.active_count == 0 {
            return None;
        }
        let fingerprint = todo_fingerprint(&presentation);
        let eligible = match snapshot.latest_emission.as_ref() {
            None => true,
            Some(latest) => {
                latest.fingerprint != fingerprint
                    || snapshot
                        .todo_progress
                        .saturating_sub(latest.todo_progress_origin)
                        >= TODO_STATUS_REMINDER_PROGRESS_INTERVAL
            }
        };
        if !eligible {
            return None;
        }
        Some(AgentStatusPayload::Todo {
            presentation,
            emission: AgentStatusEmission {
                module_id: AgentStatusModuleId::Todo,
                key: TODO_STATUS_EMISSION_KEY.to_owned(),
                fingerprint,
            },
        })
    }
}

#[derive(Debug, Clone)]
struct TodoStatusSnapshot {
    committed: TodoSnapshot,
    latest_emission: Option<AgentStatusEmissionRecord>,
    todo_progress: u64,
}

#[derive(Debug, Clone)]
enum AgentStatusModuleSnapshot {
    Time(TimeStatusSnapshot),
    Background(BackgroundStatusSnapshot),
    Todo(TodoStatusSnapshot),
}

#[derive(Debug, Clone)]
struct TimeStatusSnapshot {
    current_time: DateTime<Utc>,
    latest_visible: Option<VisibleAgentStatus>,
}

#[derive(Debug, Clone)]
struct BackgroundStatusSnapshot {
    executions: Arc<[BackgroundExecutionSnapshot]>,
    latest_visible: Option<VisibleAgentStatus>,
    non_status_messages_since: Option<usize>,
}

/// The immutable inputs shared by every module in one logical primary step.
///
/// `now` and `active_background` are captured before module evaluation starts;
/// `surface` is the one already-frozen finite Pre-Status Surface view. A
/// module never reads the live conversation or registry directly.
struct AgentStatusEvaluationSnapshot<'a> {
    now: DateTime<Utc>,
    surface: &'a AgentStatusSurfaceView,
    active_background: Arc<[BackgroundExecutionSnapshot]>,
    committed_todos: TodoSnapshot,
    emission_lookup: &'a dyn AgentStatusEmissionLookup,
}

#[derive(Debug, Clone)]
enum AgentStatusPayload {
    Temporal {
        current_time: DateTime<Utc>,
        timezone: Option<Tz>,
    },
    BackgroundExecution {
        executions: Vec<BackgroundExecutionSnapshot>,
        omitted_count: usize,
    },
    Todo {
        presentation: TodoStatusPresentation,
        emission: AgentStatusEmission,
    },
}

#[derive(Debug, Clone, Copy)]
enum ModuleFailurePhase {
    Capture,
    Evaluate,
    PayloadValidation,
    SuppressionLookup,
}

impl ModuleFailurePhase {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Evaluate => "evaluate",
            Self::PayloadValidation => "payload_validation",
            Self::SuppressionLookup => "suppression_lookup",
        }
    }
}

/// The attempt-owned closed Agent Status engine.
pub struct AgentStatusEngine {
    config: AgentStatusConfig,
    clock: Arc<dyn AgentStatusClock>,
    modules: [AgentStatusModule; 3],
    quarantined: HashSet<AgentStatusModuleId>,
    #[cfg(test)]
    test_seam: Option<AgentStatusTestSeam>,
}

impl core::fmt::Debug for AgentStatusEngine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AgentStatusEngine")
            .field("config", &self.config)
            .field("semantic_order", &["time", "background", "todo"])
            .field("quarantined", &self.quarantined)
            .finish_non_exhaustive()
    }
}

impl Default for AgentStatusEngine {
    fn default() -> Self {
        Self::new(AgentStatusConfig::default(), Arc::new(SystemClock))
    }
}

impl AgentStatusEngine {
    /// Constructs an attempt-owned engine from launch-scoped configuration.
    #[must_use]
    pub fn new(config: AgentStatusConfig, clock: Arc<dyn AgentStatusClock>) -> Self {
        Self {
            clock,
            modules: [
                AgentStatusModule::Time(TimeStatusModule {
                    config: config.time.clone(),
                }),
                AgentStatusModule::Background(BackgroundStatusModule {
                    config: config.background.clone(),
                }),
                AgentStatusModule::Todo(TodoStatusModule),
            ],
            config,
            quarantined: HashSet::new(),
            #[cfg(test)]
            test_seam: None,
        }
    }

    /// Constructs the fresh engine for a new attempt from this conversation's
    /// launch-scoped status configuration.
    ///
    /// The returned engine deliberately starts with an empty quarantine set;
    /// quarantine belongs to the attempt and must never leak into a later
    /// attempt. This explicit lifecycle operation is distinct from cloning an
    /// attempt-owned engine (which is not supported).
    #[must_use]
    pub fn for_attempt(&self) -> Self {
        let engine = Self::new(self.config.clone(), self.clock());
        #[cfg(test)]
        let engine = {
            // Share the deterministic seam with runtime-created attempts;
            // the mutable quarantine state remains local to this engine.
            let mut engine = engine;
            engine.test_seam = self.test_seam.clone();
            engine
        };
        engine
    }

    /// Returns the launch-scoped configuration carried by this engine.
    #[must_use]
    pub fn config(&self) -> &AgentStatusConfig {
        &self.config
    }

    fn clock(&self) -> Arc<dyn AgentStatusClock> {
        Arc::clone(&self.clock)
    }

    /// Attaches the deterministic in-crate failure/counting seam.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_test_seam(mut self, seam: AgentStatusTestSeam) -> Self {
        self.test_seam = Some(seam);
        self
    }

    /// Captures, evaluates, validates, and bounds one Agent Status generation.
    /// The engine's module array is traversed exactly in source order: Time,
    /// Background, then Todo. The caller supplies the one immutable Pre-Status
    /// Surface view and the conversation-owned committed Todo authority. Every
    /// module sees one finite opportunity set, even when both members are
    /// present.
    #[must_use]
    pub(crate) fn prepare_with_inputs(
        &mut self,
        opportunities: &AgentStatusOpportunitySet,
        surface: &AgentStatusSurfaceView,
        background: &ConversationBackgroundRegistry,
        todos: &ConversationTodoList,
        emission_lookup: &dyn AgentStatusEmissionLookup,
    ) -> Option<PreparedAgentStatus> {
        if opportunities.is_empty() {
            return None;
        }
        let frozen = AgentStatusEvaluationSnapshot {
            now: self.clock.now(),
            surface,
            active_background: Arc::from(background.active_snapshot().into_boxed_slice()),
            committed_todos: todos.committed(),
            emission_lookup,
        };
        #[cfg(test)]
        let seam = self.test_seam.clone();
        #[cfg(not(test))]
        let seam = None;
        let mut sections = Vec::new();
        for index in 0..self.modules.len() {
            let module = &self.modules[index];
            let id = module.id();
            if !module.enabled()
                || self.quarantined.contains(&id)
                || !module.interested_in(opportunities)
            {
                continue;
            }
            let result = (|| {
                let snapshot = module.capture(&frozen, seam.as_ref())?;
                #[cfg(test)]
                if let Some(seam) = seam.as_ref() {
                    seam.run_after_capture(id);
                }
                let Some(payload) = module.evaluate(&snapshot, frozen.now, seam.as_ref())? else {
                    return Ok(None);
                };
                validate_payload(id, payload)
            })();
            match result {
                Ok(Some(contribution)) => sections.push(contribution),
                Ok(None) => {}
                Err(phase) => self.quarantine(id, phase),
            }
        }
        let (status, emissions) = admit_sections(sections, frozen.now);
        (!status.sections.is_empty()).then_some(PreparedAgentStatus { status, emissions })
    }

    /// Test-only convenience for the pre-Todo module unit suite. Production
    /// execution always supplies the real conversation Todo authority and
    /// bounded durable emission lookup through [`Self::prepare_with_inputs`].
    #[cfg(test)]
    pub(crate) fn prepare(
        &mut self,
        opportunities: &AgentStatusOpportunitySet,
        surface: &AgentStatusSurfaceView,
        background: &ConversationBackgroundRegistry,
    ) -> Option<AgentStatus> {
        let todos = ConversationTodoList::new(crate::runtime::identity::ConversationId::new(
            "agent-status-test",
        ));
        let lookup = EmptyEmissionLookup;
        self.prepare_with_inputs(opportunities, surface, background, &todos, &lookup)
            .map(|prepared| prepared.status)
    }

    fn quarantine(&mut self, id: AgentStatusModuleId, phase: ModuleFailurePhase) {
        self.quarantined.insert(id);
        tracing::warn!(
            module = id.as_str(),
            phase = phase.as_str(),
            "Agent Status module contribution quarantined for this attempt"
        );
    }
}

/// The accepted status and its request-scoped durable emission metadata.
///
/// The Agent Loop attaches this value to the exact prepared Agent Status
/// message before the model-turn-start transaction. It is not a free-floating
/// emission list and has no durable effect during preparation.
#[derive(Debug, Clone)]
pub(crate) struct PreparedAgentStatus {
    /// The one bounded status generation.
    pub(crate) status: AgentStatus,
    /// The semantic emissions represented by admitted sections.
    pub(crate) emissions: Vec<AgentStatusEmission>,
}

#[cfg(test)]
struct EmptyEmissionLookup;

#[cfg(test)]
impl AgentStatusEmissionLookup for EmptyEmissionLookup {
    fn latest_agent_status_emission(
        &self,
        _module_id: AgentStatusModuleId,
        _key: &str,
    ) -> Result<Option<AgentStatusEmissionRecord>, crate::durable::ConversationStoreError> {
        Ok(None)
    }

    fn current_todo_progress(&self) -> Result<u64, crate::durable::ConversationStoreError> {
        Ok(0)
    }
}

fn validate_payload(
    id: AgentStatusModuleId,
    payload: AgentStatusPayload,
) -> Result<Option<AgentStatusContribution>, ModuleFailurePhase> {
    match (id, payload) {
        (
            AgentStatusModuleId::Time,
            AgentStatusPayload::Temporal {
                current_time,
                timezone,
            },
        ) => Ok(Some(AgentStatusContribution {
            section: AgentStatusSection {
                id: AgentStatusSectionId::new(id.section_id()),
                data: AgentStatusSectionData::Temporal {
                    current_time,
                    timezone,
                },
            },
            emission: None,
        })),
        (
            AgentStatusModuleId::Background,
            AgentStatusPayload::BackgroundExecution {
                executions,
                omitted_count,
            },
        ) if !executions.is_empty() || omitted_count > 0 => Ok(Some(AgentStatusContribution {
            section: AgentStatusSection {
                id: AgentStatusSectionId::new(id.section_id()),
                data: AgentStatusSectionData::BackgroundExecution {
                    executions,
                    omitted_count,
                },
            },
            emission: None,
        })),
        (AgentStatusModuleId::Background, AgentStatusPayload::BackgroundExecution { .. }) => {
            Ok(None)
        }
        (
            AgentStatusModuleId::Todo,
            AgentStatusPayload::Todo {
                presentation,
                emission,
            },
        ) if presentation.active_count > 0
            && emission.module_id == AgentStatusModuleId::Todo
            && emission.key == TODO_STATUS_EMISSION_KEY
            && !emission.fingerprint.is_empty() =>
        {
            Ok(Some(AgentStatusContribution {
                section: AgentStatusSection {
                    id: AgentStatusSectionId::new(id.section_id()),
                    data: AgentStatusSectionData::Todo { presentation },
                },
                emission: Some(emission),
            }))
        }
        _ => Err(ModuleFailurePhase::PayloadValidation),
    }
}

struct AgentStatusContribution {
    section: AgentStatusSection,
    emission: Option<AgentStatusEmission>,
}

/// Applies the global defensive byte cap.
///
/// Admission is whole-section and semantic-order based. Every candidate is
/// rendered from scratch, so separators are accounted for using the retained
/// set. If a section is too large, later sections still get a chance to fit;
/// no rendered wrapper or UTF-8 string is ever byte-sliced.
fn admit_sections(
    sections: Vec<AgentStatusContribution>,
    generated_at: DateTime<Utc>,
) -> (AgentStatus, Vec<AgentStatusEmission>) {
    let mut accepted = Vec::new();
    let mut emissions = Vec::new();
    for contribution in sections {
        let mut candidate = accepted.clone();
        candidate.push(contribution.section.clone());
        if render_sections(&candidate).len() <= GLOBAL_AGENT_STATUS_BYTE_CAP {
            accepted.push(contribution.section);
            if let Some(emission) = contribution.emission {
                emissions.push(emission);
            }
        }
    }
    let status = AgentStatus {
        generated_at,
        sections: accepted,
    };
    let rendered = render_sections(&status.sections);
    assert!(
        rendered.len() <= GLOBAL_AGENT_STATUS_BYTE_CAP,
        "Agent Status renderer exceeded its global UTF-8 byte cap"
    );
    (status, emissions)
}

fn bound_background_snapshot(
    mut snapshot: BackgroundExecutionSnapshot,
) -> BackgroundExecutionSnapshot {
    snapshot.tool_name = bound_status_text(snapshot.tool_name);
    snapshot.progress = snapshot.progress.map(|progress| ToolProgress {
        message: progress.message.map(bound_status_text),
        completed: progress.completed,
        total: progress.total,
    });
    snapshot
}

fn bound_status_text(text: String) -> String {
    bound_status_text_to(text, MAX_BACKGROUND_STATUS_TEXT_BYTES)
}

fn bound_status_text_to(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let marker = "…";
    let limit = max_bytes.saturating_sub(marker.len());
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = text[..end].to_owned();
    bounded.push_str(marker);
    bounded
}

fn todo_presentation(snapshot: &TodoSnapshot) -> TodoStatusPresentation {
    let states = snapshot
        .tasks
        .iter()
        .map(|task| (task.id, task.status))
        .collect::<BTreeMap<_, _>>();
    let active = snapshot
        .tasks
        .iter()
        .filter(|task| matches!(task.status, TodoStatus::Pending | TodoStatus::InProgress));
    let active_count = active.clone().count();
    let blocked_count = active
        .clone()
        .filter(|task| {
            task.blocked_by.iter().any(|blocker| {
                states.get(blocker).is_some_and(|status| {
                    matches!(status, TodoStatus::Pending | TodoStatus::InProgress)
                })
            })
        })
        .count();
    let completed_count = snapshot
        .tasks
        .iter()
        .filter(|task| task.status == TodoStatus::Completed)
        .count();
    let deleted_count = snapshot
        .tasks
        .iter()
        .filter(|task| task.status == TodoStatus::Deleted)
        .count();

    let current_id = snapshot
        .tasks
        .iter()
        .find(|task| task.status == TodoStatus::InProgress)
        .map(|task| task.id);
    let mut current = None;
    let mut tasks = Vec::new();
    let task_limit = if current_id.is_some() {
        MAX_TODO_STATUS_TASKS.saturating_sub(1)
    } else {
        MAX_TODO_STATUS_TASKS
    };
    for task in snapshot
        .tasks
        .iter()
        .filter(|task| matches!(task.status, TodoStatus::Pending | TodoStatus::InProgress))
    {
        let bounded = todo_status_task(task, &states);
        if Some(task.id) == current_id {
            current = Some(bounded);
        } else if tasks.len() < task_limit {
            tasks.push(bounded);
        }
    }
    let displayed = usize::from(current.is_some()) + tasks.len();
    TodoStatusPresentation {
        current,
        tasks,
        active_count,
        blocked_count,
        completed_count,
        deleted_count,
        omitted_count: active_count.saturating_sub(displayed),
    }
}

fn todo_status_task(
    task: &crate::tools::todo::TodoTask,
    states: &BTreeMap<u64, TodoStatus>,
) -> TodoStatusTask {
    TodoStatusTask {
        id: task.id,
        subject: bound_status_text_to(task.subject.clone(), MAX_TODO_STATUS_TEXT_BYTES),
        active_form: task
            .active_form
            .clone()
            .map(|value| bound_status_text_to(value, MAX_TODO_STATUS_TEXT_BYTES)),
        status: task.status,
        blocked: task.blocked_by.iter().any(|blocker| {
            states.get(blocker).is_some_and(|status| {
                matches!(status, TodoStatus::Pending | TodoStatus::InProgress)
            })
        }),
    }
}

fn todo_fingerprint(presentation: &TodoStatusPresentation) -> String {
    let encoded =
        serde_json::to_vec(presentation).expect("Todo status presentation is serializable");
    let digest = Sha256::digest(encoded);
    format!("{digest:x}")
}

fn render_instant(instant: DateTime<Utc>, timezone: Option<Tz>) -> String {
    let timezone = timezone.unwrap_or(chrono_tz::UTC);
    instant
        .with_timezone(&timezone)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn render_sections(sections: &[AgentStatusSection]) -> String {
    let mut lines = Vec::new();
    for section in sections {
        match &section.data {
            AgentStatusSectionData::Temporal {
                current_time,
                timezone,
            } => {
                lines.push(format!("Timezone: {}", timezone.map_or("UTC", Tz::name)));
                lines.push(format!(
                    "Current time: {}",
                    render_instant(*current_time, *timezone)
                ));
            }
            AgentStatusSectionData::BackgroundExecution {
                executions,
                omitted_count,
            } => {
                if executions.is_empty() && *omitted_count == 0 {
                    continue;
                }
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push("Background executions:".to_owned());
                for execution in executions {
                    let mut line = format!(
                        "- {} | {} | {}",
                        execution.execution_id.as_str(),
                        execution.tool_name,
                        execution.state.name()
                    );
                    if let Some(progress) = &execution.progress
                        && let Some(message) = &progress.message
                    {
                        line.push_str(" | ");
                        line.push_str(message);
                    }
                    lines.push(line);
                }
                if *omitted_count > 0 {
                    lines.push(format!("- … and {omitted_count} more active executions"));
                }
            }
            AgentStatusSectionData::Todo { presentation } => {
                if presentation.active_count == 0 {
                    continue;
                }
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(format!(
                    "Todo: {} active ({} blocked, {} completed, {} deleted)",
                    presentation.active_count,
                    presentation.blocked_count,
                    presentation.completed_count,
                    presentation.deleted_count,
                ));
                if let Some(current) = &presentation.current {
                    lines.push(format_todo_task("Current", current));
                }
                for task in &presentation.tasks {
                    lines.push(format_todo_task("Next", task));
                }
                if presentation.omitted_count > 0 {
                    lines.push(format!(
                        "- … and {} more active Todo tasks",
                        presentation.omitted_count
                    ));
                }
            }
        }
    }
    let mut rendered = String::from("<system-reminder>\n");
    rendered.push_str(&lines.join("\n"));
    rendered.push('\n');
    rendered.push_str("</system-reminder>");
    rendered
}

fn format_todo_task(label: &str, task: &TodoStatusTask) -> String {
    let blocked = if task.blocked { " | blocked" } else { "" };
    let subject = if task.status == TodoStatus::InProgress {
        task.active_form.as_deref().unwrap_or(&task.subject)
    } else {
        &task.subject
    };
    format!(
        "- {label} #{} | {subject} | {}{blocked}",
        task.id, task.status
    )
}

/// Renders the already-admitted Agent Status generation.
///
/// The engine performs whole-section admission before this function is
/// called. The assertion protects the canonical renderer if a future caller
/// constructs a status outside the engine.
///
/// # Panics
///
/// Panics if the supplied status renders above the global UTF-8-byte cap.
#[must_use]
pub fn render_agent_status(status: &AgentStatus) -> String {
    let rendered = render_sections(&status.sections);
    assert!(
        rendered.len() <= GLOBAL_AGENT_STATUS_BYTE_CAP,
        "Agent Status renderer exceeded its global UTF-8 byte cap"
    );
    rendered
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct AgentStatusTestSeam {
    state: Arc<AgentStatusTestState>,
}

#[cfg(not(test))]
struct AgentStatusTestSeam;

#[cfg(not(test))]
#[allow(clippy::unused_self)]
impl AgentStatusTestSeam {
    fn record_capture(&self, _module: AgentStatusModuleId) {}

    fn take_capture_failure(&self, _module: AgentStatusModuleId) -> bool {
        false
    }

    fn record_evaluate(&self, _module: AgentStatusModuleId) {}

    fn take_evaluate_failure(&self, _module: AgentStatusModuleId) -> bool {
        false
    }

    fn take_payload_mismatch(&self, _module: AgentStatusModuleId) -> bool {
        false
    }
}

#[cfg(test)]
type AfterCaptureHook = Arc<dyn Fn(AgentStatusModuleId) + Send + Sync>;

#[cfg(test)]
struct AgentStatusTestState {
    capture_time: std::sync::atomic::AtomicUsize,
    capture_background: std::sync::atomic::AtomicUsize,
    capture_todo: std::sync::atomic::AtomicUsize,
    evaluate_time: std::sync::atomic::AtomicUsize,
    evaluate_background: std::sync::atomic::AtomicUsize,
    evaluate_todo: std::sync::atomic::AtomicUsize,
    capture_failure: std::sync::Mutex<Option<AgentStatusModuleId>>,
    evaluate_failure: std::sync::Mutex<Option<AgentStatusModuleId>>,
    payload_mismatch: std::sync::Mutex<Option<AgentStatusModuleId>>,
    after_capture: std::sync::Mutex<Option<AfterCaptureHook>>,
}

#[cfg(test)]
impl AgentStatusTestSeam {
    /// Creates an empty deterministic status test seam.
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(AgentStatusTestState {
                capture_time: std::sync::atomic::AtomicUsize::new(0),
                capture_background: std::sync::atomic::AtomicUsize::new(0),
                capture_todo: std::sync::atomic::AtomicUsize::new(0),
                evaluate_time: std::sync::atomic::AtomicUsize::new(0),
                evaluate_background: std::sync::atomic::AtomicUsize::new(0),
                evaluate_todo: std::sync::atomic::AtomicUsize::new(0),
                capture_failure: std::sync::Mutex::new(None),
                evaluate_failure: std::sync::Mutex::new(None),
                payload_mismatch: std::sync::Mutex::new(None),
                after_capture: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Fails exactly one future capture of `module`.
    pub(crate) fn fail_capture_once(&self, module: AgentStatusModuleId) {
        *self
            .state
            .capture_failure
            .lock()
            .expect("capture failure lock") = Some(module);
    }

    /// Fails exactly one future evaluation of `module`.
    pub(crate) fn fail_evaluate_once(&self, module: AgentStatusModuleId) {
        *self
            .state
            .evaluate_failure
            .lock()
            .expect("evaluate failure lock") = Some(module);
    }

    /// Forces exactly one module/payload ownership mismatch.
    pub(crate) fn mismatch_once(&self, module: AgentStatusModuleId) {
        *self
            .state
            .payload_mismatch
            .lock()
            .expect("payload mismatch lock") = Some(module);
    }

    /// Installs a callback invoked between capture and evaluation.
    pub(crate) fn after_capture(
        &self,
        callback: impl Fn(AgentStatusModuleId) + Send + Sync + 'static,
    ) {
        *self.state.after_capture.lock().expect("after capture lock") = Some(Arc::new(callback));
    }

    /// Returns the exact capture count for a module.
    pub(crate) fn capture_count(&self, module: AgentStatusModuleId) -> usize {
        use std::sync::atomic::Ordering;
        match module {
            AgentStatusModuleId::Time => self.state.capture_time.load(Ordering::SeqCst),
            AgentStatusModuleId::Background => self.state.capture_background.load(Ordering::SeqCst),
            AgentStatusModuleId::Todo => self.state.capture_todo.load(Ordering::SeqCst),
        }
    }

    /// Returns the exact evaluation count for a module.
    pub(crate) fn evaluate_count(&self, module: AgentStatusModuleId) -> usize {
        use std::sync::atomic::Ordering;
        match module {
            AgentStatusModuleId::Time => self.state.evaluate_time.load(Ordering::SeqCst),
            AgentStatusModuleId::Background => {
                self.state.evaluate_background.load(Ordering::SeqCst)
            }
            AgentStatusModuleId::Todo => self.state.evaluate_todo.load(Ordering::SeqCst),
        }
    }

    fn record_capture(&self, module: AgentStatusModuleId) {
        use std::sync::atomic::Ordering;
        match module {
            AgentStatusModuleId::Time => {
                self.state.capture_time.fetch_add(1, Ordering::SeqCst);
            }
            AgentStatusModuleId::Background => {
                self.state.capture_background.fetch_add(1, Ordering::SeqCst);
            }
            AgentStatusModuleId::Todo => {
                self.state.capture_todo.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn record_evaluate(&self, module: AgentStatusModuleId) {
        use std::sync::atomic::Ordering;
        match module {
            AgentStatusModuleId::Time => {
                self.state.evaluate_time.fetch_add(1, Ordering::SeqCst);
            }
            AgentStatusModuleId::Background => {
                self.state
                    .evaluate_background
                    .fetch_add(1, Ordering::SeqCst);
            }
            AgentStatusModuleId::Todo => {
                self.state.evaluate_todo.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn take_capture_failure(&self, module: AgentStatusModuleId) -> bool {
        let mut failure = self
            .state
            .capture_failure
            .lock()
            .expect("capture failure lock");
        if *failure == Some(module) {
            *failure = None;
            true
        } else {
            false
        }
    }

    fn take_evaluate_failure(&self, module: AgentStatusModuleId) -> bool {
        let mut failure = self
            .state
            .evaluate_failure
            .lock()
            .expect("evaluate failure lock");
        if *failure == Some(module) {
            *failure = None;
            true
        } else {
            false
        }
    }

    fn take_payload_mismatch(&self, module: AgentStatusModuleId) -> bool {
        let mut mismatch = self
            .state
            .payload_mismatch
            .lock()
            .expect("payload mismatch lock");
        if *mismatch == Some(module) {
            *mismatch = None;
            true
        } else {
            false
        }
    }

    fn run_after_capture(&self, module: AgentStatusModuleId) {
        if let Some(callback) = self
            .state
            .after_capture
            .lock()
            .expect("after capture lock")
            .as_ref()
        {
            callback(module);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ConversationState;
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        CompactionSummaryMetadata, ContextKind, InboundKind, ToolMessageBlock, UserContentBlock,
        UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{RequestId, ToolCallId, ToolExecutionId, ToolId};
    use crate::tools::background::BackgroundLifecycle;
    use crate::tools::todo::{TODO_TOOL_ID, TodoCreate, TodoWriter};
    use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolResultContent};

    fn opportunity() -> AgentStatusOpportunitySet {
        AgentStatusOpportunitySet {
            fresh_inbound: Some(FreshInboundStatusOpportunity {
                target_message_id: MessageId::new("inbound"),
            }),
            post_tool_batch: None,
        }
    }

    fn empty_surface() -> AgentStatusSurfaceView {
        empty_surface_at(crate::conversation::SurfaceRevision::INITIAL)
    }

    fn empty_surface_at(revision: crate::conversation::SurfaceRevision) -> AgentStatusSurfaceView {
        empty_surface_at_with_compaction(revision, 0)
    }

    fn empty_surface_at_with_compaction(
        revision: crate::conversation::SurfaceRevision,
        compaction_generation: u64,
    ) -> AgentStatusSurfaceView {
        AgentStatusSurfaceView::for_test(
            revision,
            compaction_generation,
            Arc::from(Vec::<MessageId>::new().into_boxed_slice()),
            Arc::from(Vec::<SurfaceMessageView>::new().into_boxed_slice()),
        )
        .expect("valid empty test Surface")
    }

    fn plain_surface_message(id: &str) -> SurfaceMessageView {
        SurfaceMessageView {
            id: MessageId::new(id),
            message: MessageBlock::User(UserMessageBlock {
                id: MessageId::new(id),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: id.to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            }),
        }
    }

    fn status_surface_message(
        id: &str,
        generated_at: DateTime<Utc>,
        modules: &[AgentStatusModuleId],
        rendered_text: &str,
    ) -> SurfaceMessageView {
        let message_id = MessageId::new(id);
        SurfaceMessageView {
            id: message_id.clone(),
            message: MessageBlock::User(UserMessageBlock {
                id: message_id,
                content: vec![UserContentBlock::Text(TextBlock {
                    text: rendered_text.to_owned(),
                })],
                source: UserSource::Runtime,
                kind: InboundKind::Context(ContextKind::AgentStatus(
                    AgentStatusGenerationMetadata::new(generated_at, modules.to_vec())
                        .expect("valid test Agent Status metadata"),
                )),
                timestamp: None,
            }),
        }
    }

    fn compaction_summary(id: &str, text: &str) -> UserMessageBlock {
        UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::CompactionSummary(CompactionSummaryMetadata::empty()),
            timestamp: None,
        }
    }

    fn surface(messages: Vec<SurfaceMessageView>) -> AgentStatusSurfaceView {
        let ids = messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        AgentStatusSurfaceView::for_test(
            crate::conversation::SurfaceRevision::new(ids.len() as u64),
            0,
            Arc::from(ids.into_boxed_slice()),
            Arc::from(messages.into_boxed_slice()),
        )
        .expect("valid test Surface")
    }

    fn background_snapshot(index: usize, detail: &str) -> BackgroundExecutionSnapshot {
        BackgroundExecutionSnapshot {
            execution_id: ToolExecutionId::new(format!("exec-{index}")),
            tool_id: ToolId::new("background_task"),
            tool_name: "background_task".to_owned(),
            state: BackgroundLifecycle::Running,
            progress: Some(ToolProgress {
                message: Some(detail.to_owned()),
                completed: Some(1.0),
                total: Some(2.0),
            }),
            result: None,
        }
    }

    fn todo_result(snapshot: &TodoSnapshot) -> MessageBlock {
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("todo-status-result"),
            tool_call_id: ToolCallId::new("todo-status-call"),
            tool_id: ToolId::new(TODO_TOOL_ID),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![ToolResultContent::Json {
                    value: serde_json::to_value(snapshot).expect("Todo snapshot serializes"),
                }],
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        })
    }

    fn todo_list_with(build: impl FnOnce(&TodoWriter)) -> ConversationTodoList {
        let list = ConversationTodoList::new(crate::runtime::identity::ConversationId::new(
            "agent-status-todos",
        ));
        let batch = list.open_batch().expect("Todo batch opens");
        let writer = batch.writer();
        build(&writer);
        let snapshot = writer.snapshot().expect("Todo batch remains open");
        batch.settle(&[todo_result(&snapshot)]);
        list
    }

    fn todo_only_config() -> AgentStatusConfig {
        AgentStatusConfig {
            time: TimeStatusConfig {
                enabled: false,
                timezone: None,
            },
            background: BackgroundStatusConfig { enabled: false },
        }
    }

    fn post_tool_opportunity() -> AgentStatusOpportunitySet {
        AgentStatusOpportunitySet {
            fresh_inbound: None,
            post_tool_batch: Some(PostToolBatchStatusOpportunity),
        }
    }

    fn combined_opportunity() -> AgentStatusOpportunitySet {
        AgentStatusOpportunitySet {
            fresh_inbound: Some(FreshInboundStatusOpportunity {
                target_message_id: MessageId::new("inbound"),
            }),
            post_tool_batch: Some(PostToolBatchStatusOpportunity),
        }
    }

    struct FixedEmissionLookup {
        fingerprint: Option<String>,
        todo_progress: u64,
        latest_emission_origin: u64,
    }

    impl AgentStatusEmissionLookup for FixedEmissionLookup {
        fn latest_agent_status_emission(
            &self,
            module_id: AgentStatusModuleId,
            key: &str,
        ) -> Result<Option<AgentStatusEmissionRecord>, crate::durable::ConversationStoreError>
        {
            Ok(self
                .fingerprint
                .as_ref()
                .map(|fingerprint| AgentStatusEmissionRecord {
                    module_id,
                    key: key.to_owned(),
                    fingerprint: fingerprint.clone(),
                    emitted_at: DateTime::from_timestamp(1_754_000_000, 0).expect("timestamp"),
                    request_id: RequestId::new("request"),
                    canonical_message_id: MessageId::new("status"),
                    todo_progress_origin: self.latest_emission_origin,
                    event_sequence: 1,
                }))
        }

        fn current_todo_progress(&self) -> Result<u64, crate::durable::ConversationStoreError> {
            Ok(self.todo_progress)
        }
    }

    #[derive(Debug)]
    struct FixedClock(DateTime<Utc>);

    impl AgentStatusClock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    #[derive(Clone)]
    struct MutableClock(Arc<std::sync::Mutex<DateTime<Utc>>>);

    impl AgentStatusClock for MutableClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().expect("mutable clock lock")
        }
    }

    #[derive(Clone)]
    struct CountingClock {
        now: DateTime<Utc>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl AgentStatusClock for CountingClock {
        fn now(&self) -> DateTime<Utc> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.now
        }
    }

    fn engine(config: AgentStatusConfig) -> AgentStatusEngine {
        AgentStatusEngine::new(
            config,
            Arc::new(FixedClock(
                DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp"),
            )),
        )
    }

    fn empty_background() -> (
        crate::scripted_suites::common::ToolRuntimeFixture,
        ConversationBackgroundRegistry,
    ) {
        let fixture = crate::scripted_suites::common::tool_runtime("agent-status-tests");
        let registry = fixture.background().clone();
        (fixture, registry)
    }

    #[test]
    fn default_engine_delivers_time_before_background() {
        let (_fixture, registry) = empty_background();
        let surface = empty_surface();
        let mut engine = engine(AgentStatusConfig::default());
        let status = engine
            .prepare(&opportunity(), &surface, &registry)
            .expect("time status");
        assert_eq!(status.sections.len(), 1);
        assert_eq!(status.sections[0].id.as_str(), "temporal");
    }

    #[test]
    fn time_disabled_produces_no_time_contribution() {
        let (_fixture, registry) = empty_background();
        let surface = empty_surface();
        let mut engine = engine(AgentStatusConfig {
            time: TimeStatusConfig {
                enabled: false,
                timezone: None,
            },
            background: BackgroundStatusConfig::default(),
        });
        assert!(
            engine
                .prepare(&opportunity(), &surface, &registry)
                .is_none()
        );
    }

    #[test]
    fn background_disabled_keeps_time_without_background_contribution() {
        let (_fixture, registry) = empty_background();
        let surface = empty_surface();
        let mut engine = engine(AgentStatusConfig {
            time: TimeStatusConfig::default(),
            background: BackgroundStatusConfig { enabled: false },
        });
        let status = engine
            .prepare(&opportunity(), &surface, &registry)
            .expect("time status");
        assert_eq!(status.sections.len(), 1);
        assert_eq!(status.sections[0].id.as_str(), "temporal");
    }

    #[test]
    fn disabled_modules_produce_no_generation() {
        let (_fixture, registry) = empty_background();
        let surface = empty_surface();
        let mut engine = engine(AgentStatusConfig {
            time: TimeStatusConfig {
                enabled: false,
                timezone: None,
            },
            background: BackgroundStatusConfig { enabled: false },
        });
        assert!(
            engine
                .prepare(&opportunity(), &surface, &registry)
                .is_none()
        );
    }

    #[test]
    fn capture_and_evaluate_counts_are_once_per_generation() {
        let seam = AgentStatusTestSeam::new();
        let mut engine = engine(AgentStatusConfig::default()).with_test_seam(seam.clone());
        let (_fixture, registry) = empty_background();
        let surface = empty_surface();
        let _ = engine.prepare(&opportunity(), &surface, &registry);
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);
        assert_eq!(seam.evaluate_count(AgentStatusModuleId::Time), 1);
        assert_eq!(seam.capture_count(AgentStatusModuleId::Background), 1);
        assert_eq!(seam.evaluate_count(AgentStatusModuleId::Background), 1);
    }

    #[test]
    fn module_matching_both_opportunities_captures_and_evaluates_once() {
        let seam = AgentStatusTestSeam::new();
        let mut engine = engine(todo_only_config()).with_test_seam(seam.clone());
        let (_fixture, registry) = empty_background();
        let todos = todo_list_with(|writer| {
            writer
                .create(TodoCreate {
                    subject: "One combined opportunity".to_owned(),
                    ..TodoCreate::default()
                })
                .expect("create Todo");
        });
        let prepared = engine
            .prepare_with_inputs(
                &combined_opportunity(),
                &empty_surface(),
                &registry,
                &todos,
                &FixedEmissionLookup {
                    fingerprint: None,
                    todo_progress: 0,
                    latest_emission_origin: 0,
                },
            )
            .expect("the actionable Todo contribution");

        assert_eq!(prepared.status.sections.len(), 1);
        assert_eq!(seam.capture_count(AgentStatusModuleId::Todo), 1);
        assert_eq!(seam.evaluate_count(AgentStatusModuleId::Todo), 1);
    }

    #[test]
    fn failures_quarantine_one_module_and_fresh_attempt_retries_it() {
        let seam = AgentStatusTestSeam::new();
        seam.fail_capture_once(AgentStatusModuleId::Time);
        let (_fixture, registry) = empty_background();
        let surface = empty_surface();
        let template = engine(AgentStatusConfig::default()).with_test_seam(seam.clone());
        let mut first = template.for_attempt();
        assert!(
            first.prepare(&opportunity(), &surface, &registry).is_none(),
            "a failed Time module leaves no useful status when Background is empty"
        );
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);
        let _ = first.prepare(&opportunity(), &surface, &registry);
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);

        let mut second = template.for_attempt();
        let status = second
            .prepare(&opportunity(), &surface, &registry)
            .expect("retry");
        assert_eq!(status.sections[0].id.as_str(), "temporal");
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 2);
    }

    #[test]
    fn evaluate_failure_and_payload_mismatch_are_isolated() {
        let seam = AgentStatusTestSeam::new();
        seam.fail_evaluate_once(AgentStatusModuleId::Time);
        let mut failure_engine = engine(AgentStatusConfig::default()).with_test_seam(seam.clone());
        let (_fixture, registry) = empty_background();
        let surface = empty_surface();
        assert!(
            failure_engine
                .prepare(&opportunity(), &surface, &registry)
                .is_none()
        );
        assert_eq!(seam.capture_count(AgentStatusModuleId::Time), 1);
        assert_eq!(seam.evaluate_count(AgentStatusModuleId::Time), 1);

        let seam = AgentStatusTestSeam::new();
        seam.mismatch_once(AgentStatusModuleId::Time);
        let mut engine = engine(AgentStatusConfig::default()).with_test_seam(seam);
        assert!(
            engine
                .prepare(&opportunity(), &surface, &registry)
                .is_none()
        );
    }

    #[test]
    fn evaluation_uses_frozen_snapshot() {
        let fixed = DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp");
        let later = DateTime::from_timestamp(1_854_000_001, 0).expect("timestamp");
        let clock = Arc::new(std::sync::Mutex::new(fixed));
        let seam = AgentStatusTestSeam::new();
        let clock_after_capture = Arc::clone(&clock);
        seam.after_capture(move |module| {
            if module == AgentStatusModuleId::Time {
                *clock_after_capture.lock().expect("mutable clock lock") = later;
            }
        });
        let mut engine =
            AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(MutableClock(clock)))
                .with_test_seam(seam);
        let (_fixture, registry) = empty_background();
        let surface = empty_surface();
        let status = engine
            .prepare(&opportunity(), &surface, &registry)
            .expect("status");
        assert!(matches!(
            status.sections.first().map(|section| &section.data),
            Some(AgentStatusSectionData::Temporal { current_time, .. }) if *current_time == fixed
        ));
    }

    #[test]
    fn live_surface_mutation_after_freeze_cannot_change_the_generation() {
        let now = DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp");
        let mut live = ConversationState::from_messages([plain_surface_message("inbound").message])
            .expect("conversation");
        let frozen = live
            .freeze_active_surface()
            .expect("freeze the pre-status Surface");
        let surface = AgentStatusSurfaceView::from_snapshot(frozen).expect("valid frozen Surface");
        assert!(!surface.contains_status(AgentStatusModuleId::Time));

        live.commit(
            status_surface_message(
                "mutated-live-status",
                now - ChronoDuration::seconds(1),
                &[AgentStatusModuleId::Time],
                "renderer text is not consulted",
            )
            .message,
        )
        .expect("mutate live Surface after the freeze");

        let (_fixture, registry) = empty_background();
        let mut engine = AgentStatusEngine::new(
            AgentStatusConfig {
                time: TimeStatusConfig {
                    enabled: true,
                    timezone: None,
                },
                background: BackgroundStatusConfig { enabled: false },
            },
            Arc::new(FixedClock(now)),
        );
        let status = engine
            .prepare(&opportunity(), &surface, &registry)
            .expect("the frozen Surface has no visible Time contribution");
        assert!(
            status
                .sections
                .iter()
                .any(|section| { section.id.as_str() == AgentStatusSectionId::TEMPORAL })
        );
    }

    /// The test hook mutates the live authoritative registry after the shared
    /// capture point. Background still uses the captured active set and state,
    /// while the live registry settles independently afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_background_mutation_after_capture_cannot_change_the_generation() {
        let fixture = crate::scripted_suites::common::tool_runtime("agent-status-freeze");
        let invocation = crate::tools::types::ToolInvocation {
            call_id: crate::runtime::identity::ToolCallId::new("call-freeze"),
            tool_id: crate::runtime::identity::ToolId::new("tool-background"),
            tool_name: "background".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let (tool, release) = crate::scripted_suites::support::fake::FakeTool::parking(
            crate::scripted_suites::common::tool_policies(
                "background",
                "tool-background",
                crate::tools::types::ToolExecutionPolicy::ModelSelectable,
                crate::tools::types::ToolConcurrencyPolicy::Sequential,
            ),
            crate::scripted_suites::support::fake::success_result("done"),
        );
        let mut started = tool.started();
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .background()
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("background dispatch prepares");
        fixture
            .background()
            .commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
            .expect("background dispatch commits");
        let registry = fixture.background().clone();
        started
            .wait_for(|is_started| *is_started)
            .await
            .expect("background tool starts before the freeze");
        let before = registry.active_snapshot();
        assert_eq!(before.len(), 1);
        let execution_id = before[0].execution_id.clone();

        let seam = AgentStatusTestSeam::new();
        let mutation_registry = registry.clone();
        let mutation_id = execution_id.clone();
        seam.after_capture(move |module| {
            if module == AgentStatusModuleId::Time {
                let _ = mutation_registry.cancel(&mutation_id);
            }
        });
        let mut engine = engine(AgentStatusConfig::default()).with_test_seam(seam);
        let status = engine
            .prepare(&opportunity(), &empty_surface(), &registry)
            .expect("the frozen active execution produces Background status");

        let background = status
            .sections
            .iter()
            .find_map(|section| match &section.data {
                AgentStatusSectionData::BackgroundExecution { executions, .. } => {
                    executions.first()
                }
                AgentStatusSectionData::Temporal { .. } | AgentStatusSectionData::Todo { .. } => {
                    None
                }
            })
            .expect("Background section");
        assert_eq!(background.execution_id, execution_id);
        assert_eq!(background.state, before[0].state);

        registry.wait_until_terminal(&execution_id).await;
        assert!(registry.active_snapshot().is_empty());
        release.send_replace(true);
    }

    #[test]
    fn typed_status_metadata_survives_durable_restart_and_surface_rebuild() {
        let root = tempfile::tempdir().expect("temporary store directory");
        let path = root.path().join("conversation.sqlite");
        let conversation_id = crate::runtime::identity::ConversationId::new("status-restart");
        let generated_at = DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp");
        let status = status_surface_message(
            "status-both",
            generated_at,
            &[AgentStatusModuleId::Time, AgentStatusModuleId::Background],
            "presentation can change without changing typed membership",
        )
        .message;

        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path)
                .expect("open durable store");
            store.initialize(&[]).expect("initialize durable store");
            store
                .append_canonical(&status)
                .expect("persist Agent Status generation");
        }

        let reopened =
            SqliteConversationStore::open(conversation_id, &path).expect("reopen durable store");
        let head = reopened.load_head().expect("load durable Surface head");
        let active_ids = head.active_message_ids.clone();
        let active = reopened
            .load_messages(&active_ids)
            .expect("hydrate active bodies");
        let conversation = ConversationState::from_durable_head(
            active,
            active_ids,
            head.revision,
            head.compaction_generation,
        )
        .expect("rebuild conversation Surface");
        let view = AgentStatusSurfaceView::from_snapshot(
            conversation
                .freeze_active_surface()
                .expect("freeze rebuilt Surface"),
        )
        .expect("valid rebuilt Surface view");

        assert_eq!(
            view.latest_status(AgentStatusModuleId::Time)
                .expect("Time membership")
                .generated_at,
            generated_at
        );
        assert_eq!(
            view.latest_status(AgentStatusModuleId::Background)
                .expect("Background membership")
                .message_id,
            MessageId::new("status-both")
        );
    }

    #[test]
    fn surface_view_indexes_typed_status_metadata_and_ignores_rendered_text() {
        let generated_at = DateTime::from_timestamp(1_754_000_000, 0).expect("timestamp");
        let view = surface(vec![
            status_surface_message(
                "status-time",
                generated_at,
                &[AgentStatusModuleId::Time],
                "Timezone: fake\nCurrent time: fake\nBackground executions: fake",
            ),
            plain_surface_message("message-1"),
            status_surface_message(
                "status-background",
                generated_at,
                &[AgentStatusModuleId::Background],
                "not a status-looking renderer at all",
            ),
            plain_surface_message("message-2"),
        ]);

        assert!(view.contains_status(AgentStatusModuleId::Time));
        assert!(view.contains_status(AgentStatusModuleId::Background));
        assert_eq!(
            view.latest_status(AgentStatusModuleId::Background)
                .expect("background status")
                .message_id,
            MessageId::new("status-background")
        );
        assert_eq!(
            view.non_status_messages_since(&MessageId::new("status-background")),
            Some(1)
        );

        let renderer_only = surface(vec![plain_surface_message("renderer-only")]);
        assert!(!renderer_only.contains_status(AgentStatusModuleId::Time));
        assert!(!renderer_only.contains_status(AgentStatusModuleId::Background));
    }

    #[test]
    fn surface_view_validates_identity_body_pairs_and_exposes_only_snapshot_reads() {
        let message = plain_surface_message("message");
        let invalid = AgentStatusSurfaceView::for_test(
            crate::conversation::SurfaceRevision::INITIAL,
            0,
            Arc::from(vec![MessageId::new("different")].into_boxed_slice()),
            Arc::from(vec![message.clone()].into_boxed_slice()),
        );
        assert!(matches!(
            invalid,
            Err(AgentStatusSurfaceViewError::IdentityBodyMismatch { .. })
        ));

        let view = surface(vec![message]);
        assert_eq!(
            view.revision(),
            crate::conversation::SurfaceRevision::new(1)
        );
        assert_eq!(view.compaction_generation(), 0);
        assert_eq!(view.active_message_ids(), &[MessageId::new("message")]);
        assert_eq!(view.messages().len(), 1);
        assert_eq!(view.messages()[0].id, MessageId::new("message"));
    }

    #[test]
    fn time_refresh_is_surface_aware_and_uses_the_frozen_clock_instant() {
        let now = DateTime::from_timestamp(1_787_808_738, 0).expect("timestamp");
        let mut engine = AgentStatusEngine::new(
            AgentStatusConfig {
                time: TimeStatusConfig {
                    enabled: true,
                    timezone: Some(chrono_tz::Asia::Tokyo),
                },
                background: BackgroundStatusConfig { enabled: false },
            },
            Arc::new(FixedClock(now)),
        );

        let no_visible = engine
            .prepare(&opportunity(), &empty_surface(), &empty_background().1)
            .expect("Time is eligible without a visible Time contribution");
        assert_eq!(no_visible.generated_at, now);
        assert_eq!(
            render_agent_status(&no_visible),
            "<system-reminder>\nTimezone: Asia/Tokyo\nCurrent time: 2026-08-27 14:32:18\n</system-reminder>"
        );
        assert!(!render_agent_status(&no_visible).contains("Inbound message time"));

        let recent = surface(vec![status_surface_message(
            "time-recent",
            now - ChronoDuration::minutes(29) - ChronoDuration::seconds(59),
            &[AgentStatusModuleId::Time],
            "Current time: 1900-01-01 00:00:00",
        )]);
        assert!(
            engine
                .prepare(&opportunity(), &recent, &empty_background().1)
                .is_none()
        );

        let exact = surface(vec![status_surface_message(
            "time-exact",
            now - TIME_REFRESH_INTERVAL,
            &[AgentStatusModuleId::Time],
            "Current time: 1900-01-01 00:00:00",
        )]);
        let refreshed = engine
            .prepare(&opportunity(), &exact, &empty_background().1)
            .expect("Time refresh threshold is inclusive");
        assert_eq!(refreshed.generated_at, now);
        assert!(render_agent_status(&refreshed).contains("2026-08-27 14:32:18"));
    }

    #[test]
    fn background_policy_uses_authoritative_active_work_and_message_distance() {
        let now = DateTime::from_timestamp(1_754_000_000, 0).expect("timestamp");
        let execution = background_snapshot(0, "active");
        let evaluate = |view: AgentStatusSurfaceView| {
            BackgroundStatusModule::evaluate(&BackgroundStatusSnapshot {
                executions: Arc::from(vec![execution.clone()].into_boxed_slice()),
                latest_visible: view.latest_status(AgentStatusModuleId::Background).cloned(),
                non_status_messages_since: view
                    .latest_status(AgentStatusModuleId::Background)
                    .and_then(|latest| view.non_status_messages_since(&latest.message_id)),
            })
        };

        assert!(evaluate(empty_surface()).is_some());
        let seven = surface(
            std::iter::once(status_surface_message(
                "background-1",
                now,
                &[AgentStatusModuleId::Background],
                "old reminder",
            ))
            .chain((0..7).map(|index| plain_surface_message(&format!("message-{index}"))))
            .collect(),
        );
        assert!(evaluate(seven).is_none());

        let eight = surface(
            std::iter::once(status_surface_message(
                "background-2",
                now,
                &[AgentStatusModuleId::Background],
                "old reminder",
            ))
            .chain((0..4).map(|index| plain_surface_message(&format!("message-{index}"))))
            .chain(std::iter::once(status_surface_message(
                "time-between",
                now,
                &[AgentStatusModuleId::Time],
                "status does not count",
            )))
            .chain((4..8).map(|index| plain_surface_message(&format!("message-{index}"))))
            .collect(),
        );
        assert!(
            evaluate(eight).is_some(),
            "exactly eight non-status messages"
        );

        let empty_authority = BackgroundStatusModule::evaluate(&BackgroundStatusSnapshot {
            executions: Arc::from(Vec::<BackgroundExecutionSnapshot>::new().into_boxed_slice()),
            latest_visible: None,
            non_status_messages_since: None,
        });
        assert!(empty_authority.is_none());
    }

    #[test]
    fn compaction_retiring_visible_status_reopens_surface_eligibility() {
        let now = DateTime::from_timestamp(1_754_000_000, 0).expect("timestamp");
        let time_id = MessageId::new("time-visible");
        let background_id = MessageId::new("background-visible");
        let mut conversation = ConversationState::from_messages([
            status_surface_message(
                time_id.as_str(),
                now - ChronoDuration::minutes(10),
                &[AgentStatusModuleId::Time],
                "presentation only",
            )
            .message,
            status_surface_message(
                background_id.as_str(),
                now,
                &[AgentStatusModuleId::Background],
                "presentation only",
            )
            .message,
        ])
        .expect("conversation with two status generations");
        let (_fixture, registry) = empty_background();

        let before = AgentStatusSurfaceView::from_snapshot(
            conversation
                .freeze_active_surface()
                .expect("freeze before compaction"),
        )
        .expect("valid pre-compaction Surface view");
        let execution = background_snapshot(0, "still active");
        let background_snapshot_for = |view: &AgentStatusSurfaceView| BackgroundStatusSnapshot {
            executions: Arc::from(vec![execution.clone()].into_boxed_slice()),
            latest_visible: view.latest_status(AgentStatusModuleId::Background).cloned(),
            non_status_messages_since: view
                .latest_status(AgentStatusModuleId::Background)
                .and_then(|latest| view.non_status_messages_since(&latest.message_id)),
        };
        assert!(
            BackgroundStatusModule::evaluate(&background_snapshot_for(&before)).is_none(),
            "the visible Background generation suppresses a reminder before compaction"
        );

        let command = conversation
            .prepare_compaction(
                compaction_summary("summary-status", "retired status generations"),
                crate::conversation::SurfaceSpan::new(time_id, background_id),
            )
            .expect("prepare compaction");
        conversation
            .commit_compaction(command)
            .expect("commit compaction");

        let after = AgentStatusSurfaceView::from_snapshot(
            conversation
                .freeze_active_surface()
                .expect("freeze after compaction"),
        )
        .expect("valid post-compaction Surface view");
        assert!(!after.contains_status(AgentStatusModuleId::Time));
        assert!(!after.contains_status(AgentStatusModuleId::Background));
        assert!(
            BackgroundStatusModule::evaluate(&background_snapshot_for(&after)).is_some(),
            "active work is immediately eligible when compaction retires the visible reminder"
        );

        let mut time_engine = AgentStatusEngine::new(
            AgentStatusConfig {
                time: TimeStatusConfig {
                    enabled: true,
                    timezone: None,
                },
                background: BackgroundStatusConfig { enabled: false },
            },
            Arc::new(FixedClock(now)),
        );
        assert!(
            time_engine
                .prepare(&opportunity(), &before, &registry)
                .is_none(),
            "the recent visible Time generation is still fresh"
        );
        assert!(
            time_engine
                .prepare(&opportunity(), &after, &registry)
                .is_some(),
            "retiring the visible Time generation reopens eligibility"
        );
        assert!(
            conversation
                .ledger()
                .get(&MessageId::new("time-visible"))
                .is_some(),
            "compaction retires visibility but never mutates the canonical Ledger"
        );
    }

    #[test]
    fn latest_visible_background_generation_owns_reminder_distance() {
        let now = DateTime::from_timestamp(1_754_000_000, 0).expect("timestamp");
        let execution = background_snapshot(0, "active");
        let view = surface(
            std::iter::once(status_surface_message(
                "background-old",
                now,
                &[AgentStatusModuleId::Background],
                "old reminder",
            ))
            .chain((0..8).map(|index| plain_surface_message(&format!("old-{index}"))))
            .chain(std::iter::once(status_surface_message(
                "background-latest",
                now,
                &[AgentStatusModuleId::Background],
                "latest reminder",
            )))
            .chain((0..7).map(|index| plain_surface_message(&format!("latest-{index}"))))
            .collect(),
        );
        assert_eq!(
            view.latest_status(AgentStatusModuleId::Background)
                .expect("latest Background generation")
                .message_id,
            MessageId::new("background-latest")
        );
        let snapshot = BackgroundStatusSnapshot {
            executions: Arc::from(vec![execution].into_boxed_slice()),
            latest_visible: view.latest_status(AgentStatusModuleId::Background).cloned(),
            non_status_messages_since: view
                .latest_status(AgentStatusModuleId::Background)
                .and_then(|latest| view.non_status_messages_since(&latest.message_id)),
        };
        assert_eq!(snapshot.non_status_messages_since, Some(7));
        assert!(BackgroundStatusModule::evaluate(&snapshot).is_none());
    }

    #[test]
    fn one_status_evaluation_samples_the_clock_once() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let now = DateTime::from_timestamp(1_754_000_000, 0).expect("timestamp");
        let clock = CountingClock {
            now,
            calls: Arc::clone(&calls),
        };
        let mut engine = AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(clock));
        let registry = empty_background().1;
        let status = engine
            .prepare(&opportunity(), &empty_surface(), &registry)
            .expect("Time status");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(status.generated_at, now);
        assert!(matches!(
            status.sections[0].data,
            AgentStatusSectionData::Temporal { current_time, .. } if current_time == now
        ));
    }

    #[test]
    fn thresholds_are_evaluation_only_and_do_not_create_background_work() {
        let (_fixture, registry) = empty_background();
        let mut engine = AgentStatusEngine::new(
            AgentStatusConfig {
                time: TimeStatusConfig {
                    enabled: false,
                    timezone: None,
                },
                background: BackgroundStatusConfig::default(),
            },
            Arc::new(FixedClock(
                DateTime::from_timestamp(1_754_000_000, 0).expect("timestamp"),
            )),
        );
        let before = registry.all_snapshots();
        assert!(
            engine
                .prepare(
                    &AgentStatusOpportunitySet::default(),
                    &empty_surface(),
                    &registry
                )
                .is_none()
        );
        assert_eq!(registry.all_snapshots(), before);
        assert!(
            engine
                .prepare(&opportunity(), &empty_surface(), &registry)
                .is_none(),
            "empty authoritative active work cannot produce a Background generation"
        );
        assert_eq!(registry.all_snapshots(), before);
    }

    #[test]
    fn background_semantic_bounds_report_omitted_entries_and_bound_text() {
        let snapshots = (0..MAX_BACKGROUND_STATUS_EXECUTIONS + 3)
            .map(|index| background_snapshot(index, &"😀".repeat(400)))
            .collect::<Vec<_>>();
        let payload = BackgroundStatusModule::evaluate(&BackgroundStatusSnapshot {
            executions: Arc::from(snapshots.into_boxed_slice()),
            latest_visible: None,
            non_status_messages_since: None,
        })
        .expect("background contribution");
        let AgentStatusPayload::BackgroundExecution {
            executions,
            omitted_count,
        } = payload
        else {
            panic!("wrong payload");
        };
        assert_eq!(executions.len(), MAX_BACKGROUND_STATUS_EXECUTIONS);
        assert_eq!(omitted_count, 3);
        assert!(
            executions[0]
                .progress
                .as_ref()
                .and_then(|progress| progress.message.as_ref())
                .expect("message")
                .len()
                <= MAX_BACKGROUND_STATUS_TEXT_BYTES
        );
    }

    #[test]
    fn global_admission_uses_utf8_bytes_whole_sections_and_continues() {
        let oversized = AgentStatusSection {
            id: AgentStatusSectionId::new("oversized"),
            data: AgentStatusSectionData::BackgroundExecution {
                executions: vec![background_snapshot(
                    0,
                    // The scalar count is below the cap, but the UTF-8 byte
                    // count plus wrapper overhead is above it. A scalar-count
                    // implementation would incorrectly admit this section.
                    &"😀".repeat(1_020),
                )],
                omitted_count: 0,
            },
        };
        let small = AgentStatusSection {
            id: AgentStatusSectionId::new("small"),
            data: AgentStatusSectionData::Temporal {
                current_time: DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp"),
                timezone: None,
            },
        };
        let (status, emissions) = admit_sections(
            vec![
                AgentStatusContribution {
                    section: oversized,
                    emission: None,
                },
                AgentStatusContribution {
                    section: small,
                    emission: None,
                },
            ],
            DateTime::from_timestamp(1_754_000_001, 0).expect("timestamp"),
        );
        assert!(emissions.is_empty());
        assert_eq!(status.sections.len(), 1);
        assert_eq!(status.sections[0].id.as_str(), "small");
        let rendered = render_agent_status(&status);
        assert!(rendered.len() <= GLOBAL_AGENT_STATUS_BYTE_CAP);
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
        assert!(!rendered.contains("oversized"));
    }

    /// A live background record proves that module order is source-owned and
    /// that a failure in Background leaves the surviving Time contribution
    /// available. The registry runner is released and awaited explicitly so
    /// this test has no leaked task or timing race.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[allow(clippy::too_many_lines)]
    async fn semantic_order_and_background_failure_isolation_are_deterministic() {
        let fixture = crate::scripted_suites::common::tool_runtime("agent-status-order");
        let invocation = crate::tools::types::ToolInvocation {
            call_id: crate::runtime::identity::ToolCallId::new("call-1"),
            tool_id: crate::runtime::identity::ToolId::new("tool-background"),
            tool_name: "background".to_owned(),
            mode: crate::tools::types::ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let (tool, release) = crate::scripted_suites::support::fake::FakeTool::parking(
            crate::scripted_suites::common::tool_policies(
                "background",
                "tool-background",
                crate::tools::types::ToolExecutionPolicy::ModelSelectable,
                crate::tools::types::ToolConcurrencyPolicy::Sequential,
            ),
            crate::scripted_suites::support::fake::success_result("done"),
        );
        let executor: Arc<dyn crate::tools::executor::ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .background()
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("background dispatch prepares");
        fixture
            .background()
            .commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
            .expect("background dispatch commits");
        let registry = fixture.background().clone();
        let surface = empty_surface();

        let mut ordered = engine(AgentStatusConfig::default());
        let status = ordered
            .prepare(&opportunity(), &surface, &registry)
            .expect("Time and Background contribute");
        let ids = status
            .sections
            .iter()
            .map(|section| section.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["temporal", "background_execution"]);
        let rendered = render_agent_status(&status);
        assert!(
            rendered.find("Current time:").expect("time line")
                < rendered
                    .find("Background executions:")
                    .expect("background line")
        );

        let mut time_disabled = engine(AgentStatusConfig {
            time: TimeStatusConfig {
                enabled: false,
                timezone: None,
            },
            background: BackgroundStatusConfig::default(),
        });
        let time_disabled_status = time_disabled
            .prepare(&opportunity(), &surface, &registry)
            .expect("Background survives with Time disabled");
        assert_eq!(
            time_disabled_status.sections[0].id.as_str(),
            "background_execution"
        );

        let mut background_disabled = engine(AgentStatusConfig {
            time: TimeStatusConfig::default(),
            background: BackgroundStatusConfig { enabled: false },
        });
        let background_disabled_status = background_disabled
            .prepare(&opportunity(), &surface, &registry)
            .expect("Time survives with Background disabled");
        assert_eq!(background_disabled_status.sections.len(), 1);
        assert_eq!(
            background_disabled_status.sections[0].id.as_str(),
            "temporal"
        );

        for phase in [
            ModuleFailurePhase::Capture,
            ModuleFailurePhase::Evaluate,
            ModuleFailurePhase::PayloadValidation,
        ] {
            let seam = AgentStatusTestSeam::new();
            match phase {
                ModuleFailurePhase::Capture => {
                    seam.fail_capture_once(AgentStatusModuleId::Background);
                }
                ModuleFailurePhase::Evaluate => {
                    seam.fail_evaluate_once(AgentStatusModuleId::Background);
                }
                ModuleFailurePhase::PayloadValidation => {
                    seam.mismatch_once(AgentStatusModuleId::Background);
                }
                ModuleFailurePhase::SuppressionLookup => {
                    unreachable!("suppression lookup is not part of this loop");
                }
            }
            let mut failing = engine(AgentStatusConfig::default()).with_test_seam(seam);
            let surviving = failing
                .prepare(&opportunity(), &surface, &registry)
                .expect("Time survives a Background failure");
            assert_eq!(surviving.sections.len(), 1);
            assert_eq!(surviving.sections[0].id.as_str(), "temporal");
        }

        let execution_id = crate::runtime::identity::ToolExecutionId::new("exec_1");
        release.send_replace(true);
        registry.wait_until_terminal(&execution_id).await;
    }

    #[test]
    fn fresh_post_and_combined_opportunities_share_one_finite_evaluation() {
        let surface = empty_surface();
        let registry = empty_background().1;
        for opportunities in [
            opportunity(),
            post_tool_opportunity(),
            combined_opportunity(),
        ] {
            assert!(!opportunities.is_empty());
            let todos = todo_list_with(|writer| {
                writer
                    .create(TodoCreate {
                        subject: "Keep the plan visible".to_owned(),
                        ..TodoCreate::default()
                    })
                    .expect("create Todo");
            });
            let seam = AgentStatusTestSeam::new();
            let mut engine = engine(todo_only_config()).with_test_seam(seam.clone());
            let prepared = engine
                .prepare_with_inputs(
                    &opportunities,
                    &surface,
                    &registry,
                    &todos,
                    &FixedEmissionLookup {
                        fingerprint: None,
                        todo_progress: 0,
                        latest_emission_origin: 0,
                    },
                )
                .expect("one Todo generation");

            assert_eq!(prepared.status.sections.len(), 1);
            assert_eq!(prepared.emissions.len(), 1);
            assert_eq!(seam.capture_count(AgentStatusModuleId::Todo), 1);
            assert_eq!(seam.evaluate_count(AgentStatusModuleId::Todo), 1);
        }
    }

    #[test]
    fn todo_status_reads_only_the_committed_snapshot() {
        let list = ConversationTodoList::new(crate::runtime::identity::ConversationId::new(
            "staged-todo-status",
        ));
        let batch = list.open_batch().expect("Todo batch opens");
        let writer = batch.writer();
        writer
            .create(TodoCreate {
                subject: "Provisional work".to_owned(),
                ..TodoCreate::default()
            })
            .expect("stage Todo");

        let registry = empty_background().1;
        let lookup = FixedEmissionLookup {
            fingerprint: None,
            todo_progress: 0,
            latest_emission_origin: 0,
        };
        let mut before_commit = engine(todo_only_config());
        assert!(
            before_commit
                .prepare_with_inputs(
                    &post_tool_opportunity(),
                    &empty_surface(),
                    &registry,
                    &list,
                    &lookup,
                )
                .is_none()
        );
        assert_eq!(list.snapshot().tasks.len(), 1, "the test has staged work");
        assert!(list.committed().tasks.is_empty());

        let committed = writer.snapshot().expect("batch remains open");
        batch.settle(&[todo_result(&committed)]);
        let mut after_commit = engine(todo_only_config());
        let prepared = after_commit
            .prepare_with_inputs(
                &post_tool_opportunity(),
                &empty_surface(),
                &registry,
                &list,
                &lookup,
            )
            .expect("committed Todo work is visible");
        assert!(matches!(
            prepared.status.sections[0].data,
            AgentStatusSectionData::Todo { .. }
        ));
    }

    #[test]
    fn todo_status_has_no_reminder_for_empty_or_fully_terminal_work() {
        let registry = empty_background().1;
        let lookup = FixedEmissionLookup {
            fingerprint: None,
            todo_progress: 0,
            latest_emission_origin: 0,
        };
        for todos in [
            ConversationTodoList::new(crate::runtime::identity::ConversationId::new("empty")),
            todo_list_with(|writer| {
                let (task, _) = writer
                    .create(TodoCreate {
                        subject: "Finished work".to_owned(),
                        ..TodoCreate::default()
                    })
                    .expect("create Todo");
                writer
                    .update(
                        task.id,
                        crate::tools::todo::TodoChange {
                            status: Some(TodoStatus::Completed),
                            ..crate::tools::todo::TodoChange::default()
                        },
                    )
                    .expect("complete Todo");
            }),
        ] {
            let mut engine = engine(todo_only_config());
            assert!(
                engine
                    .prepare_with_inputs(
                        &post_tool_opportunity(),
                        &empty_surface(),
                        &registry,
                        &todos,
                        &lookup,
                    )
                    .is_none()
            );
        }
    }

    #[test]
    fn todo_status_repeats_identical_fingerprint_at_inclusive_model_progress_boundary() {
        let todos = todo_list_with(|writer| {
            writer
                .create(TodoCreate {
                    subject: "Keep reminding me".to_owned(),
                    ..TodoCreate::default()
                })
                .expect("create Todo");
        });
        let registry = empty_background().1;
        let first = engine(todo_only_config())
            .prepare_with_inputs(
                &post_tool_opportunity(),
                &empty_surface(),
                &registry,
                &todos,
                &FixedEmissionLookup {
                    fingerprint: None,
                    todo_progress: 0,
                    latest_emission_origin: 0,
                },
            )
            .expect("first actionable Todo state emits");
        let fingerprint = first.emissions[0].fingerprint.clone();

        let lookup = FixedEmissionLookup {
            fingerprint: Some(fingerprint.clone()),
            todo_progress: 1,
            latest_emission_origin: 1,
        };
        assert!(
            engine(todo_only_config())
                .prepare_with_inputs(
                    &post_tool_opportunity(),
                    &empty_surface(),
                    &registry,
                    &todos,
                    &lookup,
                )
                .is_none(),
            "the newly committed reminder has zero elapsed model progress"
        );

        let before_threshold = FixedEmissionLookup {
            fingerprint: Some(fingerprint.clone()),
            todo_progress: 1 + TODO_STATUS_REMINDER_PROGRESS_INTERVAL - 1,
            latest_emission_origin: 1,
        };
        assert!(
            engine(todo_only_config())
                .prepare_with_inputs(
                    &post_tool_opportunity(),
                    &empty_surface(),
                    &registry,
                    &todos,
                    &before_threshold,
                )
                .is_none(),
            "an identical fingerprint is suppressed strictly before the threshold"
        );

        let at_threshold = FixedEmissionLookup {
            fingerprint: Some(fingerprint.clone()),
            todo_progress: 1 + TODO_STATUS_REMINDER_PROGRESS_INTERVAL,
            latest_emission_origin: 1,
        };
        let repeated = engine(todo_only_config())
            .prepare_with_inputs(
                &post_tool_opportunity(),
                &empty_surface(),
                &registry,
                &todos,
                &at_threshold,
            )
            .expect("the identical fingerprint is eligible at the inclusive threshold");
        assert_eq!(repeated.emissions[0].fingerprint, fingerprint);

        let changed = FixedEmissionLookup {
            fingerprint: Some("different-fingerprint".to_owned()),
            todo_progress: 1,
            latest_emission_origin: 1,
        };
        let changed_generation = engine(todo_only_config())
            .prepare_with_inputs(
                &post_tool_opportunity(),
                &empty_surface(),
                &registry,
                &todos,
                &changed,
            )
            .expect("a changed fingerprint bypasses the cooldown");
        assert_eq!(changed_generation.emissions[0].fingerprint, fingerprint);
    }

    #[test]
    fn todo_status_is_bounded_deterministic_and_fingerprinted_by_semantics() {
        let todos = todo_list_with(|writer| {
            for index in 0..(MAX_TODO_STATUS_TASKS + 4) {
                writer
                    .create(TodoCreate {
                        subject: format!("Task {index} {}", "😀".repeat(200)),
                        ..TodoCreate::default()
                    })
                    .expect("create Todo");
            }
        });
        let registry = empty_background().1;
        let first_lookup = FixedEmissionLookup {
            fingerprint: None,
            todo_progress: 0,
            latest_emission_origin: 0,
        };
        let mut first_engine = engine(todo_only_config());
        let first = first_engine
            .prepare_with_inputs(
                &post_tool_opportunity(),
                &empty_surface(),
                &registry,
                &todos,
                &first_lookup,
            )
            .expect("bounded Todo status");
        let AgentStatusSectionData::Todo { presentation } = &first.status.sections[0].data else {
            panic!("expected Todo section");
        };
        assert_eq!(presentation.active_count, MAX_TODO_STATUS_TASKS + 4);
        assert_eq!(presentation.tasks.len(), MAX_TODO_STATUS_TASKS);
        assert_eq!(presentation.omitted_count, 4);
        assert!(
            presentation
                .tasks
                .iter()
                .all(|task| task.subject.len() <= MAX_TODO_STATUS_TEXT_BYTES)
        );
        assert!(render_agent_status(&first.status).len() <= GLOBAL_AGENT_STATUS_BYTE_CAP);

        let emission = &first.emissions[0];
        assert_eq!(emission.module_id, AgentStatusModuleId::Todo);
        assert_eq!(emission.key, TODO_STATUS_EMISSION_KEY);
        assert_ne!(emission.key, emission.fingerprint);

        let duplicate_lookup = FixedEmissionLookup {
            fingerprint: Some(emission.fingerprint.clone()),
            todo_progress: 0,
            latest_emission_origin: 0,
        };
        let mut duplicate_engine = engine(todo_only_config());
        assert!(
            duplicate_engine
                .prepare_with_inputs(
                    &post_tool_opportunity(),
                    &empty_surface(),
                    &registry,
                    &todos,
                    &duplicate_lookup,
                )
                .is_none()
        );

        let changed_todos = todo_list_with(|writer| {
            writer
                .create(TodoCreate {
                    subject: "A materially different task".to_owned(),
                    ..TodoCreate::default()
                })
                .expect("create changed Todo");
        });
        let mut changed_engine = engine(todo_only_config());
        let changed = changed_engine
            .prepare_with_inputs(
                &post_tool_opportunity(),
                &empty_surface(),
                &registry,
                &changed_todos,
                &duplicate_lookup,
            )
            .expect("changed Todo state is eligible");
        assert_ne!(changed.emissions[0].fingerprint, emission.fingerprint);
    }
}
