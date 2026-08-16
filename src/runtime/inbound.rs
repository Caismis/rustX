//! The conversation inbound boundary: process-local coordination over the
//! durable Pending Inbound Inbox (Issue #63).
//!
//! This module owns the narrow runtime coordination contract for
//! asynchronous user-role messages arriving while an agent attempt is
//! running. Since Issue #63 it is **coordination only**:
//!
//! ```text
//! ConversationInboundMailbox   = process-local wakeup / batching coordination
//! Pending Inbound Inbox        = accepted / not-yet-adopted durability
//! Message Ledger               = adopted canonical conversational facts
//! Conversation Surface         = current model-visible ordering
//! Event Journal                = execution facts
//! ```
//!
//! The mailbox no longer owns [`InboundSequence`] allocation and no longer
//! holds a process-local payload queue: the durable
//! [`InboundStore`](crate::durable::inbox::InboundStore) owns the accepted
//! pending state and the one per-conversation sequence domain. The mailbox
//! is the narrow acceptance/publisher seam that validates eligibility and
//! lifecycle, durably accepts through the store, then publishes the
//! process-local wake and observation. A crash may destroy the mailbox
//! without destroying accepted inbound work; the wake is a liveness
//! optimization, never the source of truth.
//!
//! The mailbox accepts only [`InboundKind::Message`] with a persisted
//! [`UserMessageBlock::timestamp`]; a runtime-provided derived compaction
//! summary is not new asynchronous work and is rejected. All `UserSource`
//! provenance shares the same ordering domain, so Human, Runtime, Agent,
//! Fleet, and `ExternalSystem` producers sequence through one store.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::durable::inbox::{AcceptedInbound, InboundDraft, InboundStore, InboundStoreError};
use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock};
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime::types::ConversationLifecycle;

/// The read-only observation seam of the conversation inbound boundary.
///
/// A mailbox fact observer receives the authoritative accept/adopt facts at
/// their linearization points. `on_enqueued` fires after durable acceptance
/// commits; `on_drained` fires after the durable canonical adoption commits.
/// An observer must never call back into the mailbox and must never mutate
/// mailbox state; the Runtime Client projection (Issue #37) treats each
/// callback as one projection fold under its own synchronization boundary.
pub trait InboundObserver: Send + Sync {
    /// Observes one durably accepted item under its assigned sequence.
    fn on_enqueued(&self, item: &InboundItem);

    /// Observes one committed finite adoption batch (watermark, count, and
    /// items), no longer pending.
    fn on_drained(&self, batch: &InboundBatch);
}

/// A conversation-scoped inbound sequence number.
///
/// The sequence identifies one item of the conversation's inbound ordering
/// domain. It is **not**
/// [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
/// and is never allocated from the Event Journal sequence; the durable
/// Pending Inbound Inbox owns allocation, and the first successful
/// acceptance of a conversation receives `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboundSequence(u64);

impl InboundSequence {
    /// Creates a sequence from a raw value.
    ///
    /// This is the reconstruction constructor used by the durable Pending
    /// Inbound Inbox when it reloads committed sequences; it is deliberately
    /// separate from allocation, which only the store performs.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw sequence value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for InboundSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One accepted inbound item with its durable sequence.
///
/// The item preserves, through the canonical [`UserMessageBlock`] plus the
/// sequence: message id, [`UserSource`](crate::message::types::UserSource),
/// [`InboundKind`], timestamp, content/payload, the original message
/// boundary, and the inbound sequence. Construction is store-owned, so an
/// item cannot be fabricated with an arbitrary sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundItem {
    sequence: InboundSequence,
    message: UserMessageBlock,
}

impl InboundItem {
    /// The durable inbound sequence of the item.
    #[must_use]
    pub fn sequence(&self) -> InboundSequence {
        self.sequence
    }

    /// The canonical inbound message.
    #[must_use]
    pub fn message(&self) -> &UserMessageBlock {
        &self.message
    }

    /// Consumes the item and returns its canonical inbound message.
    #[must_use]
    pub fn into_message(self) -> UserMessageBlock {
        self.message
    }
}

/// One finite watermark-bounded pending batch.
///
/// A batch is non-empty, belongs to exactly one [`ConversationId`], contains
/// items in strictly increasing [`InboundSequence`] order, and its watermark
/// equals the sequence of the final/highest selected item; no item sequence
/// exceeds the watermark. One pending item still produces a one-item batch,
/// and every item remains a separate canonical `UserMessageBlock`. An empty
/// inbox produces `None`, never an empty batch.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundBatch {
    conversation_id: ConversationId,
    watermark: InboundSequence,
    items: Vec<InboundItem>,
}

impl InboundBatch {
    /// The conversation the batch belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The watermark: the highest selected inbound sequence.
    #[must_use]
    pub fn watermark(&self) -> InboundSequence {
        self.watermark
    }

    /// The batch items in strictly increasing inbound sequence order.
    #[must_use]
    pub fn items(&self) -> &[InboundItem] {
        &self.items
    }

    /// Consumes the batch and returns its items in order.
    #[must_use]
    pub fn into_items(self) -> Vec<InboundItem> {
        self.items
    }
}

