//! The M8 `SQLite` backend of the durable Pending Inbound Inbox.
//!
//! [`SqliteInboundStore`] implements [`InboundStore`] over one `SQLite`
//! database. The schema enforces the invariants structurally:
//!
//! - `pending_inbound.sequence` is the primary key: one row per accepted,
//!   not-yet-adopted item in strict sequence order.
//! - `pending_inbound.message_id` and `message_ledger.message_id` are unique:
//!   a stable message identity can never be committed twice.
//! - `inbound_correlation.correlation` is the primary key: a producer retry
//!   with the same committed correlation resolves to the same acceptance
//!   deterministically (exactly-once).
//! - `message_ledger.position` is the append-order position of the durable
//!   canonical prefix.
//!
//! The sequence counter lives in `inbox_meta` so it survives adoption (which
//! deletes pending rows) and restart. There is deliberately no second
//! allocator anywhere.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::message::types::{MessageBlock, UserMessageBlock};
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime::inbound::InboundSequence;

use super::inbox::{
    AcceptedInbound, InboundDraft, InboundStore, InboundStoreError, PendingBatch,
    PendingInboundItem,
};

/// The `SQLite` durable Pending Inbound Inbox of one conversation.
pub struct SqliteInboundStore {
    conversation_id: ConversationId,
    conn: Arc<Mutex<Connection>>,
    /// Test-only fault hooks, armed around the real transaction commit
    /// boundary. Never present in production builds.
    #[cfg(test)]
    pub(crate) fail_next_accept_commit: Arc<AtomicBool>,
    #[cfg(test)]
    pub(crate) fail_next_adopt_commit: Arc<AtomicBool>,
}

impl core::fmt::Debug for SqliteInboundStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SqliteInboundStore")
            .field("conversation_id", &self.conversation_id)
            .finish_non_exhaustive()
    }
}

