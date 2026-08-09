//! Conversation inbound mailbox: deterministic runtime-owned batching.
//!
//! This module owns the narrow runtime coordination contract for
//! asynchronous user-role messages arriving while an agent attempt is
//! running. The mailbox is a per-conversation in-memory queue with a shared
//! inbound sequence domain, an atomic enqueue (sequence allocation and
//! publication under one synchronization boundary), and an atomic finite
//! drain producing one watermark-bounded [`InboundBatch`].
//!
//! Ownership boundaries:
//!
//! ```text
//! mailbox         = coordination (this module)
//! canonical history = durable conversation truth (message ledger semantics)
//! Event Journal   = execution facts
//! ```
//!
//! The mailbox is **not** canonical conversation history, not the Event
//! Journal, not a Message Ledger persistence backend, not Agent Status, not
//! a background-execution registry, and not a scheduler. Once a drained
//! ordinary inbound message is appended to canonical history by the agent
//! loop, canonical history becomes the authoritative conversation record of
//! that message. Mailbox persistence and crash recovery remain later
//! milestone work.
//!
//! The mailbox accepts only [`InboundKind::Message`] with a persisted
//! [`UserMessageBlock::timestamp`]; a runtime-provided derived compaction
//! summary is not new asynchronous work and is rejected. All `UserSource`
//! provenance shares the same ordering domain, so Human, Runtime, Agent,
//! Fleet, and `ExternalSystem` producers sequence through one mailbox.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock};
use crate::runtime::identity::{ConversationId, MessageId};

/// A conversation-scoped inbound sequence number.
///
/// The sequence identifies one item of the conversation's inbound ordering
/// domain. It is **not**
/// [`RuntimeEventEnvelope`](crate::events::types::RuntimeEventEnvelope)`::sequence`
/// and is never allocated from the Event Journal sequence; the mailbox owns
/// allocation, and the first successful enqueue of a mailbox receives `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InboundSequence(u64);

impl InboundSequence {
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

/// One enqueued inbound message with its mailbox-assigned sequence.
///
/// The item preserves, through the canonical [`UserMessageBlock`] plus the
/// sequence: message id, [`UserSource`](crate::message::types::UserSource),
/// [`InboundKind`], timestamp, content/payload, the original message
/// boundary, and the inbound sequence. Construction is mailbox-owned, so an
/// item cannot be fabricated with an arbitrary sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundItem {
    sequence: InboundSequence,
    message: UserMessageBlock,
}

impl InboundItem {
    /// The mailbox-assigned inbound sequence of the item.
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

/// One atomic finite drain of the conversation mailbox.
///
/// A batch is non-empty, belongs to exactly one [`ConversationId`], contains
/// items in strictly increasing [`InboundSequence`] order, and its watermark
/// equals the sequence of the final/highest selected item; no item sequence
/// exceeds the watermark. One pending message still produces a one-item
/// batch, and every item remains a separate canonical `UserMessageBlock`.
/// An empty mailbox produces `None`, never an empty batch.
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
/// These are API-level validation errors of the conversation mailbox
/// contract; they are not `RuntimeEvent` protocol expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxError {
    /// A runtime-derived compaction summary is not new asynchronous work
    /// and cannot be enqueued as ordinary inbound mail.
    CompactionSummaryNotEligible,
    /// An ordinary inbound message must carry its persisted UTC timestamp;
    /// no wall-clock time is fabricated by the mailbox.
    MissingTimestamp,
    /// The conversation inbound sequence space is exhausted; the mailbox
    /// fails explicitly instead of wrapping to zero.
    SequenceExhausted,
    /// The mailbox belongs to a different conversation than the operation
    /// that tried to bind it.
    ConversationMismatch {
        /// The conversation the binding operation expected.
        expected: ConversationId,
        /// The conversation the mailbox actually belongs to.
        actual: ConversationId,
    },
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
        }
    }
}

impl Error for MailboxError {}

/// The internal synchronized state of one conversation mailbox.
#[derive(Debug)]
struct MailboxState {
    /// The last successfully allocated inbound sequence (0 = none yet).
    last_sequence: u64,
    /// The pending, not-yet-drained items in enqueue order.
    pending: VecDeque<InboundItem>,
    /// Test-only synchronization hooks for controlled race tests.
    #[cfg(test)]
    probe: Option<MailboxProbe>,
}