/// A mailbox API validation failure.
///
/// These are API-level validation errors of the conversation inbound
/// contract; they are not `RuntimeEvent` protocol expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxError {
    /// A runtime-derived compaction summary is not new asynchronous work
    /// and cannot be enqueued as ordinary inbound mail.
    CompactionSummaryNotEligible,
    /// An ordinary inbound message must carry its persisted UTC timestamp;
    /// no wall-clock time is fabricated by the mailbox.
    MissingTimestamp,
    /// The conversation inbound sequence space is exhausted; the durable
    /// inbox fails explicitly instead of wrapping to zero.
    SequenceExhausted,
    /// The mailbox belongs to a different conversation than the operation
    /// that tried to bind it.
    ConversationMismatch {
        /// The conversation the binding operation expected.
        expected: ConversationId,
        /// The conversation the mailbox actually belongs to.
        actual: ConversationId,
    },
    /// The mailbox is owned by a conversation runtime that has not been
    /// activated yet, so the conversation accepts no inbound work.
    ///
    /// See [`ConversationInboundMailbox::bind_inactive`]: this is the
    /// Issue #61 activation contract, and it is what makes the pending
    /// seed of the Runtime Client bootstrap handshake provably frozen.
    ConversationInactive {
        /// The conversation that has not been activated.
        conversation_id: ConversationId,
    },
    /// The capability lease and tool runtime do not share the same
    /// conversation/workspace ownership domain.
    CapabilityOwnershipMismatch {
        /// The capability lease's conversation owner.
        capability_conversation: ConversationId,
        /// The request/runtime conversation owner.
        runtime_conversation: ConversationId,
        /// The capability lease's canonical Workspace.
        capability_workspace: PathBuf,
        /// The tool runtime's canonical Workspace.
        runtime_workspace: PathBuf,
    },
    /// The durable Pending Inbound Inbox rejected the operation.
    Inbox(InboundStoreError),
}

impl From<InboundStoreError> for MailboxError {
    fn from(error: InboundStoreError) -> Self {
        Self::Inbox(error)
    }
}

impl fmt::Display for MailboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CompactionSummaryNotEligible => {
                write!(f, "a compaction summary is not mailbox-eligible")
            }
            Self::MissingTimestamp => {
                write!(
                    f,
                    "an ordinary inbound message requires its persisted timestamp"
                )
            }
            Self::SequenceExhausted => write!(f, "the inbound sequence space is exhausted"),
            Self::ConversationMismatch { expected, actual } => write!(
                f,
                "mailbox belongs to conversation {actual}, expected {expected}"
            ),
            Self::ConversationInactive { conversation_id } => write!(
                f,
                "conversation {conversation_id} is not activated and accepts no inbound"
            ),
            Self::CapabilityOwnershipMismatch {
                capability_conversation,
                runtime_conversation,
                capability_workspace,
                runtime_workspace,
            } => write!(
                f,
                "capability owner ({capability_conversation}, {}) does not match tool runtime owner ({runtime_conversation}, {})",
                capability_workspace.display(),
                runtime_workspace.display(),
            ),
            Self::Inbox(error) => error.fmt(f),
        }
    }
}

impl Error for MailboxError {}

/// The runtime ownership binding of one conversation mailbox (Issue #61).
///
/// A mailbox with no conversation runtime bound over it is an ordinary
/// standalone coordination contract and always accepts inbound. Once a
/// [`ConversationRuntime`](crate::runtime::conversation_runtime::ConversationRuntime)
/// claims its owning tool runtime, the mailbox carries that runtime's
/// shared activation lifecycle
/// ([`ConversationLifecycle`](crate::runtime::types::ConversationLifecycle)):
/// an inactive conversation accepts no inbound work.
///
/// The mailbox stores **no activation state of its own**: runtime ownership
/// is the presence of the lifecycle handle, and active/inactive is answered
/// by the lifecycle itself.
struct MailboxState {
    /// The shared activation lifecycle of the conversation runtime that
    /// owns this mailbox, when one does. `None` = standalone/unbound.
    lifecycle: Option<ConversationLifecycle>,
    /// The read-only fact observer, installed by the owning runtime client
    /// boundary (Issue #37). It fires after the durable acceptance/adoption
    /// linearization points.
    observer: Option<Arc<dyn InboundObserver>>,
}

impl core::fmt::Debug for MailboxState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MailboxState")
            .field(
                "lifecycle",
                &self.lifecycle.as_ref().map(|lifecycle| {
                    if lifecycle.is_active() {
                        "runtime-owned/active"
                    } else {
                        "runtime-owned/inactive"
                    }
                }),
            )
            .field(
                "observer",
                &self.observer.as_ref().map(|_| "<inbound observer>"),
            )
            .finish()
    }
}

/// The explicit execution-domain identity of one fresh inbound turn.
///
/// Fresh inbound identity is explicit execution state, never inferred from
/// message role, history shape, or timestamps: a compaction summary is
/// user-role history and must never be marked fresh. Agent Status is attached
/// to the final message of exactly one `FreshInboundTurn` per model turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshInboundTurn {
    message_ids: Vec<MessageId>,
}

/// The explicit execution trigger of an attempt's first model turn.
///
/// The trigger makes the intended execution mode explicit: there is no
/// optional status field and no disable flag, so Agent Status can never be
/// silently suppressed by omitting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialTurnTrigger {
    /// The attempt's first model invocation observes a fresh inbound turn:
    /// the model has not yet observed the referenced messages. Validation
    /// against canonical history is mandatory, Agent Status is mandatory,
    /// fresh-inbound compaction protection applies, and the trigger remains
    /// pending until one successful model invocation observes it — a
    /// provider overflow failure does not consume it, while a successful
    /// `ToolCalls` response does.
    FreshInbound(FreshInboundTurn),
    /// There is intentionally no new inbound user turn for the first model
    /// invocation: the attempt continues committed canonical history, and
    /// therefore no Agent Status is attached to the first request. This is
    /// the explicit expression of a pure continuation, never a configuration
    /// switch for disabling status on inbound messages.
    Continuation,
}

