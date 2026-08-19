//! `SQLite` implementation of the semantic conversation durability contract.
//!
//! One database contains five deliberately separate authority domains:
//! Pending Inbound, the append-only Message Ledger, immutable Surface
//! operations, immutable Request Snapshots, and the append-only Event Journal.
//! The tables share transactions where rustX needs one semantic linearization
//! point, but no table is a serialized `ConversationRecord` or transcript.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use std::collections::BTreeSet;

#[cfg(test)]
use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};

use crate::conversation::{SurfaceOp, SurfaceRevision, SurfaceSpan};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{InboundKind, MessageBlock, UserMessageBlock, UserSource};
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::runtime::identity::{ConversationId, EventId, MessageId, RequestId};
use crate::runtime::inbound::InboundSequence;

use super::inbox::{
    AcceptedInbound, CanonicalMessagePage, CompactionCommitInput, ConversationStore,
    ConversationStoreError, DurableConversationHead, EventPage, InboundDraft, PendingBatch,
    PendingInboundItem, RequestSnapshotPage,
};

/// The only schema accepted by this pre-production store. Incompatible
/// databases fail explicitly; there is no migration or legacy reader.
///
/// Version 2 freezes the M9b durable format change: `RequestSnapshot` JSON
/// gained a required `request_context_ids` field, so a v1 database whose
/// snapshots predate that field must fail at store open with an explicit
/// [`ConversationStoreError::SchemaVersionMismatch`] rather than a later
/// accidental JSON decode failure.
pub const SQLITE_SCHEMA_VERSION: i64 = 2;

/// One operation in a deterministic admission fault script.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionFaultOperation {
    /// Fail the next scripted pending-batch selection.
    SelectPendingBatch,
    /// Fail the next scripted pending-batch adoption.
    AdoptPendingBatch,
}

/// One scripted compaction-stage fault used by atomicity regressions.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactionFaultOperation {
    /// Fail before the summary Ledger insert.
    BeforeSummaryInsert,
    /// Fail after the summary Ledger insert has staged.
    AfterSummaryInsert,
    /// Fail after the immutable Surface Replace has staged.
    AfterSurfaceRevision,
    /// Fail after the Surface/checkpoint head metadata has staged.
    AfterCheckpoint,
    /// Fail before the Event Journal insert.
    BeforeEventInsert,
    /// Fail after the Event Journal fact has staged.
    AfterEventInsert,
}

/// One scripted model-turn-start-stage fault used by atomicity regressions
/// (Issue #12, M9b).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestStartFaultOperation {
    /// Fail before the request-scoped context Ledger/Surface appends.
    BeforeContextAppend,
    /// Fail after the request-scoped context appends have staged.
    AfterContextAppend,
    /// Fail after the immutable Request Snapshot insert has staged.
    AfterSnapshotInsert,
    /// Fail after the `ModelRequestStarted` Event Journal fact has staged.
    AfterEventInsert,
}

/// The native durable conversation authority for one conversation.
pub struct SqliteConversationStore {
    conversation_id: ConversationId,
    conn: Arc<Mutex<Connection>>,
    #[cfg(test)]
    pub(crate) fail_accept_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_adopt_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fail_select_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_compaction_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    #[cfg(test)]
    pub(crate) fail_event_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_terminal_event_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) terminal_event_attempts: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) request_snapshot_page_reads: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) admission_fault_script: Arc<Mutex<VecDeque<AdmissionFaultOperation>>>,
    #[cfg(test)]
    pub(crate) compaction_fault_script: Arc<Mutex<VecDeque<CompactionFaultOperation>>>,
    #[cfg(test)]
    pub(crate) request_start_fault_script: Arc<Mutex<VecDeque<RequestStartFaultOperation>>>,
}

impl std::fmt::Debug for SqliteConversationStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteConversationStore")
            .field("conversation_id", &self.conversation_id)
            .finish_non_exhaustive()
    }
}

impl SqliteConversationStore {
    /// Opens a durable store at `path`, creating its development schema when
    /// the file is new.
    ///
    /// # Errors
    ///
    /// Returns an error if `SQLite` cannot be opened/configured or if the file
    /// contains an incompatible development schema.
    pub fn open(
        conversation_id: ConversationId,
        path: &Path,
    ) -> Result<Self, ConversationStoreError> {
        let mut connection = Connection::open(path)
            .map_err(|error| storage(format!("open {}: {error}", path.display())))?;
        configure_connection(&mut connection, false)?;
        Self::from_connection(conversation_id, connection)
    }

    /// Creates an in-memory store for tests and headless ephemeral runs.
    ///
    /// # Errors
    ///
    /// Returns an error if the in-memory `SQLite` connection cannot be
    /// configured or initialized.
    pub fn in_memory(conversation_id: ConversationId) -> Result<Self, ConversationStoreError> {
        let mut connection =
            Connection::open_in_memory().map_err(|error| storage(format!("in-memory: {error}")))?;
        configure_connection(&mut connection, true)?;
        Self::from_connection(conversation_id, connection)
    }

