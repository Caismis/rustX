//! The Message Ledger: immutable committed conversational facts.
//!
//! The Ledger is append-only. Once committed, a record's body is never
//! edited, replaced, or deleted, and its [`MessageId`] stays addressable
//! forever. Compaction appends another canonical message (a runtime
//! compaction summary) and rewrites the *Surface*; it never rewrites the
//! Ledger.
//!
//! The Ledger carries **no** visibility state: there is no `active`,
//! `visible`, or `shadowed` flag anywhere in this module. Visibility belongs
//! to the [`ConversationSurface`](crate::conversation::surface::ConversationSurface)
//! alone.
//!
//! Reads are instrumented so the finite-read invariant is provable: keyed
//! lookups and the explicit audit enumeration are counted separately, and a
//! test can assert that normal projection/compaction never enumerates the
//! Ledger.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::message::types::MessageBlock;
use crate::runtime::identity::MessageId;

/// A Message Ledger contract violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    /// A message with this identity is already committed. Message identity
    /// is unique within one conversation state.
    DuplicateMessageId(MessageId),
    /// The referenced message was never committed to this Ledger.
    UnknownMessage(MessageId),
}

impl core::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateMessageId(id) => {
                write!(f, "message {id} is already committed to the ledger")
            }
            Self::UnknownMessage(id) => write!(f, "message {id} is not in the ledger"),
        }
    }
}

impl std::error::Error for LedgerError {}

/// The deterministic read instrumentation of one Message Ledger.
///
/// The counters exist so the finite current-Surface read boundary is
/// provable without RSS or timing measurements: normal projection and
/// compaction resolve identities through [`MessageLedger::get`] only, and
/// [`LedgerAccess::enumerations`] must stay at zero for them.
#[derive(Debug, Default)]
pub struct LedgerAccess {
    keyed_reads: AtomicU64,
    enumerations: AtomicU64,
}

impl LedgerAccess {
    /// The number of keyed `MessageId` lookups performed so far.
    #[must_use]
    pub fn keyed_reads(&self) -> u64 {
        self.keyed_reads.load(Ordering::Relaxed)
    }

    /// The number of full-Ledger enumerations performed so far.
    ///
    /// Only the explicit audit path ([`MessageLedger::audit_records`])
    /// increments this counter.
    #[must_use]
    pub fn enumerations(&self) -> u64 {
        self.enumerations.load(Ordering::Relaxed)
    }

    /// Resets both counters. Test/diagnostic use only.
    pub fn reset(&self) {
        self.keyed_reads.store(0, Ordering::Relaxed);
        self.enumerations.store(0, Ordering::Relaxed);
    }
}

/// The append-only Message Ledger of one conversation.
///
/// The in-process ledger is intentionally a bounded hot read model:
/// commit-ordered active records plus a `MessageId` → position index. The
/// durable Message Ledger lives behind `ConversationStore`; historical rows
/// are paged from that authority rather than retained here after restart.
#[derive(Debug, Default)]
pub struct MessageLedger {
    records: Vec<MessageBlock>,
    index: HashMap<MessageId, usize>,
    access: Arc<LedgerAccess>,
}

impl PartialEq for MessageLedger {
    /// Two Ledgers are equal when they hold the same committed facts in the
    /// same commit order; read instrumentation is not part of identity.
    fn eq(&self, other: &Self) -> bool {
        self.records == other.records
    }
}

impl MessageLedger {
    /// Creates an empty Ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The shared read instrumentation handle.
    #[must_use]
    pub fn access(&self) -> &Arc<LedgerAccess> {
        &self.access
    }

    /// The number of committed records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the Ledger holds no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Appends one committed conversational fact.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError::DuplicateMessageId`] when the identity was
    /// already committed. Identity is unique and deterministic within one
    /// conversation state.
    pub fn append(&mut self, message: MessageBlock) -> Result<MessageId, LedgerError> {
        let id = message_id_of(&message);
        if self.index.contains_key(&id) {
            return Err(LedgerError::DuplicateMessageId(id));
        }
        Ok(self.append_after_validation(message))
    }