/// Test-only synchronization hooks.
///
/// Every hook fires while the mailbox lock is held, so tests can establish
/// exact linearization points: `drain_snapshot` fires after the drain
/// established its watermark and detached the items, `drain_release`
/// unblocks a drain parked inside its critical section (so a competing
/// enqueue provably contends against that section), `enqueue_computed`
/// fires after the next sequence was computed and before the item is
/// published, and `enqueue_resume` unblocks a paused enqueue. Each hook is
/// optional so a test installs exactly the hooks it controls. All signals
/// are `std` channels because the mailbox synchronization boundary is a
/// `std` mutex; the pause parks the OS thread, so the race tests run on a
/// multi-threaded runtime. These hooks exist only under `#[cfg(test)]`.
#[cfg(test)]
#[derive(Debug)]
struct MailboxProbe {
    drain_snapshot: Option<std::sync::mpsc::SyncSender<()>>,
    drain_release: Option<std::sync::mpsc::Receiver<()>>,
    enqueue_computed: Option<std::sync::mpsc::SyncSender<()>>,
    enqueue_resume: Option<std::sync::mpsc::Receiver<()>>,
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
            if let Some(previous) = previous_position {
                if position <= previous {
                    return Err(FreshInboundError::OutOfCanonicalOrder {
                        previous: message_id_of(&history[previous]),
                        next: id.clone(),
                    });
                }
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
        MessageBlock::Agent(agent) => agent.id.clone(),
        MessageBlock::Tool(tool) => tool.id.clone(),
    }
}

/// The conversation-owned inbound mailbox.
///
/// The mailbox is bound to exactly one [`ConversationId`], is cheap to
/// clone, and is shared by concurrent runtime/human producers while one
/// `AgentExecution` borrows or holds the same conversation mailbox. All
/// operations are synchronous and bounded: sequence allocation and queue
/// publication happen under one small `std::sync::Mutex` critical section,
/// so no allocated-but-unpublished sequence is ever visible to a drain.
///
/// No global registry and no distributed queue exist; the mailbox is a pure
/// in-memory runtime coordination contract.
#[derive(Clone, Debug)]
pub struct ConversationInboundMailbox {
    conversation_id: ConversationId,
    state: Arc<Mutex<MailboxState>>,
}

impl ConversationInboundMailbox {
    /// Creates a new inbound mailbox bound to one conversation.
    #[must_use]
    pub fn new(conversation_id: ConversationId) -> Self {
        Self {
            conversation_id,
            state: Arc::new(Mutex::new(MailboxState {
                last_sequence: 0,
                pending: VecDeque::new(),
                #[cfg(test)]
                probe: None,
            })),
        }
    }