    fn from_connection(
        conversation_id: ConversationId,
        mut connection: Connection,
    ) -> Result<Self, ConversationStoreError> {
        reject_legacy_schema(&connection)?;
        if has_table(&connection, "rustx_store")? {
            validate_existing_schema(&connection)?;
        } else {
            create_schema(&connection)?;
        }
        bind_identity(&mut connection, &conversation_id)?;
        Ok(Self {
            conversation_id,
            conn: Arc::new(Mutex::new(connection)),
            #[cfg(test)]
            fail_accept_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            fail_adopt_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            #[cfg(test)]
            fail_select_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            fail_compaction_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            #[cfg(test)]
            fail_event_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            fail_terminal_event_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            terminal_event_attempts: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            request_snapshot_page_reads: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            admission_fault_script: Arc::new(Mutex::new(VecDeque::new())),
            #[cfg(test)]
            compaction_fault_script: Arc::new(Mutex::new(VecDeque::new())),
            #[cfg(test)]
            request_start_fault_script: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, ConversationStoreError> {
        self.conn
            .lock()
            .map_err(|_| storage("the conversation store connection is poisoned"))
    }

    #[cfg(test)]
    pub(crate) fn arm_fail_next_accept_commit(&self) {
        self.arm_fail_accept_times(1);
    }

    #[cfg(test)]
    pub(crate) fn arm_fail_accept_times(&self, count: usize) {
        self.fail_accept_remaining
            .fetch_add(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn arm_fail_next_adopt_commit(&self) {
        self.arm_fail_adopt_times(1);
    }

    #[cfg(test)]
    pub(crate) fn arm_fail_adopt_times(&self, count: usize) {
        self.fail_adopt_remaining.fetch_add(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn arm_fail_select_times(&self, count: usize) {
        self.fail_select_remaining
            .fetch_add(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn arm_fail_compaction_times(&self, count: usize) {
        self.fail_compaction_remaining
            .fetch_add(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn arm_fail_event_times(&self, count: usize) {
        self.fail_event_remaining.fetch_add(count, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn arm_fail_next_terminal_event(&self) {
        self.fail_terminal_event_remaining
            .fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn terminal_event_attempts(&self) -> usize {
        self.terminal_event_attempts.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn request_snapshot_page_reads(&self) -> usize {
        self.request_snapshot_page_reads.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn arm_admission_fault_script(
        &self,
        operations: impl IntoIterator<Item = AdmissionFaultOperation>,
    ) {
        self.admission_fault_script
            .lock()
            .expect("admission fault script lock")
            .extend(operations);
    }

    #[cfg(test)]
    pub(crate) fn arm_compaction_fault_script(
        &self,
        operations: impl IntoIterator<Item = CompactionFaultOperation>,
    ) {
        self.compaction_fault_script
            .lock()
            .expect("compaction fault script lock")
            .extend(operations);
    }

    #[cfg(test)]
    fn consume_admission_fault(&self, operation: AdmissionFaultOperation) -> bool {
        let mut script = self
            .admission_fault_script
            .lock()
            .expect("admission fault script lock");
        if script.front().copied() == Some(operation) {
            script.pop_front();
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn consume_compaction_fault(&self, operation: CompactionFaultOperation) -> bool {
        let mut script = self
            .compaction_fault_script
            .lock()
            .expect("compaction fault script lock");
        if script.front().copied() == Some(operation) {
            script.pop_front();
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub(crate) fn arm_request_start_fault_script(
        &self,
        operations: impl IntoIterator<Item = RequestStartFaultOperation>,
    ) {
        self.request_start_fault_script
            .lock()
            .expect("request-start fault script lock")
            .extend(operations);
    }

    /// Inserts the fresh model-turn start facts into `transaction`: the
    /// request-scoped canonical context, the frozen snapshot, the
    /// `ModelRequestStarted` event, and the sequence binding (Issue #12,
    /// M9b).
    fn insert_fresh_start_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        context: &[MessageBlock],
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::BeforeContextAppend) {
            return Err(storage(
                "fault injected: before request-start context append",
            ));
        }
        if !context.is_empty() {
            ensure_surface_head(transaction)?;
        }
        for message in context {
            append_message_and_surface(transaction, message)?;
        }
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::AfterContextAppend) {
            return Err(storage(
                "fault injected: after request-start context append",
            ));
        }
        validate_surface_revision(transaction, snapshot.surface_revision)?;
        let ids = reconstruct_surface_tx(transaction, snapshot.surface_revision)?;
        for id in ids {
            let _: MessageBlock = load_message_tx(transaction, &id)?;
        }
        let json = encode(snapshot, "request snapshot")?;
        transaction
            .execute(
                "INSERT INTO request_snapshots(request_id,surface_revision,snapshot_json,started_sequence) VALUES(?1,?2,?3,NULL)",
                params![
                    snapshot.request_id.as_str(),
                    seq_to_i64(snapshot.surface_revision.get())?,
                    json
                ],
            )
            .map_err(|error| storage(format!("insert request snapshot: {error}")))?;
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::AfterSnapshotInsert) {
            return Err(storage(
                "fault injected: after request-start snapshot insert",
            ));
        }
        let event = RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: self.conversation_id.clone(),
            attempt_id: Some(snapshot.identity.attempt_id.clone()),
            turn_id: Some(snapshot.identity.turn.clone()),
            timestamp,
            event: RuntimeEvent::ModelRequestStarted {
                request_id: snapshot.request_id.clone(),
                model: snapshot.invocation.model.clone(),
            },
        };
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage(
                "fault injected: request-start event journal commit",
            ));
        }
        let persisted = persist_event_tx(transaction, &self.conversation_id, event)?;
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::AfterEventInsert) {
            return Err(storage("fault injected: after request-start event insert"));
        }
        transaction
            .execute(
                "UPDATE request_snapshots SET started_sequence=?1 WHERE request_id=?2",
                params![
                    seq_to_i64(persisted.sequence)?,
                    snapshot.request_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("bind request start sequence: {error}")))?;
        Ok(persisted)
    }

    #[cfg(test)]
    fn consume_request_start_fault(&self, operation: RequestStartFaultOperation) -> bool {
        let mut script = self
            .request_start_fault_script
            .lock()
            .expect("request-start fault script lock");
        if script.front().copied() == Some(operation) {
            script.pop_front();
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn consume(counter: &AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }

    /// Test-only sequence exhaustion hook retained for deterministic boundary
    /// tests; it changes no process-local allocator because the value is in
    /// the durable store.
    #[cfg(test)]
    pub(crate) fn force_next_sequence_for_test(&self, value: i64) {
        self.conn
            .lock()
            .expect("connection")
            .execute(
                "UPDATE rustx_store SET next_inbound_sequence = ?1 WHERE id = 1",
                [value],
            )
            .expect("force sequence");
    }
}

impl ConversationStore for SqliteConversationStore {
    fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    #[allow(clippy::too_many_lines)]
    fn accept_inbound(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("accept transaction: {error}")))?;
        let accepted = accept_inbound_tx(self, &transaction, draft)?;
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: accept commit"));
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("accept commit: {error}")))?;
        Ok(accepted)
    }

    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        mut event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("accept/event transaction: {error}")))?;
        let accepted = accept_inbound_tx(self, &transaction, draft)?;
        if event.event_id.as_str().is_empty() {
            match &event.event {
                RuntimeEvent::BackgroundTerminalPublished { execution_id, .. } => {
                    event.event_id =
                        EventId::new(format!("background-terminal-event:{execution_id}"));
                }
                RuntimeEvent::SubagentTerminalPublished { subagent_id, .. } => {
                    event.event_id = EventId::new(format!("subagent-terminal-event:{subagent_id}"));
                }
                _ => {}
            }
        }
        let event_message_id = match &event.event {
            RuntimeEvent::BackgroundTerminalPublished { message_id, .. }
            | RuntimeEvent::SubagentTerminalPublished { message_id, .. } => message_id.clone(),
            _ => {
                return Err(ConversationStoreError::InvalidReference(
                    "inbound/event acceptance requires a detached terminal fact".to_owned(),
                ));
            }
        };
        if event_message_id != accepted.message_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "detached terminal fact references {}, accepted inbound is {}",
                event_message_id, accepted.message_id
            )));
        }
        if accepted.retried {
            let Some(existing) = find_event_by_id(&transaction, &event.event_id)? else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "correlated inbound {} has no durable terminal fact",
                    accepted.message_id
                )));
            };
            if existing.event != event.event {
                return Err(ConversationStoreError::InvalidReference(
                    "correlated inbound terminal fact conflicts with the stored fact".to_owned(),
                ));
            }
            transaction
                .commit()
                .map_err(|error| storage(format!("accept/event retry commit: {error}")))?;
            return Ok((accepted, existing));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: accept/event commit"));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: accept/event journal commit"));
        }
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("accept/event commit: {error}")))?;
        Ok((accepted, persisted))
    }

    fn select_pending_batch(&self) -> Result<Option<PendingBatch>, ConversationStoreError> {
        #[cfg(test)]
        {
            if self.consume_admission_fault(AdmissionFaultOperation::SelectPendingBatch)
                || Self::consume(&self.fail_select_remaining)
            {
                return Err(storage("fault injected: select pending batch"));
            }
        }
        let connection = self.lock()?;
        let items = load_pending_rows(&connection)?;
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
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("adopt transaction: {error}")))?;
        ensure_surface_head(&transaction)?;
        let items = load_pending_until(&transaction, watermark)?;
        let mut adopted = Vec::with_capacity(items.len());
        for item in &items {
            let block = MessageBlock::User(item.message.clone());
            append_adopted_message_and_surface(&transaction, &block, &item.message_id)?;
            adopted.push(block);
        }
        transaction
            .execute(
                "DELETE FROM pending_inbound WHERE sequence <= ?1",
                [seq_to_i64(watermark.get())?],
            )
            .map_err(|error| storage(format!("adopt pending delete: {error}")))?;
        #[cfg(test)]
        if self.consume_admission_fault(AdmissionFaultOperation::AdoptPendingBatch)
            || Self::consume(&self.fail_adopt_remaining)
        {
            return Err(storage("fault injected: adopt commit"));
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("adopt commit: {error}")))?;
        Ok(adopted)
    }

    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
        let connection = self.lock()?;
        load_pending_rows(&connection)
    }

    fn initialize(&self, messages: &[MessageBlock]) -> Result<(), ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("initialize transaction: {error}")))?;
        let bootstrap: Option<(i64, String)> = transaction
            .query_row(
                "SELECT message_count,history_digest FROM bootstrap_identity WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage(format!("bootstrap probe: {error}")))?;
        if let Some((count, digest)) = bootstrap {
            let supplied_count = i64::try_from(messages.len())
                .map_err(|_| storage("initial message count is not representable"))?;
            if count != supplied_count || digest != initial_history_digest(messages)? {
                return Err(ConversationStoreError::InitialHistoryMismatch);
            }
        } else {
            let existing: i64 = transaction
                .query_row("SELECT COUNT(*) FROM message_ledger", [], |row| row.get(0))
                .map_err(|error| storage(format!("bootstrap ledger probe: {error}")))?;
            if existing != 0 {
                return Err(storage(
                    "canonical Ledger exists without bootstrap identity",
                ));
            }
            ensure_surface_head(&transaction)?;
            for message in messages {
                append_message_and_surface(&transaction, message)?;
            }
            transaction
                .execute(
                    "INSERT INTO bootstrap_identity(id,message_count,history_digest) VALUES(1,?1,?2)",
                    params![
                        i64::try_from(messages.len()).map_err(|_| storage("initial message count is not representable"))?,
                        initial_history_digest(messages)?
                    ],
                )
                .map_err(|error| storage(format!("insert bootstrap identity: {error}")))?;
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("initialize commit: {error}")))
    }

    fn load_head(&self) -> Result<DurableConversationHead, ConversationStoreError> {
        let connection = self.lock()?;
        load_head(&connection)
    }

    fn load_messages(
        &self,
        ids: &[MessageId],
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let connection = self.lock()?;
        ids.iter().map(|id| load_message(&connection, id)).collect()
    }

    fn reconstruct_surface(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageId>, ConversationStoreError> {
        let connection = self.lock()?;
        reconstruct_surface(&connection, revision)
    }

    fn append_canonical(&self, message: &MessageBlock) -> Result<(), ConversationStoreError> {
        append_canonical_messages(self, std::slice::from_ref(message))
    }

    fn append_canonical_batch(
        &self,
        messages: &[MessageBlock],
    ) -> Result<(), ConversationStoreError> {
        append_canonical_messages(self, messages)
    }

    fn append_canonical_with_event(
        &self,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        validate_canonical_event_for_message(message, &event.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical event transaction: {error}")))?;
        ensure_surface_head(&transaction)?;
        append_message_and_surface(&transaction, message)?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical event journal commit"));
        }
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical event commit: {error}")))?;
        Ok(persisted)
    }

    fn append_canonical_batch_with_events(
        &self,
        messages: &[MessageBlock],
        events: &[RuntimeEventEnvelope],
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
        if messages.len() != events.len() {
            return Err(storage("canonical event batch lengths differ"));
        }
        for (message, event) in messages.iter().zip(events) {
            validate_canonical_event_for_message(message, &event.event)?;
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical event batch transaction: {error}")))?;
        ensure_surface_head(&transaction)?;
        for message in messages {
            append_message_and_surface(&transaction, message)?;
        }
        let mut persisted = Vec::with_capacity(events.len());
        for event in events {
            #[cfg(test)]
            if Self::consume(&self.fail_event_remaining) {
                return Err(storage(
                    "fault injected: canonical batch event journal commit",
                ));
            }
            persisted.push(persist_event_tx(
                &transaction,
                &self.conversation_id,
                event.clone(),
            )?);
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical event batch commit: {error}")))?;
        Ok(persisted)
    }

    fn commit_compaction(
        &self,
        input: CompactionCommitInput,
    ) -> Result<(SurfaceRevision, u64, RuntimeEventEnvelope), ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("compaction transaction: {error}")))?;
        let head = load_head(&transaction)?;
        if head.revision != input.expected_revision {
            return Err(ConversationStoreError::InvalidReference(format!(
                "compaction expected Surface {}, current is {}",
                input.expected_revision, head.revision
            )));
        }
        if input.summary.source != UserSource::Runtime
            || input.summary.kind != InboundKind::CompactionSummary
        {
            return Err(ConversationStoreError::InvalidReference(
                "compaction requires a User(Runtime / CompactionSummary) Ledger message".to_owned(),
            ));
        }
        let (start, end) = span_indices(&head.active_message_ids, &input.span)?;
        if head.active_message_ids.contains(&input.summary.id) {
            return Err(ConversationStoreError::DuplicateMessageId(input.summary.id));
        }
        #[cfg(test)]
        if self.consume_compaction_fault(CompactionFaultOperation::BeforeSummaryInsert) {
            return Err(storage("fault injected: before compaction summary insert"));
        }
        append_message_ledger(
            &transaction,
            &MessageBlock::User(input.summary.clone()),
            None,
        )?;
        #[cfg(test)]
        if self.consume_compaction_fault(CompactionFaultOperation::AfterSummaryInsert) {
            return Err(storage("fault injected: after compaction summary insert"));
        }
        let revision = input.expected_revision.next();
        let generation = head
            .compaction_generation
            .checked_add(1)
            .ok_or_else(|| storage("compaction generation exhausted"))?;
        let op = SurfaceOp::Replace {
            start: input.span.start.clone(),
            end: input.span.end.clone(),
            replacement: input.summary.id.clone(),
        };
        append_surface_op(&transaction, revision, generation, &op)?;
        #[cfg(test)]
        if self.consume_compaction_fault(CompactionFaultOperation::AfterSurfaceRevision) {
            return Err(storage("fault injected: after compaction Surface revision"));
        }
        let active = replacement_active(&head.active_message_ids, start, end, &input.summary.id);
        update_surface_head(&transaction, revision, generation, &active)?;
        update_checkpoint(&transaction, revision, generation, &active)?;
        #[cfg(test)]
        if self.consume_compaction_fault(CompactionFaultOperation::AfterCheckpoint) {
            return Err(storage("fault injected: after compaction checkpoint"));
        }
        let event = RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: self.conversation_id.clone(),
            attempt_id: input.attempt_id,
            turn_id: input.turn_id,
            timestamp: input.timestamp,
            event: RuntimeEvent::CompactionCompleted {
                generation,
                summary_message_id: input.summary.id.clone(),
                surface_revision: revision,
                tokens_before: input.tokens_before,
                estimated_tokens_after: input.estimated_tokens_after,
            },
        };
        #[cfg(test)]
        if self.consume_compaction_fault(CompactionFaultOperation::BeforeEventInsert) {
            return Err(storage("fault injected: before compaction event insert"));
        }
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        #[cfg(test)]
        if self.consume_compaction_fault(CompactionFaultOperation::AfterEventInsert) {
            return Err(storage("fault injected: after compaction event insert"));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_compaction_remaining) {
            return Err(storage("fault injected: compaction commit"));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: compaction event journal commit"));
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("compaction commit: {error}")))?;
        Ok((revision, generation, persisted))
    }

    fn load_canonical(&self) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let connection = self.lock()?;
        load_canonical_rows(&connection)
    }

    fn load_canonical_page(
        &self,
        after_position: Option<u64>,
        limit: usize,
    ) -> Result<CanonicalMessagePage, ConversationStoreError> {
        let connection = self.lock()?;
        let after = after_position.unwrap_or(0);
        let limit = i64::try_from(limit).map_err(|_| storage("page limit is too large"))?;
        let mut statement = connection
            .prepare("SELECT position,message_json,message_id FROM message_ledger WHERE position > ?1 ORDER BY position LIMIT ?2")
            .map_err(|error| storage(format!("canonical page: {error}")))?;
        let mut rows = statement
            .query(params![after, limit])
            .map_err(|error| storage(format!("canonical page query: {error}")))?;
        let mut messages = Vec::new();
        let mut next = None;
        while let Some(row) = rows
            .next()
            .map_err(|error| storage(format!("canonical page row: {error}")))?
        {
            let position: i64 = row
                .get(0)
                .map_err(|error| storage(format!("canonical position: {error}")))?;
            let json: String = row
                .get(1)
                .map_err(|error| storage(format!("canonical json: {error}")))?;
            let message: MessageBlock = decode(&json, "canonical page")?;
            let position_id = crate::conversation::message_id_of(&message);
            let stored_id: String = row
                .get(2)
                .map_err(|error| storage(format!("canonical message id: {error}")))?;
            if position_id.as_str() != stored_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Ledger page row {stored_id} contains message {position_id}"
                )));
            }
            messages.push(message);
            next =
                Some(u64::try_from(position).map_err(|_| storage("negative canonical position"))?);
        }
        Ok(CanonicalMessagePage {
            messages,
            next_position: next,
        })
    }

    fn commit_model_turn_start(
        &self,
        context: &[MessageBlock],
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        validate_snapshot_identity(snapshot)?;
        // The one input validation rule shared by the fresh commit and the
        // idempotent retry (Issue #12, M9b): the exact ordered
        // request-scoped context the caller supplies must equal the frozen
        // `snapshot.request_context_ids` before the store chooses a path or
        // touches durable state. A fresh commit must never be able to append
        // context while persisting a snapshot whose `request_context_ids`
        // disagrees with what it just appended.
        validate_request_context(snapshot, context)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("request start transaction: {error}")))?;
        let existing: Option<(String, Option<i64>)> = transaction
            .query_row(
                "SELECT snapshot_json,started_sequence FROM request_snapshots WHERE request_id = ?1",
                [snapshot.request_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage(format!("request snapshot probe: {error}")))?;
        if let Some((json, started_sequence)) = existing {
            let started = verify_committed_start_tx(
                &transaction,
                &json,
                started_sequence,
                context,
                snapshot,
            )?;
            transaction
                .commit()
                .map_err(|error| storage(format!("request start retry commit: {error}")))?;
            return Ok(started);
        }
        // The request-scoped canonical context commits first, inside the
        // same transaction as the snapshot and the start fact: a failure
        // anywhere below rolls all of it back, so request-scoped context
        // can never become canonical without its request starting.
        let persisted = self.insert_fresh_start_tx(&transaction, context, snapshot, timestamp)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("request start commit: {error}")))?;
        Ok(persisted)
    }

    fn load_request_snapshot(
        &self,
        request_id: &RequestId,
    ) -> Result<RequestSnapshot, ConversationStoreError> {
        let connection = self.lock()?;
        let (stored_surface_revision, json, started_sequence): (i64, String, Option<i64>) = connection
            .query_row(
                "SELECT surface_revision,snapshot_json,started_sequence FROM request_snapshots WHERE request_id = ?1",
                [request_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| storage(format!("request snapshot lookup: {error}")))?
            .ok_or_else(|| {
                ConversationStoreError::RequestNotFound(request_id.clone())
            })?;
        let started_sequence = started_sequence.ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "request snapshot {request_id} has no durable start sequence"
            ))
        })?;
        let sequence = sequence_from_i64(started_sequence)?;
        let event_json: String = connection
            .query_row(
                "SELECT event_json FROM events WHERE sequence=?1",
                [started_sequence],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("request start event lookup: {error}")))?
            .ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "request snapshot {request_id} start event is unavailable"
                ))
            })?;
        let snapshot: RequestSnapshot = decode(&json, "request snapshot")?;
        validate_snapshot_identity(&snapshot)?;
        if stored_surface_revision != seq_to_i64(snapshot.surface_revision.get())? {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request snapshot {request_id} Surface column disagrees with its frozen snapshot"
            )));
        }
        if snapshot.request_id != *request_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request snapshot row {request_id} contains a different RequestId"
            )));
        }
        let event: RuntimeEventEnvelope = decode(&event_json, "request start event")?;
        if event.sequence != sequence {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request snapshot {request_id} start event sequence disagrees"
            )));
        }
        validate_request_start_metadata(&snapshot, &event)?;
        Ok(snapshot)
    }

    fn reconstruct_model_request(
        &self,
        request_id: &RequestId,
    ) -> Result<ModelRequest, ConversationStoreError> {
        let snapshot = self.load_request_snapshot(request_id)?;
        let ids = self.reconstruct_surface(snapshot.surface_revision)?;
        let messages = self.load_messages(&ids)?;
        Ok(ModelRequest {
            invocation: snapshot.invocation.clone(),
            messages,
            tools: snapshot.tool_definitions.clone(),
            effective_system_prompt: snapshot.effective_system_prompt.clone(),
            continuation: snapshot.continuation.clone(),
        })
    }

    fn read_request_snapshots(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<RequestSnapshotPage, ConversationStoreError> {
        #[cfg(test)]
        self.request_snapshot_page_reads
            .fetch_add(1, Ordering::SeqCst);
        let connection = self.lock()?;
        let after = seq_to_i64(after_sequence.unwrap_or(0))?;
        let limit = i64::try_from(limit)
            .map_err(|_| storage("request snapshot page limit is too large"))?;
        let mut statement = connection
            .prepare(
                "SELECT request_id,started_sequence FROM request_snapshots
                 WHERE started_sequence IS NOT NULL AND started_sequence > ?1
                 ORDER BY started_sequence, request_id LIMIT ?2",
            )
            .map_err(|error| storage(format!("request snapshot page: {error}")))?;
        let rows = statement
            .query_map(params![after, limit], |row| {
                let request_id: String = row.get(0)?;
                let sequence: i64 = row.get(1)?;
                Ok((request_id, sequence))
            })
            .map_err(|error| storage(format!("request snapshot page query: {error}")))?;
        let rows: Vec<(RequestId, u64)> = rows
            .map(|row| {
                let (request_id, sequence) =
                    row.map_err(|error| storage(format!("request snapshot page row: {error}")))?;
                Ok((RequestId::new(request_id), sequence_from_i64(sequence)?))
            })
            .collect::<Result<_, ConversationStoreError>>()?;
        drop(statement);
        drop(connection);

        let next_sequence = rows.last().map(|(_, sequence)| *sequence);
        let snapshots = rows
            .into_iter()
            .map(|(request_id, _)| self.load_request_snapshot(&request_id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RequestSnapshotPage {
            snapshots,
            next_sequence,
        })
    }

    fn append_event(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        if requires_compound_transaction(&event.event) {
            return Err(ConversationStoreError::InvalidReference(
                "this event kind must use its canonical fact transaction".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("event transaction: {error}")))?;
        #[cfg(test)]
        if matches!(
            &event.event,
            RuntimeEvent::AttemptCompleted { .. }
                | RuntimeEvent::AttemptCancelled { .. }
                | RuntimeEvent::AttemptTimedOut { .. }
                | RuntimeEvent::AttemptLimitExceeded { .. }
                | RuntimeEvent::AttemptFailed { .. }
        ) {
            self.terminal_event_attempts.fetch_add(1, Ordering::SeqCst);
            if Self::consume(&self.fail_terminal_event_remaining) {
                return Err(storage("fault injected: terminal event commit"));
            }
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: event commit"));
        }
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("event commit: {error}")))?;
        Ok(persisted)
    }

    fn read_events(
        &self,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<EventPage, ConversationStoreError> {
        let connection = self.lock()?;
        let after = after_sequence.unwrap_or(0);
        let limit = i64::try_from(limit).map_err(|_| storage("event page limit is too large"))?;
        let mut statement = connection
            .prepare(
                "SELECT sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json
                 FROM events WHERE sequence > ?1 ORDER BY sequence LIMIT ?2",
            )
            .map_err(|error| storage(format!("event page: {error}")))?;
        let mut rows = statement
            .query(params![after, limit])
            .map_err(|error| storage(format!("event page query: {error}")))?;
        let mut events = Vec::new();
        let mut next = None;
        while let Some(row) = rows
            .next()
            .map_err(|error| storage(format!("event page row: {error}")))?
        {
            let sequence: i64 = row
                .get(0)
                .map_err(|error| storage(format!("event sequence: {error}")))?;
            let stored_event_id: String = row
                .get(1)
                .map_err(|error| storage(format!("event identity: {error}")))?;
            let stored_schema_version: i64 = row
                .get(2)
                .map_err(|error| storage(format!("event schema version: {error}")))?;
            let stored_conversation_id: String = row
                .get(3)
                .map_err(|error| storage(format!("event conversation identity: {error}")))?;
            let stored_attempt_id: Option<String> = row
                .get(4)
                .map_err(|error| storage(format!("event attempt identity: {error}")))?;
            let stored_turn_id: Option<String> = row
                .get(5)
                .map_err(|error| storage(format!("event turn identity: {error}")))?;
            let json: String = row
                .get(6)
                .map_err(|error| storage(format!("event json: {error}")))?;
            let event: RuntimeEventEnvelope = decode(&json, "event page")?;
            let sequence =
                u64::try_from(sequence).map_err(|_| storage("negative event sequence"))?;
            let stored_schema_version = u16::try_from(stored_schema_version).map_err(|_| {
                ConversationStoreError::InvalidReference(
                    "Event Journal row has an invalid schema version".to_owned(),
                )
            })?;
            if event.sequence != sequence
                || event.event_id.as_str() != stored_event_id
                || event.schema_version != stored_schema_version
                || event.schema_version != EVENT_SCHEMA_VERSION
                || event.conversation_id.as_str() != stored_conversation_id
                || event.conversation_id != self.conversation_id
                || event.attempt_id.as_ref().map(ToString::to_string) != stored_attempt_id
                || event.turn_id.as_ref().map(ToString::to_string) != stored_turn_id
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Event Journal row metadata disagrees with its envelope".to_owned(),
                ));
            }
            events.push(event);
            next = Some(sequence);
        }
        Ok(EventPage {
            events,
            next_sequence: next,
        })
    }
}