/// A `FreshInboundTurn` contract violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshInboundError {
    /// A fresh inbound turn must contain at least one message.
    Empty,
    /// The turn contains the same message id twice.
    DuplicateIds(MessageId),
    /// The turn references a message that is not in canonical history.
    UnknownMessage(MessageId),
    /// A referenced message is not a `MessageBlock::User`.
    NotUserRole(MessageId),
    /// A referenced message is not an ordinary [`InboundKind::Message`] (for
    /// example a compaction summary).
    NotInboundMessage(MessageId),
    /// A referenced message has no persisted timestamp.
    MissingTimestamp(MessageId),
    /// The referenced messages do not occur in canonical history in the
    /// caller-supplied order: `next` precedes `previous` in canonical
    /// position. A fresh inbound turn must name canonical messages in
    /// strictly increasing canonical order, and the runtime never
    /// reinterprets or sorts a caller-supplied order.
    OutOfCanonicalOrder {
        /// The previously validated message, canonically earlier.
        previous: MessageId,
        /// The message that violates the strictly increasing canonical
        /// order.
        next: MessageId,
    },
}

impl core::fmt::Display for FreshInboundError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "a fresh inbound turn must contain at least one message"),
            Self::DuplicateIds(id) => {
                write!(f, "message {id} appears twice in the fresh inbound turn")
            }
            Self::UnknownMessage(id) => write!(f, "message {id} is not in canonical history"),
            Self::NotUserRole(id) => write!(f, "message {id} is not a user-role message"),
            Self::NotInboundMessage(id) => {
                write!(f, "message {id} is not an ordinary inbound message")
            }
            Self::MissingTimestamp(id) => write!(f, "message {id} has no persisted timestamp"),
            Self::OutOfCanonicalOrder { previous, next } => write!(
                f,
                "message {next} precedes {previous} in canonical history; \
                 fresh inbound message ids must be in strictly increasing canonical order"
            ),
        }
    }
}

impl Error for FreshInboundError {}

impl FreshInboundTurn {
    /// Creates a fresh inbound turn from its ordered message ids.
    ///
    /// The ids are ordered in the intended inbound order; the final id is the
    /// message to which Agent Status is attached. Duplicate ids are invalid.
    /// Canonical ordering is not checked here — a caller-supplied order must
    /// name messages in strictly increasing canonical order, which
    /// [`FreshInboundTurn::validate_against`] enforces against canonical
    /// history.
    ///
    /// # Errors
    ///
    /// Returns [`FreshInboundError::Empty`] for an empty id list and
    /// [`FreshInboundError::DuplicateIds`] for a repeated id.
    pub fn new(message_ids: Vec<MessageId>) -> Result<Self, FreshInboundError> {
        if message_ids.is_empty() {
            return Err(FreshInboundError::Empty);
        }
        let mut seen = std::collections::BTreeSet::new();
        for id in &message_ids {
            if !seen.insert(id.clone()) {
                return Err(FreshInboundError::DuplicateIds(id.clone()));
            }
        }
        Ok(Self { message_ids })
    }

    /// The ordered message ids of the turn.
    #[must_use]
    pub fn message_ids(&self) -> &[MessageId] {
        &self.message_ids
    }

    /// The final message id: the message to which Agent Status is attached.
    ///
    /// # Panics
    ///
    /// Panics only when the turn is empty, which is impossible by
    /// construction ([`FreshInboundTurn::new`] rejects empty turns).
    #[must_use]
    pub fn last_message_id(&self) -> &MessageId {
        self.message_ids
            .last()
            .expect("a fresh inbound turn is never empty")
    }

    /// Validates the turn against one canonical history.
    ///
    /// Every referenced message must exist in the history, be
    /// `MessageBlock::User` with [`InboundKind::Message`], and carry a
    /// persisted timestamp. A compaction summary (user-role history that the
    /// model has already observed through the summary projection) can never
    /// be marked fresh. The referenced messages must also occur in canonical
    /// history in the caller-supplied order: their canonical positions must
    /// be strictly increasing in `message_ids` order. The runtime never
    /// sorts or reinterprets a caller-supplied turn order; an invalid
    /// execution state fails explicitly.
    ///
    /// # Errors
    ///
    /// Returns the specific [`FreshInboundError`] of the first violation.
    pub fn validate_against(&self, history: &[MessageBlock]) -> Result<(), FreshInboundError> {
        let mut previous_position: Option<usize> = None;
        for id in &self.message_ids {
            let position = history
                .iter()
                .position(|message| message_id_of(message) == *id)
                .ok_or_else(|| FreshInboundError::UnknownMessage(id.clone()))?;
            if let Some(previous) = previous_position
                && position <= previous
            {
                return Err(FreshInboundError::OutOfCanonicalOrder {
                    previous: message_id_of(&history[previous]),
                    next: id.clone(),
                });
            }
            let message = &history[position];
            match message {
                MessageBlock::User(user) => {
                    if user.kind != InboundKind::Message {
                        return Err(FreshInboundError::NotInboundMessage(id.clone()));
                    }
                    if user.timestamp.is_none() {
                        return Err(FreshInboundError::MissingTimestamp(id.clone()));
                    }
                }
                _ => return Err(FreshInboundError::NotUserRole(id.clone())),
            }
            previous_position = Some(position);
        }
        Ok(())
    }
}

fn message_id_of(message: &MessageBlock) -> MessageId {
    match message {
        MessageBlock::System(system) => system.id.clone(),
        MessageBlock::User(user) => user.id.clone(),
        MessageBlock::Assistant(assistant) => assistant.id.clone(),
        MessageBlock::Tool(tool) => tool.id.clone(),
    }
}

