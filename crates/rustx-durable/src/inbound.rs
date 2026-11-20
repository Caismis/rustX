//! Inbound acceptance vocabulary owned by the durable Pending Inbound Inbox.

use rustx_types::durable::canonical::TranscriptCursor;
use rustx_types::message::types::{InboundKind, MessageBlock, UserMessageBlock};
use rustx_types::runtime::identity::{ConversationId, MessageId};
use rustx_types::runtime::types::ConversationLifecycle;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::ConversationStoreError;

/// The read-only observation seam of the conversation inbound boundary.
///
/// A mailbox fact observer receives the authoritative accept/adopt facts at
/// their linearization points. `on_enqueued` fires after durable acceptance
/// commits; `on_drained` fires after the durable canonical adoption commits.
/// An observer must never call back into the mailbox and must never mutate
/// mailbox state; the Runtime Client projection (Issue #37) treats each
/// callback as one projection fold under its own synchronization boundary.
pub trait InboundObserver: Send + Sync {
    /// Observes the finite native pending head captured by the commit transaction.
    /// Missing derived evidence fences display reads without changing commitment.
    fn on_pending(
        &self,
        receipt: Option<&Arc<crate::PendingObservationReceipt>>,
        cut: &crate::reading::ConversationReadCut,
    );
}

/// A conversation-scoped inbound sequence number.
///
/// The sequence identifies one item of the conversation's inbound ordering
/// domain. It is **not**
/// [`RuntimeEventEnvelope`](rustx_types::events::types::RuntimeEventEnvelope)`::sequence`
/// and is never allocated from the Event Journal sequence; the durable
/// Pending Inbound Inbox owns allocation, and the first successful
/// acceptance of a conversation receives `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[derive(schemars::JsonSchema)]
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
/// sequence: message id, [`UserSource`](rustx_types::message::types::UserSource),
/// [`InboundKind`], timestamp, content/payload, the original message
/// boundary, and the inbound sequence. Construction is store-owned, so an
/// item cannot be fabricated with an arbitrary sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundItem {
    pub revision: u64,
    pub sequence: InboundSequence,
    pub message: UserMessageBlock,
    pub transcript_cursor: Option<TranscriptCursor>,
    /// Public app-server input id, already projected from durable correlation.
    pub input_id: Option<String>,
}

impl InboundItem {
    /// Native content revision.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn from_observation_header(
        revision: u64,
        sequence: u64,
        message: UserMessageBlock,
        input_id: Option<String>,
    ) -> Self {
        Self {
            revision,
            sequence: InboundSequence::new(sequence),
            message,
            transcript_cursor: None,
            input_id,
        }
    }

    /// Original public app-server input id, when this row carried one.
    #[must_use]
    pub fn input_id(&self) -> Option<&str> {
        self.input_id.as_deref()
    }

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

    /// The durable transcript position allocated at acceptance, when this
    /// item is visible in the ordinary transcript.
    #[must_use]
    pub fn transcript_cursor(&self) -> Option<TranscriptCursor> {
        self.transcript_cursor
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
    pub conversation_id: ConversationId,
    pub watermark: InboundSequence,
    pub items: Vec<InboundItem>,
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
    /// See `ConversationInboundMailbox::bind_inactive`: this is the
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
    Durable(ConversationStoreError),
}

impl From<ConversationStoreError> for MailboxError {
    fn from(error: ConversationStoreError) -> Self {
        Self::Durable(error)
    }
}

impl fmt::Display for MailboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            Self::Durable(error) => error.fmt(f),
        }
    }
}

impl Error for MailboxError {}

/// The runtime ownership binding of one conversation mailbox (Issue #61).
///
/// A mailbox with no conversation runtime bound over it is an ordinary
/// standalone coordination contract and always accepts inbound. Once a
/// `ConversationRuntime`
/// claims its owning tool runtime, the mailbox carries that runtime's
/// shared activation lifecycle
/// ([`ConversationLifecycle`]):
/// a conversation accepts new inbound only while its lifecycle is `Running`.
///
/// The mailbox stores **no lifecycle state of its own**: runtime ownership is
/// the presence of the lifecycle handle, and admission is answered by the
/// lifecycle itself.
pub struct MailboxState {
    /// The shared activation lifecycle of the conversation runtime that
    /// owns this mailbox, when one does. `None` = standalone/unbound.
    pub lifecycle: Option<ConversationLifecycle>,
    /// The read-only fact observer, installed by the owning runtime client
    /// boundary (Issue #37). It fires after the durable acceptance/adoption
    /// linearization points.
    pub observer: Option<Arc<dyn InboundObserver>>,
}

impl core::fmt::Debug for MailboxState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MailboxState")
            .field(
                "lifecycle",
                &self.lifecycle.as_ref().map(|lifecycle| {
                    if lifecycle.is_running() {
                        "runtime-owned/running"
                    } else {
                        "runtime-owned/not-running"
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
/// The trigger makes the intended execution mode explicit. A `FreshInbound`
/// value offers the optional Agent Status opportunity; it is not a status
/// compatibility switch or a request to create another turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialTurnTrigger {
    /// The attempt's first model invocation observes a fresh inbound turn:
    /// the model has not yet observed the referenced messages. Validation
    /// against canonical history is mandatory, the optional `FreshInbound`
    /// Agent Status opportunity is eligible, fresh-inbound compaction
    /// protection applies, and the trigger remains
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
                    if !matches!(
                        user.kind,
                        InboundKind::Message | InboundKind::GoalContinuation(_)
                    ) {
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

pub struct AdoptedInbound {
    pub canonical_receipts: Vec<crate::TranscriptCommitReceipt>,
    /// The adopted items, in strict sequence order.
    pub items: Vec<InboundItem>,
    /// The committed adoption fact, present exactly when `items` is non-empty.
    pub adoption: Option<rustx_types::events::types::RuntimeEventEnvelope>,
}