    /// The conversation this mailbox belongs to.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// Enqueues one ordinary inbound message.
    ///
    /// The message must be an [`InboundKind::Message`] carrying a persisted
    /// [`UserMessageBlock::timestamp`]. Sequence allocation and publication
    /// into the pending queue happen under the same synchronization
    /// boundary: the next sequence is checked, the item is constructed and
    /// pushed, and only then is the sequence committed, so a drain can never
    /// observe an allocated-but-unpublished sequence. The first successful
    /// enqueue receives sequence `1` and successful enqueues advance
    /// strictly monotonically with checked arithmetic; exhaustion fails
    /// explicitly instead of wrapping.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::CompactionSummaryNotEligible`] for a
    /// compaction summary, [`MailboxError::MissingTimestamp`] for an
    /// ordinary message without a persisted timestamp, and
    /// [`MailboxError::SequenceExhausted`] when the sequence space is
    /// exhausted. A failed enqueue consumes no sequence.
    ///
    /// # Panics
    ///
    /// Panics only if the mailbox lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub fn enqueue(&self, message: UserMessageBlock) -> Result<InboundSequence, MailboxError> {
        if message.kind == InboundKind::CompactionSummary {
            return Err(MailboxError::CompactionSummaryNotEligible);
        }
        if message.timestamp.is_none() {
            return Err(MailboxError::MissingTimestamp);
        }
        let mut state = self.state.lock().expect("inbound mailbox lock poisoned");
        let sequence = state
            .last_sequence
            .checked_add(1)
            .ok_or(MailboxError::SequenceExhausted)?;
        let item = InboundItem {
            sequence: InboundSequence(sequence),
            message,
        };
        #[cfg(test)]
        if let Some(probe) = &state.probe {
            if let Some(computed) = &probe.enqueue_computed {
                let _ = computed.send(());
            }
            if let Some(resume) = &probe.enqueue_resume {
                let _ = resume.recv();
            }
        }
        state.pending.push_back(item);
        state.last_sequence = sequence;
        Ok(InboundSequence(sequence))
    }

    /// Atomically drains one finite batch of the currently pending items.
    ///
    /// Under the mailbox lock: an empty mailbox returns `None`; otherwise
    /// the watermark is established as the highest sequence currently
    /// present, exactly the currently pending items through that watermark
    /// are detached, and one non-empty [`InboundBatch`] is returned. Because
    /// enqueue uses the same lock, an enqueue occurring after this
    /// operation's linearization point can never extend the batch; an
    /// arrival after the watermark waits for the next drain.
    ///
    /// One safe agent-loop boundary performs at most one finite drain; the
    /// drain never re-inspects the queue for newly arriving items.
    ///
    /// # Panics
    ///
    /// Panics only if the mailbox lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    #[must_use]
    pub fn drain(&self) -> Option<InboundBatch> {
        let mut state = self.state.lock().expect("inbound mailbox lock poisoned");
        let watermark = state.pending.back().map(|item| item.sequence)?;
        let items = state.pending.drain(..).collect::<Vec<_>>();
        debug_assert!(
            items.iter().all(|item| item.sequence <= watermark),
            "no batch item may exceed the watermark"
        );
        debug_assert!(
            items.last().is_some_and(|item| item.sequence == watermark),
            "the watermark is the highest selected sequence"
        );
        #[cfg(test)]
        if let Some(probe) = &state.probe {
            if let Some(snapshot) = &probe.drain_snapshot {
                let _ = snapshot.send(());
            }
            if let Some(release) = &probe.drain_release {
                let _ = release.recv();
            }
        }
        drop(state);
        Some(InboundBatch {
            conversation_id: self.conversation_id.clone(),
            watermark,
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConversationInboundMailbox, FreshInboundError, FreshInboundTurn, InboundSequence,
        MailboxError, MailboxProbe,
    };
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{ConversationId, MessageId};
    use chrono::{DateTime, TimeZone, Utc};
    use std::sync::Arc;
    use std::sync::mpsc::sync_channel;

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

    /// The first successful enqueue receives sequence 1.
    #[test]
    fn first_enqueue_receives_sequence_one() {
        let mailbox = mailbox();
        assert_eq!(
            mailbox.enqueue(human("m1", "hi")).expect("enqueue").get(),
            1
        );
    }

    /// Subsequent successful enqueues strictly increment.
    #[test]
    fn enqueues_strictly_increment() {
        let mailbox = mailbox();
        let first = mailbox.enqueue(human("m1", "a")).expect("enqueue a");
        let second = mailbox.enqueue(runtime("m2", "b")).expect("enqueue b");
        let third = mailbox.enqueue(human("m3", "c")).expect("enqueue c");
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        assert_eq!(third.get(), 3);
        assert!(first < second && second < third);
    }

    /// Human and Runtime producers share the same sequence domain.
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

    /// A failed enqueue consumes no sequence.
    #[test]
    fn failed_enqueue_consumes_no_sequence() {
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
        let first = mailbox.enqueue(human("m3", "ok")).expect("enqueue");
        assert_eq!(first.get(), 1, "no sequence was consumed by failures");
    }

    /// The sequence cannot wrap on exhaustion: the mailbox fails explicitly.
    #[test]
    fn sequence_exhaustion_fails_explicitly() {
        let mailbox = mailbox();
        mailbox.state.lock().expect("mailbox lock").last_sequence = u64::MAX;
        assert_eq!(
            mailbox.enqueue(human("m1", "late")).expect_err("exhausted"),
            MailboxError::SequenceExhausted
        );
        assert_eq!(
            mailbox.enqueue(human("m2", "later")).expect_err("still"),
            MailboxError::SequenceExhausted
        );
    }

    /// An empty mailbox drains to None, never an empty batch.
    #[test]
    fn empty_mailbox_drains_to_none() {
        let mailbox = mailbox();
        assert_eq!(mailbox.drain(), None);
    }

    /// One pending item produces a one-item batch.
    #[test]
    fn one_pending_item_produces_one_item_batch() {
        let mailbox = mailbox();
        mailbox.enqueue(human("m1", "single")).expect("enqueue");
        let batch = mailbox.drain().expect("one batch");
        assert_eq!(batch.items().len(), 1);
        assert_eq!(batch.watermark(), InboundSequence(1));
    }

    /// Multiple items drain strictly sequence-ordered with a correct
    /// watermark, and a later drain is empty.
    #[test]
    fn drain_orders_items_and_clears_the_queue() {
        let mailbox = mailbox();
        mailbox.enqueue(human("m1", "a")).expect("a");
        mailbox.enqueue(runtime("m2", "b")).expect("b");
        mailbox.enqueue(human("m3", "c")).expect("c");
        let batch = mailbox.drain().expect("one batch");
        assert_eq!(batch.watermark(), InboundSequence(3));
        let sequences: Vec<u64> = batch
            .items()
            .iter()
            .map(|item| item.sequence().get())
            .collect();
        assert_eq!(sequences, vec![1, 2, 3]);
        assert!(
            batch
                .items()
                .iter()
                .all(|item| item.sequence() <= batch.watermark()),
            "no item sequence may exceed the watermark"
        );
        assert_eq!(batch.conversation_id(), &ConversationId::new("conv-1"));
        assert_eq!(
            mailbox.drain(),
            None,
            "a complete drain empties the mailbox"
        );
    }

    /// Batch membership follows the drain's linearization point: an enqueue
    /// after the drain completes waits for the next batch.
    #[test]
    fn arrival_after_drain_waits_for_the_next_batch() {
        let mailbox = mailbox();
        mailbox.enqueue(human("m1", "a")).expect("a");
        mailbox.enqueue(human("m2", "b")).expect("b");
        let first = mailbox.drain().expect("first batch");
        assert_eq!(first.watermark(), InboundSequence(2));
        mailbox.enqueue(runtime("m3", "c")).expect("c");
        let second = mailbox.drain().expect("second batch");
        assert_eq!(second.watermark(), InboundSequence(3));
        assert_eq!(
            second.items()[0].sequence().get(),
            3,
            "the post-watermark arrival opens the next batch"
        );
    }

    /// The item preserves every piece of the canonical inbound message
    /// exactly: id, source, kind, timestamp, content, and sequence.
    #[test]
    fn metadata_is_preserved_exactly() {
        let mailbox = mailbox();
        let original = human("msg-inbound-7", "deploy it");
        let sequence = mailbox.enqueue(original.clone()).expect("enqueue");
        let batch = mailbox.drain().expect("batch");
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

    /// Race A — arrival after the drain snapshot: the drain's watermark is
    /// established before a competing enqueue completes, so the enqueue
    /// joins the next batch. The drain parks inside its critical section,
    /// so the enqueue provably begins against (and blocks on) that section:
    /// the drain cannot release the lock before the test releases it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn race_a_arrival_after_snapshot_never_extends_the_batch() {
        let (drain_tx, drain_rx) = sync_channel(1);
        // Capacity 2: one token releases the parked drain's critical
        // section, and a second token is consumed by the final verification
        // drain of the test — the probe parks every drain, so every drain
        // needs its release token.
        let (release_tx, release_rx) = sync_channel(2);
        let mailbox = ConversationInboundMailbox {
            conversation_id: ConversationId::new("conv-1"),
            state: Arc::new(std::sync::Mutex::new(super::MailboxState {
                last_sequence: 0,
                pending: std::collections::VecDeque::new(),
                probe: Some(MailboxProbe {
                    drain_snapshot: Some(drain_tx),
                    drain_release: Some(release_rx),
                    enqueue_computed: None,
                    enqueue_resume: None,
                }),
            })),
        };
        mailbox.enqueue(human("m1", "A")).expect("enqueue A");

        let draining = mailbox.clone();
        let drain_task = tokio::task::spawn_blocking(move || draining.drain());
        // The drain holds the mailbox lock, established its watermark for A,
        // detached the item, and parked inside its critical section.
        drain_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("drain snapshot established");
        // B attempts to enqueue while the drain is still parked inside its
        // critical section: B's enqueue can only ever acquire the lock after
        // the drain releases it, so it provably blocks against that section
        // and joins the next batch after the snapshot.
        let enqueueing = mailbox.clone();
        let enqueue_task = tokio::task::spawn_blocking(move || {
            enqueueing.enqueue(human("m2", "B")).expect("enqueue B")
        });
        // Release the drain: the critical section ends, the batch with A is
        // returned, and only then can B's enqueue proceed. The second token
        // stays buffered for the final verification drain below.
        release_tx.send(()).expect("release the drain");
        release_tx
            .send(())
            .expect("release the final verification drain");
        let first = drain_task
            .await
            .expect("drain task")
            .expect("first batch must contain A");
        let sequence_b = enqueue_task.await.expect("enqueue task");
        let second = mailbox.drain().expect("second batch must contain B");

        assert_eq!(first.items().len(), 1);
        assert_eq!(first.watermark(), InboundSequence(1));
        assert_eq!(first.items()[0].message().id, MessageId::new("m1"));
        assert_eq!(sequence_b, InboundSequence(2));
        assert_eq!(second.items().len(), 1);
        assert_eq!(second.watermark(), InboundSequence(2));
        assert_eq!(second.items()[0].message().id, MessageId::new("m2"));
    }

    /// Race B — no allocated-but-unpublished sequence: while the enqueue
    /// holds the mailbox lock after computing its sequence and before
    /// publishing the item, a competing drain cannot observe the allocated
    /// sequence; after publication the drain contains the complete item.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn race_b_no_allocated_but_unpublished_sequence() {
        let (computed_tx, computed_rx) = sync_channel(1);
        let (resume_tx, resume_rx) = sync_channel(1);
        let mailbox = ConversationInboundMailbox {
            conversation_id: ConversationId::new("conv-1"),
            state: Arc::new(std::sync::Mutex::new(super::MailboxState {
                last_sequence: 0,
                pending: std::collections::VecDeque::new(),
                probe: Some(MailboxProbe {
                    drain_snapshot: None,
                    drain_release: None,
                    enqueue_computed: Some(computed_tx),
                    enqueue_resume: Some(resume_rx),
                }),
            })),
        };

        let enqueueing = mailbox.clone();
        let enqueue_task = tokio::task::spawn_blocking(move || {
            enqueueing.enqueue(human("m1", "item")).expect("enqueue")
        });
        // The enqueue is inside its critical section: sequence 1 computed,
        // item not yet published.
        computed_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("enqueue sequence computed");
        let draining = mailbox.clone();
        let drain_task = tokio::task::spawn_blocking(move || draining.drain());
        // Release the enqueue: it publishes the item and releases the lock.
        // The drain must then see the complete published item, never a
        // watermark referencing an absent sequence.
        resume_tx.send(()).expect("resume enqueue");
        let sequence = enqueue_task.await.expect("enqueue task");
        let batch = drain_task.await.expect("drain task").expect("batch");
        assert_eq!(sequence, InboundSequence(1));
        assert_eq!(batch.watermark(), InboundSequence(1));
        assert_eq!(batch.items().len(), 1);
        assert_eq!(batch.items()[0].sequence(), InboundSequence(1));
        assert_eq!(mailbox.drain(), None, "the queue holds no hidden item");
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

    /// Canonical ordering: a mailbox-drained A/B batch history with the
    /// drained-batch fresh turn `[A, B]` remains valid even when earlier
    /// canonical messages (for example a committed agent turn) intervene.
    #[test]
    fn drained_batch_order_remains_valid_in_mixed_history() {
        use crate::message::types::{AgentContentBlock, AgentMessageBlock};
        let history = vec![
            MessageBlock::User(human("m-u0", "start")),
            MessageBlock::Agent(AgentMessageBlock {
                id: MessageId::new("m-agent-1"),
                content: vec![AgentContentBlock::Text(
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
        use super::InitialTurnTrigger;
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