/// The conversation-owned inbound boundary.
///
/// The mailbox is bound to exactly one [`ConversationId`] (the durable
/// store's conversation), is cheap to clone, and is shared by concurrent
/// runtime/human producers while one `AgentExecution` borrows or holds the
/// same conversation. Acceptance, selection, and adoption all pass through
/// the one durable store; the mailbox owns validation, lifecycle gating,
/// observation, and the process-local wake.
#[derive(Clone)]
pub struct ConversationInboundMailbox {
    conversation_id: ConversationId,
    state: Arc<Mutex<MailboxState>>,
    store: Arc<dyn InboundStore>,
    /// The shared admission wake handle: every successful acceptance
    /// notifies it, so an idle conversation coordinator (Issue #61) wakes and
    /// admits the asynchronous inbound without any client request. The wake
    /// carries no payload and stores one permit even with no waiter, so an
    /// acceptance between two waits is never missed. The wake is leaf-only:
    /// the coordinator waits on it and never notifies it.
    wake: Arc<tokio::sync::Notify>,
}

impl ConversationInboundMailbox {
    /// Creates a fresh inbound boundary over one conversation backed by an
    /// in-memory durable inbox.
    ///
    /// This is the standalone/test convenience constructor: it uses the same
    /// durable-store API as production, just over an in-memory `SQLite`
    /// database, so acceptance, sequence allocation, and adoption semantics
    /// are identical. Production wiring uses
    /// [`ConversationInboundMailbox::over_store`] with a file-backed store.
    ///
    /// # Panics
    ///
    /// Panics only if the in-memory `SQLite` database cannot be opened,
    /// which is unreachable for an in-memory database.
    #[must_use]
    pub fn new(conversation_id: ConversationId) -> Self {
        let store = Arc::new(
            crate::durable::SqliteInboundStore::in_memory(conversation_id)
                .expect("an in-memory durable inbox always opens"),
        );
        Self::over_store(store)
    }