fn accept_inbound_tx(
    store: &SqliteConversationStore,
    transaction: &Transaction<'_>,
    draft: InboundDraft,
) -> Result<AcceptedInbound, ConversationStoreError> {
    if draft.content.is_empty() {
        return Err(ConversationStoreError::EmptyContent);
    }
    if draft.kind == InboundKind::CompactionSummary {
        return Err(ConversationStoreError::InvalidReference(
            "compaction summaries must use the atomic commit_compaction transition".to_owned(),
        ));
    }
    if let Some(correlation) = draft.correlation.as_deref() {
        let existing: Option<(i64, String)> = transaction
            .query_row(
                "SELECT sequence, message_id FROM inbound_correlation WHERE correlation = ?1",
                [correlation],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage(format!("correlation probe: {error}")))?;
        if let Some((sequence, message_id)) = existing {
            let message =
                load_correlated_user_message(transaction, &MessageId::new(message_id.clone()))?;
            let same = draft
                .message_id
                .as_ref()
                .is_none_or(|id| id.as_str() == message_id)
                && draft.source == message.source
                && draft.kind == message.kind
                && draft.content == message.content
                && message.timestamp == Some(draft.timestamp);
            if !same {
                return Err(ConversationStoreError::CorrelationConflict {
                    correlation: correlation.to_owned(),
                });
            }
            return Ok(AcceptedInbound {
                sequence: InboundSequence::new(sequence_from_i64(sequence)?),
                message_id: MessageId::new(message_id),
                message,
                retried: true,
            });
        }
    }
    let current: i64 = transaction
        .query_row(
            "SELECT next_inbound_sequence FROM rustx_store WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read inbound sequence: {error}")))?;
    let next = current
        .checked_add(1)
        .ok_or(ConversationStoreError::SequenceExhausted)?;
    let sequence = u64::try_from(next).map_err(|_| ConversationStoreError::SequenceExhausted)?;
    let message_id = draft
        .message_id
        .clone()
        .unwrap_or_else(|| MessageId::new(format!("{}-inbound-{sequence}", store.conversation_id)));
    if message_exists(transaction, &message_id)? {
        return Err(ConversationStoreError::DuplicateMessageId(message_id));
    }
    let message = UserMessageBlock {
        id: message_id.clone(),
        content: draft.content,
        source: draft.source,
        kind: draft.kind,
        timestamp: Some(draft.timestamp),
    };
    let json = encode(&message, "inbound")?;
    transaction
        .execute(
            "INSERT INTO pending_inbound(sequence,message_id,message_json,correlation) VALUES(?1,?2,?3,?4)",
            params![next, message_id.as_str(), json, draft.correlation.as_deref()],
        )
        .map_err(|error| map_insert_error(&error, &message_id))?;
    transaction
        .execute(
            "UPDATE rustx_store SET next_inbound_sequence = ?1 WHERE id = 1",
            [next],
        )
        .map_err(|error| storage(format!("update inbound sequence: {error}")))?;
    if let Some(correlation) = draft.correlation.as_deref() {
        transaction
            .execute(
                "INSERT INTO inbound_correlation(correlation,sequence,message_id) VALUES(?1,?2,?3)",
                params![correlation, next, message_id.as_str()],
            )
            .map_err(|error| storage(format!("insert correlation: {error}")))?;
    }
    Ok(AcceptedInbound {
        sequence: InboundSequence::new(sequence),
        message_id,
        message,
        retried: false,
    })
}

fn append_canonical_messages(
    store: &SqliteConversationStore,
    messages: &[MessageBlock],
) -> Result<(), ConversationStoreError> {
    if messages.is_empty() {
        return Ok(());
    }
    let mut connection = store.lock()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage(format!("canonical transaction: {error}")))?;
    ensure_surface_head(&transaction)?;
    for message in messages {
        append_message_and_surface(&transaction, message)?;
    }
    transaction
        .commit()
        .map_err(|error| storage(format!("canonical commit: {error}")))
}

/// Verifies a retried model-turn start against the already-committed durable
/// facts (Issue #12, M9b): the frozen snapshot, the exact ordered
/// request-scoped context committed atomically with it, the request-start
/// event, and the sequence binding must all match exactly.
fn verify_committed_start_tx(
    transaction: &rusqlite::Transaction<'_>,
    json: &str,
    started_sequence: Option<i64>,
    context: &[MessageBlock],
    snapshot: &RequestSnapshot,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    let Some(started_sequence) = started_sequence else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} exists without a durable start sequence",
            snapshot.request_id
        )));
    };
    let stored: RequestSnapshot = decode(json, "request snapshot")?;
    if stored != *snapshot {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {} was started with different frozen inputs",
            snapshot.request_id
        )));
    }
    // The durable request-start authority binds the exact ordered
    // request-scoped context of that request: the retried start must carry
    // the complete ordered context the original start committed atomically
    // with it. The frozen `MessageId`s prove exact equality — an empty
    // retry, a prefix, a reorder, or an extra message all fail here — and
    // the per-message body check below rejects same-ids/different-body.
    let supplied_ids: Vec<MessageId> = context
        .iter()
        .map(crate::conversation::message_id_of)
        .collect();
    if supplied_ids != stored.request_context_ids {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {} was retried with a different request-scoped context",
            snapshot.request_id
        )));
    }
    for message in context {
        let id = crate::conversation::message_id_of(message);
        let stored_message = load_message_tx(transaction, &id)?;
        if stored_message != *message {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request {} context message {id} differs from the committed fact",
                snapshot.request_id
            )));
        }
    }
    let Some(started) = find_request_start_event(transaction, &snapshot.request_id)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} exists without its request-start fact",
            snapshot.request_id
        )));
    };
    if seq_to_i64(started.sequence)? != started_sequence {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} start sequence disagrees with its event",
            snapshot.request_id
        )));
    }
    validate_request_start_metadata(&stored, &started)?;
    Ok(started)
}

fn configure_connection(
    connection: &mut Connection,
    in_memory: bool,
) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000;",
        )
        .map_err(|error| storage(format!("configure SQLite: {error}")))?;
    if !in_memory {
        connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|error| storage(format!("configure SQLite journal: {error}")))?;
    }
    Ok(())
}

fn reject_legacy_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    let legacy: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='inbox_meta'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("probe schema: {error}")))?;
    if legacy.is_some() {
        return Err(ConversationStoreError::SchemaVersionMismatch {
            stored: 0,
            expected: SQLITE_SCHEMA_VERSION,
        });
    }
    if !has_table(connection, "rustx_store")? {
        let unrelated: Option<String> = connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("probe partial schema: {error}")))?;
        if let Some(table) = unrelated {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "database contains table {table} but no rustx_store schema root"
            )));
        }
    }
    Ok(())
}

fn has_table(connection: &Connection, table: &str) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("probe table {table}: {error}")))
}

fn validate_existing_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    let version: i64 = connection
        .query_row(
            "SELECT schema_version FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read existing schema version: {error}")))?;
    if version != SQLITE_SCHEMA_VERSION {
        return Err(ConversationStoreError::SchemaVersionMismatch {
            stored: version,
            expected: SQLITE_SCHEMA_VERSION,
        });
    }
    verify_schema_shape(connection)
}

fn create_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS rustx_store (
                id INTEGER PRIMARY KEY CHECK(id=1),
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL,
                next_inbound_sequence INTEGER NOT NULL CHECK(next_inbound_sequence >= 0),
                next_event_sequence INTEGER NOT NULL CHECK(next_event_sequence >= 0)
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
                message_id TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS message_ledger (
                position INTEGER PRIMARY KEY,
                message_id TEXT NOT NULL UNIQUE,
                message_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS bootstrap_identity (
                id INTEGER PRIMARY KEY CHECK(id=1),
                message_count INTEGER NOT NULL,
                history_digest TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS surface_ops (
                revision INTEGER PRIMARY KEY,
                compaction_generation INTEGER NOT NULL,
                op_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS surface_head (
                id INTEGER PRIMARY KEY CHECK(id=1),
                revision INTEGER NOT NULL,
                compaction_generation INTEGER NOT NULL,
                active_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS context_checkpoints (
                id INTEGER PRIMARY KEY CHECK(id=1),
                revision INTEGER NOT NULL,
                compaction_generation INTEGER NOT NULL,
                active_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS request_snapshots (
                request_id TEXT PRIMARY KEY,
                surface_revision INTEGER NOT NULL,
                snapshot_json TEXT NOT NULL,
                started_sequence INTEGER
            );
            CREATE TABLE IF NOT EXISTS events (
                sequence INTEGER PRIMARY KEY,
                event_id TEXT NOT NULL UNIQUE,
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL,
                attempt_id TEXT,
                turn_id TEXT,
                event_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS lifecycle_state (
                lifecycle_key TEXT PRIMARY KEY,
                terminal_event_id TEXT
            );
            CREATE INDEX IF NOT EXISTS pending_inbound_sequence_idx ON pending_inbound(sequence);
            CREATE INDEX IF NOT EXISTS message_ledger_id_idx ON message_ledger(message_id);
            CREATE INDEX IF NOT EXISTS surface_ops_revision_idx ON surface_ops(revision);
            CREATE INDEX IF NOT EXISTS events_sequence_idx ON events(sequence);
            CREATE INDEX IF NOT EXISTS events_attempt_idx ON events(attempt_id, sequence);
            CREATE INDEX IF NOT EXISTS request_snapshots_surface_idx ON request_snapshots(surface_revision);",
        )
        .map_err(|error| storage(format!("create schema: {error}")))?;
    connection
        .execute(
            "INSERT OR IGNORE INTO rustx_store(id,schema_version,conversation_id,next_inbound_sequence,next_event_sequence) VALUES(1,?1,'',0,0)",
            params![SQLITE_SCHEMA_VERSION],
        )
        .map_err(|error| storage(format!("create schema root: {error}")))?;
    let version: i64 = connection
        .query_row(
            "SELECT schema_version FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read schema version: {error}")))?;
    if version != SQLITE_SCHEMA_VERSION {
        return Err(ConversationStoreError::SchemaVersionMismatch {
            stored: version,
            expected: SQLITE_SCHEMA_VERSION,
        });
    }
    verify_schema_shape(connection)?;
    Ok(())
}

fn verify_schema_shape(connection: &Connection) -> Result<(), ConversationStoreError> {
    let required = [
        (
            "rustx_store",
            &[
                "schema_version",
                "conversation_id",
                "next_inbound_sequence",
                "next_event_sequence",
            ] as &[&str],
        ),
        (
            "pending_inbound",
            &["sequence", "message_id", "message_json", "correlation"],
        ),
        (
            "inbound_correlation",
            &["correlation", "sequence", "message_id"],
        ),
        (
            "message_ledger",
            &["position", "message_id", "message_json"],
        ),
        ("bootstrap_identity", &["message_count", "history_digest"]),
        (
            "surface_ops",
            &["revision", "compaction_generation", "op_json"],
        ),
        (
            "surface_head",
            &["revision", "compaction_generation", "active_json"],
        ),
        (
            "context_checkpoints",
            &["revision", "compaction_generation", "active_json"],
        ),
        (
            "request_snapshots",
            &[
                "request_id",
                "surface_revision",
                "snapshot_json",
                "started_sequence",
            ],
        ),
        (
            "events",
            &[
                "sequence",
                "event_id",
                "schema_version",
                "conversation_id",
                "attempt_id",
                "turn_id",
                "event_json",
            ],
        ),
        ("lifecycle_state", &["lifecycle_key", "terminal_event_id"]),
    ];
    for (table, required_columns) in required {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| storage(format!("inspect schema table {table}: {error}")))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| storage(format!("inspect schema columns {table}: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("read schema columns {table}: {error}")))?;
        for column in required_columns {
            if !columns.iter().any(|candidate| candidate == column) {
                return Err(ConversationStoreError::IncompatibleSchema(format!(
                    "table {table} is missing required column {column}"
                )));
            }
        }
    }
    for (table, column) in [
        ("pending_inbound", "message_id"),
        ("inbound_correlation", "message_id"),
        ("message_ledger", "message_id"),
        ("events", "event_id"),
        ("request_snapshots", "request_id"),
    ] {
        verify_unique_column(connection, table, column)?;
    }
    Ok(())
}

fn verify_unique_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<(), ConversationStoreError> {
    let mut indexes = connection
        .prepare(&format!("PRAGMA index_list({table})"))
        .map_err(|error| storage(format!("inspect schema indexes {table}: {error}")))?;
    let index_rows = indexes
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })
        .map_err(|error| storage(format!("inspect schema indexes {table}: {error}")))?;
    for index_row in index_rows {
        let (index, unique) =
            index_row.map_err(|error| storage(format!("read schema index {table}: {error}")))?;
        if unique == 0 {
            continue;
        }
        let quoted = index.replace('"', "\"\"");
        let mut info = connection
            .prepare(&format!("PRAGMA index_info(\"{quoted}\")"))
            .map_err(|error| storage(format!("inspect schema index {index}: {error}")))?;
        let columns = info
            .query_map([], |row| row.get::<_, String>(2))
            .map_err(|error| storage(format!("inspect schema index {index}: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage(format!("read schema index {index}: {error}")))?;
        if columns == [column.to_owned()] {
            return Ok(());
        }
    }
    Err(ConversationStoreError::IncompatibleSchema(format!(
        "table {table} is missing a unique constraint on {column}"
    )))
}

fn bind_identity(
    connection: &mut Connection,
    conversation_id: &ConversationId,
) -> Result<(), ConversationStoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage(format!("bind conversation transaction: {error}")))?;
    let stored: String = transaction
        .query_row(
            "SELECT conversation_id FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read conversation identity: {error}")))?;
    if stored.is_empty() {
        transaction
            .execute(
                "UPDATE rustx_store SET conversation_id=?1 WHERE id=1",
                [conversation_id.as_str()],
            )
            .map_err(|error| storage(format!("bind conversation identity: {error}")))?;
    } else if stored != conversation_id.as_str() {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: ConversationId::new(stored),
            requested: conversation_id.clone(),
        });
    }
    transaction
        .commit()
        .map_err(|error| storage(format!("bind conversation commit: {error}")))?;
    Ok(())
}