impl SqliteInboundStore {
    /// Opens (creating when absent) the durable store at `path` and runs the
    /// schema.
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::Storage`] when the database cannot be
    /// opened or migrated.
    pub fn open(conversation_id: ConversationId, path: &Path) -> Result<Self, InboundStoreError> {
        let conn = Connection::open(path)
            .map_err(|error| storage(format!("open {}: {error}", path.display())))?;
        Self::from_connection(conversation_id, conn)
    }

    /// Creates an in-memory store (tests/conformance only). An in-memory
    /// database does not survive reopen; durability regressions use
    /// [`SqliteInboundStore::open`] over a temp file.
    ///
    /// # Errors
    ///
    /// Returns [`InboundStoreError::Storage`] when the in-memory database
    /// cannot be opened or migrated.
    pub fn in_memory(conversation_id: ConversationId) -> Result<Self, InboundStoreError> {
        let conn =
            Connection::open_in_memory().map_err(|error| storage(format!("in-memory: {error}")))?;
        Self::from_connection(conversation_id, conn)
    }

    fn from_connection(
        conversation_id: ConversationId,
        conn: Connection,
    ) -> Result<Self, InboundStoreError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS inbox_meta (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS pending_inbound (
                sequence INTEGER PRIMARY KEY,
                message_id TEXT NOT NULL UNIQUE,
                message_json TEXT NOT NULL,
                correlation TEXT
            );
            CREATE TABLE IF NOT EXISTS inbound_correlation (
                correlation TEXT PRIMARY KEY,
                sequence INTEGER NOT NULL,
                message_id TEXT NOT NULL,
                message_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS message_ledger (
                position INTEGER PRIMARY KEY,
                message_id TEXT NOT NULL UNIQUE,
                message_json TEXT NOT NULL
            );
            INSERT OR IGNORE INTO inbox_meta (key, value)
                VALUES ('next_inbound_sequence', 0);",
        )
        .map_err(|error| storage(format!("migrate: {error}")))?;
        Ok(Self {
            conversation_id,
            conn: Arc::new(Mutex::new(conn)),
            #[cfg(test)]
            fail_next_accept_commit: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            fail_next_adopt_commit: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Arms the next acceptance to roll back immediately before its commit
    /// (the transaction boundary itself). Test-only.
    #[cfg(test)]
    pub(crate) fn arm_fail_next_accept_commit(&self) {
        self.fail_next_accept_commit.store(true, Ordering::SeqCst);
    }

    /// Arms the next adoption to roll back immediately before its commit.
    /// Test-only.
    #[cfg(test)]
    pub(crate) fn arm_fail_next_adopt_commit(&self) {
        self.fail_next_adopt_commit.store(true, Ordering::SeqCst);
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, InboundStoreError> {
        self.conn
            .lock()
            .map_err(|_| storage("the inbound store connection is poisoned"))
    }

    /// Forces the sequence counter for the exhaustion regression. Test-only.
    #[cfg(test)]
    pub(crate) fn force_next_sequence_for_test(&self, value: i64) {
        self.conn
            .lock()
            .expect("connection")
            .execute(
                "UPDATE inbox_meta SET value = ?1 WHERE key = 'next_inbound_sequence'",
                [value],
            )
            .expect("force counter");
    }

    /// The next unallocated inbound sequence.
    ///
    /// The counter is stored as a signed 64-bit integer (`SQLite` `INTEGER`),
    /// so exhaustion is checked at the storage representation: the next
    /// value must still be representable as a non-negative `i64`.
    fn next_sequence(conn: &Connection) -> Result<u64, InboundStoreError> {
        let current: i64 = conn
            .query_row(
                "SELECT value FROM inbox_meta WHERE key = 'next_inbound_sequence'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("read sequence: {error}")))?;
        let next = current
            .checked_add(1)
            .ok_or(InboundStoreError::SequenceExhausted)?;
        u64::try_from(next).map_err(|_| InboundStoreError::SequenceExhausted)
    }

    /// Whether a message identity is already committed to the durable
    /// pending or canonical domain.
    fn message_id_exists(
        conn: &Connection,
        message_id: &MessageId,
    ) -> Result<bool, InboundStoreError> {
        let id = message_id.as_str();
        let pending: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_inbound WHERE message_id = ?1)",
                [id],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("pending id probe: {error}")))?;
        if pending {
            return Ok(true);
        }
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM message_ledger WHERE message_id = ?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("ledger id probe: {error}")))
    }
}

impl InboundStore for SqliteInboundStore {
    fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    fn accept_inbound(&self, draft: InboundDraft) -> Result<AcceptedInbound, InboundStoreError> {
        if draft.content.is_empty() {
            return Err(InboundStoreError::EmptyContent);
        }
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("accept transaction: {error}")))?;

        // Idempotency: a producer retry with the same committed correlation
        // resolves to the original acceptance without allocating a new
        // sequence. The correlation table survives adoption, so this is
        // exactly-once across the pending/canonical boundary.
        let correlation_hit = match draft.correlation.as_deref() {
            Some(correlation) => tx
                .query_row(
                    "SELECT sequence, message_id, message_json
                     FROM inbound_correlation WHERE correlation = ?1",
                    [correlation],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| storage(format!("correlation probe: {error}")))?,
            None => None,
        };
        if let Some((sequence_value, message_id_value, message_json_value)) = correlation_hit {
            let sequence = InboundSequence::new(sequence_from_i64(sequence_value)?);
            let message_id = MessageId::new(message_id_value);
            let message: UserMessageBlock = serde_json::from_str(&message_json_value)
                .map_err(|error| storage(format!("correlation decode: {error}")))?;
            return Ok(AcceptedInbound {
                sequence,
                message_id,
                message,
                retried: true,
            });
        }

        let sequence = Self::next_sequence(&tx)?;
        let sequence_i64 = seq_to_i64(sequence)?;
        let message_id = draft.message_id.clone().unwrap_or_else(|| {
            MessageId::new(format!("{}-inbound-{sequence}", self.conversation_id))
        });
        if Self::message_id_exists(&tx, &message_id)? {
            return Err(InboundStoreError::DuplicateMessageId(message_id));
        }
        let message = UserMessageBlock {
            id: message_id.clone(),
            content: draft.content.clone(),
            source: draft.source.clone(),
            kind: draft.kind.clone(),
            timestamp: Some(draft.timestamp),
        };
        let message_json = serde_json::to_string(&message)
            .map_err(|error| storage(format!("serialize inbound: {error}")))?;
        tx.execute(
            "INSERT INTO pending_inbound (sequence, message_id, message_json, correlation)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                sequence_i64,
                message_id.as_str(),
                message_json,
                draft.correlation.as_deref()
            ],
        )
        .map_err(|error| map_insert_error(&error, &message_id))?;
        tx.execute(
            "UPDATE inbox_meta SET value = ?1 WHERE key = 'next_inbound_sequence'",
            [sequence_i64],
        )
        .map_err(|error| storage(format!("update sequence: {error}")))?;
        if let Some(correlation) = draft.correlation.as_deref() {
            tx.execute(
                "INSERT INTO inbound_correlation (correlation, sequence, message_id, message_json)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![correlation, sequence_i64, message_id.as_str(), message_json],
            )
            .map_err(|error| storage(format!("insert correlation: {error}")))?;
        }
        #[cfg(test)]
        if self.fail_next_accept_commit.swap(false, Ordering::SeqCst) {
            return Err(storage("fault injected: accept commit"));
        }
        tx.commit()
            .map_err(|error| storage(format!("accept commit: {error}")))?;
        Ok(AcceptedInbound {
            sequence: InboundSequence::new(sequence),
            message_id,
            message,
            retried: false,
        })
    }

    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, InboundStoreError> {
        let conn = self.lock()?;
        let items = load_pending_rows(&conn)?;
        let Some(watermark) = items.last().map(|item| item.sequence) else {
            return Ok(None);
        };
        Ok(Some(PendingBatch {
            conversation_id: self.conversation_id.clone(),
            watermark,
            items,
        }))
    }

    fn adopt_pending_batch(
        &self,
        watermark: InboundSequence,
    ) -> Result<Vec<MessageBlock>, InboundStoreError> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("adopt transaction: {error}")))?;
        let watermark_i64 = seq_to_i64(watermark.get())?;
        let items: Vec<PendingInboundItem> = {
            let mut statement = tx
                .prepare(
                    "SELECT sequence, message_id, message_json, correlation
                     FROM pending_inbound WHERE sequence <= ?1 ORDER BY sequence",
                )
                .map_err(|error| storage(format!("adopt select: {error}")))?;
            statement
                .query_map([watermark_i64], |row| {
                    let sequence = InboundSequence::new(read_sequence(row.get::<_, i64>(0)?)?);
                    let message_id = MessageId::new(row.get::<_, String>(1)?);
                    let message: UserMessageBlock = serde_json::from_str(&row.get::<_, String>(2)?)
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?;
                    Ok(PendingInboundItem {
                        sequence,
                        message_id,
                        message,
                        correlation: row.get::<_, Option<String>>(3)?,
                    })
                })
                .map_err(|error| storage(format!("adopt map: {error}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| storage(format!("adopt collect: {error}")))?
        };
        if items.is_empty() {
            tx.commit()
                .map_err(|error| storage(format!("adopt commit (empty): {error}")))?;
            return Ok(Vec::new());
        }
        // The append position continues from the current durable canonical
        // prefix; it is computed once under the serialized write transaction.
        let mut position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), 0) FROM message_ledger",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("adopt position: {error}")))?;
        let mut adopted = Vec::with_capacity(items.len());
        for item in &items {
            position = position.checked_add(1).ok_or_else(|| {
                InboundStoreError::Storage("canonical ledger position exhausted".to_owned())
            })?;
            let block = super::inbox::canonical_block(&item.message);
            let message_json = serde_json::to_string(&block)
                .map_err(|error| storage(format!("serialize canonical: {error}")))?;
            tx.execute(
                "INSERT INTO message_ledger (position, message_id, message_json)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![position, item.message_id.as_str(), message_json],
            )
            .map_err(|error| map_insert_error(&error, &item.message_id))?;
            adopted.push(block);
        }
        tx.execute(
            "DELETE FROM pending_inbound WHERE sequence <= ?1",
            [watermark_i64],
        )
        .map_err(|error| storage(format!("adopt delete: {error}")))?;
        #[cfg(test)]
        if self.fail_next_adopt_commit.swap(false, Ordering::SeqCst) {
            return Err(storage("fault injected: adopt commit"));
        }
        tx.commit()
            .map_err(|error| storage(format!("adopt commit: {error}")))?;
        Ok(adopted)
    }

    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, InboundStoreError> {
        let conn = self.lock()?;
        load_pending_rows(&conn)
    }

    fn seed_canonical(&self, messages: &[MessageBlock]) -> Result<(), InboundStoreError> {
        if messages.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("seed transaction: {error}")))?;
        let mut position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), 0) FROM message_ledger",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("seed position: {error}")))?;
        if position > 0 {
            // The canonical prefix is already seeded (recovery): leave it
            // unchanged. The caller re-supplies the same deterministic
            // initial messages across restarts.
            return Ok(());
        }
        for message in messages {
            position = position.checked_add(1).ok_or_else(|| {
                InboundStoreError::Storage("canonical ledger position exhausted".to_owned())
            })?;
            let id = crate::conversation::message_id_of(message);
            let message_json = serde_json::to_string(message)
                .map_err(|error| storage(format!("serialize seed: {error}")))?;
            tx.execute(
                "INSERT INTO message_ledger (position, message_id, message_json)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![position, id.as_str(), message_json],
            )
            .map_err(|error| map_insert_error(&error, &id))?;
        }
        tx.commit()
            .map_err(|error| storage(format!("seed commit: {error}")))?;
        Ok(())
    }

    fn load_canonical(&self) -> Result<Vec<MessageBlock>, InboundStoreError> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare("SELECT message_json FROM message_ledger ORDER BY position")
            .map_err(|error| storage(format!("load canonical: {error}")))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| storage(format!("load canonical map: {error}")))?;
        let mut messages = Vec::new();
        for row in rows {
            let json = row.map_err(|error| storage(format!("load canonical row: {error}")))?;
            let message: MessageBlock = serde_json::from_str(&json)
                .map_err(|error| storage(format!("decode canonical: {error}")))?;
            messages.push(message);
        }
        Ok(messages)
    }
}

/// Loads every pending row in strict sequence order.
fn load_pending_rows(conn: &Connection) -> Result<Vec<PendingInboundItem>, InboundStoreError> {
    let mut statement = conn
        .prepare(
            "SELECT sequence, message_id, message_json, correlation
             FROM pending_inbound ORDER BY sequence",
        )
        .map_err(|error| storage(format!("load pending: {error}")))?;
    let rows = statement
        .query_map([], |row| {
            let sequence = InboundSequence::new(read_sequence(row.get::<_, i64>(0)?)?);
            let message_id = MessageId::new(row.get::<_, String>(1)?);
            let message: UserMessageBlock = serde_json::from_str(&row.get::<_, String>(2)?)
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            Ok(PendingInboundItem {
                sequence,
                message_id,
                message,
                correlation: row.get::<_, Option<String>>(3)?,
            })
        })
        .map_err(|error| storage(format!("load pending map: {error}")))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(format!("load pending collect: {error}")))
}

fn sequence_from_i64(value: i64) -> Result<u64, InboundStoreError> {
    u64::try_from(value).map_err(|_| InboundStoreError::SequenceExhausted)
}

/// Converts a sequence value to its signed 64-bit storage representation.
fn seq_to_i64(value: u64) -> Result<i64, InboundStoreError> {
    i64::try_from(value).map_err(|_| InboundStoreError::SequenceExhausted)
}

/// Reads a non-negative sequence column inside a `query_map` closure, mapping
/// an out-of-range value to a rusqlite conversion error.
fn read_sequence(value: i64) -> Result<u64, rusqlite::Error> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn storage(message: impl Into<String>) -> InboundStoreError {
    InboundStoreError::Storage(message.into())
}

/// Maps an insert failure to the domain error: a uniqueness violation is a
/// duplicate message identity, anything else is a storage failure.
fn map_insert_error(error: &rusqlite::Error, message_id: &MessageId) -> InboundStoreError {
    if is_constraint_violation(error) {
        InboundStoreError::DuplicateMessageId(message_id.clone())
    } else {
        storage(format!("insert {message_id}: {error}"))
    }
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(error, _)
            if error.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::content::TextBlock;
    use crate::message::types::{InboundKind, UserContentBlock, UserSource};
    use chrono::{DateTime, TimeZone, Utc};

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
            .single()
            .expect("valid fixed time")
    }

    fn human(text: &str) -> InboundDraft {
        InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            timestamp: fixed_time(),
            correlation: None,
        }
    }

    fn store() -> SqliteInboundStore {
        SqliteInboundStore::in_memory(ConversationId::new("conv-1")).expect("in-memory store")
    }

    /// A failed acceptance (fault injected at the commit boundary) exposes no
    /// pending record and consumes no sequence: the next acceptance is
    /// sequence 1.
    #[test]
    fn accept_commit_failure_exposes_no_pending_and_no_sequence() {
        let store = store();
        store.arm_fail_next_accept_commit();
        assert!(store.accept_inbound(human("boom")).is_err());
        assert!(
            store.load_pending().expect("load").is_empty(),
            "no pending record survives the failed transaction"
        );
        let accepted = store.accept_inbound(human("ok")).expect("accept");
        assert_eq!(accepted.sequence.get(), 1, "no sequence was consumed");
        assert!(!accepted.retried);
    }

    /// A failed adoption (fault injected at the commit boundary) leaves the
    /// selected items pending and the canonical ledger unchanged; a retry
    /// adopts them exactly once.
    #[test]
    fn adopt_commit_failure_leaves_items_recoverably_pending() {
        let store = store();
        let first = store.accept_inbound(human("A")).expect("accept A");
        store.arm_fail_next_adopt_commit();
        let batch = store
            .select_pending_batch()
            .expect("select")
            .expect("batch");
        assert_eq!(batch.watermark, first.sequence);
        assert!(store.adopt_pending_batch(batch.watermark).is_err());
        assert_eq!(
            store.load_pending().expect("load").len(),
            1,
            "the selected item remains pending"
        );
        assert!(
            store.load_canonical().expect("load").is_empty(),
            "no canonical append survives the failed transaction"
        );
        let adopted = store
            .adopt_pending_batch(first.sequence)
            .expect("retry adopts");
        assert_eq!(adopted.len(), 1);
        assert!(store.load_pending().expect("load").is_empty());
        assert_eq!(store.load_canonical().expect("load").len(), 1);
    }

    /// A producer retry with the same committed correlation resolves to the
    /// same acceptance without allocating a second sequence.
    #[test]
    fn correlation_retry_is_exactly_once() {
        let store = store();
        let draft = InboundDraft {
            message_id: Some(MessageId::new("background-exec_1-terminal")),
            source: UserSource::Runtime,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "settled".to_owned(),
            })],
            timestamp: fixed_time(),
            correlation: Some("background-terminal:exec_1".to_owned()),
        };
        let first = store.accept_inbound(draft.clone()).expect("accept");
        assert_eq!(first.sequence.get(), 1);
        assert!(!first.retried);
        let retry = store.accept_inbound(draft).expect("retry");
        assert_eq!(retry.sequence, first.sequence, "same sequence, no new one");
        assert_eq!(retry.message_id, first.message_id);
        assert!(retry.retried);
        assert_eq!(
            store.load_pending().expect("load").len(),
            1,
            "exactly one pending record for one correlation"
        );
    }
}