    /// Creates an inbound boundary over one durable inbound store.
    #[must_use]
    pub fn over_store(store: Arc<dyn InboundStore>) -> Self {
        let conversation_id = store.conversation_id().clone();
        Self {
            conversation_id,
            state: Arc::new(Mutex::new(MailboxState {
                lifecycle: None,
                observer: None,
            })),
            store,
            wake: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Installs the observer and captures the currently pending items as
    /// one atomic boundary.
    ///
    /// This is the mailbox half of the Issue #61 adapter bootstrap
    /// handshake. Because an inactive conversation refuses acceptance, the
    /// durable pending set is frozen across the handshake; the observer is
    /// installed first and the pending seed is then read from the durable
    /// store, so no acceptance can be lost between the seed and the live
    /// observation stream and none can be applied twice.
    ///
    /// # Panics
    ///
    /// Panics only if the mailbox lock is poisoned.
    pub(crate) fn install_observer_and_pending(
        &self,
        observer: Arc<dyn InboundObserver>,
    ) -> Vec<InboundItem> {
        {
            let mut state = self.state.lock().expect("inbound mailbox lock poisoned");
            debug_assert!(
                state
                    .lifecycle
                    .as_ref()
                    .is_some_and(|lifecycle| !lifecycle.is_active()),
                "the bootstrap handshake runs only while the owning runtime is inactive"
            );
            state.observer = Some(observer);
        }
        // The durable store is the pending authority; the mailbox keeps no
        // process-local queue that could drift from it.
        self.store
            .load_pending()
            .unwrap_or_default()
            .into_iter()
            .map(|item| InboundItem {
                sequence: item.sequence,
                message: item.message,
            })
            .collect()
    }

    /// Marks this mailbox as owned by a conversation runtime that has not
    /// been activated yet: inbound is refused until the owning runtime's
    /// shared lifecycle transitions to `Active`.
    ///
    /// This is part of the tool-runtime ownership transfer (Issue #61). The
    /// mailbox keeps no activation state of its own: the lifecycle handle
    /// *is* the runtime ownership.
    ///
    /// # Panics
    ///
    /// Panics only if the mailbox lock is poisoned.
    pub(crate) fn bind_inactive(&self, lifecycle: &ConversationLifecycle) {
        self.state
            .lock()
            .expect("inbound mailbox lock poisoned")
            .lifecycle = Some(lifecycle.clone());
    }

    /// Reverts [`ConversationInboundMailbox::bind_inactive`] back to the
    /// standalone unbound state.
    ///
    /// # Panics
    ///
    /// Panics only if the mailbox lock is poisoned.
    pub(crate) fn unbind(&self) {
        self.state
            .lock()
            .expect("inbound mailbox lock poisoned")
            .lifecycle = None;
    }

    /// Whether a conversation runtime owns this mailbox and its shared
    /// lifecycle has not transitioned to `Active` yet.
    ///
    /// # Panics
    ///
    /// Panics only if the mailbox lock is poisoned.
    #[must_use]
    pub(crate) fn is_bound_inactive(&self) -> bool {
        let state = self.state.lock().expect("inbound mailbox lock poisoned");
        state
            .lifecycle
            .as_ref()
            .is_some_and(|lifecycle| !lifecycle.is_active())
    }

    /// The conversation this mailbox belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The durable Pending Inbound Inbox this mailbox coordinates over.
    pub(crate) fn store(&self) -> Arc<dyn InboundStore> {
        Arc::clone(&self.store)
    }

    /// The shared admission wake handle of this mailbox.
    pub(crate) fn wake(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.wake)
    }

    /// Enqueues one ordinary inbound message through the durable acceptance
    /// owner.
    ///
    /// The message must be an [`InboundKind::Message`] carrying a persisted
    /// [`UserMessageBlock::timestamp`]. Sequence allocation and pending
    /// persistence commit in one durable transaction before success is
    /// returned; only after that commit does the process-local wake fire.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::CompactionSummaryNotEligible`] for a
    /// compaction summary, [`MailboxError::MissingTimestamp`] for an
    /// ordinary message without a persisted timestamp,
    /// [`MailboxError::ConversationInactive`] when a conversation runtime
    /// owns this mailbox and has not been activated, and
    /// [`MailboxError::Inbox`] for a durable acceptance failure. A failed
    /// acceptance consumes no sequence.
    ///
    /// # Panics
    ///
    /// Panics only if the mailbox lock is poisoned.
    pub fn enqueue(&self, message: UserMessageBlock) -> Result<InboundSequence, MailboxError> {
        if message.kind == InboundKind::CompactionSummary {
            return Err(MailboxError::CompactionSummaryNotEligible);
        }
        let timestamp = message.timestamp.ok_or(MailboxError::MissingTimestamp)?;
        let accepted = self.accept_draft(InboundDraft {
            message_id: Some(message.id),
            source: message.source,
            kind: message.kind,
            content: message.content,
            timestamp,
            correlation: None,
        })?;
        Ok(accepted.sequence)
    }

    /// Enqueues one producer-correlated ordinary inbound message through the
    /// durable acceptance owner (exactly-once producer retry semantics).
    ///
    /// A retry with the same committed `correlation` resolves to the same
    /// acceptance without allocating a second sequence.
    ///
    /// # Errors
    ///
    /// Returns the same [`MailboxError`] variants as
    /// [`ConversationInboundMailbox::enqueue`].
    pub fn enqueue_correlated(
        &self,
        message: UserMessageBlock,
        correlation: String,
    ) -> Result<InboundSequence, MailboxError> {
        if message.kind == InboundKind::CompactionSummary {
            return Err(MailboxError::CompactionSummaryNotEligible);
        }
        let timestamp = message.timestamp.ok_or(MailboxError::MissingTimestamp)?;
        let accepted = self.accept_draft(InboundDraft {
            message_id: Some(message.id),
            source: message.source,
            kind: message.kind,
            content: message.content,
            timestamp,
            correlation: Some(correlation),
        })?;
        Ok(accepted.sequence)
    }

    /// The one durable acceptance linearization point: validate the draft,
    /// durably accept it, then publish the process-local observation and
    /// wake.
    ///
    /// Producer success may be reported only after this method returns `Ok`,
    /// which happens only after the durable transaction commits. The wake is
    /// a liveness optimization that fires strictly after that commit.
    pub(crate) fn accept_draft(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, MailboxError> {
        if draft.kind == InboundKind::CompactionSummary {
            return Err(MailboxError::CompactionSummaryNotEligible);
        }
        {
            let state = self.state.lock().expect("inbound mailbox lock poisoned");
            if state
                .lifecycle
                .as_ref()
                .is_some_and(|lifecycle| !lifecycle.is_active())
            {
                return Err(MailboxError::ConversationInactive {
                    conversation_id: self.conversation_id.clone(),
                });
            }
        }
        let accepted = self.store.accept_inbound(draft)?;
        {
            let state = self.state.lock().expect("inbound mailbox lock poisoned");
            let item = InboundItem {
                sequence: accepted.sequence,
                message: accepted.message.clone(),
            };
            if let Some(observer) = &state.observer {
                observer.on_enqueued(&item);
            }
        }
        self.wake.notify_one();
        Ok(accepted)
    }

    /// Selects the currently pending items as one finite watermark-bounded
    /// batch, without consuming them.
    ///
    /// Selection is a durable read: it never mutates the Pending Inbound
    /// Inbox, so a crash after selection leaves the items pending. Adoption
    /// is the separate [`ConversationInboundMailbox::adopt_pending_batch`]
    /// transaction.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::Inbox`] on a durable read failure.
    pub fn select_pending_batch(&self) -> Result<Option<InboundBatch>, MailboxError> {
        let Some(batch) = self.store.select_pending_batch()? else {
            return Ok(None);
        };
        Ok(Some(InboundBatch {
            conversation_id: self.conversation_id.clone(),
            watermark: batch.watermark,
            items: batch
                .items
                .into_iter()
                .map(|item| InboundItem {
                    sequence: item.sequence,
                    message: item.message,
                })
                .collect(),
        }))
    }

    /// Atomically adopts the selected batch into the durable canonical
    /// message ledger and removes the pending records.
    ///
    /// This is the canonical-adoption linearization point: the durable
    /// append and the pending removal share one transaction, so a crash can
    /// never observe a pending record whose canonical message is absent nor
    /// a canonical message that remains independently re-adoptable. The
    /// returned messages are the adopted canonical `User` messages in strict
    /// sequence order.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::Inbox`] on a durable adoption failure, in
    /// which case the selected items remain pending and recoverable.
    ///
    /// # Panics
    ///
    /// Panics only if the mailbox lock is poisoned.
    pub fn adopt_pending_batch(
        &self,
        batch: &InboundBatch,
    ) -> Result<Vec<MessageBlock>, MailboxError> {
        let adopted = self.store.adopt_pending_batch(batch.watermark())?;
        {
            let state = self.state.lock().expect("inbound mailbox lock poisoned");
            if let Some(observer) = &state.observer {
                observer.on_drained(batch);
            }
        }
        Ok(adopted)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConversationInboundMailbox, FreshInboundError, FreshInboundTurn, InboundSequence,
        InitialTurnTrigger, MailboxError,
    };
    use crate::durable::sqlite::SqliteInboundStore;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{ConversationId, MessageId};
    use chrono::{DateTime, TimeZone, Utc};
    use std::sync::Arc;

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
            .single()
            .expect("valid fixed time")
    }

    fn message(id: &str, text: &str, source: UserSource) -> UserMessageBlock {
        UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source,
            kind: InboundKind::Message,
            timestamp: Some(fixed_time()),
        }
    }