fn ensure_surface_head(transaction: &Transaction<'_>) -> Result<(), ConversationStoreError> {
    let head_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM surface_head WHERE id=1)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("probe Surface head: {error}")))?;
    let checkpoint_exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM context_checkpoints WHERE id=1)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("probe context checkpoint: {error}")))?;
    match (head_exists, checkpoint_exists) {
        (true, true) => return Ok(()),
        (true, false) | (false, true) => {
            return Err(ConversationStoreError::InvalidReference(
                "Surface head and context checkpoint are only partially present".to_owned(),
            ));
        }
        (false, false) => {}
    }
    let ledger_count: i64 = transaction
        .query_row("SELECT COUNT(*) FROM message_ledger", [], |row| row.get(0))
        .map_err(|error| storage(format!("probe Ledger before Surface bootstrap: {error}")))?;
    let operation_count: i64 = transaction
        .query_row("SELECT COUNT(*) FROM surface_ops", [], |row| row.get(0))
        .map_err(|error| storage(format!("probe Surface history before bootstrap: {error}")))?;
    if ledger_count != 0 || operation_count != 0 {
        return Err(ConversationStoreError::InvalidReference(
            "the durable Ledger or Surface history exists without its current head".to_owned(),
        ));
    }
    transaction
        .execute(
            "INSERT OR IGNORE INTO surface_head(id,revision,compaction_generation,active_json) VALUES(1,0,0,?1)",
            [encode(&Vec::<MessageId>::new(), "empty Surface")?],
        )
        .map_err(|error| storage(format!("ensure Surface head: {error}")))?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO context_checkpoints(id,revision,compaction_generation,active_json) VALUES(1,0,0,?1)",
            [encode(&Vec::<MessageId>::new(), "empty context checkpoint")?],
        )
        .map_err(|error| storage(format!("ensure context checkpoint: {error}")))?;
    Ok(())
}

fn read_surface_head(
    connection: &Connection,
) -> Result<Option<(i64, i64, String)>, ConversationStoreError> {
    connection
        .query_row(
            "SELECT revision,compaction_generation,active_json FROM surface_head WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage(format!("load Surface head: {error}")))
}

fn load_head(connection: &Connection) -> Result<DurableConversationHead, ConversationStoreError> {
    let Some((revision, generation, active_json)) = read_surface_head(connection)? else {
        let checkpoint_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM context_checkpoints WHERE id=1)",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("probe context checkpoint: {error}")))?;
        if checkpoint_exists {
            return Err(ConversationStoreError::InvalidReference(
                "context checkpoint exists without a Surface head".to_owned(),
            ));
        }
        let ledger_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM message_ledger", [], |row| row.get(0))
            .map_err(|error| storage(format!("probe Ledger without Surface head: {error}")))?;
        let operation_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM surface_ops", [], |row| row.get(0))
            .map_err(|error| storage(format!("probe Surface history without head: {error}")))?;
        if ledger_count != 0 || operation_count != 0 {
            return Err(ConversationStoreError::InvalidReference(
                "the durable Ledger or Surface history exists without its current head".to_owned(),
            ));
        }
        return Ok(DurableConversationHead {
            revision: SurfaceRevision::INITIAL,
            compaction_generation: 0,
            active_message_ids: Vec::new(),
        });
    };
    let head = DurableConversationHead {
        revision: SurfaceRevision::new(nonnegative(revision, "Surface revision")?),
        compaction_generation: nonnegative(generation, "compaction generation")?,
        active_message_ids: decode(&active_json, "Surface head")?,
    };
    let checkpoint: Option<(i64, i64, String)> = connection
        .query_row(
            "SELECT revision,compaction_generation,active_json FROM context_checkpoints WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage(format!("load context checkpoint: {error}")))?;
    let Some((checkpoint_revision, checkpoint_generation, checkpoint_json)) = checkpoint else {
        return Err(ConversationStoreError::InvalidReference(
            "Surface head has no context checkpoint".to_owned(),
        ));
    };
    if checkpoint_revision != revision
        || checkpoint_generation != generation
        || checkpoint_json != active_json
    {
        return Err(ConversationStoreError::InvalidReference(
            "Surface head and context checkpoint disagree".to_owned(),
        ));
    }
    let operation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM surface_ops", [], |row| row.get(0))
        .map_err(|error| storage(format!("count Surface operations: {error}")))?;
    let max_operation_revision: Option<i64> = connection
        .query_row("SELECT MAX(revision) FROM surface_ops", [], |row| {
            row.get(0)
        })
        .map_err(|error| storage(format!("read latest Surface operation: {error}")))?;
    let operation_history_matches_head = if revision == 0 {
        operation_count == 0 && max_operation_revision.is_none()
    } else {
        operation_count == revision && max_operation_revision == Some(revision)
    };
    if !operation_history_matches_head {
        return Err(ConversationStoreError::InvalidReference(
            "Surface head does not match the complete immutable operation history".to_owned(),
        ));
    }
    if head
        .active_message_ids
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        != head.active_message_ids.len()
    {
        return Err(ConversationStoreError::InvalidReference(
            "Surface head contains duplicate active identities".to_owned(),
        ));
    }
    let reconstructed = reconstruct_surface(connection, head.revision)?;
    if reconstructed != head.active_message_ids {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface head {} does not match its immutable operation history",
            head.revision
        )));
    }
    for id in &head.active_message_ids {
        let _ = load_message(connection, id)?;
    }
    Ok(head)
}

fn update_surface_head(
    transaction: &Transaction<'_>,
    revision: SurfaceRevision,
    generation: u64,
    active: &[MessageId],
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "UPDATE surface_head SET revision=?1,compaction_generation=?2,active_json=?3 WHERE id=1",
            params![
                seq_to_i64(revision.get())?,
                seq_to_i64(generation)?,
                encode(&active, "Surface head")?
            ],
        )
        .map_err(|error| storage(format!("update Surface head: {error}")))?;
    Ok(())
}

fn update_checkpoint(
    transaction: &Transaction<'_>,
    revision: SurfaceRevision,
    generation: u64,
    active: &[MessageId],
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "UPDATE context_checkpoints SET revision=?1,compaction_generation=?2,active_json=?3 WHERE id=1",
            params![
                seq_to_i64(revision.get())?,
                seq_to_i64(generation)?,
                encode(&active, "context checkpoint")?
            ],
        )
        .map_err(|error| storage(format!("update context checkpoint: {error}")))?;
    Ok(())
}

fn append_message_and_surface(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    append_message_and_surface_internal(transaction, message, None)
}

fn append_adopted_message_and_surface(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    pending_message_id: &MessageId,
) -> Result<(), ConversationStoreError> {
    append_message_and_surface_internal(transaction, message, Some(pending_message_id))
}

fn append_message_and_surface_internal(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    allowed_pending_message_id: Option<&MessageId>,
) -> Result<(), ConversationStoreError> {
    if matches!(
        message,
        MessageBlock::User(user)
            if user.kind == crate::message::types::InboundKind::CompactionSummary
    ) {
        return Err(ConversationStoreError::InvalidReference(
            "a compaction summary must use the atomic commit_compaction transition".to_owned(),
        ));
    }
    let id = crate::conversation::message_id_of(message);
    let head = load_head(transaction)?;
    if head.active_message_ids.contains(&id) {
        return Err(ConversationStoreError::DuplicateMessageId(id));
    }
    append_message_ledger(transaction, message, allowed_pending_message_id)?;
    let mut active = head.active_message_ids;
    active.push(id.clone());
    let revision = head.revision.next();
    append_surface_op(
        transaction,
        revision,
        head.compaction_generation,
        &SurfaceOp::Append { message_id: id },
    )?;
    update_surface_head(transaction, revision, head.compaction_generation, &active)?;
    update_checkpoint(transaction, revision, head.compaction_generation, &active)
}

fn append_message_ledger(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    allowed_pending_message_id: Option<&MessageId>,
) -> Result<(), ConversationStoreError> {
    let id = crate::conversation::message_id_of(message);
    if pending_message_exists(transaction, &id)? && allowed_pending_message_id != Some(&id) {
        return Err(ConversationStoreError::DuplicateMessageId(id));
    }
    if ledger_message_exists(transaction, &id)? {
        return Err(ConversationStoreError::DuplicateMessageId(id));
    }
    let position: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(position),0)+1 FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("next Ledger position: {error}")))?;
    transaction
        .execute(
            "INSERT INTO message_ledger(position,message_id,message_json) VALUES(?1,?2,?3)",
            params![position, id.as_str(), encode(message, "canonical message")?],
        )
        .map_err(|error| map_insert_error(&error, &id))?;
    Ok(())
}

fn pending_message_exists(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_inbound WHERE message_id=?1)",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("pending message identity probe: {error}")))
}

fn append_surface_op(
    transaction: &Transaction<'_>,
    revision: SurfaceRevision,
    generation: u64,
    operation: &SurfaceOp,
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "INSERT INTO surface_ops(revision,compaction_generation,op_json) VALUES(?1,?2,?3)",
            params![
                seq_to_i64(revision.get())?,
                seq_to_i64(generation)?,
                encode(operation, "Surface operation")?
            ],
        )
        .map_err(|error| storage(format!("append Surface operation: {error}")))?;
    Ok(())
}

fn message_exists(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_inbound WHERE message_id=?1) OR EXISTS(SELECT 1 FROM message_ledger WHERE message_id=?1)",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("message identity probe: {error}")))
}

fn load_message(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<MessageBlock, ConversationStoreError> {
    let json: String = connection
        .query_row(
            "SELECT message_json FROM message_ledger WHERE message_id=?1",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("Ledger message lookup: {error}")))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!("message {message_id} is unavailable"))
        })?;
    let message: MessageBlock = decode(&json, "Ledger message")?;
    if crate::conversation::message_id_of(&message) != *message_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Ledger row for {message_id} contains a different message identity"
        )));
    }
    Ok(message)
}

fn load_message_tx(
    transaction: &Transaction<'_>,
    message_id: &MessageId,
) -> Result<MessageBlock, ConversationStoreError> {
    load_message(transaction, message_id)
}

fn load_correlated_user_message(
    transaction: &Transaction<'_>,
    message_id: &MessageId,
) -> Result<UserMessageBlock, ConversationStoreError> {
    let pending_json: Option<String> = transaction
        .query_row(
            "SELECT message_json FROM pending_inbound WHERE message_id=?1",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("correlated pending lookup: {error}")))?;
    if let Some(json) = pending_json {
        let message: UserMessageBlock = decode(&json, "correlated pending message")?;
        if message.id != *message_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "correlation for {message_id} points to a pending body with a different identity"
            )));
        }
        return Ok(message);
    }
    let message = load_message_tx(transaction, message_id)?;
    let MessageBlock::User(message) = message else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "correlation for {message_id} points to a non-User Ledger fact"
        )));
    };
    if message.id != *message_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "correlation for {message_id} points to a Ledger body with a different identity"
        )));
    }
    Ok(message)
}

fn load_pending_rows(
    connection: &Connection,
) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
    let mut statement = connection
        .prepare("SELECT sequence,message_id,message_json,correlation FROM pending_inbound ORDER BY sequence")
        .map_err(|error| storage(format!("load pending: {error}")))?;
    let rows = statement
        .query_map([], |row| {
            let sequence: i64 = row.get(0)?;
            let message_id = MessageId::new(row.get::<_, String>(1)?);
            let json: String = row.get(2)?;
            let message: UserMessageBlock = serde_json::from_str(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(PendingInboundItem {
                sequence: InboundSequence::new(read_sequence(sequence)?),
                message_id,
                message,
                correlation: row.get(3)?,
            })
        })
        .map_err(|error| storage(format!("pending query: {error}")))?;
    let items: Vec<PendingInboundItem> = rows
        .map(|row| row.map_err(|error| storage(format!("pending row: {error}"))))
        .collect::<Result<_, _>>()?;
    for item in &items {
        if item.message.id != item.message_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "pending row {} contains a different message identity",
                item.message_id
            )));
        }
    }
    Ok(items)
}

fn load_pending_until(
    transaction: &Transaction<'_>,
    watermark: InboundSequence,
) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
    Ok(load_pending_rows(transaction)?
        .into_iter()
        .filter(|item| item.sequence <= watermark)
        .collect())
}

fn load_canonical_rows(
    connection: &Connection,
) -> Result<Vec<MessageBlock>, ConversationStoreError> {
    let mut statement = connection
        .prepare("SELECT message_id,message_json FROM message_ledger ORDER BY position")
        .map_err(|error| storage(format!("load Ledger: {error}")))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| storage(format!("load Ledger query: {error}")))?;
    rows.map(|row| {
        let (stored_id, json) =
            row.map_err(|error| storage(format!("load Ledger row: {error}")))?;
        let message: MessageBlock = decode(&json, "Ledger message")?;
        let actual_id = crate::conversation::message_id_of(&message);
        if actual_id.as_str() != stored_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Ledger row {stored_id} contains message {actual_id}"
            )));
        }
        Ok(message)
    })
    .collect()
}

fn reconstruct_surface(
    connection: &Connection,
    revision: SurfaceRevision,
) -> Result<Vec<MessageId>, ConversationStoreError> {
    let Some((head_revision, _, _)) = read_surface_head(connection)? else {
        if revision == SurfaceRevision::INITIAL {
            return Ok(Vec::new());
        }
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {revision} has no durable head"
        )));
    };
    let head_revision = SurfaceRevision::new(nonnegative(head_revision, "Surface revision")?);
    if revision > head_revision {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {revision} is newer than head {head_revision}"
        )));
    }
    let mut active = Vec::new();
    let mut statement = connection
        .prepare(
            "SELECT revision,compaction_generation,op_json
             FROM surface_ops WHERE revision <= ?1 ORDER BY revision",
        )
        .map_err(|error| storage(format!("read Surface history: {error}")))?;
    let rows = statement
        .query_map([seq_to_i64(revision.get())?], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| storage(format!("read Surface history query: {error}")))?;
    let mut expected_revision = 1_u64;
    let mut expected_generation = 0_u64;
    for row in rows {
        let (stored_revision, stored_generation, json) =
            row.map_err(|error| storage(format!("Surface history row: {error}")))?;
        if u64::try_from(stored_revision).ok() != Some(expected_revision) {
            return Err(ConversationStoreError::InvalidReference(
                "Surface operation revisions are not contiguous from revision 1".to_owned(),
            ));
        }
        let stored_generation = nonnegative(stored_generation, "Surface compaction generation")?;
        let operation: SurfaceOp = decode(&json, "Surface operation")?;
        validate_surface_operation_references(connection, &operation)?;
        let is_replace = matches!(&operation, SurfaceOp::Replace { .. });
        let next_generation = if is_replace {
            expected_generation
                .checked_add(1)
                .ok_or_else(|| storage("Surface compaction generation is exhausted"))?
        } else {
            expected_generation
        };
        if stored_generation != next_generation {
            return Err(ConversationStoreError::InvalidReference(
                "Surface operation compaction generation is inconsistent".to_owned(),
            ));
        }
        expected_generation = next_generation;
        apply_surface_op(&mut active, operation)?;
        expected_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| storage("Surface revision is exhausted"))?;
    }
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM surface_ops WHERE revision <= ?1",
            [seq_to_i64(revision.get())?],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("Surface history count: {error}")))?;
    let expected_next_revision = revision
        .get()
        .checked_add(1)
        .ok_or_else(|| storage("Surface revision is exhausted"))?;
    if expected_revision != expected_next_revision || count != seq_to_i64(revision.get())? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {revision} has a non-contiguous operation history"
        )));
    }
    Ok(active)
}