    /// Appends a commit after the caller has already validated that the
    /// identity is absent under exclusive ownership.
    ///
    /// This is the infallible half of the prepare→install canonical-commit
    /// seam (Issue #63): once a caller has validated the Ledger and Surface
    /// identities against the current state, the ordinary [`MessageLedger::append`]
    /// failure can no longer occur.
    pub(crate) fn append_after_validation(&mut self, message: MessageBlock) -> MessageId {
        let id = message_id_of(&message);
        debug_assert!(!self.index.contains_key(&id));
        self.index.insert(id.clone(), self.records.len());
        self.records.push(message);
        id
    }

    /// The committed record of one identity, by keyed lookup.
    ///
    /// This is the only read path normal projection and compaction use, and
    /// it is counted as a keyed read.
    #[must_use]
    pub fn get(&self, message_id: &MessageId) -> Option<&MessageBlock> {
        self.access.keyed_reads.fetch_add(1, Ordering::Relaxed);
        self.index
            .get(message_id)
            .map(|position| &self.records[*position])
    }

    /// Whether an identity is committed. Counted as a keyed read.
    #[must_use]
    pub fn contains(&self, message_id: &MessageId) -> bool {
        self.access.keyed_reads.fetch_add(1, Ordering::Relaxed);
        self.index.contains_key(message_id)
    }

    /// The complete committed history currently resident in the hot read
    /// model, in commit order. After durable bootstrap this is bounded to the
    /// current Surface; the complete historical Ledger is paged from the
    /// `ConversationStore`.
    ///
    /// This is the explicit **audit/read** path: an actual caller (the
    /// Runtime Client read model bootstrap, a diagnostic dump) may ask for
    /// it, and it is counted as one full enumeration. The Context Engine's
    /// normal projection and compaction never call it.
    #[must_use]
    pub fn audit_records(&self) -> &[MessageBlock] {
        self.access.enumerations.fetch_add(1, Ordering::Relaxed);
        &self.records
    }
}

/// The canonical identity of one message block.
#[must_use]
pub fn message_id_of(message: &MessageBlock) -> MessageId {
    match message {
        MessageBlock::User(user) => user.id.clone(),
        MessageBlock::Assistant(assistant) => assistant.id.clone(),
        MessageBlock::Tool(tool) => tool.id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{LedgerError, MessageLedger, message_id_of};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::MessageId;

    fn user(id: &str, text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    /// Appends preserve commit order and stay keyed-addressable.
    #[test]
    fn appends_preserve_order_and_keyed_lookup() {
        let mut ledger = MessageLedger::new();
        assert!(ledger.is_empty());
        ledger.append(user("a", "one")).expect("append a");
        ledger.append(user("b", "two")).expect("append b");
        assert_eq!(ledger.len(), 2);
        assert_eq!(
            ledger.get(&MessageId::new("a")).map(message_id_of),
            Some(MessageId::new("a"))
        );
        assert_eq!(
            ledger
                .audit_records()
                .iter()
                .map(message_id_of)
                .collect::<Vec<_>>(),
            vec![MessageId::new("a"), MessageId::new("b")]
        );
        assert!(ledger.get(&MessageId::new("ghost")).is_none());
    }

    /// A duplicate identity is rejected; the Ledger stays unchanged.
    #[test]
    fn duplicate_message_ids_are_rejected() {
        let mut ledger = MessageLedger::new();
        ledger.append(user("a", "one")).expect("append");
        assert_eq!(
            ledger.append(user("a", "again")).expect_err("duplicate"),
            LedgerError::DuplicateMessageId(MessageId::new("a"))
        );
        assert_eq!(ledger.len(), 1);
    }

    /// The read instrumentation separates keyed lookups from enumerations.
    #[test]
    fn read_instrumentation_separates_keyed_reads_from_enumerations() {
        let mut ledger = MessageLedger::new();
        ledger.append(user("a", "one")).expect("append");
        ledger.access().reset();
        assert_eq!(ledger.access().keyed_reads(), 0);
        assert_eq!(ledger.access().enumerations(), 0);
        let _ = ledger.get(&MessageId::new("a"));
        let _ = ledger.contains(&MessageId::new("a"));
        assert_eq!(ledger.access().keyed_reads(), 2);
        assert_eq!(ledger.access().enumerations(), 0);
        let _ = ledger.audit_records();
        assert_eq!(ledger.access().enumerations(), 1);
    }
}