    fn human(id: &str, text: &str) -> UserMessageBlock {
        message(id, text, UserSource::Human)
    }

    fn runtime(id: &str, text: &str) -> UserMessageBlock {
        message(id, text, UserSource::Runtime)
    }

    fn mailbox() -> ConversationInboundMailbox {
        ConversationInboundMailbox::new(ConversationId::new("conv-1"))
    }

    /// The first successful acceptance receives sequence 1.
    #[test]
    fn first_acceptance_receives_sequence_one() {
        let mailbox = mailbox();
        assert_eq!(mailbox.enqueue(human("m1", "hi")).expect("accept").get(), 1);
    }

    /// A mailbox claimed by an inactive conversation runtime (Issue #61)
    /// refuses inbound with the typed error and consumes no sequence; the
    /// shared lifecycle's activation transition restores admission.
    #[test]
    fn acceptance_is_refused_while_the_owning_runtime_is_inactive() {
        let mailbox = mailbox();
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        mailbox.bind_inactive(&lifecycle);
        assert_eq!(
            mailbox.enqueue(human("m1", "early")),
            Err(MailboxError::ConversationInactive {
                conversation_id: ConversationId::new("conv-1"),
            })
        );
        assert!(mailbox.select_pending_batch().expect("select").is_none());
        assert!(lifecycle.activate(), "the first lifecycle transition wins");
        assert_eq!(
            mailbox
                .enqueue(human("m2", "after activation"))
                .expect("accept")
                .get(),
            1,
            "the refused acceptance consumed no sequence"
        );
    }

    /// Subsequent successful acceptances strictly increment.
    #[test]
    fn acceptances_strictly_increment() {
        let mailbox = mailbox();
        let first = mailbox.enqueue(human("m1", "a")).expect("accept a");
        let second = mailbox.enqueue(runtime("m2", "b")).expect("accept b");
        let third = mailbox.enqueue(human("m3", "c")).expect("accept c");
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        assert_eq!(third.get(), 3);
        assert!(first < second && second < third);
    }

    /// Human and Runtime producers share the same durable sequence domain.
    #[test]
    fn human_and_runtime_share_one_sequence_domain() {
        let mailbox = mailbox();
        let human_seq = mailbox.enqueue(human("m1", "a")).expect("human");
        let runtime_seq = mailbox.enqueue(runtime("m2", "b")).expect("runtime");
        let human_seq_2 = mailbox.enqueue(human("m3", "c")).expect("human again");
        assert_eq!(human_seq.get(), 1);
        assert_eq!(runtime_seq.get(), 2);
        assert_eq!(human_seq_2.get(), 3);
    }

    /// A failed acceptance consumes no sequence.
    #[test]
    fn failed_acceptance_consumes_no_sequence() {
        let mailbox = mailbox();
        assert_eq!(
            mailbox
                .enqueue(UserMessageBlock {
                    kind: InboundKind::CompactionSummary,
                    ..human("m1", "derived")
                })
                .expect_err("compaction summary rejected")
                .to_string(),
            "a compaction summary is not mailbox-eligible"
        );
        assert_eq!(
            mailbox
                .enqueue(UserMessageBlock {
                    timestamp: None,
                    ..human("m2", "no time")
                })
                .expect_err("missing timestamp rejected")
                .to_string(),
            "an ordinary inbound message requires its persisted timestamp"
        );
        let first = mailbox.enqueue(human("m3", "ok")).expect("accept");
        assert_eq!(first.get(), 1, "no sequence was consumed by failures");
    }

    /// The durable sequence domain cannot wrap: exhaustion fails explicitly.
    #[test]
    fn sequence_exhaustion_fails_explicitly() {
        let store =
            SqliteInboundStore::in_memory(ConversationId::new("conv-1")).expect("in-memory store");
        // Force the counter to the max value directly in the database.
        store.force_next_sequence_for_test(i64::MAX);
        let mailbox = ConversationInboundMailbox::over_store(Arc::new(store));
        assert!(matches!(
            mailbox.enqueue(human("m1", "late")),
            Err(MailboxError::Inbox(_))
        ));
    }

    /// An empty inbox selects to None, never an empty batch.
    #[test]
    fn empty_inbox_selects_to_none() {
        let mailbox = mailbox();
        assert_eq!(mailbox.select_pending_batch().expect("select"), None);
    }

    /// One pending item produces a one-item batch.
    #[test]
    fn one_pending_item_produces_one_item_batch() {
        let mailbox = mailbox();
        mailbox.enqueue(human("m1", "single")).expect("accept");
        let batch = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("one batch");
        assert_eq!(batch.items().len(), 1);
        assert_eq!(batch.watermark(), InboundSequence(1));
    }