fn validate_surface_operation_references(
    connection: &Connection,
    operation: &SurfaceOp,
) -> Result<(), ConversationStoreError> {
    let references: [&MessageId; 3] = match operation {
        SurfaceOp::Append { message_id } => [message_id, message_id, message_id],
        SurfaceOp::Replace {
            start,
            end,
            replacement,
        } => [start, end, replacement],
    };
    for message_id in references {
        if !ledger_message_exists(connection, message_id)? {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Surface operation references missing Ledger message {message_id}"
            )));
        }
    }
    if let SurfaceOp::Replace { replacement, .. } = operation {
        let message = load_message(connection, replacement)?;
        if !matches!(
            message,
            MessageBlock::User(user)
                if user.source == UserSource::Runtime
                    && user.kind == InboundKind::CompactionSummary
        ) {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Surface Replace replacement {replacement} is not a User(Runtime / CompactionSummary) message"
            )));
        }
    }
    Ok(())
}

fn reconstruct_surface_tx(
    transaction: &Transaction<'_>,
    revision: SurfaceRevision,
) -> Result<Vec<MessageId>, ConversationStoreError> {
    reconstruct_surface(transaction, revision)
}

fn validate_surface_revision(
    transaction: &Transaction<'_>,
    revision: SurfaceRevision,
) -> Result<(), ConversationStoreError> {
    let _ = reconstruct_surface_tx(transaction, revision)?;
    Ok(())
}

fn apply_surface_op(
    active: &mut Vec<MessageId>,
    operation: SurfaceOp,
) -> Result<(), ConversationStoreError> {
    match operation {
        SurfaceOp::Append { message_id } => {
            if active.contains(&message_id) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Surface Append repeats active message {message_id}"
                )));
            }
            active.push(message_id);
        }
        SurfaceOp::Replace {
            start,
            end,
            replacement,
        } => {
            let from = active.iter().position(|id| id == &start).ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "Surface Replace start {start} is not active"
                ))
            })?;
            let to = active.iter().position(|id| id == &end).ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "Surface Replace end {end} is not active"
                ))
            })?;
            if to < from || active.contains(&replacement) {
                return Err(ConversationStoreError::InvalidReference(
                    "Surface Replace has an invalid span or active replacement".to_owned(),
                ));
            }
            active.splice(from..=to, [replacement]);
        }
    }
    Ok(())
}

fn span_indices(
    active: &[MessageId],
    span: &SurfaceSpan,
) -> Result<(usize, usize), ConversationStoreError> {
    let start = active
        .iter()
        .position(|id| id == &span.start)
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "compaction start {} is not active",
                span.start
            ))
        })?;
    let end = active
        .iter()
        .position(|id| id == &span.end)
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "compaction end {} is not active",
                span.end
            ))
        })?;
    if end < start {
        return Err(ConversationStoreError::InvalidReference(
            "compaction span is reversed".to_owned(),
        ));
    }
    Ok((start, end))
}

fn replacement_active(
    active: &[MessageId],
    start: usize,
    end: usize,
    replacement: &MessageId,
) -> Vec<MessageId> {
    let mut next = active.to_vec();
    next.splice(start..=end, [replacement.clone()]);
    next
}

fn persist_event_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    mut event: RuntimeEventEnvelope,
) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
    if event.schema_version != EVENT_SCHEMA_VERSION {
        return Err(ConversationStoreError::InvalidReference(format!(
            "event schema {} is not {}",
            event.schema_version, EVENT_SCHEMA_VERSION
        )));
    }
    if event.conversation_id != *conversation_id {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: conversation_id.clone(),
            requested: event.conversation_id,
        });
    }
    validate_event_identity(&event)?;
    validate_attempt_start_uniqueness(transaction, &event)?;
    validate_event_reference(transaction, &event)?;
    let current: i64 = transaction
        .query_row(
            "SELECT next_event_sequence FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read event sequence: {error}")))?;
    let sequence = current
        .checked_add(1)
        .ok_or_else(|| storage("event sequence exhausted"))?;
    if event.event_id.as_str().is_empty() {
        event.event_id = EventId::new(format!("{conversation_id}-event-{sequence}"));
    }
    event.sequence = u64::try_from(sequence).map_err(|_| storage("event sequence exhausted"))?;
    let lifecycles = lifecycle_keys(&event);
    for (key, _) in &lifecycles {
        let existing: Option<Option<String>> = transaction
            .query_row(
                "SELECT terminal_event_id FROM lifecycle_state WHERE lifecycle_key=?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("lifecycle probe: {error}")))?;
        if existing.as_ref().is_some_and(Option::is_some) {
            return Err(ConversationStoreError::TerminalViolation(format!(
                "lifecycle {key} is already terminal"
            )));
        }
    }
    for (key, terminal) in lifecycles {
        if terminal {
            transaction
                .execute(
                    "INSERT INTO lifecycle_state(lifecycle_key,terminal_event_id) VALUES(?1,?2) ON CONFLICT(lifecycle_key) DO UPDATE SET terminal_event_id=excluded.terminal_event_id",
                    params![key, event.event_id.as_str()],
                )
                .map_err(|error| storage(format!("record terminal lifecycle: {error}")))?;
        } else {
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(|error| storage(format!("lifecycle existence probe: {error}")))?;
            if !exists {
                transaction
                    .execute(
                        "INSERT INTO lifecycle_state(lifecycle_key,terminal_event_id) VALUES(?1,NULL)",
                        [&key],
                    )
                    .map_err(|error| storage(format!("record lifecycle: {error}")))?;
            }
        }
    }
    let json = encode(&event, "runtime event")?;
    transaction
        .execute(
            "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                sequence,
                event.event_id.as_str(),
                i64::from(event.schema_version),
                event.conversation_id.as_str(),
                event.attempt_id.as_ref().map(ToString::to_string),
                event.turn_id.as_ref().map(ToString::to_string),
                json
            ],
        )
        .map_err(|error| storage(format!("insert runtime event: {error}")))?;
    transaction
        .execute(
            "UPDATE rustx_store SET next_event_sequence=?1 WHERE id=1",
            [sequence],
        )
        .map_err(|error| storage(format!("update event sequence: {error}")))?;
    Ok(event)
}

fn validate_event_identity(envelope: &RuntimeEventEnvelope) -> Result<(), ConversationStoreError> {
    if matches!(
        &envelope.event,
        RuntimeEvent::TurnStarted | RuntimeEvent::TurnCompleted
    ) && envelope.turn_id.is_none()
    {
        return Err(ConversationStoreError::InvalidReference(
            "turn lifecycle event has no turn identity".to_owned(),
        ));
    }
    let payload_attempt = match &envelope.event {
        RuntimeEvent::AttemptStarted { attempt_id }
        | RuntimeEvent::AttemptCompleted { attempt_id, .. }
        | RuntimeEvent::AttemptCancelled { attempt_id, .. }
        | RuntimeEvent::AttemptTimedOut { attempt_id }
        | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
        | RuntimeEvent::AttemptFailed { attempt_id, .. } => Some(attempt_id),
        _ => None,
    };
    if let (Some(payload_attempt), Some(envelope_attempt)) =
        (payload_attempt, envelope.attempt_id.as_ref())
        && payload_attempt != envelope_attempt
    {
        return Err(ConversationStoreError::InvalidReference(
            "event envelope attempt identity disagrees with its typed payload".to_owned(),
        ));
    }
    Ok(())
}

/// An attempt identity starts exactly once in durable authority (Issue #12,
/// M9a).
///
/// `AttemptStarted` is the first event of every attempt, so its lifecycle key
/// cannot already exist. Rejecting a second start makes accidental identity
/// reuse — most importantly a process-local attempt ordinal reset after a
/// restart — a typed durable failure instead of two logical attempts sharing
/// one durable identity.
fn validate_attempt_start_uniqueness(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::AttemptStarted { attempt_id } = &envelope.event else {
        return Ok(());
    };
    let key = format!("attempt:{attempt_id}");
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
            [&key],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("attempt start uniqueness probe: {error}")))?;
    if exists {
        return Err(ConversationStoreError::TerminalViolation(format!(
            "attempt {attempt_id} already entered durable authority; an attempt identity starts exactly once"
        )));
    }
    Ok(())
}

fn find_request_start_event(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let mut statement = transaction
        .prepare("SELECT event_json FROM events ORDER BY sequence")
        .map_err(|error| storage(format!("request-start event probe: {error}")))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| storage(format!("request-start event query: {error}")))?;
    let mut found = None;
    for row in rows {
        let json = row.map_err(|error| storage(format!("request-start event row: {error}")))?;
        let event: RuntimeEventEnvelope = decode(&json, "request-start event")?;
        if matches!(
            &event.event,
            RuntimeEvent::ModelRequestStarted {
                request_id: candidate,
                ..
            } if candidate == request_id
        ) {
            if found.is_some() {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "request {request_id} has duplicate request-start facts"
                )));
            }
            found = Some(event);
        }
    }
    Ok(found)
}

fn find_event_by_id(
    transaction: &Transaction<'_>,
    event_id: &EventId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let Some(json) = transaction
        .query_row(
            "SELECT event_json FROM events WHERE event_id=?1",
            [event_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| storage(format!("event identity probe: {error}")))?
    else {
        return Ok(None);
    };
    Ok(Some(decode(&json, "event identity")?))
}

#[allow(clippy::too_many_lines)] // Keeps all cross-domain reference checks at one transaction seam.
fn validate_event_reference(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    match &envelope.event {
        RuntimeEvent::AssistantMessageCommitted { message_id } => {
            if !ledger_message_exists(transaction, message_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "committed-message event references missing message {message_id}"
                )));
            }
            if !matches!(
                load_message_tx(transaction, message_id)?,
                MessageBlock::Assistant(_)
            ) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Assistant committed-message event references non-Assistant message {message_id}"
                )));
            }
        }
        RuntimeEvent::ToolMessageCommitted {
            message_id,
            tool_call_id,
        } => {
            if !ledger_message_exists(transaction, message_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "committed-message event references missing message {message_id}"
                )));
            }
            let message = load_message_tx(transaction, message_id)?;
            let MessageBlock::Tool(tool) = message else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Tool committed-message event references non-Tool message {message_id}"
                )));
            };
            if tool.tool_call_id != *tool_call_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Tool committed-message event for {message_id} references tool call {}, body answers {}",
                    tool_call_id, tool.tool_call_id
                )));
            }
        }
        RuntimeEvent::CompactionCompleted {
            generation,
            summary_message_id,
            surface_revision,
            ..
        } => {
            if !ledger_message_exists(transaction, summary_message_id)? {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "compaction event references missing summary {summary_message_id}"
                )));
            }
            let active = reconstruct_surface_tx(transaction, *surface_revision)?;
            if !active.contains(summary_message_id) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "compaction event references Surface revision {surface_revision} without its summary"
                )));
            }
            let surface_op: Option<(i64, String)> = transaction
                .query_row(
                    "SELECT compaction_generation,op_json FROM surface_ops WHERE revision=?1",
                    [seq_to_i64(surface_revision.get())?],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| {
                    storage(format!("compaction Surface operation lookup: {error}"))
                })?;
            let Some((stored_generation, op_json)) = surface_op else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "compaction event references missing Surface operation {surface_revision}"
                )));
            };
            let operation: SurfaceOp = decode(&op_json, "compaction Surface operation")?;
            if u64::try_from(stored_generation).ok() != Some(*generation)
                || !matches!(
                    operation,
                    SurfaceOp::Replace { replacement, .. } if replacement == *summary_message_id
                )
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "compaction event does not match Surface revision {surface_revision}"
                )));
            }
            let summary = load_message_tx(transaction, summary_message_id)?;
            if !matches!(
                summary,
                MessageBlock::User(user)
                    if user.source == UserSource::Runtime
                        && user.kind == crate::message::types::InboundKind::CompactionSummary
            ) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "compaction event references a non-summary message {summary_message_id}"
                )));
            }
        }
        RuntimeEvent::ModelRequestStarted { request_id, .. } => {
            let json: Option<String> = transaction
                .query_row(
                    "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
                    [request_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(format!("request event reference: {error}")))?;
            let Some(json) = json else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "request-start event references missing Request Snapshot {request_id}"
                )));
            };
            let snapshot: RequestSnapshot = decode(&json, "request-start snapshot")?;
            if snapshot.request_id != *request_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "request-start snapshot identity does not match {request_id}"
                )));
            }
            validate_request_start_metadata(&snapshot, envelope)?;
            for message_id in reconstruct_surface_tx(transaction, snapshot.surface_revision)? {
                let _ = load_message_tx(transaction, &message_id)?;
            }
        }
        RuntimeEvent::BackgroundTerminalPublished { message_id, .. } => {
            let notification = load_user_notification_tx(transaction, message_id, "background")?;
            if notification.source != UserSource::Runtime
                || notification.kind != InboundKind::Message
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "background terminal fact references an ineligible notification {message_id}"
                )));
            }
        }
        RuntimeEvent::SubagentTerminalPublished {
            child_agent_id,
            message_id,
            state,
            ..
        } => {
            let publication = load_user_notification_tx(transaction, message_id, "subagent")?;
            let provenance_ok = match state {
                // A successful child answer is authored by the child agent;
                // every other terminal is a runtime-authored notice.
                crate::events::types::SubagentTerminalState::Succeeded => {
                    publication.source
                        == UserSource::Agent {
                            agent_id: child_agent_id.clone(),
                        }
                }
                crate::events::types::SubagentTerminalState::Failed
                | crate::events::types::SubagentTerminalState::Cancelled
                | crate::events::types::SubagentTerminalState::Interrupted => {
                    publication.source == UserSource::Runtime
                }
            };
            if !provenance_ok || publication.kind != InboundKind::Message {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent terminal fact references an ineligible publication {message_id}"
                )));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Loads the User notification a detached terminal fact references: the
/// pending row when it has not been adopted yet, otherwise the canonical
/// Ledger row.
fn load_user_notification_tx(
    transaction: &Transaction<'_>,
    message_id: &MessageId,
    domain: &str,
) -> Result<UserMessageBlock, ConversationStoreError> {
    let pending_json: Option<String> = transaction
        .query_row(
            "SELECT message_json FROM pending_inbound WHERE message_id=?1",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("{domain} notification reference: {error}")))?;
    let message = if let Some(json) = pending_json {
        MessageBlock::User(decode::<UserMessageBlock>(
            &json,
            "detached pending notification",
        )?)
    } else {
        load_message_tx(transaction, message_id)?
    };
    let MessageBlock::User(notification) = message else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "{domain} terminal fact references non-User notification {message_id}"
        )));
    };
    if notification.id != *message_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "{domain} terminal fact references {message_id}, but the stored notification is {}",
            notification.id
        )));
    }
    Ok(notification)
}

fn requires_compound_transaction(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::BackgroundTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
    )
}

fn validate_canonical_event_for_message(
    message: &MessageBlock,
    event: &RuntimeEvent,
) -> Result<(), ConversationStoreError> {
    let message_id = crate::conversation::message_id_of(message);
    match (message, event) {
        (
            MessageBlock::Assistant(_),
            RuntimeEvent::AssistantMessageCommitted {
                message_id: event_id,
            },
        ) if event_id == &message_id => Ok(()),
        (
            MessageBlock::Tool(tool),
            RuntimeEvent::ToolMessageCommitted {
                message_id: event_id,
                tool_call_id,
            },
        ) if event_id == &message_id && tool_call_id == &tool.tool_call_id => Ok(()),
        _ => Err(ConversationStoreError::InvalidReference(format!(
            "canonical event does not identify the exact committed message {message_id}"
        ))),
    }
}

fn validate_request_start_metadata(
    snapshot: &RequestSnapshot,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::ModelRequestStarted { request_id, model } = &envelope.event else {
        return Err(ConversationStoreError::InvalidReference(
            "request snapshot is paired with a non-request-start event".to_owned(),
        ));
    };
    if request_id != &snapshot.request_id
        || model != &snapshot.invocation.model
        || envelope.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
        || envelope.turn_id.as_ref() != Some(&snapshot.identity.turn)
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start fact does not exactly identify Request Snapshot {}",
            snapshot.request_id
        )));
    }
    Ok(())
}

fn validate_snapshot_identity(snapshot: &RequestSnapshot) -> Result<(), ConversationStoreError> {
    let derived = snapshot.identity.request_id();
    if snapshot.request_id != derived {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Request Snapshot identity {} disagrees with its RequestIdentity-derived id {derived}",
            snapshot.request_id
        )));
    }
    Ok(())
}

/// Validates that the exact ordered request-scoped context supplied for a
/// model-turn start equals the frozen `request_context_ids` of its snapshot
/// (Issue #12, M9b).
///
/// This is the one input validation rule shared by the fresh commit and the
/// idempotent retry: the complete ordered `MessageId` set must match before
/// the store chooses a path or touches durable state, so a fresh commit can
/// never persist a snapshot that disagrees with the request-scoped context it
/// appends atomically. The frozen ids prove exact ordered equality — an empty
/// set, a prefix, a reorder, or an extra message all fail here — and the
/// retry path additionally proves per-message body equality against the
/// already-committed fact.
fn validate_request_context(
    snapshot: &RequestSnapshot,
    context: &[MessageBlock],
) -> Result<(), ConversationStoreError> {
    let supplied_ids: Vec<MessageId> = context
        .iter()
        .map(crate::conversation::message_id_of)
        .collect();
    if supplied_ids != snapshot.request_context_ids {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {} supplied a request-scoped context different from its frozen snapshot",
            snapshot.request_id
        )));
    }
    Ok(())
}

fn ledger_message_exists(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<bool, ConversationStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM message_ledger WHERE message_id=?1)",
            [message_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("event Ledger reference: {error}")))
}

fn lifecycle_keys(event: &RuntimeEventEnvelope) -> Vec<(String, bool)> {
    // The detached-execution lifecycle is opened by the ownership commit and
    // closed exactly once by the terminal publication. Recording the open
    // state durably is what lets a restart tell "owned and unsettled" from
    // "never existed", and what makes a second terminal publication — live
    // or recovery-generated — a typed `TerminalViolation` rather than a
    // duplicate model-visible notification.
    if let RuntimeEvent::BackgroundExecutionCommitted { execution_id, .. } = &event.event {
        return vec![(format!("background:{execution_id}"), false)];
    }
    if let RuntimeEvent::BackgroundTerminalPublished { execution_id, .. } = &event.event {
        return vec![(format!("background:{execution_id}"), true)];
    }
    // The subagent lifecycle is the same shape: opened by the ownership
    // commit (before the child may begin any semantic work) and closed
    // exactly once by the terminal publication, so a restart can tell an
    // owned-but-unsettled child from one that never existed, and a second
    // terminal publication is a typed `TerminalViolation`.
    if let RuntimeEvent::SubagentOwnershipCommitted { subagent_id, .. } = &event.event {
        return vec![(format!("subagent:{subagent_id}"), false)];
    }
    if let RuntimeEvent::SubagentTerminalPublished { subagent_id, .. } = &event.event {
        return vec![(format!("subagent:{subagent_id}"), true)];
    }
    let attempt = event.attempt_id.as_ref().or(match &event.event {
        RuntimeEvent::AttemptStarted { attempt_id }
        | RuntimeEvent::AttemptCompleted { attempt_id, .. }
        | RuntimeEvent::AttemptCancelled { attempt_id, .. }
        | RuntimeEvent::AttemptTimedOut { attempt_id }
        | RuntimeEvent::AttemptLimitExceeded { attempt_id, .. }
        | RuntimeEvent::AttemptFailed { attempt_id, .. } => Some(attempt_id),
        _ => None,
    });
    let mut keys = Vec::new();
    let attempt_terminal = is_terminal(&event.event);
    if let Some(attempt) = attempt {
        keys.push((format!("attempt:{attempt}"), attempt_terminal));
    }
    if !attempt_terminal && let Some(turn) = event.turn_id.as_ref() {
        let owner = event
            .attempt_id
            .as_ref()
            .map_or_else(|| event.conversation_id.to_string(), ToString::to_string);
        keys.push((
            format!("turn:{owner}:{turn}"),
            matches!(&event.event, RuntimeEvent::TurnCompleted),
        ));
    }
    keys
}

fn is_terminal(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::AttemptCompleted { .. }
            | RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::BackgroundTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
    )
}