    /// Selection is non-destructive and strictly sequence-ordered with a
    /// correct watermark; adoption consumes exactly the selected batch.
    #[test]
    fn selection_orders_items_and_adoption_consumes_them() {
        let mailbox = mailbox();
        mailbox.enqueue(human("m1", "a")).expect("a");
        mailbox.enqueue(runtime("m2", "b")).expect("b");
        mailbox.enqueue(human("m3", "c")).expect("c");
        let batch = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("one batch");
        assert_eq!(batch.watermark(), InboundSequence(3));
        let sequences: Vec<u64> = batch
            .items()
            .iter()
            .map(|item| item.sequence().get())
            .collect();
        assert_eq!(sequences, vec![1, 2, 3]);
        assert_eq!(batch.conversation_id(), &ConversationId::new("conv-1"));
        // Selection is non-destructive: the batch is still pending.
        let again = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("still there");
        assert_eq!(again.watermark(), InboundSequence(3));
        // Adoption transfers exactly the watermark and removes the records.
        let adopted = mailbox.adopt_pending_batch(&batch).expect("adopt");
        assert_eq!(adopted.len(), 3);
        assert!(
            mailbox.select_pending_batch().expect("select").is_none(),
            "adoption consumes the selected pending batch"
        );
    }

    /// Batch membership follows the selection watermark: an acceptance after
    /// the selection is excluded from the selected batch and belongs to the
    /// next selection.
    #[test]
    fn arrival_after_selection_belongs_to_the_next_batch() {
        let mailbox = mailbox();
        mailbox.enqueue(human("m1", "a")).expect("a");
        mailbox.enqueue(human("m2", "b")).expect("b");
        let first = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("first batch");
        assert_eq!(first.watermark(), InboundSequence(2));
        mailbox.enqueue(runtime("m3", "c")).expect("c");
        // The selected watermark did not move; the new item is in the next
        // selection.
        let second = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("second batch");
        assert_eq!(second.watermark(), InboundSequence(3));
        assert_eq!(
            second.items()[0].sequence().get(),
            1,
            "selection is non-destructive: prior items are still pending"
        );
        // Adopt the first watermark only: item 3 remains pending.
        let adopted = mailbox.adopt_pending_batch(&first).expect("adopt first");
        assert_eq!(adopted.len(), 2);
        let remaining = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("remaining");
        assert_eq!(remaining.watermark(), InboundSequence(3));
        assert_eq!(remaining.items().len(), 1);
        assert_eq!(remaining.items()[0].message().id, MessageId::new("m3"));
    }

    /// The item preserves every piece of the canonical inbound message
    /// exactly: id, source, kind, timestamp, content, and sequence.
    #[test]
    fn metadata_is_preserved_exactly() {
        let mailbox = mailbox();
        let original = human("msg-inbound-7", "deploy it");
        let sequence = mailbox.enqueue(original.clone()).expect("accept");
        let batch = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("batch");
        let item = &batch.items()[0];
        assert_eq!(item.sequence(), sequence);
        assert_eq!(item.message().id, MessageId::new("msg-inbound-7"));
        assert_eq!(item.message().source, UserSource::Human);
        assert_eq!(item.message().kind, InboundKind::Message);
        assert_eq!(item.message().timestamp, Some(fixed_time()));
        assert_eq!(item.message().content, original.content);
        assert_eq!(item.message().id, original.id);
    }

    /// Eligibility: a compaction summary is rejected, an ordinary message
    /// without a timestamp is rejected, and a timestamped ordinary message
    /// is accepted — regardless of provenance.
    #[test]
    fn eligibility_rules() {
        let mailbox = mailbox();
        assert_eq!(
            mailbox
                .enqueue(UserMessageBlock {
                    id: MessageId::new("summary-1"),
                    content: vec![UserContentBlock::Text(TextBlock {
                        text: "earlier history".to_owned(),
                    })],
                    source: UserSource::Runtime,
                    kind: InboundKind::CompactionSummary,
                    timestamp: Some(fixed_time()),
                })
                .expect_err("compaction summary rejected"),
            MailboxError::CompactionSummaryNotEligible
        );
        assert_eq!(
            mailbox
                .enqueue(UserMessageBlock {
                    timestamp: None,
                    ..human("plain-1", "no time")
                })
                .expect_err("missing timestamp rejected"),
            MailboxError::MissingTimestamp
        );
        assert!(mailbox.enqueue(human("ok-1", "fine")).is_ok());
        assert!(mailbox.enqueue(runtime("ok-2", "fine")).is_ok());
    }

    /// A fresh inbound turn is non-empty, ordered, and duplicate-free.
    #[test]
    fn fresh_inbound_turn_contract() {
        assert_eq!(FreshInboundTurn::new(vec![]), Err(FreshInboundError::Empty));
        let duplicate = FreshInboundTurn::new(vec![MessageId::new("m1"), MessageId::new("m1")]);
        assert_eq!(
            duplicate,
            Err(FreshInboundError::DuplicateIds(MessageId::new("m1")))
        );
        let turn = FreshInboundTurn::new(vec![MessageId::new("m1"), MessageId::new("m2")])
            .expect("valid turn");
        assert_eq!(turn.message_ids().len(), 2);
        assert_eq!(turn.last_message_id(), &MessageId::new("m2"));
    }