fn initial_history_digest(messages: &[MessageBlock]) -> Result<String, ConversationStoreError> {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(b"rustx-bootstrap-v2\n");
    for message in messages {
        let bytes = serde_json::to_vec(message)
            .map_err(|error| storage(format!("serialize bootstrap: {error}")))?;
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    let mut digest = String::new();
    for byte in hasher.finalize() {
        let _ = write!(digest, "{byte:02x}");
    }
    Ok(digest)
}

fn encode<T: Serialize>(value: &T, name: &str) -> Result<String, ConversationStoreError> {
    serde_json::to_string(value).map_err(|error| storage(format!("serialize {name}: {error}")))
}

fn decode<T: for<'de> Deserialize<'de>>(
    json: &str,
    name: &str,
) -> Result<T, ConversationStoreError> {
    serde_json::from_str(json).map_err(|error| storage(format!("decode {name}: {error}")))
}

fn sequence_from_i64(value: i64) -> Result<u64, ConversationStoreError> {
    u64::try_from(value).map_err(|_| ConversationStoreError::SequenceExhausted)
}

fn seq_to_i64(value: u64) -> Result<i64, ConversationStoreError> {
    i64::try_from(value).map_err(|_| storage("durable integer identity exhausted"))
}

fn nonnegative(value: i64, name: &str) -> Result<u64, ConversationStoreError> {
    u64::try_from(value).map_err(|_| storage(format!("{name} is negative")))
}

fn read_sequence(value: i64) -> Result<u64, rusqlite::Error> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn storage(message: impl Into<String>) -> ConversationStoreError {
    ConversationStoreError::Storage(message.into())
}

fn map_insert_error(error: &rusqlite::Error, message_id: &MessageId) -> ConversationStoreError {
    if is_constraint_violation(error) {
        ConversationStoreError::DuplicateMessageId(message_id.clone())
    } else {
        storage(format!("insert {message_id}: {error}"))
    }
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::assembly::ContextGeneration;
    use crate::conversation::ConversationState;
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, UserContentBlock,
        UserMessageBlock, UserSource,
    };
    use crate::model::catalog::ModelCapabilities;
    use crate::model::catalog::ModelCompat;
    use crate::model::finish::ModelFinishReason;
    use crate::model::invocation::{ModelInvocationConfig, RequestParams};
    use crate::model::snapshot::{RequestIdentity, RequestSnapshot};
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::identity::{AttemptId, EventId, TurnId};
    use crate::runtime::types::{TokenMeasurement, TokenMeasurementSource};
    use chrono::{TimeZone, Utc};

    fn draft(text: &str) -> InboundDraft {
        InboundDraft {
            message_id: None,
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
            correlation: None,
        }
    }

    fn store() -> SqliteConversationStore {
        SqliteConversationStore::in_memory(ConversationId::new("conv-1")).unwrap()
    }

    fn user_message(id: &str, text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap()),
        })
    }

    fn summary_message(id: &str, text: &str) -> UserMessageBlock {
        UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::CompactionSummary,
            timestamp: None,
        }
    }

    fn assistant_message(id: &str, text: &str) -> MessageBlock {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: vec![AssistantContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
        })
    }

    fn envelope(
        conversation_id: &ConversationId,
        event_id: &str,
        attempt_id: Option<AttemptId>,
        event: RuntimeEvent,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(event_id),
            sequence: 0,
            conversation_id: conversation_id.clone(),
            attempt_id,
            turn_id: None,
            timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
            event,
        }
    }

    fn invocation() -> ModelInvocationConfig {
        ModelInvocationConfig {
            model: "model-before-restart".to_owned(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 128,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        }
    }

    #[test]
    fn acceptance_and_adoption_share_durable_identity() {
        let store = store();
        let accepted = store.accept_inbound(draft("hello")).unwrap();
        let adopted = store.adopt_pending_batch(accepted.sequence).unwrap();
        assert_eq!(adopted.len(), 1);
        assert!(store.load_pending().unwrap().is_empty());
        assert_eq!(store.load_head().unwrap().active_message_ids.len(), 1);
    }

    #[test]
    fn historical_surface_survives_later_append() {
        let store = store();
        let a = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("a"),
            content: draft("a").content,
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        });
        let b = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("b"),
            content: draft("b").content,
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        });
        store.initialize(std::slice::from_ref(&a)).unwrap();
        let first = store.load_head().unwrap().revision;
        store.append_canonical(&b).unwrap();
        assert_eq!(
            store.reconstruct_surface(first).unwrap(),
            vec![MessageId::new("a")]
        );
    }

    #[test]
    fn raw_compaction_summary_append_cannot_bypass_atomic_transition() {
        let store = store();
        let original = user_message("a", "A");
        store.initialize(&[original]).unwrap();
        let before = store.load_head().unwrap();
        let result = store.append_canonical(&MessageBlock::User(summary_message(
            "summary-raw",
            "summary",
        )));
        assert!(matches!(
            result,
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(store.load_head().unwrap(), before);
        assert_eq!(store.load_canonical().unwrap().len(), 1);
        assert!(store.read_events(None, 10).unwrap().events.is_empty());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn immutable_surface_revisions_survive_compactions_and_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let conversation_id = ConversationId::new("conv-surface-history");
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        let a = user_message("a", "A");
        let b = user_message("b", "B");
        let c = user_message("c", "C");
        let d = user_message("d", "D");
        store.initialize(&[a, b, c]).unwrap();
        let s1 = store.load_head().unwrap().revision;

        let summary1 = summary_message("summary-1", "Summary 1");
        let first_compaction = |expected_revision| CompactionCommitInput {
            summary: summary1.clone(),
            span: SurfaceSpan::new(MessageId::new("a"), MessageId::new("a")),
            expected_revision,
            tokens_before: TokenMeasurement {
                input_tokens: 30,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 20,
            attempt_id: None,
            turn_id: None,
            timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
        };
        store.arm_fail_compaction_times(1);
        assert!(store.commit_compaction(first_compaction(s1)).is_err());
        assert_eq!(store.load_head().unwrap().revision, s1);
        assert!(
            !store
                .load_canonical()
                .unwrap()
                .iter()
                .any(|message| crate::conversation::message_id_of(message)
                    == MessageId::new("summary-1"))
        );

        let (s2, _, _) = store.commit_compaction(first_compaction(s1)).unwrap();
        assert_eq!(
            store.reconstruct_surface(s2).unwrap(),
            vec![
                MessageId::new("summary-1"),
                MessageId::new("b"),
                MessageId::new("c")
            ]
        );
        store.append_canonical(&d).unwrap();
        let s3 = store.load_head().unwrap().revision;
        assert_eq!(
            store.reconstruct_surface(s3).unwrap(),
            vec![
                MessageId::new("summary-1"),
                MessageId::new("b"),
                MessageId::new("c"),
                MessageId::new("d")
            ]
        );

        let summary2 = summary_message("summary-2", "Summary 2");
        store.arm_fail_event_times(1);
        assert!(
            store
                .commit_compaction(CompactionCommitInput {
                    summary: summary2.clone(),
                    span: SurfaceSpan::new(MessageId::new("summary-1"), MessageId::new("b")),
                    expected_revision: s3,
                    tokens_before: TokenMeasurement {
                        input_tokens: 20,
                        source: TokenMeasurementSource::Estimated,
                    },
                    estimated_tokens_after: 12,
                    attempt_id: None,
                    turn_id: None,
                    timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
                })
                .is_err()
        );
        assert_eq!(store.load_head().unwrap().revision, s3);
        let (s4, _, _) = store
            .commit_compaction(CompactionCommitInput {
                summary: summary2,
                span: SurfaceSpan::new(MessageId::new("summary-1"), MessageId::new("b")),
                expected_revision: s3,
                tokens_before: TokenMeasurement {
                    input_tokens: 20,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 12,
                attempt_id: None,
                turn_id: None,
                timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
            })
            .unwrap();
        assert_eq!(
            store.reconstruct_surface(s4).unwrap(),
            vec![
                MessageId::new("summary-2"),
                MessageId::new("c"),
                MessageId::new("d")
            ]
        );
        drop(store);

        let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
        assert_eq!(
            reopened.reconstruct_surface(s1).unwrap(),
            vec![
                MessageId::new("a"),
                MessageId::new("b"),
                MessageId::new("c")
            ]
        );
        assert_eq!(
            reopened.reconstruct_surface(s2).unwrap(),
            vec![
                MessageId::new("summary-1"),
                MessageId::new("b"),
                MessageId::new("c")
            ]
        );
        assert_eq!(
            reopened.reconstruct_surface(s3).unwrap(),
            vec![
                MessageId::new("summary-1"),
                MessageId::new("b"),
                MessageId::new("c"),
                MessageId::new("d")
            ]
        );
        assert_eq!(
            reopened.reconstruct_surface(s4).unwrap(),
            vec![
                MessageId::new("summary-2"),
                MessageId::new("c"),
                MessageId::new("d")
            ]
        );
        assert!(
            reopened
                .load_canonical()
                .unwrap()
                .iter()
                .any(|message| crate::conversation::message_id_of(message) == MessageId::new("a"))
        );
    }

    #[test]
    fn every_compaction_stage_fault_exposes_only_the_old_complete_state() {
        let faults = [
            CompactionFaultOperation::BeforeSummaryInsert,
            CompactionFaultOperation::AfterSummaryInsert,
            CompactionFaultOperation::AfterSurfaceRevision,
            CompactionFaultOperation::AfterCheckpoint,
            CompactionFaultOperation::BeforeEventInsert,
            CompactionFaultOperation::AfterEventInsert,
        ];
        for (index, fault) in faults.into_iter().enumerate() {
            let store = SqliteConversationStore::in_memory(ConversationId::new(format!(
                "conv-compaction-fault-{index}"
            )))
            .unwrap();
            let a = user_message("a", "A");
            let b = user_message("b", "B");
            store.initialize(&[a, b]).unwrap();
            let old_revision = store.load_head().unwrap().revision;
            let summary_id = MessageId::new(format!("summary-{index}"));
            let input = || CompactionCommitInput {
                summary: summary_message(summary_id.as_str(), "summary"),
                span: SurfaceSpan::new(MessageId::new("a"), MessageId::new("b")),
                expected_revision: old_revision,
                tokens_before: TokenMeasurement {
                    input_tokens: 10,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 2,
                attempt_id: None,
                turn_id: None,
                timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
            };
            store.arm_compaction_fault_script([fault]);
            assert!(store.commit_compaction(input()).is_err());
            assert_eq!(store.load_head().unwrap().revision, old_revision);
            assert_eq!(
                store.reconstruct_surface(old_revision).unwrap(),
                vec![MessageId::new("a"), MessageId::new("b")]
            );
            assert!(
                !store
                    .load_canonical()
                    .unwrap()
                    .iter()
                    .any(|message| crate::conversation::message_id_of(message) == summary_id)
            );
            assert!(store.read_events(None, 20).unwrap().events.is_empty());

            let (new_revision, _, event) = store.commit_compaction(input()).unwrap();
            assert_eq!(new_revision, old_revision.next());
            assert!(matches!(
                event.event,
                RuntimeEvent::CompactionCompleted {
                    summary_message_id,
                    surface_revision,
                    ..
                } if summary_message_id == summary_id && surface_revision == new_revision
            ));
            assert_eq!(
                store.reconstruct_surface(new_revision).unwrap(),
                vec![summary_id]
            );
        }
    }

    #[test]
    fn inbound_adoption_fault_keeps_pending_and_surface_atomic() {
        let store = store();
        store.arm_fail_next_accept_commit();
        assert!(store.accept_inbound(draft("not committed")).is_err());
        assert!(store.load_pending().unwrap().is_empty());

        let accepted = store.accept_inbound(draft("pending")).unwrap();
        store.arm_fail_next_adopt_commit();
        assert!(store.adopt_pending_batch(accepted.sequence).is_err());
        assert_eq!(store.load_pending().unwrap().len(), 1);
        assert!(store.load_canonical().unwrap().is_empty());
        assert_eq!(
            store.load_head().unwrap().revision,
            SurfaceRevision::INITIAL
        );

        let adopted = store.adopt_pending_batch(accepted.sequence).unwrap();
        assert_eq!(adopted.len(), 1);
        assert!(store.load_pending().unwrap().is_empty());
        assert_eq!(store.load_head().unwrap().active_message_ids.len(), 1);
    }

    #[test]
    fn pending_message_identity_cannot_be_reused_by_a_canonical_append() {
        let store = store();
        let accepted = store
            .accept_inbound(InboundDraft {
                message_id: Some(MessageId::new("shared-message-id")),
                ..draft("pending")
            })
            .unwrap();
        let conflicting = user_message("shared-message-id", "canonical");
        assert!(matches!(
            store.append_canonical(&conflicting),
            Err(ConversationStoreError::DuplicateMessageId(id))
                if id == MessageId::new("shared-message-id")
        ));
        assert_eq!(store.load_pending().unwrap().len(), 1);
        assert_eq!(store.load_pending().unwrap()[0].sequence, accepted.sequence);
        assert!(store.load_canonical().unwrap().is_empty());
    }

    #[test]
    fn corrupt_surface_operation_reference_fails_closed() {
        let store = store();
        store.initialize(&[user_message("a", "A")]).unwrap();
        let missing = MessageId::new("missing");
        let active = serde_json::to_string(&vec![MessageId::new("a"), missing.clone()]).unwrap();
        let operation = serde_json::to_string(&SurfaceOp::Append {
            message_id: missing.clone(),
        })
        .unwrap();
        let connection = store.conn.lock().unwrap();
        connection
            .execute(
                "INSERT INTO surface_ops(revision,compaction_generation,op_json) VALUES(2,0,?1)",
                [operation],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE surface_head SET revision=2,active_json=?1 WHERE id=1",
                [active.clone()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE context_checkpoints SET revision=2,active_json=?1 WHERE id=1",
                [active],
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            store.load_head(),
            Err(ConversationStoreError::InvalidReference(detail))
                if detail.contains("missing Ledger message")
        ));
    }

    /// The frozen request snapshot used by the request-start tests.
    fn test_request_snapshot(
        revision: crate::conversation::SurfaceRevision,
        invocation: ModelInvocationConfig,
    ) -> RequestSnapshot {
        RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("attempt-1"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            revision,
            "system frozen before restart".to_owned(),
            invocation,
            4096,
            None,
            false,
            Vec::new(),
            crate::runtime::identity::CapabilityRevision::new(7),
            ContextGeneration {
                id: 9,
                contributors: Vec::new(),
            },
            None,
            Vec::new(),
        )
    }

    /// A request-start snapshot with a caller-chosen ordered context id set.
    fn context_start_snapshot(
        revision: crate::conversation::SurfaceRevision,
        request_context_ids: Vec<MessageId>,
    ) -> RequestSnapshot {
        RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("attempt-1"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            revision,
            "frozen".to_owned(),
            invocation(),
            1024,
            None,
            false,
            Vec::new(),
            crate::runtime::identity::CapabilityRevision::new(1),
            ContextGeneration {
                id: 1,
                contributors: Vec::new(),
            },
            None,
            request_context_ids,
        )
    }

    #[test]
    fn request_start_is_atomic_and_reconstructs_from_durable_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let conversation_id = ConversationId::new("conv-request-history");
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        let a = user_message("a", "A");
        let b = user_message("b", "B");
        store.initialize(&[a.clone(), b.clone()]).unwrap();
        let revision = store.load_head().unwrap().revision;
        let invocation = invocation();
        let snapshot = test_request_snapshot(revision, invocation.clone());
        store.arm_request_start_fault_script([RequestStartFaultOperation::BeforeContextAppend]);
        assert!(
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .is_err()
        );
        assert!(
            store
                .read_request_snapshots(None, 32)
                .unwrap()
                .snapshots
                .is_empty()
        );
        assert!(store.read_events(None, 20).unwrap().events.is_empty());
        store.arm_fail_event_times(1);
        assert!(
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .is_err()
        );
        assert!(
            store
                .read_request_snapshots(None, 32)
                .unwrap()
                .snapshots
                .is_empty()
        );

        let started = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        assert_eq!(started.sequence, 1);
        assert!(matches!(
            started.event,
            RuntimeEvent::ModelRequestStarted { ref request_id, .. } if request_id == &snapshot.request_id
        ));
        let expected = ModelRequest {
            invocation,
            messages: vec![a, b],
            tools: Vec::new(),
            effective_system_prompt: "system frozen before restart".to_owned(),
            continuation: None,
        };
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            expected
        );

        store
            .append_canonical(&user_message("later", "later"))
            .unwrap();
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            expected
        );
        drop(store);
        let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
        assert_eq!(
            reopened
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            expected
        );
        assert_eq!(
            reopened
                .read_request_snapshots(None, 32)
                .unwrap()
                .snapshots
                .len(),
            1
        );
        assert_eq!(
            reopened
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .unwrap()
                .sequence,
            1
        );
    }

    /// Issue #12 (M9b): the request-scoped context, the Request Snapshot,
    /// and the `ModelRequestStarted` fact commit in one transaction; the
    /// snapshot's Surface revision is the head the context appends created.
    #[test]
    fn model_turn_start_commits_context_snapshot_and_event_atomically() {
        let store = store();
        let a = user_message("a", "A");
        store.initialize(std::slice::from_ref(&a)).unwrap();
        let base_revision = store.load_head().unwrap().revision;
        let context = user_message("ctx-1", "request-scoped context");
        let snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("attempt-1"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            base_revision.next(),
            "frozen".to_owned(),
            invocation(),
            1024,
            None,
            false,
            Vec::new(),
            crate::runtime::identity::CapabilityRevision::new(1),
            ContextGeneration {
                id: 1,
                contributors: Vec::new(),
            },
            None,
            vec![MessageId::new("ctx-1")],
        );
        let started = store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
            .expect("start commits");
        assert!(matches!(
            started.event,
            RuntimeEvent::ModelRequestStarted { ref request_id, .. } if request_id == &snapshot.request_id
        ));
        let head = store.load_head().unwrap();
        assert_eq!(head.revision, base_revision.next());
        assert_eq!(head.active_message_ids.len(), 2);
        assert_eq!(store.load_canonical().unwrap().len(), 2);
        // The snapshot's revision resolves to the post-context surface.
        let reconstructed = store
            .reconstruct_model_request(&snapshot.request_id)
            .expect("reconstruct");
        assert_eq!(reconstructed.messages, vec![a, context.clone()]);
        // An exact retry is idempotent and returns the original start fact.
        let retried = store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
            .expect("exact retry");
        assert_eq!(retried.sequence, started.sequence);
        // A retry whose context differs from the committed facts fails.
        let different = user_message("ctx-1", "different content");
        assert!(matches!(
            store.commit_model_turn_start(std::slice::from_ref(&different), &snapshot, Utc::now()),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        // A retry missing a committed context message fails.
        let missing = user_message("ctx-2", "never committed");
        assert!(
            store
                .commit_model_turn_start(std::slice::from_ref(&missing), &snapshot, Utc::now())
                .is_err()
        );
    }

    /// Issue #12 (M9b): the durable request-start authority binds the exact
    /// ordered request-scoped context of a request. An idempotent retry must
    /// prove exact ordered equality — an empty retry, a prefix, a reorder, an
    /// extra message, and a same-ids/different-body retry all fail — so the
    /// retried start can never substitute a different context set.
    #[test]
    fn model_turn_start_retry_enforces_exact_ordered_context_equality() {
        let store = store();
        let a = user_message("a", "A");
        store.initialize(std::slice::from_ref(&a)).unwrap();
        let base_revision = store.load_head().unwrap().revision;
        let ctx1 = user_message("ctx-1", "first context");
        let ctx2 = user_message("ctx-2", "second context");
        let snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("attempt-1"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            base_revision.next().next(),
            "frozen".to_owned(),
            invocation(),
            1024,
            None,
            false,
            Vec::new(),
            crate::runtime::identity::CapabilityRevision::new(1),
            ContextGeneration {
                id: 1,
                contributors: Vec::new(),
            },
            None,
            vec![MessageId::new("ctx-1"), MessageId::new("ctx-2")],
        );
        let started = store
            .commit_model_turn_start(&[ctx1.clone(), ctx2.clone()], &snapshot, Utc::now())
            .expect("start commits");

        // The exact ordered retry is idempotent and returns the original fact.
        let retried = store
            .commit_model_turn_start(&[ctx1.clone(), ctx2.clone()], &snapshot, Utc::now())
            .expect("exact ordered retry");
        assert_eq!(retried.sequence, started.sequence);

        // Empty retry fails: the complete ordered set is required.
        assert!(
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .is_err()
        );
        // Prefix retry fails.
        assert!(
            store
                .commit_model_turn_start(std::slice::from_ref(&ctx1), &snapshot, Utc::now())
                .is_err()
        );
        // Reordered retry fails.
        assert!(
            store
                .commit_model_turn_start(&[ctx2.clone(), ctx1.clone()], &snapshot, Utc::now())
                .is_err()
        );
        // Extra message fails.
        let extra = user_message("ctx-3", "extra");
        assert!(
            store
                .commit_model_turn_start(
                    &[ctx1.clone(), ctx2.clone(), extra],
                    &snapshot,
                    Utc::now()
                )
                .is_err()
        );
        // Same ids with a different body fails.
        let different = user_message("ctx-1", "changed body");
        assert!(
            store
                .commit_model_turn_start(&[different, ctx2], &snapshot, Utc::now())
                .is_err()
        );
    }

    /// Issue #12 (M9b): the fresh model-turn start validates the exact
    /// ordered request-scoped context against the snapshot's frozen
    /// `request_context_ids` BEFORE any durable mutation. The one input
    /// validation rule is shared with the retry path, so an invalid first
    /// commit can never append context while persisting a snapshot whose
    /// `request_context_ids` disagrees with what it just appended.
    ///
    /// The frozen authority is the ordered `MessageId` set: the snapshot
    /// does not duplicate request-scoped message bodies, so on the fresh
    /// path the supplied bodies are the truth being committed (there is no
    /// prior fact to disagree with). Same-ids/different-body is therefore a
    /// retry-path invariant, proved by
    /// `model_turn_start_retry_enforces_exact_ordered_context_equality`.
    #[test]
    fn model_turn_start_fresh_commit_rejects_context_snapshot_mismatch() {
        let a = user_message("a", "A");
        let ctx1 = user_message("ctx-1", "first context");
        let ctx2 = user_message("ctx-2", "second context");

        // A fresh start carrying the exact ordered context commits.
        let committed = store();
        committed.initialize(std::slice::from_ref(&a)).unwrap();
        let base_revision = committed.load_head().unwrap().revision;
        let exact = context_start_snapshot(
            base_revision.next().next(),
            vec![MessageId::new("ctx-1"), MessageId::new("ctx-2")],
        );
        committed
            .commit_model_turn_start(&[ctx1.clone(), ctx2.clone()], &exact, Utc::now())
            .expect("the exact ordered fresh start commits");

        // Every mismatched ordered id set is rejected up front: the Surface
        // never advances, the canonical Ledger never gains the context, and
        // no RequestSnapshot or ModelRequestStarted fact is committed.
        let mismatches: Vec<(Vec<MessageBlock>, Vec<MessageId>)> = vec![
            (vec![ctx1.clone(), ctx2.clone()], vec![]),
            (
                vec![ctx1.clone(), ctx2.clone()],
                vec![MessageId::new("ctx-1")],
            ),
            (
                vec![ctx1.clone(), ctx2.clone()],
                vec![MessageId::new("ctx-2"), MessageId::new("ctx-1")],
            ),
            (
                vec![ctx1.clone(), ctx2.clone()],
                vec![
                    MessageId::new("ctx-1"),
                    MessageId::new("ctx-2"),
                    MessageId::new("ctx-3"),
                ],
            ),
        ];
        for (supplied, frozen_ids) in mismatches {
            let store = store();
            store.initialize(std::slice::from_ref(&a)).unwrap();
            let revision = store.load_head().unwrap().revision;
            let snapshot = context_start_snapshot(revision.next(), frozen_ids);
            assert!(
                matches!(
                    store.commit_model_turn_start(&supplied, &snapshot, Utc::now()),
                    Err(ConversationStoreError::InvalidReference(_))
                ),
                "a fresh start whose supplied context differs from its frozen snapshot must be rejected"
            );
            assert_eq!(
                store.load_head().unwrap().revision,
                revision,
                "the Surface never advanced"
            );
            assert_eq!(
                store.load_canonical().unwrap().len(),
                1,
                "the canonical Ledger never gained the context"
            );
            assert!(
                store
                    .read_request_snapshots(None, 32)
                    .unwrap()
                    .snapshots
                    .is_empty(),
                "no RequestSnapshot was committed"
            );
            assert!(
                store.read_events(None, 32).unwrap().events.is_empty(),
                "no ModelRequestStarted fact was committed"
            );
        }
    }

    /// Issue #12 (M9b): a failure at any internal stage of the start
    /// transaction rolls back the request-scoped context, the snapshot, and
    /// the start fact together.
    #[test]
    fn model_turn_start_fault_at_each_stage_rolls_back_everything() {
        for fault in [
            RequestStartFaultOperation::BeforeContextAppend,
            RequestStartFaultOperation::AfterContextAppend,
            RequestStartFaultOperation::AfterSnapshotInsert,
            RequestStartFaultOperation::AfterEventInsert,
        ] {
            let store = store();
            let a = user_message("a", "A");
            store.initialize(std::slice::from_ref(&a)).unwrap();
            let base_revision = store.load_head().unwrap().revision;
            let context = user_message("ctx-1", "request-scoped context");
            let snapshot = RequestSnapshot::new(
                RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: TurnId::new("1"),
                    retry_number: 0,
                },
                base_revision.next(),
                "frozen".to_owned(),
                invocation(),
                1024,
                None,
                false,
                Vec::new(),
                crate::runtime::identity::CapabilityRevision::new(1),
                ContextGeneration {
                    id: 1,
                    contributors: Vec::new(),
                },
                None,
                vec![MessageId::new("ctx-1")],
            );
            store.arm_request_start_fault_script([fault]);
            assert!(
                store
                    .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
                    .is_err(),
                "{fault:?} fails the commit"
            );
            let head = store.load_head().unwrap();
            assert_eq!(
                head.revision, base_revision,
                "{fault:?}: the Surface never advanced"
            );
            assert_eq!(
                store.load_canonical().unwrap().len(),
                1,
                "{fault:?}: the request-scoped context never became canonical"
            );
            assert!(
                store
                    .read_request_snapshots(None, 32)
                    .unwrap()
                    .snapshots
                    .is_empty(),
                "{fault:?}: no snapshot exists"
            );
            assert!(
                store.read_events(None, 32).unwrap().events.is_empty(),
                "{fault:?}: no start fact exists"
            );
            // The store recovers: the identical start commits cleanly.
            store
                .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
                .expect("the identical start commits after the injected failure");
        }
    }

    #[test]
    fn request_snapshot_history_is_bounded_and_cursor_paged() {
        let store = store();
        let message = user_message("request-page-message", "request page");
        store.initialize(std::slice::from_ref(&message)).unwrap();
        let revision = store.load_head().unwrap().revision;

        for index in 0..7_u64 {
            let snapshot = RequestSnapshot::new(
                RequestIdentity {
                    attempt_id: AttemptId::new(format!("request-page-attempt-{index}")),
                    turn: TurnId::new("1"),
                    retry_number: 0,
                },
                revision,
                "frozen".to_owned(),
                invocation(),
                1024,
                None,
                false,
                Vec::new(),
                crate::runtime::identity::CapabilityRevision::new(1),
                ContextGeneration {
                    id: index,
                    contributors: Vec::new(),
                },
                None,
                Vec::new(),
            );
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .expect("persist request snapshot");
        }

        let empty = store.read_request_snapshots(None, 0).expect("empty page");
        assert!(empty.snapshots.is_empty());
        assert_eq!(empty.next_sequence, None);

        let page_one = store.read_request_snapshots(None, 3).expect("first page");
        let page_two = store
            .read_request_snapshots(page_one.next_sequence, 3)
            .expect("second page");
        let page_three = store
            .read_request_snapshots(page_two.next_sequence, 3)
            .expect("third page");
        let page_four = store
            .read_request_snapshots(page_three.next_sequence, 3)
            .expect("terminal empty page");

        assert_eq!(page_one.snapshots.len(), 3);
        assert_eq!(page_two.snapshots.len(), 3);
        assert_eq!(page_three.snapshots.len(), 1);
        assert_eq!(page_one.next_sequence, Some(3));
        assert_eq!(page_two.next_sequence, Some(6));
        assert_eq!(page_three.next_sequence, Some(7));
        assert!(page_four.snapshots.is_empty());
        assert_eq!(page_four.next_sequence, None);

        let pages = [page_one, page_two, page_three];
        let mut ids = Vec::new();
        for page in pages {
            for snapshot in page.snapshots {
                assert!(
                    !ids.contains(&snapshot.request_id),
                    "cursor pages must not repeat a Request Snapshot"
                );
                ids.push(snapshot.request_id);
            }
        }
        assert_eq!(ids.len(), 7);
        assert_eq!(
            store
                .load_request_snapshot(&ids[4])
                .expect("keyed snapshot lookup")
                .request_id,
            ids[4]
        );
    }

    #[test]
    fn background_terminal_publication_is_idempotent_and_terminal_unique() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let execution_id = crate::runtime::identity::ToolExecutionId::new("execution-1");
        let message_id = MessageId::new("background-notification-1");
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
        let notification = UserMessageBlock {
            id: message_id.clone(),
            content: draft("background").content,
            source: UserSource::Runtime,
            kind: InboundKind::Message,
            timestamp: Some(timestamp),
        };
        let event = envelope(
            &conversation_id,
            "background-event-1",
            None,
            RuntimeEvent::BackgroundTerminalPublished {
                execution_id: execution_id.clone(),
                message_id: message_id.clone(),
                state: crate::events::types::BackgroundTerminalState::Succeeded,
            },
        );
        let draft_for_store = || InboundDraft {
            message_id: Some(message_id.clone()),
            source: notification.source.clone(),
            kind: notification.kind.clone(),
            content: notification.content.clone(),
            timestamp,
            correlation: Some("background:execution-1".to_owned()),
        };
        let (accepted, persisted) = store
            .accept_inbound_with_event(draft_for_store(), event.clone())
            .unwrap();
        assert!(!accepted.retried);
        assert_eq!(persisted.sequence, 1);

        let (retried, persisted_retry) = store
            .accept_inbound_with_event(draft_for_store(), event)
            .unwrap();
        assert!(retried.retried);
        assert_eq!(persisted_retry.sequence, persisted.sequence);

        let second_message_id = MessageId::new("background-notification-2");
        let second_event = envelope(
            &conversation_id,
            "background-event-2",
            None,
            RuntimeEvent::BackgroundTerminalPublished {
                execution_id,
                message_id: second_message_id.clone(),
                state: crate::events::types::BackgroundTerminalState::Succeeded,
            },
        );
        let second_draft = InboundDraft {
            message_id: Some(second_message_id),
            source: UserSource::Runtime,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "second".to_owned(),
            })],
            timestamp,
            correlation: Some("background:execution-2".to_owned()),
        };
        assert!(matches!(
            store.accept_inbound_with_event(second_draft, second_event),
            Err(ConversationStoreError::TerminalViolation(_))
        ));
        assert_eq!(store.load_pending().unwrap().len(), 1);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn event_journal_enforces_dependencies_order_and_terminal_absorption() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let attempt_id = AttemptId::new("attempt-events");
        let started = store
            .append_event(envelope(
                &conversation_id,
                "attempt-started",
                Some(attempt_id.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                },
            ))
            .unwrap();
        assert_eq!(started.sequence, 1);

        assert!(matches!(
            store.append_event(envelope(
                &conversation_id,
                "attempt-identity-conflict",
                Some(attempt_id.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: AttemptId::new("different-attempt"),
                },
            )),
            Err(ConversationStoreError::InvalidReference(detail))
                if detail.contains("attempt identity")
        ));

        let mut turn_started = envelope(
            &conversation_id,
            "turn-started",
            Some(attempt_id.clone()),
            RuntimeEvent::TurnStarted,
        );
        turn_started.turn_id = Some(TurnId::new("turn-events"));
        assert_eq!(store.append_event(turn_started).unwrap().sequence, 2);

        let missing = envelope(
            &conversation_id,
            "missing-assistant",
            Some(attempt_id.clone()),
            RuntimeEvent::AssistantMessageCommitted {
                message_id: MessageId::new("assistant-missing"),
            },
        );
        assert!(matches!(
            store.append_event(missing),
            Err(ConversationStoreError::InvalidReference(_))
        ));

        let assistant = assistant_message("assistant-1", "provider body");
        let committed = store
            .append_canonical_with_event(
                &assistant,
                envelope(
                    &conversation_id,
                    "assistant-committed",
                    Some(attempt_id.clone()),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .unwrap();
        assert_eq!(committed.sequence, 3);
        assert_eq!(store.load_canonical().unwrap().len(), 1);

        let mut turn_completed = envelope(
            &conversation_id,
            "turn-completed",
            Some(attempt_id.clone()),
            RuntimeEvent::TurnCompleted,
        );
        turn_completed.turn_id = Some(TurnId::new("turn-events"));
        assert_eq!(store.append_event(turn_completed).unwrap().sequence, 4);
        assert!(matches!(
            store.append_event({
                let mut duplicate = envelope(
                    &conversation_id,
                    "turn-terminal-duplicate",
                    Some(attempt_id.clone()),
                    RuntimeEvent::TurnCompleted,
                );
                duplicate.turn_id = Some(TurnId::new("turn-events"));
                duplicate
            }),
            Err(ConversationStoreError::TerminalViolation(_))
        ));
        assert!(matches!(
            store.append_event({
                let mut late = envelope(
                    &conversation_id,
                    "after-turn-terminal",
                    Some(attempt_id.clone()),
                    RuntimeEvent::ModelRequestCompleted {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    },
                );
                late.turn_id = Some(TurnId::new("turn-events"));
                late
            }),
            Err(ConversationStoreError::TerminalViolation(_))
        ));

        let terminal = store
            .append_event(envelope(
                &conversation_id,
                "attempt-terminal",
                Some(attempt_id.clone()),
                RuntimeEvent::AttemptCompleted {
                    attempt_id: attempt_id.clone(),
                    finish_reason: ModelFinishReason::Stop,
                },
            ))
            .unwrap();
        assert_eq!(terminal.sequence, 5);
        assert!(matches!(
            store.append_event({
                let mut after = envelope(
                    &conversation_id,
                    "after-terminal",
                    Some(attempt_id),
                    RuntimeEvent::TurnCompleted,
                );
                after.turn_id = Some(TurnId::new("turn-events"));
                after
            }),
            Err(ConversationStoreError::TerminalViolation(_))
        ));

        store.arm_fail_event_times(1);
        assert!(
            store
                .append_event(envelope(
                    &conversation_id,
                    "standalone-event",
                    None,
                    RuntimeEvent::CompactionStarted,
                ))
                .is_err()
        );
        let standalone = store
            .append_event(envelope(
                &conversation_id,
                "standalone-event",
                None,
                RuntimeEvent::CompactionStarted,
            ))
            .unwrap();
        assert_eq!(standalone.sequence, 6);
        assert!(
            store
                .append_event(envelope(
                    &conversation_id,
                    "standalone-event",
                    None,
                    RuntimeEvent::CompactionStarted,
                ))
                .is_err()
        );
        let page = store.read_events(None, 2).unwrap();
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[0].sequence, 1);
        assert_eq!(page.next_sequence, Some(2));
        let rest = store.read_events(page.next_sequence, 10).unwrap();
        assert_eq!(
            rest.events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![3, 4, 5, 6]
        );
        let journal_json = serde_json::to_string(&rest.events).unwrap();
        assert!(!journal_json.contains("provider body"));
    }

    #[test]
    fn durable_hot_bootstrap_is_bounded_to_current_surface() {
        let store = store();
        let messages: Vec<_> = (0..100)
            .map(|index| user_message(&format!("m{index}"), "history"))
            .collect();
        store.initialize(&messages).unwrap();
        let revision = store.load_head().unwrap().revision;
        let summary = summary_message("summary", "bounded");
        let (revision, _, _) = store
            .commit_compaction(CompactionCommitInput {
                summary,
                span: SurfaceSpan::new(MessageId::new("m0"), MessageId::new("m98")),
                expected_revision: revision,
                tokens_before: TokenMeasurement {
                    input_tokens: 100,
                    source: TokenMeasurementSource::Estimated,
                },
                estimated_tokens_after: 2,
                attempt_id: None,
                turn_id: None,
                timestamp: Utc::now(),
            })
            .unwrap();
        let head = store.load_head().unwrap();
        let active = store.load_messages(&head.active_message_ids).unwrap();
        let state = ConversationState::from_durable_head(
            active,
            head.active_message_ids,
            revision,
            head.compaction_generation,
        )
        .unwrap();
        assert_eq!(state.ledger().len(), 2);
        assert_eq!(state.surface().len(), 2);
        assert_eq!(state.surface_access().history_enumerations(), 0);
        assert_eq!(state.ledger().access().enumerations(), 0);
        assert_eq!(
            store.load_canonical_page(None, 3).unwrap().messages.len(),
            3
        );
    }

    #[test]
    fn reopening_a_store_for_the_wrong_conversation_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let conversation_id = ConversationId::new("conversation-a");
        drop(SqliteConversationStore::open(conversation_id, &path).unwrap());

        assert!(matches!(
            SqliteConversationStore::open(ConversationId::new("conversation-b"), &path),
            Err(ConversationStoreError::ConversationIdMismatch { stored, requested })
                if stored == ConversationId::new("conversation-a")
                    && requested == ConversationId::new("conversation-b")
        ));
    }

    #[test]
    fn corrupt_request_surface_or_ledger_references_fail_closed() {
        let request_store = store();
        let message = user_message("request-message", "request body");
        request_store
            .initialize(std::slice::from_ref(&message))
            .unwrap();
        let snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("request-corruption-attempt"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            request_store.load_head().unwrap().revision,
            "frozen".to_owned(),
            invocation(),
            1024,
            None,
            false,
            Vec::new(),
            crate::runtime::identity::CapabilityRevision::new(1),
            ContextGeneration {
                id: 1,
                contributors: Vec::new(),
            },
            None,
            Vec::new(),
        );
        request_store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        {
            let connection = request_store.conn.lock().unwrap();
            let json: String = connection
                .query_row(
                    "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
                    [snapshot.request_id.as_str()],
                    |row| row.get(0),
                )
                .unwrap();
            let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
            value["surface_revision"] = serde_json::json!(999_u64);
            connection
                .execute(
                    "UPDATE request_snapshots SET surface_revision=999,snapshot_json=?1 WHERE request_id=?2",
                    params![value.to_string(), snapshot.request_id.as_str()],
                )
                .unwrap();
        }
        assert!(matches!(
            request_store.reconstruct_model_request(&snapshot.request_id),
            Err(ConversationStoreError::InvalidReference(detail))
                if detail.contains("newer than head")
        ));

        let missing_message_store = store();
        let missing_message = user_message("missing-request-message", "request body");
        missing_message_store
            .initialize(std::slice::from_ref(&missing_message))
            .unwrap();
        let missing_snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("missing-message-attempt"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            missing_message_store.load_head().unwrap().revision,
            "frozen".to_owned(),
            invocation(),
            1024,
            None,
            false,
            Vec::new(),
            crate::runtime::identity::CapabilityRevision::new(1),
            ContextGeneration {
                id: 1,
                contributors: Vec::new(),
            },
            None,
            Vec::new(),
        );
        missing_message_store
            .commit_model_turn_start(&[], &missing_snapshot, Utc::now())
            .unwrap();
        {
            let connection = missing_message_store.conn.lock().unwrap();
            let missing_message_id = crate::conversation::message_id_of(&missing_message);
            connection
                .execute(
                    "DELETE FROM message_ledger WHERE message_id=?1",
                    [missing_message_id.as_str()],
                )
                .unwrap();
        }
        assert!(matches!(
            missing_message_store.reconstruct_model_request(&missing_snapshot.request_id),
            Err(ConversationStoreError::InvalidReference(detail))
                if detail.contains("missing Ledger message")
        ));
    }

    #[test]
    fn incompatible_schema_fails_without_migration_or_repair() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("incompatible.sqlite");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE rustx_store (id INTEGER PRIMARY KEY, schema_version INTEGER NOT NULL, conversation_id TEXT NOT NULL, next_inbound_sequence INTEGER NOT NULL, next_event_sequence INTEGER NOT NULL); INSERT INTO rustx_store VALUES (1, 99, 'conv-1', 0, 0);",
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            SqliteConversationStore::open(ConversationId::new("conv-1"), &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 99,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    /// Issue #12 (M9b): a database written by the pre-M9b development schema
    /// (version 1, whose `RequestSnapshot` JSON lacks the required
    /// `request_context_ids` field) is rejected explicitly at store open with
    /// a typed `SchemaVersionMismatch` — never a later accidental JSON decode
    /// failure — and there is no migration, legacy reader, or compatibility
    /// mode.
    #[test]
    fn pre_m9b_schema_version_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-m9b.sqlite");
        let conversation_id = ConversationId::new("conv-pre-m9b");
        {
            // Build a fully-shaped store at the current schema, then downgrade
            // only the schema version to the pre-M9b value: the table shape is
            // unchanged, but the serialized `RequestSnapshot` format gained a
            // required field, which is exactly why the version must gate open.
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute("UPDATE rustx_store SET schema_version = 1 WHERE id = 1", [])
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 1,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }
}