    /// Freshness is never inferred from role: a compaction summary is
    /// user-role history and is rejected by validation.
    #[test]
    fn fresh_inbound_rejects_compaction_summaries_and_missing_timestamps() {
        let ordinary = MessageBlock::User(human("u1", "hi"));
        let summary = MessageBlock::User(UserMessageBlock {
            kind: InboundKind::CompactionSummary,
            ..human("u2", "derived history")
        });
        let no_time = MessageBlock::User(UserMessageBlock {
            timestamp: None,
            ..human("u3", "legacy")
        });
        let history = vec![ordinary, summary, no_time];
        let turn = FreshInboundTurn::new(vec![MessageId::new("u1"), MessageId::new("u2")])
            .expect("valid ids");
        assert_eq!(
            turn.validate_against(&history),
            Err(FreshInboundError::NotInboundMessage(MessageId::new("u2")))
        );
        let turn = FreshInboundTurn::new(vec![MessageId::new("u1"), MessageId::new("u3")])
            .expect("valid ids");
        assert_eq!(
            turn.validate_against(&history),
            Err(FreshInboundError::MissingTimestamp(MessageId::new("u3")))
        );
        let turn = FreshInboundTurn::new(vec![MessageId::new("u1"), MessageId::new("ghost")])
            .expect("valid ids");
        assert_eq!(
            turn.validate_against(&history),
            Err(FreshInboundError::UnknownMessage(MessageId::new("ghost")))
        );
        let tool = MessageBlock::Tool(crate::message::types::ToolMessageBlock {
            id: MessageId::new("tool-1"),
            tool_call_id: crate::runtime::identity::ToolCallId::new("call-1"),
            tool_id: crate::runtime::identity::ToolId::new("tool-alpha"),
            result: crate::tools::types::ToolExecutionResult {
                status: crate::tools::types::ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 1,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
            },
        });
        let turn = FreshInboundTurn::new(vec![MessageId::new("tool-1")]).expect("valid ids");
        assert_eq!(
            turn.validate_against(&[tool]),
            Err(FreshInboundError::NotUserRole(MessageId::new("tool-1")))
        );
        let turn = FreshInboundTurn::new(vec![MessageId::new("u1")]).expect("valid ids");
        assert!(
            turn.validate_against(&[MessageBlock::User(human("u1", "ok"))])
                .is_ok()
        );
    }

    /// Canonical ordering: canonical `[A, B]` with a fresh `[A, B]` turn is
    /// valid.
    #[test]
    fn fresh_turn_matching_canonical_order_is_valid() {
        let history = vec![
            MessageBlock::User(human("m-a", "A")),
            MessageBlock::User(human("m-b", "B")),
        ];
        let turn = FreshInboundTurn::new(vec![MessageId::new("m-a"), MessageId::new("m-b")])
            .expect("valid ids");
        assert!(turn.validate_against(&history).is_ok());
    }

    /// Canonical ordering: canonical `[A, B]` with a fresh `[B, A]` turn is
    /// rejected explicitly; the runtime never reinterprets caller-supplied
    /// turn order.
    #[test]
    fn fresh_turn_out_of_canonical_order_is_rejected() {
        let history = vec![
            MessageBlock::User(human("m-a", "A")),
            MessageBlock::User(human("m-b", "B")),
        ];
        let turn = FreshInboundTurn::new(vec![MessageId::new("m-b"), MessageId::new("m-a")])
            .expect("valid ids");
        assert_eq!(
            turn.validate_against(&history),
            Err(FreshInboundError::OutOfCanonicalOrder {
                previous: MessageId::new("m-b"),
                next: MessageId::new("m-a"),
            })
        );
    }

    /// Canonical ordering: a selected A/B batch history with the
    /// batch fresh turn `[A, B]` remains valid even when earlier canonical
    /// messages (for example a committed Assistant turn) intervene.
    #[test]
    fn drained_batch_order_remains_valid_in_mixed_history() {
        use crate::message::types::{AssistantContentBlock, AssistantMessageBlock};
        let history = vec![
            MessageBlock::User(human("m-u0", "start")),
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("m-agent-1"),
                content: vec![AssistantContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: "done".to_owned(),
                    },
                )],
            }),
            MessageBlock::User(human("m-a", "A")),
            MessageBlock::User(human("m-b", "B")),
        ];
        let turn = FreshInboundTurn::new(vec![MessageId::new("m-a"), MessageId::new("m-b")])
            .expect("valid ids");
        assert!(turn.validate_against(&history).is_ok());
    }

    /// Canonical ordering: identical messages in non-monotonic timestamp
    /// order are still ordered by canonical position — the final fresh
    /// message in canonical inbound order is the last id, never a timestamp
    /// maximum.
    #[test]
    fn non_monotonic_timestamps_follow_canonical_position() {
        let later = UserMessageBlock {
            timestamp: Some(fixed_time()),
            ..human("m-a", "A")
        };
        let earlier = UserMessageBlock {
            timestamp: Some(fixed_time() - chrono::Duration::hours(2)),
            ..human("m-b", "B")
        };
        let history = vec![MessageBlock::User(later), MessageBlock::User(earlier)];
        let turn = FreshInboundTurn::new(vec![MessageId::new("m-a"), MessageId::new("m-b")])
            .expect("valid ids");
        assert!(turn.validate_against(&history).is_ok());
        assert_eq!(turn.last_message_id(), &MessageId::new("m-b"));
    }

    /// The explicit initial-turn trigger distinguishes a fresh inbound turn
    /// from a pure continuation; a continuation is not an Agent Status
    /// disable switch.
    #[test]
    fn initial_turn_trigger_is_explicit() {
        let fresh = InitialTurnTrigger::FreshInbound(
            FreshInboundTurn::new(vec![MessageId::new("m-1")]).expect("valid turn"),
        );
        assert!(matches!(fresh, InitialTurnTrigger::FreshInbound(_)));
        assert_eq!(
            InitialTurnTrigger::Continuation,
            InitialTurnTrigger::Continuation
        );
        assert_ne!(fresh, InitialTurnTrigger::Continuation);
    }
}
