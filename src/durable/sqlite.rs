//! `SQLite` implementation of the semantic conversation durability contract.
//!
//! One database contains the deliberately separate authority domains:
//! Pending Inbound, the append-only Message Ledger, immutable Surface
//! operations, immutable Request Snapshots, the append-only Event Journal, and
//! the durable publication plane (Issue #108). A narrow transcript ordering
//! spine stores references only; it is not a body store or a second history.
//! The tables share transactions where rustX needs one semantic linearization
//! point, but no table is a serialized `ConversationRecord` or transcript.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};

use crate::conversation::{SurfaceOp, SurfaceRevision, SurfaceSpan};
use crate::events::interaction::{
    InteractionSubject, interaction_arguments_digest, validate_interaction_settlement,
    validate_interaction_subject,
};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::types::{
    AssistantContentBlock, ContentBlockIndex, InboundKind, MessageBlock, UserMessageBlock,
    UserSource,
};
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelRequest;
use crate::publication::{
    PublicationAudit, PublicationAuditBlock, PublicationFrame, PublicationPayload,
    PublicationSettlement, PublicationStreamRecord, PublicationStreamStart,
    consolidate_audit_content,
};
use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, EventId, InteractionId, MessageId, PublicationStreamId,
    RequestId, ToolCallId, ToolId, TurnId,
};
use crate::runtime::inbound::InboundSequence;

use super::inbox::{
    AcceptedInbound, CanonicalMessagePage, CompactionCommitInput, ConversationStore,
    ConversationStoreError, DurableConversationHead, EventPage, InboundDraft, PendingBatch,
    PendingInboundItem, RequestSnapshotPage, SurfaceUserMessageBoundary,
    SurfaceUserMessageBoundaryPage, TRANSCRIPT_PAGE_LIMIT_MAX, TranscriptCommitReceipt,
    TranscriptCursor, TranscriptEntry, TranscriptItem, TranscriptPage,
};

/// The only schema accepted by this pre-production store. Incompatible
/// databases fail explicitly; there is no migration or legacy reader.
///
/// Version 3 froze the Issue #106 durable format change: canonical System
/// messages were removed and `RequestSnapshot` gained exact ordered System
/// Sections plus the process-local resource revision.
///
/// Version 4 froze the Issue #108 durable publication plane: the
/// `publication_streams`, `publication_frames`, `publication_proposals`, and
/// `publication_audits` tables, the `request_snapshots.completed_sequence`
/// P-marker column, and the `ModelRequestCompleted` / `ModelRequestFailed`
/// payload change that names the exact request.
///
/// Version 5 froze the store-enforced generation and proposal ownership
/// rules: a Request Snapshot carries its provisional Assistant identity, and
/// publication proposals are owned by `(stream_id, call_id)` because provider
/// `ToolCallIds` are request/publication-scoped rather than conversation-global.
///
/// Version 6 completes that ownership contract. Every proposal row now stores
/// its frozen block/tool/name identity and its explicit `started` or
/// `completed` staging state.
///
/// Version 7 froze the Issue #109 durable interaction audit: the
/// `InteractionRequested` / `InteractionSettled` Journal vocabulary and the
/// `interaction:{id}` lifecycle domain that makes a settlement exactly-once
/// and forbids a settled fact without its requested fact. The pair is pinned
/// to one conversation + attempt + turn envelope; an Approval subject must
/// describe the canonical Assistant `ToolCall` of the exact generation its
/// envelope names, resolved through the v5/v6 `(stream_id, call_id)` proposal
/// ownership and the stream's frozen attempt/turn/message identity, and that
/// owning message must still be on the active Surface; payload bounds are
/// store invariants; and a Question settlement must satisfy the exact
/// requested Question. No new table or column was needed — the generation
/// proof reuses the retained publication ownership v5 and v6 introduced.
/// Version 8 froze the Issue #110 derived transcript ordering spine: accepted
/// inbound, visible canonical messages, publication audits, and interaction
/// audit facts receive one durable reference position. Bodies remain owned by
/// Pending Inbound, the Message Ledger, the publication plane, or the Event
/// Journal. A v3/v4/v5/v6/v7 database must fail at store open; there is no
/// migration or compatibility path.
pub const SQLITE_SCHEMA_VERSION: i64 = 8;

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
    #[cfg(test)]
    pub(crate) fail_publication_frames_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_publication_terminal_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_publication_audit_remaining: Arc<AtomicUsize>,
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
            #[cfg(test)]
            fail_publication_frames_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            fail_publication_terminal_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            fail_publication_audit_remaining: Arc::new(AtomicUsize::new(0)),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, ConversationStoreError> {
        self.conn
            .lock()
            .map_err(|_| storage("the conversation store connection is poisoned"))
    }

    /// Fails the next `count` publication staging commits.
    #[cfg(test)]
    pub(crate) fn arm_fail_publication_frames_times(&self, count: usize) {
        self.fail_publication_frames_remaining
            .store(count, Ordering::SeqCst);
    }

    /// Fails the next `count` publication terminal (U) commits.
    #[cfg(test)]
    pub(crate) fn arm_fail_publication_terminal_times(&self, count: usize) {
        self.fail_publication_terminal_remaining
            .store(count, Ordering::SeqCst);
    }

    /// Fails the next `count` publication audit terminalizations.
    #[cfg(test)]
    pub(crate) fn arm_fail_publication_audit_times(&self, count: usize) {
        self.fail_publication_audit_remaining
            .store(count, Ordering::SeqCst);
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
            if transcript_visible_message(message) {
                return Err(ConversationStoreError::InvalidReference(
                    "request-scoped context must use a hidden Context-kind User message".to_owned(),
                ));
            }
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
                    seq_to_i64(persisted.event.sequence)?,
                    snapshot.request_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("bind request start sequence: {error}")))?;
        Ok(persisted.event)
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
        Ok((accepted, persisted.event))
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
            let cursor =
                append_adopted_message_and_surface(&transaction, &block, &item.message_id)?;
            if cursor != item.transcript_cursor {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "adopted inbound {} changed its transcript cursor from {:?} to {:?}",
                    item.message_id, item.transcript_cursor, cursor
                )));
            }
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

    fn load_bootstrap_history(&self) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let connection = self.lock()?;
        let count: Option<i64> = connection
            .query_row(
                "SELECT message_count FROM bootstrap_identity WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("bootstrap history count: {error}")))?;
        let Some(count) = count else {
            // A low-level non-session composition may be opening a fresh
            // store. ConversationRuntime::initialize will establish the
            // empty bootstrap identity; no historical seed exists yet.
            return Ok(Vec::new());
        };
        let mut statement = connection
            .prepare(
                "SELECT message_json FROM message_ledger
                 ORDER BY position LIMIT ?1",
            )
            .map_err(|error| storage(format!("bootstrap history query: {error}")))?;
        let rows = statement
            .query_map([count], |row| row.get::<_, String>(0))
            .map_err(|error| storage(format!("bootstrap history rows: {error}")))?;
        rows.map(|row| {
            let json = row.map_err(|error| storage(format!("bootstrap history row: {error}")))?;
            decode(&json, "bootstrap history")
        })
        .collect()
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

    fn load_surface_snapshot(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        let connection = self.lock()?;
        let ids = reconstruct_surface(&connection, revision)?;
        ids.iter().map(|id| load_message(&connection, id)).collect()
    }

    fn load_user_message_boundaries(
        &self,
        through: SurfaceRevision,
    ) -> Result<Vec<SurfaceUserMessageBoundary>, ConversationStoreError> {
        let connection = self.lock()?;
        load_user_message_boundaries(&connection, through)
    }

    fn load_user_message_boundaries_page(
        &self,
        through: SurfaceRevision,
        offset: usize,
        limit: usize,
    ) -> Result<SurfaceUserMessageBoundaryPage, ConversationStoreError> {
        let connection = self.lock()?;
        load_user_message_boundaries_page(&connection, through, offset, limit)
    }

    fn append_canonical(
        &self,
        message: &MessageBlock,
    ) -> Result<TranscriptCommitReceipt, ConversationStoreError> {
        let mut receipts = append_canonical_messages(self, std::slice::from_ref(message))?;
        Ok(receipts.remove(0))
    }

    fn append_canonical_batch(
        &self,
        messages: &[MessageBlock],
    ) -> Result<Vec<TranscriptCommitReceipt>, ConversationStoreError> {
        append_canonical_messages(self, messages)
    }

    fn append_canonical_with_event(
        &self,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCommitReceipt), ConversationStoreError> {
        validate_canonical_event_for_message(message, &event.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical event transaction: {error}")))?;
        ensure_surface_head(&transaction)?;
        let transcript_cursor = append_message_and_surface(&transaction, message)?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical event journal commit"));
        }
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical event commit: {error}")))?;
        Ok((
            persisted.event,
            TranscriptCommitReceipt { transcript_cursor },
        ))
    }

    fn append_canonical_batch_with_events(
        &self,
        messages: &[MessageBlock],
        events: &[RuntimeEventEnvelope],
    ) -> Result<(Vec<RuntimeEventEnvelope>, Vec<TranscriptCommitReceipt>), ConversationStoreError>
    {
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
        let mut receipts = Vec::with_capacity(messages.len());
        for message in messages {
            receipts.push(TranscriptCommitReceipt {
                transcript_cursor: append_message_and_surface(&transaction, message)?,
            });
        }
        let mut persisted = Vec::with_capacity(events.len());
        for event in events {
            #[cfg(test)]
            if Self::consume(&self.fail_event_remaining) {
                return Err(storage(
                    "fault injected: canonical batch event journal commit",
                ));
            }
            persisted
                .push(persist_event_tx(&transaction, &self.conversation_id, event.clone())?.event);
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical event batch commit: {error}")))?;
        Ok((persisted, receipts))
    }

    fn commit_compaction(
        &self,
        input: CompactionCommitInput,
    ) -> Result<
        (SurfaceRevision, u64, RuntimeEventEnvelope, TranscriptCursor),
        ConversationStoreError,
    > {
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
        let summary_cursor = append_message_ledger(
            &transaction,
            &MessageBlock::User(input.summary.clone()),
            None,
        )?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "a compaction summary must receive a transcript cursor".to_owned(),
            )
        })?;
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
        Ok((revision, generation, persisted.event, summary_cursor))
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

    fn load_transcript_page(
        &self,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<TranscriptPage, ConversationStoreError> {
        let connection = self.lock()?;
        load_transcript_page(&connection, before, limit)
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
        if event.conversation_id != self.conversation_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "request snapshot {request_id} start event belongs to a foreign conversation"
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
        Ok(persisted.event)
    }

    fn append_interaction_audit(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
        if !matches!(
            event.event,
            RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
        ) {
            return Err(ConversationStoreError::InvalidReference(
                "the interaction audit transition accepts only interaction facts".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("interaction audit transaction: {error}")))?;
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        let transcript_cursor = persisted.transcript_cursor.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "interaction audit did not receive a transcript cursor".to_owned(),
            )
        })?;
        transaction
            .commit()
            .map_err(|error| storage(format!("interaction audit commit: {error}")))?;
        Ok((persisted.event, transcript_cursor))
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

    fn open_publication_stream(
        &self,
        start: &PublicationStreamStart,
    ) -> Result<(), ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication open transaction: {error}")))?;
        // The first open is the publication generation admission boundary.
        // Prove the complete Request Snapshot/start-event identity before an
        // idempotent reopen probe or any publication row can be written.
        let (snapshot, _) =
            require_started_request_tx(&transaction, &self.conversation_id, &start.request_id)?;
        validate_publication_generation(&snapshot, start)?;
        if let Some(existing) = read_publication_stream(&transaction, &start.stream_id)? {
            // Re-opening is idempotent only for the identical frozen identity:
            // a stream may never be re-associated with another request,
            // attempt, turn, or provisional message.
            if existing.start != *start {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "publication stream {} is already open under a different request generation",
                    start.stream_id
                )));
            }
            if existing.settlement.is_some() {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "publication stream {} already settled and cannot reopen",
                    start.stream_id
                )));
            }
            return Ok(());
        }
        transaction
            .execute(
                "INSERT INTO publication_streams(stream_id,attempt_id,turn_id,request_id,message_id,next_frame_sequence,terminal_sequence,settlement)
                 VALUES(?1,?2,?3,?4,?5,0,NULL,NULL)",
                params![
                    start.stream_id.as_str(),
                    start.attempt_id.as_str(),
                    start.turn_id.as_str(),
                    start.request_id.as_str(),
                    start.message_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("open publication stream: {error}")))?;
        transaction
            .commit()
            .map_err(|error| storage(format!("publication open commit: {error}")))?;
        Ok(())
    }

    fn stage_publication_frames(
        &self,
        frames: &[PublicationFrame],
    ) -> Result<(), ConversationStoreError> {
        if frames.is_empty() {
            return Err(ConversationStoreError::PublicationViolation(
                "a publication staging transaction must carry at least one frame".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication staging transaction: {error}")))?;
        #[cfg(test)]
        if Self::consume(&self.fail_publication_frames_remaining) {
            return Err(storage("fault injected: publication staging commit"));
        }
        let stream = stage_frames_tx(&transaction, frames)?;
        if stream.terminal_sequence.is_some() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {} already reached its terminal boundary",
                stream.start.stream_id
            )));
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("publication staging commit: {error}")))?;
        Ok(())
    }

    fn commit_publication_terminal(
        &self,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<(), ConversationStoreError> {
        if frames.is_empty() {
            return Err(ConversationStoreError::PublicationViolation(
                "the publication terminal transaction must carry a final frame".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication terminal transaction: {error}")))?;
        #[cfg(test)]
        if Self::consume(&self.fail_publication_terminal_remaining) {
            return Err(storage("fault injected: publication terminal commit"));
        }
        let stream = require_publication_stream(&transaction, stream_id)?;
        let (snapshot, _) = require_started_request_tx(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        validate_publication_generation(&snapshot, &stream.start)?;
        // U may never precede P: prove the exact provider outcome before the
        // frame/proposal staging transaction does any durable work.
        if !request_outcome_is_durable(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )? {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication terminal for {stream_id} has no durable provider outcome for request {}",
                stream.start.request_id
            )));
        }
        let stream = stage_frames_tx(&transaction, frames)?;
        if stream.start.stream_id != *stream_id {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "terminal frames belong to publication stream {}, not {stream_id}",
                stream.start.stream_id
            )));
        }
        if stream.terminal_sequence.is_some() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} already committed its terminal boundary"
            )));
        }
        let terminal_sequence = frames
            .last()
            .map(|frame| frame.sequence)
            .ok_or_else(|| storage("terminal frames are empty"))?;
        // The final frame and the terminal marker share this one transaction.
        transaction
            .execute(
                "UPDATE publication_streams SET terminal_sequence=?1 WHERE stream_id=?2",
                params![seq_to_i64(terminal_sequence)?, stream_id.as_str()],
            )
            .map_err(|error| storage(format!("commit publication terminal: {error}")))?;
        transaction
            .commit()
            .map_err(|error| storage(format!("publication terminal commit: {error}")))?;
        Ok(())
    }

    fn commit_canonical_publication(
        &self,
        stream_id: &PublicationStreamId,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCursor), ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical publication transaction: {error}")))?;
        let stream = require_publication_stream(&transaction, stream_id)?;
        let (snapshot, _) = require_started_request_tx(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        validate_publication_generation(&snapshot, &stream.start)?;
        validate_canonical_publication_event(
            &self.conversation_id,
            &stream.start,
            message,
            &event,
        )?;
        validate_canonical_tool_proposals(&transaction, stream_id, message)?;
        if let Some(settlement) = stream.settlement {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} already settled as {}; canonical acceptance is permanently forbidden",
                settlement.as_str()
            )));
        }
        // C may never precede U for a published stream.
        if stream.terminal_sequence.is_none() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} has no durable publication terminal; canonical acceptance requires U"
            )));
        }
        if !matches!(message, MessageBlock::Assistant(_)) {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "canonical acceptance of publication stream {stream_id} requires an Assistant message"
            )));
        }
        if !request_outcome_is_durable(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )? {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "canonical acceptance of publication stream {stream_id} has no durable provider outcome for request {}",
                stream.start.request_id
            )));
        }
        ensure_surface_head(&transaction)?;
        let transcript_cursor =
            append_message_and_surface(&transaction, message)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "canonical publication must receive a transcript cursor".to_owned(),
                )
            })?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical publication commit"));
        }
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        // The canonical transition clears the lifecycle staging of the stream:
        // the Ledger is now the long-term authority and no transient frame
        // survives it.
        clear_publication_staging(&transaction, stream_id)?;
        // Proposal ownership remains as one bounded `(stream_id, call_id)`
        // fact after C. Recovery ToolResult repair and later Tool Plane
        // transitions must still resolve the exact accepted proposal rather
        // than falling back to a bare call id.
        transaction
            .execute(
                "UPDATE publication_proposals SET settlement=?1 WHERE stream_id=?2",
                params![
                    PublicationSettlement::Canonical.as_str(),
                    stream_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("settle canonical publication proposals: {error}")))?;
        transaction
            .execute(
                "UPDATE publication_streams SET settlement=?1 WHERE stream_id=?2",
                params![
                    PublicationSettlement::Canonical.as_str(),
                    stream_id.as_str()
                ],
            )
            .map_err(|error| storage(format!("settle canonical publication: {error}")))?;
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical publication commit: {error}")))?;
        Ok((persisted.event, transcript_cursor))
    }

    fn terminalize_publication_audit(
        &self,
        stream_id: &PublicationStreamId,
        timestamp: DateTime<Utc>,
    ) -> Result<(PublicationAudit, TranscriptCursor), ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication audit transaction: {error}")))?;
        #[cfg(test)]
        if Self::consume(&self.fail_publication_audit_remaining) {
            return Err(storage("fault injected: publication audit commit"));
        }
        let stream = require_publication_stream(&transaction, stream_id)?;
        let (snapshot, _) = require_started_request_tx(
            &transaction,
            &self.conversation_id,
            &stream.start.request_id,
        )?;
        validate_publication_generation(&snapshot, &stream.start)?;
        if let Some(settlement) = stream.settlement {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} already settled as {}",
                settlement.as_str()
            )));
        }
        if stream.terminal_sequence.is_some()
            && !request_outcome_is_durable(
                &transaction,
                &self.conversation_id,
                &stream.start.request_id,
            )?
        {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication audit for {stream_id} has U without the exact successful provider outcome"
            )));
        }
        // The kind is derived from durable evidence, never supplied: U
        // present means the released output was complete but never accepted;
        // U absent means publication never reached its own terminal boundary.
        let kind = stream.audit_kind();
        let executed: Option<String> = transaction
            .query_row(
                "SELECT call_id FROM publication_proposals WHERE stream_id=?1 AND executed=1 LIMIT 1",
                [stream_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("publication proposal execution probe: {error}")))?;
        if let Some(call_id) = executed {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {stream_id} cannot terminalize as an audit: proposal {call_id} already has a Tool Plane execution fact"
            )));
        }
        let frames = read_publication_frames(&transaction, stream_id)?;
        let content = consolidate_audit_content(&frames);
        validate_audit_proposal_ownership(&transaction, stream_id, &content)?;
        let audit = PublicationAudit {
            stream_id: stream_id.clone(),
            attempt_id: stream.start.attempt_id.clone(),
            turn_id: stream.start.turn_id.clone(),
            request_id: stream.start.request_id.clone(),
            message_id: stream.start.message_id.clone(),
            kind,
            content,
            settled_at: timestamp,
        };
        let settlement = PublicationSettlement::from(kind);
        transaction
            .execute(
                "INSERT INTO publication_audits(stream_id,audit_json) VALUES(?1,?2)",
                params![stream_id.as_str(), encode(&audit, "publication audit")?],
            )
            .map_err(|error| storage(format!("insert publication audit: {error}")))?;
        let transcript_cursor =
            append_transcript_reference(&transaction, "publication_audit", stream_id.as_str())?;
        // Consolidation replaces the transient frames: the durable footprint
        // of a settled audit is one bounded object, never O(frames) rows.
        clear_publication_staging(&transaction, stream_id)?;
        transaction
            .execute(
                "UPDATE publication_proposals SET settlement=?1 WHERE stream_id=?2",
                params![settlement.as_str(), stream_id.as_str()],
            )
            .map_err(|error| storage(format!("ban audited tool proposals: {error}")))?;
        transaction
            .execute(
                "UPDATE publication_streams SET settlement=?1 WHERE stream_id=?2",
                params![settlement.as_str(), stream_id.as_str()],
            )
            .map_err(|error| storage(format!("settle publication audit: {error}")))?;
        transaction
            .commit()
            .map_err(|error| storage(format!("publication audit commit: {error}")))?;
        Ok((audit, transcript_cursor))
    }

    fn load_unsettled_publication_streams(
        &self,
    ) -> Result<Vec<PublicationStreamRecord>, ConversationStoreError> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT stream_id,attempt_id,turn_id,request_id,message_id,terminal_sequence,settlement
                 FROM publication_streams WHERE settlement IS NULL ORDER BY stream_id",
            )
            .map_err(|error| storage(format!("unsettled publication query: {error}")))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|error| storage(format!("unsettled publication rows: {error}")))?;
        let mut records = Vec::new();
        for row in rows {
            let row =
                row.map_err(|error| storage(format!("unsettled publication row: {error}")))?;
            records.push(publication_record(row)?);
        }
        Ok(records)
    }

    fn load_publication_audit(
        &self,
        stream_id: &PublicationStreamId,
    ) -> Result<Option<PublicationAudit>, ConversationStoreError> {
        let connection = self.lock()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT audit_json FROM publication_audits WHERE stream_id=?1",
                [stream_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(format!("publication audit read: {error}")))?;
        json.map(|json| decode(&json, "publication audit"))
            .transpose()
    }
}

/// Reads one publication stream record, when it exists.
fn read_publication_stream(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
) -> Result<Option<PublicationStreamRecord>, ConversationStoreError> {
    let row = transaction
        .query_row(
            "SELECT stream_id,attempt_id,turn_id,request_id,message_id,terminal_sequence,settlement
             FROM publication_streams WHERE stream_id=?1",
            [stream_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| storage(format!("publication stream read: {error}")))?;
    row.map(publication_record).transpose()
}

fn require_publication_stream(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
) -> Result<PublicationStreamRecord, ConversationStoreError> {
    read_publication_stream(transaction, stream_id)?.ok_or_else(|| {
        ConversationStoreError::PublicationViolation(format!(
            "publication stream {stream_id} was never opened"
        ))
    })
}

type PublicationStreamRow = (
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    Option<String>,
);

fn publication_record(
    row: PublicationStreamRow,
) -> Result<PublicationStreamRecord, ConversationStoreError> {
    let (stream_id, attempt_id, turn_id, request_id, message_id, terminal_sequence, settlement) =
        row;
    let settlement = settlement
        .map(|value| {
            PublicationSettlement::parse(&value).ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "publication stream {stream_id} has an unknown settlement {value}"
                ))
            })
        })
        .transpose()?;
    let terminal_sequence = terminal_sequence
        .map(|value| nonnegative(value, "publication terminal sequence"))
        .transpose()?;
    Ok(PublicationStreamRecord {
        start: PublicationStreamStart {
            stream_id: PublicationStreamId::new(stream_id),
            attempt_id: AttemptId::new(attempt_id),
            turn_id: TurnId::new(turn_id),
            request_id: RequestId::new(request_id),
            message_id: MessageId::new(message_id),
        },
        terminal_sequence,
        settlement,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationProposalState {
    Started,
    Completed,
}

impl PublicationProposalState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
        }
    }

    fn parse(value: &str) -> Result<Self, ConversationStoreError> {
        match value {
            "started" => Ok(Self::Started),
            "completed" => Ok(Self::Completed),
            other => Err(ConversationStoreError::InvalidReference(format!(
                "publication proposal has unknown state {other}"
            ))),
        }
    }
}

/// The store-owned identity and assembly state of one staged proposal.
///
/// `persisted_state` is `None` for a proposal first created by the current
/// transaction. Keeping it alongside the working state lets one transaction
/// stage `Started` and `Completed` together without inserting an intermediate
/// row or losing the exact state transition.
#[derive(Debug, Clone)]
struct PublicationProposalOwner {
    call_id: ToolCallId,
    block_index: ContentBlockIndex,
    tool_id: ToolId,
    name: String,
    state: PublicationProposalState,
    persisted_state: Option<PublicationProposalState>,
}

fn load_publication_proposal_owners(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
) -> Result<Vec<PublicationProposalOwner>, ConversationStoreError> {
    let mut statement = transaction
        .prepare(
            "SELECT call_id,block_index,tool_id,tool_name,state
             FROM publication_proposals WHERE stream_id=?1 ORDER BY call_id",
        )
        .map_err(|error| storage(format!("publication proposal owner query: {error}")))?;
    let rows = statement
        .query_map([stream_id.as_str()], |row| {
            let block_index: i64 = row.get(1)?;
            let state: String = row.get(4)?;
            Ok((
                row.get::<_, String>(0)?,
                block_index,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                state,
            ))
        })
        .map_err(|error| storage(format!("publication proposal owner rows: {error}")))?;
    rows.map(|row| {
        let (call_id, block_index, tool_id, name, state) =
            row.map_err(|error| storage(format!("publication proposal owner row: {error}")))?;
        let block_index = u32::try_from(block_index).map_err(|_| {
            ConversationStoreError::InvalidReference(
                "publication proposal block index is invalid".to_owned(),
            )
        })?;
        let state = PublicationProposalState::parse(&state)?;
        Ok(PublicationProposalOwner {
            call_id: ToolCallId::new(call_id),
            block_index: ContentBlockIndex::new(block_index),
            tool_id: ToolId::new(tool_id),
            name,
            state,
            persisted_state: Some(state),
        })
    })
    .collect()
}

fn proposal_owner<'a>(
    owners: &'a mut [PublicationProposalOwner],
    call_id: &ToolCallId,
) -> Option<&'a mut PublicationProposalOwner> {
    owners.iter_mut().find(|owner| owner.call_id == *call_id)
}

fn proposal_violation(
    stream_id: &PublicationStreamId,
    detail: impl Into<String>,
) -> ConversationStoreError {
    ConversationStoreError::PublicationViolation(format!(
        "publication stream {stream_id} proposal violation: {}",
        detail.into()
    ))
}

fn require_started_proposal<'a>(
    owners: &'a mut [PublicationProposalOwner],
    stream_id: &PublicationStreamId,
    call_id: &ToolCallId,
) -> Result<&'a mut PublicationProposalOwner, ConversationStoreError> {
    let owner = proposal_owner(owners, call_id).ok_or_else(|| {
        proposal_violation(
            stream_id,
            format!("tool proposal {call_id} has no Started frame"),
        )
    })?;
    if owner.state == PublicationProposalState::Completed {
        return Err(proposal_violation(
            stream_id,
            format!("tool proposal {call_id} is already completed"),
        ));
    }
    Ok(owner)
}

fn validate_proposal_frames(
    stream_id: &PublicationStreamId,
    frames: &[PublicationFrame],
    owners: &mut Vec<PublicationProposalOwner>,
) -> Result<(), ConversationStoreError> {
    for frame in frames {
        match &frame.payload {
            PublicationPayload::ProposedToolCallStarted { block_index, call } => {
                if proposal_owner(owners, &call.id).is_some() {
                    return Err(proposal_violation(
                        stream_id,
                        format!("tool proposal {} already has a Started frame", call.id),
                    ));
                }
                owners.push(PublicationProposalOwner {
                    call_id: call.id.clone(),
                    block_index: *block_index,
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                    state: PublicationProposalState::Started,
                    persisted_state: None,
                });
            }
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index,
                call_id,
                ..
            } => {
                let owner = require_started_proposal(owners, stream_id, call_id)?;
                if owner.block_index != *block_index {
                    return Err(proposal_violation(
                        stream_id,
                        format!(
                            "tool proposal {call_id} arguments use block {block_index}, but block {} is frozen",
                            owner.block_index
                        ),
                    ));
                }
            }
            PublicationPayload::ProposedToolCallCompleted { block_index, call } => {
                let owner = require_started_proposal(owners, stream_id, &call.id)?;
                if owner.block_index != *block_index {
                    return Err(proposal_violation(
                        stream_id,
                        format!(
                            "tool proposal {} completes block {block_index}, but block {} is frozen",
                            call.id, owner.block_index
                        ),
                    ));
                }
                if owner.tool_id != call.tool_id {
                    return Err(proposal_violation(
                        stream_id,
                        format!("tool proposal {} completion changes tool id", call.id),
                    ));
                }
                if owner.name != call.name {
                    return Err(proposal_violation(
                        stream_id,
                        format!("tool proposal {} completion changes tool name", call.id),
                    ));
                }
                owner.state = PublicationProposalState::Completed;
            }
            PublicationPayload::TextSuffix { .. }
            | PublicationPayload::ReasoningSuffix { .. }
            | PublicationPayload::RefusalSuffix { .. }
            | PublicationPayload::TerminalOnly => {}
        }
    }
    Ok(())
}

/// Stages one contiguous run of frames onto an open unsettled stream and
/// returns the stream as it was before the staging.
fn stage_frames_tx(
    transaction: &Transaction<'_>,
    frames: &[PublicationFrame],
) -> Result<PublicationStreamRecord, ConversationStoreError> {
    let stream_id = &frames[0].stream_id;
    if frames.iter().any(|frame| frame.stream_id != *stream_id) {
        return Err(ConversationStoreError::PublicationViolation(
            "one publication transaction may only stage frames of one stream".to_owned(),
        ));
    }
    let stream = require_publication_stream(transaction, stream_id)?;
    if let Some(settlement) = stream.settlement {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "publication stream {stream_id} already settled as {}",
            settlement.as_str()
        )));
    }
    let mut next: i64 = transaction
        .query_row(
            "SELECT next_frame_sequence FROM publication_streams WHERE stream_id=?1",
            [stream_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("publication sequence read: {error}")))?;
    let mut owners = load_publication_proposal_owners(transaction, stream_id)?;

    // Preflight the complete transaction before inserting a frame or changing
    // an ownership row. In particular, a suffix/completion cannot become an
    // orphaned audit proposal merely because it appeared in a later batch.
    for frame in frames {
        if frame.message_id != stream.start.message_id {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication frame of {stream_id} names a foreign message identity"
            )));
        }
        let expected = nonnegative(next, "publication frame sequence")?;
        if frame.sequence != expected {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication frame of {stream_id} is sequence {} where {expected} was required",
                frame.sequence
            )));
        }
        next = next
            .checked_add(1)
            .ok_or_else(|| storage("publication frame sequence overflow"))?;
    }
    validate_proposal_frames(stream_id, frames, &mut owners)?;

    for frame in frames {
        transaction
            .execute(
                "INSERT INTO publication_frames(stream_id,sequence,frame_json) VALUES(?1,?2,?3)",
                params![
                    stream_id.as_str(),
                    seq_to_i64(frame.sequence)?,
                    encode(frame, "publication frame")?
                ],
            )
            .map_err(|error| storage(format!("stage publication frame: {error}")))?;
    }
    for owner in owners {
        match owner.persisted_state {
            None => {
                transaction
                    .execute(
                        "INSERT INTO publication_proposals(stream_id,call_id,block_index,tool_id,tool_name,state,executed,settlement)
                         VALUES(?1,?2,?3,?4,?5,?6,0,NULL)",
                        params![
                            stream_id.as_str(),
                            owner.call_id.as_str(),
                            i64::from(owner.block_index.get()),
                            owner.tool_id.as_str(),
                            owner.name,
                            owner.state.as_str(),
                        ],
                    )
                    .map_err(|error| storage(format!("register tool proposal: {error}")))?;
            }
            Some(previous) if previous != owner.state => {
                transaction
                    .execute(
                        "UPDATE publication_proposals SET state=?1 WHERE stream_id=?2 AND call_id=?3",
                        params![owner.state.as_str(), stream_id.as_str(), owner.call_id.as_str()],
                    )
                    .map_err(|error| storage(format!("advance tool proposal state: {error}")))?;
            }
            Some(_) => {}
        }
    }
    transaction
        .execute(
            "UPDATE publication_streams SET next_frame_sequence=?1 WHERE stream_id=?2",
            params![next, stream_id.as_str()],
        )
        .map_err(|error| storage(format!("advance publication sequence: {error}")))?;
    Ok(stream)
}

fn read_publication_frames(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
) -> Result<Vec<PublicationFrame>, ConversationStoreError> {
    let mut statement = transaction
        .prepare("SELECT frame_json FROM publication_frames WHERE stream_id=?1 ORDER BY sequence")
        .map_err(|error| storage(format!("publication frame query: {error}")))?;
    let rows = statement
        .query_map([stream_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(|error| storage(format!("publication frame rows: {error}")))?;
    let mut frames = Vec::new();
    for row in rows {
        let json = row.map_err(|error| storage(format!("publication frame row: {error}")))?;
        frames.push(decode(&json, "publication frame")?);
    }
    Ok(frames)
}

/// Proves that every proposal materialized into an audit is backed by the
/// stream-local durable owner created by the staging state machine.
fn validate_audit_proposal_ownership(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
    content: &[PublicationAuditBlock],
) -> Result<(), ConversationStoreError> {
    for block in content {
        let PublicationAuditBlock::ProposedToolCall {
            block_index,
            call_id,
            tool_id,
            name,
            complete,
            ..
        } = block
        else {
            continue;
        };
        let owner: Option<(i64, String, String, String)> = transaction
            .query_row(
                "SELECT block_index,tool_id,tool_name,state
                 FROM publication_proposals WHERE stream_id=?1 AND call_id=?2",
                params![stream_id.as_str(), call_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| storage(format!("audit proposal owner probe: {error}")))?;
        let Some((stored_block_index, stored_tool_id, stored_name, stored_state)) = owner else {
            return Err(proposal_violation(
                stream_id,
                format!("audit proposal {call_id} has no durable Started owner"),
            ));
        };
        let stored_block_index = u32::try_from(stored_block_index).map_err(|_| {
            ConversationStoreError::InvalidReference(
                "publication proposal block index is invalid".to_owned(),
            )
        })?;
        let stored_state = PublicationProposalState::parse(&stored_state)?;
        if ContentBlockIndex::new(stored_block_index) != *block_index
            || stored_tool_id != tool_id.as_str()
            || stored_name != *name
        {
            return Err(proposal_violation(
                stream_id,
                format!("audit proposal {call_id} disagrees with its frozen owner identity"),
            ));
        }
        let expected_state = if *complete {
            PublicationProposalState::Completed
        } else {
            PublicationProposalState::Started
        };
        if stored_state != expected_state {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "audit proposal {call_id} has state {}, expected {}",
                    stored_state.as_str(),
                    expected_state.as_str()
                ),
            ));
        }
    }
    Ok(())
}

/// C must accept exactly the completed proposal set that the publication
/// stream durably assembled. This is the C-side owner of the final
/// proposal-state boundary; Tool Plane events can then resolve the retained
/// canonical owner without reconstructing frame JSON.
fn validate_canonical_tool_proposals(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    let MessageBlock::Assistant(assistant) = message else {
        return Ok(());
    };
    let owners = load_publication_proposal_owners(transaction, stream_id)?;
    let mut accepted_call_ids = BTreeSet::new();
    for (index, block) in assistant.content.iter().enumerate() {
        let AssistantContentBlock::ToolCall(call) = block else {
            continue;
        };
        if !accepted_call_ids.insert(call.id.as_str().to_owned()) {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "canonical Assistant contains duplicate ToolCall {}",
                    call.id
                ),
            ));
        }
        let block_index = u32::try_from(index)
            .map(ContentBlockIndex::new)
            .map_err(|_| storage("canonical Assistant block index overflow"))?;
        let Some(owner) = owners.iter().find(|owner| owner.call_id == call.id) else {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "canonical ToolCall {} has no stream-local Started owner",
                    call.id
                ),
            ));
        };
        if owner.block_index != block_index
            || owner.tool_id != call.tool_id
            || owner.name != call.name
        {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "canonical ToolCall {} disagrees with its frozen owner",
                    call.id
                ),
            ));
        }
        if owner.state != PublicationProposalState::Completed {
            return Err(proposal_violation(
                stream_id,
                format!(
                    "canonical ToolCall {} has no durable Completed frame",
                    call.id
                ),
            ));
        }
    }
    for owner in owners {
        match owner.state {
            PublicationProposalState::Started => {
                return Err(proposal_violation(
                    stream_id,
                    format!(
                        "canonical acceptance leaves Started-only proposal {}",
                        owner.call_id
                    ),
                ));
            }
            PublicationProposalState::Completed
                if !accepted_call_ids.contains(owner.call_id.as_str()) =>
            {
                return Err(proposal_violation(
                    stream_id,
                    format!(
                        "canonical Assistant omits Completed proposal {}",
                        owner.call_id
                    ),
                ));
            }
            PublicationProposalState::Completed => {}
        }
    }
    Ok(())
}

fn clear_publication_staging(
    transaction: &Transaction<'_>,
    stream_id: &PublicationStreamId,
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "DELETE FROM publication_frames WHERE stream_id=?1",
            [stream_id.as_str()],
        )
        .map_err(|error| storage(format!("clear publication staging: {error}")))?;
    Ok(())
}

/// Whether **P** is durable for one exact provider request.
fn request_outcome_is_durable(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    request_id: &RequestId,
) -> Result<bool, ConversationStoreError> {
    let Some((snapshot, _, completed_sequence)) =
        read_request_snapshot_tx(transaction, request_id)?
    else {
        return Ok(false);
    };
    let _ = require_started_request_tx(transaction, conversation_id, request_id)?;
    let Some(completed_sequence) = completed_sequence else {
        return Ok(false);
    };
    let event_json: String = transaction
        .query_row(
            "SELECT event_json FROM events WHERE sequence=?1",
            [seq_to_i64(completed_sequence)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("request outcome lookup: {error}")))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "request {request_id} completed marker has no Event Journal fact"
            ))
        })?;
    let event: RuntimeEventEnvelope = decode(&event_json, "request outcome")?;
    if event.sequence != completed_sequence
        || event.conversation_id != *conversation_id
        || event.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
        || event.turn_id.as_ref() != Some(&snapshot.identity.turn)
        || !matches!(
            &event.event,
            RuntimeEvent::ModelRequestCompleted {
                request_id: completed_request,
                ..
            } if completed_request == request_id
        )
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {request_id} completed marker does not identify its exact successful provider outcome"
        )));
    }
    Ok(true)
}

/// Rejects every dependent Tool Plane transition for a proposal that belongs
/// to a settled publication audit, and records the dependency otherwise.
///
/// This is the durable half of the hard Issue #108 invariant: no tool
/// proposal from an Incomplete or Unaccepted publication may have a dependent
/// `ToolExecutionStarted`, `ToolResult`, or side-effect authorization.
#[allow(clippy::too_many_lines)] // One store-layer owner keeps every dependency path identical.
fn record_tool_proposal_dependency(
    transaction: &Transaction<'_>,
    call_id: &ToolCallId,
    attempt_id: Option<&AttemptId>,
    turn_id: Option<&TurnId>,
    expected_tool_id: Option<&ToolId>,
    dependency: &str,
    allow_unowned_canonical: bool,
) -> Result<(), ConversationStoreError> {
    // Detached authorization facts intentionally outlive an attempt and carry
    // no envelope generation. Resolve those facts through the current durable
    // Surface: an active canonical Assistant is the only valid owner, and the
    // active Surface rejects duplicate ToolCallIds. This keeps a historical
    // audited/canonical reuse from becoming a bare-call-id alias.
    let detached_active_messages = if attempt_id.is_none() && turn_id.is_none() {
        Some(load_head(transaction)?.active_message_ids)
    } else {
        None
    };
    let mut statement = transaction
        .prepare(
            "SELECT p.stream_id,s.attempt_id,s.turn_id,s.message_id,p.tool_id,p.settlement,p.state
             FROM publication_proposals p
             JOIN publication_streams s ON s.stream_id=p.stream_id
             WHERE p.call_id=?1
             ORDER BY p.stream_id",
        )
        .map_err(|error| storage(format!("tool proposal probe: {error}")))?;
    let rows = statement
        .query_map([call_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| storage(format!("tool proposal rows: {error}")))?;
    let candidates = rows
        .map(|row| row.map_err(|error| storage(format!("tool proposal row: {error}"))))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if candidates.is_empty() {
        // A direct canonical Assistant commit predates the publication plane
        // in a few durable recovery prefixes. It is a valid owner only when
        // the active Surface still contains that exact ToolCall. An arbitrary
        // call id with no publication owner and no canonical Assistant owner
        // is a malformed foreground dependency, not an idempotent no-op. A
        // direct canonical ToolMessage or detached lifecycle opening may
        // still be admitted without a model proposal: neither is a
        // ToolResult or execution fact for a publication proposal.
        if let Some(owner_tool_id) = canonical_surface_tool_id(transaction, call_id)? {
            if let Some(expected_tool_id) = expected_tool_id
                && *expected_tool_id != owner_tool_id
            {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "{dependency} for tool call {call_id} uses tool id {expected_tool_id}, but the canonical Assistant owner freezes {owner_tool_id}"
                )));
            }
            return Ok(());
        }
        if allow_unowned_canonical {
            return Ok(());
        }
        return Err(ConversationStoreError::PublicationViolation(format!(
            "{dependency} for tool call {call_id} has no durable proposal or canonical Assistant owner"
        )));
    }
    let matching: Vec<_> = candidates
        .iter()
        .filter(
            |(_, candidate_attempt, candidate_turn, candidate_message, _, _, _)| {
                attempt_id.is_none_or(|attempt| candidate_attempt == attempt.as_str())
                    && turn_id.is_none_or(|turn| candidate_turn == turn.as_str())
                    && detached_active_messages.as_ref().is_none_or(|active| {
                        active
                            .iter()
                            .any(|message_id| message_id.as_str() == candidate_message)
                    })
            },
        )
        .collect();
    if matching.is_empty() {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "{dependency} for tool call {call_id} does not match any exact publication generation"
        )));
    }
    let canonical: Vec<_> = matching
        .iter()
        .filter(|(_, _, _, _, _, settlement, _)| {
            settlement.as_deref() == Some(PublicationSettlement::Canonical.as_str())
        })
        .collect();
    let selected = match canonical.len().cmp(&1) {
        std::cmp::Ordering::Equal => canonical[0],
        std::cmp::Ordering::Greater => {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "{dependency} for tool call {call_id} has ambiguous canonical proposal ownership"
            )));
        }
        std::cmp::Ordering::Less => {
            let audited = matching.iter().find_map(|(_, _, _, _, _, settlement, _)| {
                settlement.as_deref().filter(|settlement| {
                    *settlement == PublicationSettlement::Incomplete.as_str()
                        || *settlement == PublicationSettlement::Unaccepted.as_str()
                })
            });
            if let Some(settlement) = audited {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "tool call {call_id} is a model proposal of an {settlement} publication and may never execute or acquire {dependency}"
                )));
            }
            if matching.len() != 1 {
                return Err(ConversationStoreError::PublicationViolation(format!(
                    "{dependency} for tool call {call_id} has ambiguous unsettled proposal ownership"
                )));
            }
            matching[0]
        }
    };
    if let Some(expected_tool_id) = expected_tool_id
        && selected.4.as_str() != expected_tool_id.as_str()
    {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "{dependency} for tool call {call_id} uses tool id {expected_tool_id}, but the frozen proposal owner uses {}",
            selected.4
        )));
    }
    let state = &selected.6;
    let state = PublicationProposalState::parse(state)?;
    if state != PublicationProposalState::Completed {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "{dependency} for tool call {call_id} has no durable Completed proposal frame"
        )));
    }
    transaction
        .execute(
            "UPDATE publication_proposals SET executed=1 WHERE stream_id=?1 AND call_id=?2",
            params![selected.0.as_str(), call_id.as_str()],
        )
        .map_err(|error| storage(format!("record tool proposal dependency: {error}")))?;
    Ok(())
}

/// Resolves the non-publication canonical path used by older recovery
/// prefixes without turning a missing publication owner into a bare call-id
/// authority. The active canonical Assistant is the only durable fact that
/// can authorize a Tool Plane transition when no publication proposal row
/// exists, and it freezes the tool ID used for that authorization.
fn canonical_surface_tool_id(
    transaction: &Transaction<'_>,
    call_id: &ToolCallId,
) -> Result<Option<ToolId>, ConversationStoreError> {
    let head = load_head(transaction)?;
    for message_id in head.active_message_ids {
        let message = load_message_tx(transaction, &message_id)?;
        let MessageBlock::Assistant(assistant) = message else {
            continue;
        };
        if let Some(tool_id) = assistant.content.iter().find_map(|block| match block {
            AssistantContentBlock::ToolCall(call) if call.id == *call_id => {
                Some(call.tool_id.clone())
            }
            _ => None,
        }) {
            return Ok(Some(tool_id));
        }
    }
    Ok(None)
}

/// The canonical Assistant message that owns `call_id` inside the exact
/// durable generation `(attempt_id, turn_id)` names.
///
/// A `ToolCallId` is request/publication-scoped, never conversation-global, so
/// "a canonical `ToolCall` with this id exists somewhere" is not ownership.
/// The retained FND-03 publication owner is: the proposal row is keyed by
/// `(stream_id, call_id)`, its stream froze the attempt, turn, and Assistant
/// message identity when it opened, and canonical acceptance stamped both rows
/// `canonical`. Resolving through that join is what makes the generation part
/// of the identity rather than an afterthought.
///
/// Returns `None` when this generation proposed no such canonical call. An
/// ambiguous canonical owner is a durable contradiction and is reported as
/// one, matching the guard [`record_tool_proposal_dependency`] already applies.
fn canonical_generation_owner(
    transaction: &Transaction<'_>,
    call_id: &ToolCallId,
    attempt_id: &AttemptId,
    turn_id: &TurnId,
) -> Result<Option<MessageId>, ConversationStoreError> {
    let mut statement = transaction
        .prepare(
            "SELECT s.message_id
             FROM publication_proposals p
             JOIN publication_streams s ON s.stream_id=p.stream_id
             WHERE p.call_id=?1 AND p.settlement=?2
               AND s.attempt_id=?3 AND s.turn_id=?4
             ORDER BY s.message_id",
        )
        .map_err(|error| storage(format!("canonical generation owner probe: {error}")))?;
    let rows = statement
        .query_map(
            params![
                call_id.as_str(),
                PublicationSettlement::Canonical.as_str(),
                attempt_id.as_str(),
                turn_id.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| storage(format!("canonical generation owner rows: {error}")))?;
    let owners = rows
        .map(|row| row.map_err(|error| storage(format!("canonical generation owner row: {error}"))))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    match owners.len() {
        0 => Ok(None),
        1 => Ok(Some(MessageId::new(owners[0].clone()))),
        _ => Err(ConversationStoreError::PublicationViolation(format!(
            "tool call {call_id} has ambiguous canonical proposal ownership in attempt {attempt_id} turn {turn_id}"
        ))),
    }
}

/// The attempt and turn envelope one interaction audit fact is pinned to.
///
/// An interaction is always owned by exactly one model turn of exactly one
/// attempt, so an audit fact with no attempt or no turn could neither be
/// compared to its partner nor resolved to a durable generation — and an
/// unpinnable fact is not a pinned fact.
fn require_interaction_envelope<'a>(
    envelope: &'a RuntimeEventEnvelope,
    interaction_id: &InteractionId,
) -> Result<(&'a AttemptId, &'a TurnId), ConversationStoreError> {
    let (Some(attempt_id), Some(turn_id)) = (&envelope.attempt_id, &envelope.turn_id) else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} audit facts must carry their owning attempt and turn"
        )));
    };
    Ok((attempt_id, turn_id))
}

/// Verifies that one Approval audit subject describes the canonical `ToolCall`
/// it names **in the generation its own envelope names** (Issue #109).
///
/// The subject is deliberately bounded: it names the call and tool identity by
/// value and pins the exact arguments by digest rather than copying them,
/// because the canonical `ToolCall` in the Message Ledger already holds those
/// arguments by value. That economy is only honest if the durable authority
/// checks the correspondence, and the correspondence is only meaningful if it
/// includes the generation. Content equality alone is not ownership: two turns
/// of one attempt, or two attempts of one conversation, can hold canonical
/// calls that compare equal in every field the subject carries.
///
/// The proof therefore composes the two ownerships the durable model already
/// retains, and never falls back to a conversation-global bare `call_id`:
///
/// ```text
/// retained canonical publication generation   (attempt, turn) -> message_id
///                     +
/// active canonical Surface ownership          message_id is still canonical
///                     =
/// the exact canonical Assistant ToolCall this approval may describe
/// ```
///
/// Every real Approval reaches this with a publication owner: the Agent Loop
/// commits an Assistant message through `commit_canonical_publication` and
/// refuses to commit one at all without an open stream, so a canonical
/// `ToolCall` without a frozen `(attempt, turn, message_id)` owner cannot occur
/// on the approval path. There is deliberately no lenient fallback for a state
/// the runtime cannot produce and no database can already contain.
fn validate_approval_subject_against_canonical(
    transaction: &Transaction<'_>,
    interaction_id: &InteractionId,
    attempt_id: &AttemptId,
    turn_id: &TurnId,
    subject: &InteractionSubject,
) -> Result<(), ConversationStoreError> {
    let InteractionSubject::Approval {
        call_id,
        tool_id,
        tool_name,
        arguments_digest,
        ..
    } = subject
    else {
        return Ok(());
    };
    // 1. The generation that is asking must be the generation that proposed
    //    the call. This is the check that makes "turn 2 approved turn 1's
    //    ToolCall" — a permanently false audit fact — unrepresentable.
    let Some(owner) = canonical_generation_owner(transaction, call_id, attempt_id, turn_id)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} approves tool call {call_id}, which attempt {attempt_id} turn {turn_id} never canonically proposed"
        )));
    };
    // 2. That owner must still be canonical: an Assistant message that left
    //    the active Surface is no longer a call the conversation is making.
    if !load_head(transaction)?.active_message_ids.contains(&owner) {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} approves tool call {call_id}, whose canonical owner {owner} is no longer on the active Surface"
        )));
    }
    let MessageBlock::Assistant(assistant) = load_message_tx(transaction, &owner)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "the canonical owner {owner} of tool call {call_id} is not an Assistant message"
        )));
    };
    let Some(call) = assistant.content.iter().find_map(|block| match block {
        AssistantContentBlock::ToolCall(call) if call.id == *call_id => Some(call),
        _ => None,
    }) else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "the canonical owner {owner} of tool call {call_id} does not contain that call"
        )));
    };
    // 3. And the frozen identity and arguments must match the subject exactly.
    if call.tool_id != *tool_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} names tool id {tool_id} for call {call_id}, but the canonical ToolCall froze {}",
            call.tool_id
        )));
    }
    if call.name != *tool_name {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} names tool {tool_name} for call {call_id}, but the canonical ToolCall froze {}",
            call.name
        )));
    }
    let canonical = interaction_arguments_digest(&call.arguments);
    if canonical != *arguments_digest {
        return Err(ConversationStoreError::InvalidReference(format!(
            "interaction {interaction_id} pins arguments digest {arguments_digest} for call {call_id}, but the canonical ToolCall arguments digest to {canonical}"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // One acceptance transaction validates the full inbound contract.
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
            let transcript_cursor = if matches!(message.kind, InboundKind::Context(_)) {
                None
            } else {
                Some(
                    load_transcript_reference(transaction, "message", &message_id)?.ok_or_else(
                        || {
                            ConversationStoreError::InvalidReference(format!(
                                "accepted inbound {message_id} is missing its transcript reference"
                            ))
                        },
                    )?,
                )
            };
            return Ok(AcceptedInbound {
                sequence: InboundSequence::new(sequence_from_i64(sequence)?),
                message_id: MessageId::new(message_id),
                message,
                transcript_cursor,
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
    // Acceptance is the durable-before-display frontier for an ordinary
    // inbound user message. Context facts are durable model input but are not
    // ordinary transcript content, so they never receive an ordering row.
    // Adoption later reuses this same reference when a visible body moves
    // into the Message Ledger.
    let transcript_cursor = if matches!(message.kind, InboundKind::Context(_)) {
        None
    } else {
        Some(append_transcript_reference(
            transaction,
            "message",
            message_id.as_str(),
        )?)
    };
    Ok(AcceptedInbound {
        sequence: InboundSequence::new(sequence),
        message_id,
        message,
        transcript_cursor,
        retried: false,
    })
}

fn append_canonical_messages(
    store: &SqliteConversationStore,
    messages: &[MessageBlock],
) -> Result<Vec<TranscriptCommitReceipt>, ConversationStoreError> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let mut connection = store.lock()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage(format!("canonical transaction: {error}")))?;
    ensure_surface_head(&transaction)?;
    let mut receipts = Vec::with_capacity(messages.len());
    for message in messages {
        if let MessageBlock::Tool(tool) = message {
            record_tool_proposal_dependency(
                &transaction,
                &tool.tool_call_id,
                None,
                None,
                Some(&tool.tool_id),
                "canonical ToolMessage",
                true,
            )?;
        }
        receipts.push(TranscriptCommitReceipt {
            transcript_cursor: append_message_and_surface(&transaction, message)?,
        });
    }
    transaction
        .commit()
        .map_err(|error| storage(format!("canonical commit: {error}")))?;
    Ok(receipts)
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

#[allow(clippy::too_many_lines)] // One schema, one place.
fn create_schema(connection: &Connection) -> Result<(), ConversationStoreError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS rustx_store (
                id INTEGER PRIMARY KEY CHECK(id=1),
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL,
                next_inbound_sequence INTEGER NOT NULL CHECK(next_inbound_sequence >= 0),
                next_event_sequence INTEGER NOT NULL CHECK(next_event_sequence >= 0),
                next_transcript_position INTEGER NOT NULL CHECK(next_transcript_position >= 0)
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
            CREATE TABLE IF NOT EXISTS transcript_order (
                position INTEGER PRIMARY KEY,
                reference_kind TEXT NOT NULL,
                reference_id TEXT NOT NULL,
                UNIQUE(reference_kind,reference_id)
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
                started_sequence INTEGER,
                completed_sequence INTEGER
            );
            CREATE TABLE IF NOT EXISTS publication_streams (
                stream_id TEXT PRIMARY KEY,
                attempt_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                request_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                next_frame_sequence INTEGER NOT NULL CHECK(next_frame_sequence >= 0),
                terminal_sequence INTEGER,
                settlement TEXT
            );
            CREATE TABLE IF NOT EXISTS publication_frames (
                stream_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                frame_json TEXT NOT NULL,
                PRIMARY KEY(stream_id, sequence)
            );
            CREATE TABLE IF NOT EXISTS publication_proposals (
                stream_id TEXT NOT NULL,
                call_id TEXT NOT NULL,
                block_index INTEGER NOT NULL CHECK(block_index >= 0),
                tool_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('started','completed')),
                executed INTEGER NOT NULL CHECK(executed IN (0,1)),
                settlement TEXT,
                PRIMARY KEY(stream_id, call_id)
            );
            CREATE TABLE IF NOT EXISTS publication_audits (
                stream_id TEXT PRIMARY KEY,
                audit_json TEXT NOT NULL
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
            CREATE INDEX IF NOT EXISTS transcript_order_reference_idx ON transcript_order(reference_kind, reference_id);
            CREATE INDEX IF NOT EXISTS surface_ops_revision_idx ON surface_ops(revision);
            CREATE INDEX IF NOT EXISTS events_sequence_idx ON events(sequence);
            CREATE INDEX IF NOT EXISTS events_attempt_idx ON events(attempt_id, sequence);
            CREATE INDEX IF NOT EXISTS request_snapshots_surface_idx ON request_snapshots(surface_revision);
            CREATE INDEX IF NOT EXISTS publication_frames_stream_idx ON publication_frames(stream_id, sequence);
            CREATE INDEX IF NOT EXISTS publication_streams_settlement_idx ON publication_streams(settlement);",
        )
        .map_err(|error| storage(format!("create schema: {error}")))?;
    connection
        .execute(
            "INSERT OR IGNORE INTO rustx_store(id,schema_version,conversation_id,next_inbound_sequence,next_event_sequence,next_transcript_position) VALUES(1,?1,'',0,0,0)",
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

#[allow(clippy::too_many_lines)] // One required-shape table, one place.
fn verify_schema_shape(connection: &Connection) -> Result<(), ConversationStoreError> {
    let required = [
        (
            "rustx_store",
            &[
                "schema_version",
                "conversation_id",
                "next_inbound_sequence",
                "next_event_sequence",
                "next_transcript_position",
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
        (
            "transcript_order",
            &["position", "reference_kind", "reference_id"],
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
                "completed_sequence",
            ],
        ),
        (
            "publication_streams",
            &[
                "stream_id",
                "attempt_id",
                "turn_id",
                "request_id",
                "message_id",
                "next_frame_sequence",
                "terminal_sequence",
                "settlement",
            ],
        ),
        (
            "publication_frames",
            &["stream_id", "sequence", "frame_json"],
        ),
        (
            "publication_proposals",
            &[
                "call_id",
                "stream_id",
                "block_index",
                "tool_id",
                "tool_name",
                "state",
                "executed",
                "settlement",
            ],
        ),
        ("publication_audits", &["stream_id", "audit_json"]),
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
        ("publication_streams", "stream_id"),
        ("publication_audits", "stream_id"),
    ] {
        verify_unique_column(connection, table, column)?;
    }
    verify_unique_columns(
        connection,
        "publication_proposals",
        &["stream_id", "call_id"],
    )?;
    verify_unique_columns(
        connection,
        "transcript_order",
        &["reference_kind", "reference_id"],
    )?;
    Ok(())
}

fn verify_unique_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<(), ConversationStoreError> {
    verify_unique_columns(connection, table, &[column])
}

fn verify_unique_columns(
    connection: &Connection,
    table: &str,
    expected: &[&str],
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
        if columns
            == expected
                .iter()
                .map(|column| (*column).to_owned())
                .collect::<Vec<_>>()
        {
            return Ok(());
        }
    }
    Err(ConversationStoreError::IncompatibleSchema(format!(
        "table {table} is missing a unique constraint on ({})",
        expected.join(", ")
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
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    append_message_and_surface_internal(transaction, message, None)
}

fn append_adopted_message_and_surface(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    pending_message_id: &MessageId,
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    append_message_and_surface_internal(transaction, message, Some(pending_message_id))
}

fn append_message_and_surface_internal(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    allowed_pending_message_id: Option<&MessageId>,
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
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
    let transcript_cursor =
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
    update_checkpoint(transaction, revision, head.compaction_generation, &active)?;
    Ok(transcript_cursor)
}

fn append_message_ledger(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    allowed_pending_message_id: Option<&MessageId>,
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
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
    if transcript_visible_message(message) {
        Ok(Some(append_transcript_reference(
            transaction,
            "message",
            id.as_str(),
        )?))
    } else {
        Ok(None)
    }
}

/// Whether one canonical message is ordinary user-facing transcript content.
///
/// Context messages remain durable model history but are deliberately absent
/// from the normal transcript. User messages, including compaction summaries,
/// Assistant generations, and Tool results are visible semantic content.
fn transcript_visible_message(message: &MessageBlock) -> bool {
    !matches!(
        message,
        MessageBlock::User(user)
            if matches!(user.kind, InboundKind::Context(_))
    )
}

/// Appends one low-frequency transcript reference, or verifies the existing
/// reference when an accepted Pending Inbound message is adopted into the
/// Ledger. No message, audit, or event body is stored here.
fn append_transcript_reference(
    transaction: &Transaction<'_>,
    reference_kind: &str,
    reference_id: &str,
) -> Result<TranscriptCursor, ConversationStoreError> {
    let existing: Option<i64> = transaction
        .query_row(
            "SELECT position FROM transcript_order
             WHERE reference_kind=?1 AND reference_id=?2",
            params![reference_kind, reference_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("transcript reference probe: {error}")))?;
    if let Some(position) = existing {
        return Ok(TranscriptCursor::new(
            u64::try_from(position).map_err(|_| storage("negative transcript position"))?,
        ));
    }
    let current: i64 = transaction
        .query_row(
            "SELECT next_transcript_position FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read transcript position: {error}")))?;
    let position = current
        .checked_add(1)
        .ok_or_else(|| storage("transcript position exhausted"))?;
    transaction
        .execute(
            "INSERT INTO transcript_order(position,reference_kind,reference_id) VALUES(?1,?2,?3)",
            params![position, reference_kind, reference_id],
        )
        .map_err(|error| storage(format!("insert transcript reference: {error}")))?;
    transaction
        .execute(
            "UPDATE rustx_store SET next_transcript_position=?1 WHERE id=1",
            [position],
        )
        .map_err(|error| storage(format!("update transcript position: {error}")))?;
    Ok(TranscriptCursor::new(
        u64::try_from(position).map_err(|_| storage("negative transcript position"))?,
    ))
}

fn load_transcript_reference(
    transaction: &Transaction<'_>,
    reference_kind: &str,
    reference_id: &str,
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT position FROM transcript_order
             WHERE reference_kind=?1 AND reference_id=?2",
            params![reference_kind, reference_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| storage(format!("transcript reference lookup: {error}")))?
        .map(|position| {
            u64::try_from(position)
                .map(TranscriptCursor::new)
                .map_err(|_| storage("negative transcript position"))
        })
        .transpose()
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
        .prepare(
            "SELECT p.sequence,p.message_id,p.message_json,p.correlation,t.position
             FROM pending_inbound p
             LEFT JOIN transcript_order t
               ON t.reference_kind='message' AND t.reference_id=p.message_id
             ORDER BY p.sequence",
        )
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
                transcript_cursor: row
                    .get::<_, Option<i64>>(4)?
                    .map(|position| {
                        u64::try_from(position)
                            .map(TranscriptCursor::new)
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Integer,
                                    Box::new(error),
                                )
                            })
                    })
                    .transpose()?,
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
        let visible = !matches!(item.message.kind, InboundKind::Context(_));
        if visible != item.transcript_cursor.is_some() {
            return Err(ConversationStoreError::InvalidReference(format!(
                "pending inbound {} has an invalid transcript reference",
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

/// Reads the bounded transcript ordering spine newest-first, resolves each
/// reference through its canonical durable owner, and returns the selected
/// rows in chronological order.
fn load_transcript_page(
    connection: &Connection,
    before: Option<TranscriptCursor>,
    limit: usize,
) -> Result<TranscriptPage, ConversationStoreError> {
    if limit == 0 {
        return Ok(TranscriptPage {
            entries: Vec::new(),
            next_cursor: None,
        });
    }
    if limit > TRANSCRIPT_PAGE_LIMIT_MAX {
        return Err(storage(format!(
            "transcript page limit {limit} exceeds maximum {TRANSCRIPT_PAGE_LIMIT_MAX}"
        )));
    }
    let before = before
        .map(|cursor| seq_to_i64(cursor.get()))
        .transpose()?
        .unwrap_or(i64::MAX);
    let fetch_limit = i64::try_from(limit + 1)
        .map_err(|_| storage("transcript page limit is not representable"))?;
    let mut statement = connection
        .prepare(
            "SELECT position,reference_kind,reference_id
             FROM transcript_order
             WHERE position < ?1
             ORDER BY position DESC
             LIMIT ?2",
        )
        .map_err(|error| storage(format!("transcript page: {error}")))?;
    let rows = statement
        .query_map(params![before, fetch_limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| storage(format!("transcript page query: {error}")))?;
    let mut references = rows
        .map(|row| row.map_err(|error| storage(format!("transcript page row: {error}"))))
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let has_more = references.len() > limit;
    references.truncate(limit);
    let next_cursor = has_more
        .then(|| references.last().map(|(position, _, _)| *position))
        .flatten()
        .map(|position| {
            TranscriptCursor::new(
                u64::try_from(position).expect("transcript positions are non-negative"),
            )
        });
    references.reverse();
    let entries = references
        .into_iter()
        .map(|(position, reference_kind, reference_id)| {
            let cursor = TranscriptCursor::new(
                u64::try_from(position).map_err(|_| storage("negative transcript position"))?,
            );
            let item = load_transcript_item(connection, &reference_kind, &reference_id)?;
            Ok(TranscriptEntry { cursor, item })
        })
        .collect::<Result<Vec<_>, ConversationStoreError>>()?;
    Ok(TranscriptPage {
        entries,
        next_cursor,
    })
}

/// Resolves one transcript reference without copying its body into the
/// ordering table.
fn load_transcript_item(
    connection: &Connection,
    reference_kind: &str,
    reference_id: &str,
) -> Result<TranscriptItem, ConversationStoreError> {
    match reference_kind {
        "message" => {
            let ledger_json: Option<String> = connection
                .query_row(
                    "SELECT message_json FROM message_ledger WHERE message_id=?1",
                    [reference_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(format!("transcript Ledger lookup: {error}")))?;
            let message = if let Some(json) = ledger_json {
                decode::<MessageBlock>(&json, "transcript message")?
            } else {
                let json: String = connection
                    .query_row(
                        "SELECT message_json FROM pending_inbound WHERE message_id=?1",
                        [reference_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| storage(format!("transcript pending lookup: {error}")))?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(format!(
                            "transcript message reference {reference_id} has no durable owner"
                        ))
                    })?;
                MessageBlock::User(decode::<UserMessageBlock>(
                    &json,
                    "transcript pending message",
                )?)
            };
            if crate::conversation::message_id_of(&message).as_str() != reference_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "transcript message reference {reference_id} contains a different message"
                )));
            }
            Ok(TranscriptItem::Message { message })
        }
        "publication_audit" => {
            let json: String = connection
                .query_row(
                    "SELECT audit_json FROM publication_audits WHERE stream_id=?1",
                    [reference_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(format!("transcript publication audit lookup: {error}")))?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "transcript publication audit reference {reference_id} has no durable audit"
                    ))
                })?;
            let audit: PublicationAudit = decode(&json, "transcript publication audit")?;
            if audit.stream_id.as_str() != reference_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "transcript publication audit reference {reference_id} contains a different stream"
                )));
            }
            Ok(TranscriptItem::PublicationAudit { audit })
        }
        "interaction_event" => {
            let json: String = connection
                .query_row(
                    "SELECT event_json FROM events WHERE event_id=?1",
                    [reference_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(format!("transcript interaction lookup: {error}")))?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "transcript interaction reference {reference_id} has no durable event"
                    ))
                })?;
            let event: RuntimeEventEnvelope = decode(&json, "transcript interaction")?;
            if event.event_id.as_str() != reference_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "transcript interaction reference {reference_id} contains a different event"
                )));
            }
            match &event.event {
                RuntimeEvent::InteractionRequested { .. } => {
                    Ok(TranscriptItem::InteractionRequested { event })
                }
                RuntimeEvent::InteractionSettled { .. } => {
                    Ok(TranscriptItem::InteractionSettled { event })
                }
                _ => Err(ConversationStoreError::InvalidReference(format!(
                    "transcript interaction reference {reference_id} is not an interaction audit"
                ))),
            }
        }
        other => Err(ConversationStoreError::InvalidReference(format!(
            "unknown transcript reference kind {other}"
        ))),
    }
}

#[allow(clippy::too_many_lines)]
fn load_user_message_boundaries(
    connection: &Connection,
    through: SurfaceRevision,
) -> Result<Vec<SurfaceUserMessageBoundary>, ConversationStoreError> {
    Ok(load_user_message_boundaries_page_internal(connection, through, None)?.boundaries)
}

#[allow(clippy::too_many_lines)]
fn load_user_message_boundaries_page_internal(
    connection: &Connection,
    through: SurfaceRevision,
    page: Option<(usize, usize)>,
) -> Result<SurfaceUserMessageBoundaryPage, ConversationStoreError> {
    if let Some((_, limit)) = page
        && limit == 0
    {
        return Err(storage(
            "historical user-message boundary page limit must be positive",
        ));
    }
    let Some((head_revision, _, head_active_json)) = read_surface_head(connection)? else {
        if through == SurfaceRevision::INITIAL {
            return Ok(SurfaceUserMessageBoundaryPage {
                boundaries: Vec::new(),
                next_offset: None,
            });
        }
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {through} has no durable head"
        )));
    };
    let head_revision = SurfaceRevision::new(nonnegative(head_revision, "Surface revision")?);
    if through > head_revision {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {through} is newer than head {head_revision}"
        )));
    }

    let ledger: BTreeMap<MessageId, MessageBlock> = load_canonical_rows(connection)?
        .into_iter()
        .map(|message| {
            let id = crate::conversation::message_id_of(&message);
            (id, message)
        })
        .collect();
    let mut active = Vec::new();
    let mut boundaries = Vec::new();
    let mut boundary_count = 0_usize;
    let mut seen = BTreeSet::new();
    let mut statement = connection
        .prepare(
            "SELECT revision,compaction_generation,op_json
             FROM surface_ops WHERE revision <= ?1 ORDER BY revision",
        )
        .map_err(|error| storage(format!("read Surface history: {error}")))?;
    let rows = statement
        .query_map([seq_to_i64(through.get())?], |row| {
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
        validate_surface_operation_references_from_ledger(&ledger, &operation)?;
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
        let appended_message_id = match &operation {
            SurfaceOp::Append { message_id } => Some(message_id.clone()),
            SurfaceOp::Replace { .. } => None,
        };
        apply_surface_op(&mut active, operation)?;
        if let Some(message_id) = appended_message_id
            && let Some(MessageBlock::User(user)) = ledger.get(&message_id)
            && user.kind == InboundKind::Message
            && seen.insert(message_id)
        {
            let boundary = SurfaceUserMessageBoundary {
                surface_revision: SurfaceRevision::new(expected_revision),
                message: user.clone(),
            };
            let include = page.is_none_or(|(offset, limit)| {
                boundary_count >= offset && boundary_count < offset.saturating_add(limit)
            });
            if include {
                boundaries.push(boundary);
            }
            boundary_count = boundary_count
                .checked_add(1)
                .ok_or_else(|| storage("historical user-message boundary count exhausted"))?;
        }
        expected_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| storage("Surface revision is exhausted"))?;
    }
    let expected_next_revision = through
        .get()
        .checked_add(1)
        .ok_or_else(|| storage("Surface revision is exhausted"))?;
    if expected_revision != expected_next_revision {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Surface revision {through} has a non-contiguous operation history"
        )));
    }
    if through == head_revision {
        let head_active: Vec<MessageId> = decode(&head_active_json, "Surface head")?;
        if active != head_active {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Surface head {head_revision} does not match its immutable operation history"
            )));
        }
    }
    let next_offset = page.and_then(|(offset, _)| {
        let end = offset.saturating_add(boundaries.len());
        (end < boundary_count).then_some(end)
    });
    Ok(SurfaceUserMessageBoundaryPage {
        boundaries,
        next_offset,
    })
}

fn load_user_message_boundaries_page(
    connection: &Connection,
    through: SurfaceRevision,
    offset: usize,
    limit: usize,
) -> Result<SurfaceUserMessageBoundaryPage, ConversationStoreError> {
    load_user_message_boundaries_page_internal(connection, through, Some((offset, limit)))
}

fn validate_surface_operation_references_from_ledger(
    ledger: &BTreeMap<MessageId, MessageBlock>,
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
        if !ledger.contains_key(message_id) {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Surface operation references missing Ledger message {message_id}"
            )));
        }
    }
    if let SurfaceOp::Replace { replacement, .. } = operation {
        let message = ledger
            .get(replacement)
            .expect("replacement was checked above");
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

#[derive(Debug)]
struct PersistedEvent {
    event: RuntimeEventEnvelope,
    transcript_cursor: Option<TranscriptCursor>,
}

#[allow(clippy::too_many_lines)] // One event persistence contract, one place.
fn persist_event_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    mut event: RuntimeEventEnvelope,
) -> Result<PersistedEvent, ConversationStoreError> {
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
    let transcript_cursor = if matches!(
        &event.event,
        RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
    ) {
        Some(append_transcript_reference(
            transaction,
            "interaction_event",
            event.event_id.as_str(),
        )?)
    } else {
        None
    };
    // The **P** marker: a durable provider outcome is recorded against its
    // exact request, so the publication plane can reject U-without-P with one
    // keyed lookup instead of a Journal scan.
    if let RuntimeEvent::ModelRequestCompleted { request_id, .. } = &event.event {
        let updated = transaction
            .execute(
                "UPDATE request_snapshots SET completed_sequence=?1 WHERE request_id=?2 AND completed_sequence IS NULL",
                params![sequence, request_id.as_str()],
            )
            .map_err(|error| storage(format!("record provider outcome: {error}")))?;
        if updated != 1 {
            return Err(ConversationStoreError::TerminalViolation(format!(
                "request {request_id} already has a durable provider outcome"
            )));
        }
    }
    Ok(PersistedEvent {
        event,
        transcript_cursor,
    })
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

/// Reads one Request Snapshot row through the same durable decoder used by
/// request history. The row's denormalized Surface and sequence columns are
/// checked against the frozen JSON so publication cannot trust a mismatched
/// tuple assembled by a caller.
type RequestSnapshotRow = (RequestSnapshot, Option<u64>, Option<u64>);

fn read_request_snapshot_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RequestSnapshotRow>, ConversationStoreError> {
    let row: Option<(i64, String, Option<i64>, Option<i64>)> = transaction
        .query_row(
            "SELECT surface_revision,snapshot_json,started_sequence,completed_sequence
             FROM request_snapshots WHERE request_id=?1",
            [request_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| storage(format!("request snapshot transaction lookup: {error}")))?;
    let Some((stored_surface_revision, json, started_sequence, completed_sequence)) = row else {
        return Ok(None);
    };
    let snapshot: RequestSnapshot = decode(&json, "request snapshot")?;
    validate_snapshot_identity(&snapshot)?;
    if snapshot.request_id != *request_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot row {request_id} contains a different RequestId"
        )));
    }
    if stored_surface_revision != seq_to_i64(snapshot.surface_revision.get())? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} Surface column disagrees with its frozen snapshot"
        )));
    }
    Ok(Some((
        snapshot,
        started_sequence.map(sequence_from_i64).transpose()?,
        completed_sequence.map(sequence_from_i64).transpose()?,
    )))
}

/// Proves that a Request Snapshot has exactly one durable request-start fact
/// and that the fact's envelope identifies the same conversation, attempt,
/// turn, request, and model.
fn require_started_request_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    request_id: &RequestId,
) -> Result<(RequestSnapshot, u64), ConversationStoreError> {
    let Some((snapshot, started_sequence, _)) = read_request_snapshot_tx(transaction, request_id)?
    else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} does not exist"
        )));
    };
    let Some(started_sequence) = started_sequence else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} has no durable start sequence"
        )));
    };
    let Some(started) = find_request_start_event(transaction, request_id)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} has no durable request-start fact"
        )));
    };
    if started.sequence != started_sequence {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} start sequence disagrees with its request-start fact"
        )));
    }
    if started.conversation_id != *conversation_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start fact for {request_id} belongs to a foreign conversation"
        )));
    }
    validate_request_start_metadata(&snapshot, &started)?;
    Ok((snapshot, started_sequence))
}

/// Finds any already durable provider terminal for one request. Both success
/// and failure are terminal outcomes: a contradictory second outcome is never
/// accepted, while only a successful outcome establishes P.
fn find_request_outcome_event(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let mut statement = transaction
        .prepare("SELECT event_json FROM events ORDER BY sequence")
        .map_err(|error| storage(format!("request outcome probe: {error}")))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| storage(format!("request outcome query: {error}")))?;
    for row in rows {
        let json = row.map_err(|error| storage(format!("request outcome row: {error}")))?;
        let event: RuntimeEventEnvelope = decode(&json, "request outcome")?;
        if matches!(
            &event.event,
            RuntimeEvent::ModelRequestCompleted {
                request_id: candidate,
                ..
            }
                | RuntimeEvent::ModelRequestFailed {
                    request_id: candidate,
                    ..
                } if candidate == request_id
        ) {
            return Ok(Some(event));
        }
    }
    Ok(None)
}

/// Validates the complete immutable identity of a publication stream against
/// the Request Snapshot that owns it.
fn validate_publication_generation(
    snapshot: &RequestSnapshot,
    start: &PublicationStreamStart,
) -> Result<(), ConversationStoreError> {
    let expected_stream = PublicationStreamId::for_request(
        &snapshot.identity.attempt_id,
        &snapshot.provisional_message_id,
    );
    if start.request_id != snapshot.request_id
        || start.attempt_id != snapshot.identity.attempt_id
        || start.turn_id != snapshot.identity.turn
        || start.message_id != snapshot.provisional_message_id
        || start.stream_id != expected_stream
    {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "publication stream {} does not identify the exact Request Snapshot generation {}",
            start.stream_id, snapshot.request_id
        )));
    }
    Ok(())
}

/// Validates C's event envelope and message identity against the frozen
/// publication generation before the compound Ledger/Surface/Journal
/// transaction can mutate anything.
fn validate_canonical_publication_event(
    conversation_id: &ConversationId,
    start: &PublicationStreamStart,
    message: &MessageBlock,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    validate_canonical_event_for_message(message, &event.event)?;
    if !matches!(message, MessageBlock::Assistant(_))
        || !matches!(
            &event.event,
            RuntimeEvent::AssistantMessageCommitted { message_id }
                if message_id == &start.message_id
        )
        || crate::conversation::message_id_of(message) != start.message_id
        || event.conversation_id != *conversation_id
        || event.attempt_id.as_ref() != Some(&start.attempt_id)
        || event.turn_id.as_ref() != Some(&start.turn_id)
    {
        return Err(ConversationStoreError::PublicationViolation(format!(
            "canonical acceptance of publication stream {} does not identify its exact Assistant generation",
            start.stream_id
        )));
    }
    Ok(())
}

fn runtime_event_dependency_name(event: &RuntimeEvent) -> &'static str {
    match event {
        RuntimeEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
        RuntimeEvent::ToolExecutionProgress { .. } => "ToolExecutionProgress",
        RuntimeEvent::ToolExecutionCompleted { .. } => "ToolExecutionCompleted",
        RuntimeEvent::ToolExecutionFailed { .. } => "ToolExecutionFailed",
        RuntimeEvent::ToolMessageCommitted { .. } => "ToolMessageCommitted",
        RuntimeEvent::BackgroundExecutionCommitted { .. } => "background side-effect authorization",
        RuntimeEvent::SubagentOwnershipCommitted { .. } => "subagent side-effect authorization",
        _ => unreachable!("dependency name requested for a non-tool event"),
    }
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

/// Resolves the authoritative child agent identity from the one durable
/// ownership opening fact of a subagent lifecycle.
///
/// Terminal callers restate `child_agent_id` so the compound event remains
/// self-describing, but that repeated field is not authority. The ownership
/// fact has one deterministic event identity
/// ([`subagent_ownership_event_id`](crate::runtime::subagent::subagent_ownership_event_id)),
/// so the authoritative child identity resolves through the unique
/// `event_id` index in bounded time instead of scanning the event journal.
/// The embedded `SubagentId` is revalidated defensively: the located fact
/// must actually belong to the requested child before its `child_agent_id`
/// is trusted as provenance authority. Duplicate ownership facts are
/// already rejected at commit time by the `lifecycle_state` uniqueness
/// probe, so a deterministic lookup cannot miss a second opening fact.
fn find_subagent_ownership_child(
    transaction: &Transaction<'_>,
    subagent_id: &crate::runtime::identity::SubagentId,
) -> Result<AgentId, ConversationStoreError> {
    let event_id = crate::runtime::subagent::subagent_ownership_event_id(subagent_id);
    let envelope = find_event_by_id(transaction, &event_id)?.ok_or_else(|| {
        ConversationStoreError::InvalidReference(format!(
            "subagent terminal has no durable ownership fact for {subagent_id}"
        ))
    })?;
    match envelope.event {
        RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: embedded,
            child_agent_id,
            ..
        } if embedded == *subagent_id => Ok(child_agent_id),
        RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id: embedded,
            ..
        } => Err(ConversationStoreError::InvalidReference(format!(
            "subagent ownership event {event_id} belongs to {embedded}, not {subagent_id}"
        ))),
        _ => Err(ConversationStoreError::InvalidReference(format!(
            "the subagent ownership event {event_id} is not the typed ownership fact"
        ))),
    }
}

#[allow(clippy::too_many_lines)] // Keeps all cross-domain reference checks at one transaction seam.
fn validate_event_reference(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    match &envelope.event {
        // A provider outcome names an actual started request. Without this
        // the P marker could be recorded against a request that never had a
        // durable start fact.
        RuntimeEvent::ModelRequestCompleted { request_id, .. }
        | RuntimeEvent::ModelRequestFailed { request_id, .. } => {
            let Some((snapshot, _, _)) = read_request_snapshot_tx(transaction, request_id)? else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "model request outcome references request {request_id}, which never started"
                )));
            };
            let _ = require_started_request_tx(transaction, &envelope.conversation_id, request_id)?;
            if envelope.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
                || envelope.turn_id.as_ref() != Some(&snapshot.identity.turn)
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "model request outcome for {request_id} has a foreign attempt or turn envelope"
                )));
            }
            if find_request_outcome_event(transaction, request_id)?.is_some() {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "request {request_id} already has a durable provider outcome"
                )));
            }
        }
        // Every Tool Plane transition that names a proposal uses the same
        // semantic owner. This includes execution starts/progress/outcomes
        // and detached background authorization.
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id,
            tool_id,
        }
        | RuntimeEvent::ToolExecutionProgress {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionCompleted {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionFailed {
            tool_call_id,
            tool_id,
            ..
        } => {
            record_tool_proposal_dependency(
                transaction,
                tool_call_id,
                envelope.attempt_id.as_ref(),
                envelope.turn_id.as_ref(),
                Some(tool_id),
                runtime_event_dependency_name(&envelope.event),
                false,
            )?;
        }
        RuntimeEvent::BackgroundExecutionCommitted {
            tool_call_id,
            tool_id,
            ..
        } => {
            record_tool_proposal_dependency(
                transaction,
                tool_call_id,
                envelope.attempt_id.as_ref(),
                envelope.turn_id.as_ref(),
                Some(tool_id),
                runtime_event_dependency_name(&envelope.event),
                true,
            )?;
        }
        RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id,
            tool_call_id,
            ..
        } => {
            record_tool_proposal_dependency(
                transaction,
                tool_call_id,
                envelope.attempt_id.as_ref(),
                envelope.turn_id.as_ref(),
                None,
                runtime_event_dependency_name(&envelope.event),
                true,
            )?;
            // The durable identity of an ownership fact is canonical: the
            // EventId must be the deterministic `subagent-committed-event:{id}`
            // derived from the very SubagentId embedded in the payload. A
            // mismatched pair is malformed and must never enter durable
            // authority; the authority rejects it rather than silently
            // rewriting or accepting it.
            let canonical = crate::runtime::subagent::subagent_ownership_event_id(subagent_id);
            if envelope.event_id != canonical {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent ownership event identity {} does not match the canonical identity {canonical} for {subagent_id}",
                    envelope.event_id
                )));
            }
            let key = format!("subagent:{subagent_id}");
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    storage(format!("subagent ownership uniqueness probe: {error}"))
                })?;
            if exists {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "subagent {subagent_id} already has a durable ownership fact"
                )));
            }
        }
        // The interaction audit plane (Issue #109). Both facts carry the
        // canonical event identity of their interaction, so the durable
        // authority resolves the pair through the unique `event_id` index in
        // bounded time instead of scanning the Journal.
        RuntimeEvent::InteractionRequested {
            interaction_id,
            subject,
        } => {
            let canonical =
                crate::runtime::interaction::interaction_requested_event_id(interaction_id);
            if envelope.event_id != canonical {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "interaction requested event identity {} does not match the canonical identity {canonical} for {interaction_id}",
                    envelope.event_id
                )));
            }
            let (attempt_id, turn_id) = require_interaction_envelope(envelope, interaction_id)?;
            // Bounded payloads are a durable invariant, not a coordinator
            // convention: an interaction fact constructed or deserialized
            // outside the live coordinator is refused here by the very same
            // contract the coordinator validates against.
            validate_interaction_subject(subject).map_err(|message| {
                ConversationStoreError::InvalidReference(format!(
                    "interaction {interaction_id} requested an unbounded subject: {message}"
                ))
            })?;
            validate_approval_subject_against_canonical(
                transaction,
                interaction_id,
                attempt_id,
                turn_id,
                subject,
            )?;
            let key = format!("interaction:{interaction_id}");
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(|error| storage(format!("interaction uniqueness probe: {error}")))?;
            if exists {
                return Err(ConversationStoreError::TerminalViolation(format!(
                    "interaction {interaction_id} already has a durable requested fact"
                )));
            }
        }
        RuntimeEvent::InteractionSettled {
            interaction_id,
            settlement,
        } => {
            let canonical =
                crate::runtime::interaction::interaction_settled_event_id(interaction_id);
            if envelope.event_id != canonical {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "interaction settled event identity {} does not match the canonical identity {canonical} for {interaction_id}",
                    envelope.event_id
                )));
            }
            // A settlement is meaningless without the request it settles: a
            // settled-without-requested fact would assert that a decision was
            // made about a prompt that never durably existed.
            let requested_id =
                crate::runtime::interaction::interaction_requested_event_id(interaction_id);
            let requested = find_event_by_id(transaction, &requested_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(format!(
                    "interaction settlement has no durable requested fact for {interaction_id}"
                ))
            })?;
            let RuntimeEvent::InteractionRequested {
                interaction_id: embedded,
                subject,
            } = &requested.event
            else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "the interaction requested event {requested_id} is not the typed requested fact"
                )));
            };
            if embedded != interaction_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "interaction requested event {requested_id} belongs to {embedded}, not {interaction_id}"
                )));
            }
            let _ = require_interaction_envelope(envelope, interaction_id)?;
            // Both facts of one interaction belong to the exact same
            // conversation + attempt + turn envelope. The conversation is
            // already guaranteed by `persist_event_tx`, which refuses a
            // foreign conversation outright; the attempt and the turn are
            // pinned here. Checking only the attempt would durably admit a
            // settlement committed under a later turn of the same attempt,
            // which the contract of a pinned audit pair forbids — and the
            // durable authority must not rely on the coordinator happening to
            // rebuild the same turn.
            if requested.attempt_id != envelope.attempt_id || requested.turn_id != envelope.turn_id
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "interaction {interaction_id} settled under a foreign attempt or turn envelope"
                )));
            }
            validate_interaction_settlement(subject, settlement).map_err(|message| {
                ConversationStoreError::InvalidReference(format!(
                    "interaction {interaction_id} settlement is not one its requested subject could produce: {message}"
                ))
            })?;
        }
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
            record_tool_proposal_dependency(
                transaction,
                tool_call_id,
                envelope.attempt_id.as_ref(),
                envelope.turn_id.as_ref(),
                Some(&tool.tool_id),
                runtime_event_dependency_name(&envelope.event),
                false,
            )?;
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
            validate_snapshot_identity(&snapshot)?;
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
            subagent_id,
            child_agent_id,
            message_id,
            state,
            ..
        } => {
            let committed_child_agent_id = find_subagent_ownership_child(transaction, subagent_id)?;
            if committed_child_agent_id != *child_agent_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal claims child agent {child_agent_id}, but durable ownership committed {committed_child_agent_id}"
                )));
            }
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
    let derived_message = snapshot.identity.provisional_message_id();
    if snapshot.provisional_message_id != derived_message {
        return Err(ConversationStoreError::InvalidReference(format!(
            "Request Snapshot {} provisional message identity disagrees with its RequestIdentity-derived id {derived_message}",
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
    // The interaction lifecycle (Issue #109) is the same shape: opened by the
    // requested fact — which commits before the prompt reaches a client — and
    // closed exactly once by the settled fact. It is deliberately its own
    // lifecycle domain and never touches the enclosing `attempt:`/`turn:`
    // keys, so an interaction audit fact can neither be blocked by nor block
    // the attempt's own terminal transition. An interaction that stays open
    // across a restart is durable evidence of an unanswered prompt; it is
    // never an instruction to recreate a waiter.
    if let RuntimeEvent::InteractionRequested { interaction_id, .. } = &event.event {
        return vec![(format!("interaction:{interaction_id}"), false)];
    }
    if let RuntimeEvent::InteractionSettled { interaction_id, .. } = &event.event {
        return vec![(format!("interaction:{interaction_id}"), true)];
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
            | RuntimeEvent::InteractionSettled { .. }
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
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope, SubagentTerminalState};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContextKind, InboundKind, MessageBlock,
        UserContentBlock, UserMessageBlock, UserSource,
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

    fn request_context_message(id: &str, text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::Context(ContextKind::RuntimeToolObservation),
            timestamp: None,
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

        let (s2, _, _, _) = store.commit_compaction(first_compaction(s1)).unwrap();
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
        let (s4, _, _, _) = store
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

            let (new_revision, _, event, _) = store.commit_compaction(input()).unwrap();
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
            Vec::new(),
            crate::runtime::RuntimeResourceRevision::new(1),
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
            Vec::new(),
            crate::runtime::RuntimeResourceRevision::new(1),
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
        let context = request_context_message("ctx-1", "request-scoped context");
        let snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("attempt-1"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            base_revision.next(),
            "frozen".to_owned(),
            Vec::new(),
            crate::runtime::RuntimeResourceRevision::new(1),
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
        let different = request_context_message("ctx-1", "different content");
        assert!(matches!(
            store.commit_model_turn_start(std::slice::from_ref(&different), &snapshot, Utc::now()),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        // A retry missing a committed context message fails.
        let missing = request_context_message("ctx-2", "never committed");
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
        let ctx1 = request_context_message("ctx-1", "first context");
        let ctx2 = request_context_message("ctx-2", "second context");
        let snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("attempt-1"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            base_revision.next().next(),
            "frozen".to_owned(),
            Vec::new(),
            crate::runtime::RuntimeResourceRevision::new(1),
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
        let extra = request_context_message("ctx-3", "extra");
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
        let different = request_context_message("ctx-1", "changed body");
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
        let ctx1 = request_context_message("ctx-1", "first context");
        let ctx2 = request_context_message("ctx-2", "second context");

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
            let context = request_context_message("ctx-1", "request-scoped context");
            let snapshot = RequestSnapshot::new(
                RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: TurnId::new("1"),
                    retry_number: 0,
                },
                base_revision.next(),
                "frozen".to_owned(),
                Vec::new(),
                crate::runtime::RuntimeResourceRevision::new(1),
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
                Vec::new(),
                crate::runtime::RuntimeResourceRevision::new(1),
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

    /// A subagent terminal publication is a correlated User inbound plus the
    /// `SubagentTerminalPublished` fact in one transaction: the retry is an
    /// idempotent no-op, a second terminal for the same child violates the
    /// lifecycle, and the provenance rules (Agent-authored success versus
    /// Runtime-authored notice) are enforced.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn subagent_terminal_publication_is_idempotent_and_terminal_unique() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
        let message_id = MessageId::new("subagent-notification-1");
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
        // The ownership commit opens the lifecycle before the terminal. The
        // event identity is the canonical
        // `subagent-committed-event:{id}` the registry writes in production
        // (the shared `subagent_ownership_event_id` helper); terminal
        // validation resolves the ownership fact through that unique event
        // identity.
        store
            .append_event(envelope(
                &conversation_id,
                crate::runtime::subagent::subagent_ownership_event_id(&subagent_id).as_ref(),
                None,
                RuntimeEvent::SubagentOwnershipCommitted {
                    subagent_id: subagent_id.clone(),
                    child_agent_id: child_agent_id.clone(),
                    child_conversation_id: crate::runtime::identity::ConversationId::new(
                        subagent_id.as_str(),
                    ),
                    tool_call_id: crate::runtime::identity::ToolCallId::new("call-sub"),
                    profile: "explore".to_owned(),
                },
            ))
            .unwrap();
        let event = envelope(
            &conversation_id,
            "subagent-event-1",
            None,
            RuntimeEvent::SubagentTerminalPublished {
                subagent_id: subagent_id.clone(),
                child_agent_id: child_agent_id.clone(),
                message_id: message_id.clone(),
                state: crate::events::types::SubagentTerminalState::Succeeded,
            },
        );
        // A successful child answer is authored by the child agent.
        let draft_for_store = || InboundDraft {
            message_id: Some(message_id.clone()),
            source: UserSource::Agent {
                agent_id: child_agent_id.clone(),
            },
            kind: InboundKind::Message,
            content: draft("subagent").content,
            timestamp,
            correlation: Some(format!("subagent-terminal:{subagent_id}")),
        };
        let (accepted, persisted) = store
            .accept_inbound_with_event(draft_for_store(), event.clone())
            .unwrap();
        assert!(!accepted.retried);
        assert_eq!(persisted.sequence, 2, "the ownership commit is sequence 1");

        let (retried, persisted_retry) = store
            .accept_inbound_with_event(draft_for_store(), event)
            .unwrap();
        assert!(retried.retried);
        assert_eq!(persisted_retry.sequence, persisted.sequence);

        // The terminal fact closed the lifecycle, so a second terminal for
        // the same child violates it.
        let second_message_id = MessageId::new("subagent-notification-2");
        let second_event = envelope(
            &conversation_id,
            "subagent-event-2",
            None,
            RuntimeEvent::SubagentTerminalPublished {
                subagent_id: subagent_id.clone(),
                child_agent_id: child_agent_id.clone(),
                message_id: second_message_id.clone(),
                state: crate::events::types::SubagentTerminalState::Failed,
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
            correlation: Some("subagent-terminal:other".to_owned()),
        };
        assert!(matches!(
            store.accept_inbound_with_event(second_draft, second_event),
            Err(ConversationStoreError::TerminalViolation(_))
        ));
        assert_eq!(store.load_pending().unwrap().len(), 1);

        // Provenance: a failed terminal must be a Runtime-authored notice —
        // an Agent-authored one is an ineligible publication.
        let other_id = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
        let other_message_id = MessageId::new("subagent-notification-3");
        let wrong_provenance = envelope(
            &conversation_id,
            "subagent-event-3",
            None,
            RuntimeEvent::SubagentTerminalPublished {
                subagent_id: other_id.clone(),
                child_agent_id: crate::runtime::identity::AgentId::new("agent-other"),
                message_id: other_message_id.clone(),
                state: crate::events::types::SubagentTerminalState::Failed,
            },
        );
        let wrong_draft = InboundDraft {
            message_id: Some(other_message_id),
            source: UserSource::Agent {
                agent_id: crate::runtime::identity::AgentId::new("agent-other"),
            },
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "wrong".to_owned(),
            })],
            timestamp,
            correlation: Some(format!("subagent-terminal:{other_id}")),
        };
        assert!(matches!(
            store.accept_inbound_with_event(wrong_draft, wrong_provenance),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(store.load_pending().unwrap().len(), 1);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn subagent_terminal_provenance_is_authorized_by_ownership_fact() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();

        let own = |ordinal: u64, child_agent_id: &AgentId| {
            let subagent_id =
                crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
            store
                .append_event(envelope(
                    &conversation_id,
                    crate::runtime::subagent::subagent_ownership_event_id(&subagent_id).as_ref(),
                    None,
                    RuntimeEvent::SubagentOwnershipCommitted {
                        subagent_id,
                        child_agent_id: child_agent_id.clone(),
                        child_conversation_id: crate::runtime::identity::ConversationId::new(
                            format!("child-{ordinal}"),
                        ),
                        tool_call_id: crate::runtime::identity::ToolCallId::new(format!(
                            "call-{ordinal}"
                        )),
                        profile: "explore".to_owned(),
                    },
                ))
                .expect("ownership fact");
        };
        let publish = |ordinal: u64,
                       claimed_child: &AgentId,
                       state: SubagentTerminalState,
                       source: UserSource,
                       suffix: &str|
         -> Result<(), ConversationStoreError> {
            let subagent_id =
                crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
            let message_id = MessageId::new(format!("terminal-{ordinal}-{suffix}"));
            let event = envelope(
                &conversation_id,
                &format!("terminal-{ordinal}-{suffix}"),
                None,
                RuntimeEvent::SubagentTerminalPublished {
                    subagent_id: subagent_id.clone(),
                    child_agent_id: claimed_child.clone(),
                    message_id: message_id.clone(),
                    state,
                },
            );
            let draft = InboundDraft {
                message_id: Some(message_id),
                source,
                kind: InboundKind::Message,
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "terminal".to_owned(),
                })],
                timestamp,
                correlation: Some(format!("subagent-terminal:{subagent_id}")),
            };
            store.accept_inbound_with_event(draft, event).map(|_| ())
        };

        let child_a = AgentId::new("agent-a");
        let child_b = AgentId::new("agent-b");

        // A terminal for a child with no durable ownership fact is an
        // invalid reference: the deterministic ownership-identity lookup
        // finds no opening fact.
        assert!(matches!(
            publish(
                50,
                &child_a,
                SubagentTerminalState::Succeeded,
                UserSource::Agent {
                    agent_id: child_a.clone()
                },
                "missing-ownership"
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));

        // The event's repeated child identity is not authority: S owns A,
        // so a success claiming B and authored by B is rejected.
        own(10, &child_a);
        assert!(matches!(
            publish(
                10,
                &child_b,
                SubagentTerminalState::Succeeded,
                UserSource::Agent {
                    agent_id: child_b.clone()
                },
                "wrong-child"
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));

        // Correct child provenance succeeds.
        own(11, &child_a);
        publish(
            11,
            &child_a,
            SubagentTerminalState::Succeeded,
            UserSource::Agent {
                agent_id: child_a.clone(),
            },
            "success",
        )
        .expect("correct Agent(A) success");

        // Success must not be Runtime-authored.
        own(12, &child_a);
        assert!(matches!(
            publish(
                12,
                &child_a,
                SubagentTerminalState::Succeeded,
                UserSource::Runtime,
                "runtime-success"
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));

        // A failed/cancelled/interrupted terminal must not be Agent-authored.
        own(13, &child_a);
        assert!(matches!(
            publish(
                13,
                &child_a,
                SubagentTerminalState::Failed,
                UserSource::Agent {
                    agent_id: child_a.clone()
                },
                "agent-failure"
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));

        for (ordinal, state) in [
            (14, SubagentTerminalState::Failed),
            (15, SubagentTerminalState::Cancelled),
            (16, SubagentTerminalState::Interrupted),
        ] {
            own(ordinal, &child_a);
            publish(
                ordinal,
                &child_a,
                state,
                UserSource::Runtime,
                "runtime-terminal",
            )
            .expect("Runtime-authored terminal");
        }
    }

    /// The durable identity of a `SubagentOwnershipCommitted` fact is
    /// canonical: the `EventId` must be the deterministic
    /// `subagent-committed-event:{id}` derived from the very `SubagentId`
    /// embedded in the payload. A mismatched pair is malformed and must
    /// never enter durable authority — no Event Journal row and no
    /// lifecycle opening.
    #[test]
    fn subagent_ownership_rejects_a_mismatched_event_identity() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let s1 = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let s2 = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
        // The body names S1 but the EventId is the canonical identity of S2:
        // the pair must be rejected by the write-side validation.
        let malformed = envelope(
            &conversation_id,
            crate::runtime::subagent::subagent_ownership_event_id(&s2).as_ref(),
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id: s1.clone(),
                child_agent_id: AgentId::new("agent-a"),
                child_conversation_id: crate::runtime::identity::ConversationId::new("child-a"),
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-a"),
                profile: "explore".to_owned(),
            },
        );
        assert!(matches!(
            store.append_event(malformed),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert!(
            store
                .read_events(None, 64)
                .expect("events")
                .events
                .is_empty(),
            "no Event Journal row was committed"
        );
        // The correct canonical binding for the same body succeeds.
        let canonical = envelope(
            &conversation_id,
            crate::runtime::subagent::subagent_ownership_event_id(&s1).as_ref(),
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id: s1.clone(),
                child_agent_id: AgentId::new("agent-a"),
                child_conversation_id: crate::runtime::identity::ConversationId::new("child-a"),
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-a"),
                profile: "explore".to_owned(),
            },
        );
        store
            .append_event(canonical)
            .expect("canonical binding succeeds");
    }

    /// Even when a malformed ownership fact exists (reachable only through
    /// a raw database row, since the write path rejects it), the
    /// deterministic terminal-provenance lookup defensively revalidates the
    /// embedded `SubagentId` before trusting `child_agent_id`: the located
    /// fact must actually belong to the requested child.
    #[test]
    fn subagent_terminal_provenance_revalidates_the_embedded_subagent_identity() {
        let store = store();
        store.initialize(&[]).expect("initialize");
        let conversation_id = store.conversation_id().clone();
        let s1 = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let s2 = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
        // Raw-insert a fact whose EventId is canonical for S1 but whose
        // embedded SubagentId is S2 — the state only a bypass of the write
        // path could produce.
        let malformed = envelope(
            &conversation_id,
            crate::runtime::subagent::subagent_ownership_event_id(&s1).as_ref(),
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id: s2.clone(),
                child_agent_id: AgentId::new("agent-b"),
                child_conversation_id: crate::runtime::identity::ConversationId::new("child-b"),
                tool_call_id: crate::runtime::identity::ToolCallId::new("call-b"),
                profile: "explore".to_owned(),
            },
        );
        {
            let mut connection = store.lock().expect("store lock");
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .expect("transaction");
            transaction
                .execute(
                    "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json) VALUES(?1,?2,?3,?4,NULL,NULL,?5)",
                    params![
                        1i64,
                        malformed.event_id.as_str(),
                        i64::from(malformed.schema_version),
                        conversation_id.as_str(),
                        encode(&malformed, "raw subagent ownership").expect("encode")
                    ],
                )
                .expect("raw ownership row");
            transaction
                .execute(
                    "INSERT INTO lifecycle_state(lifecycle_key,terminal_event_id) VALUES(?1,NULL)",
                    [format!("subagent:{s1}")],
                )
                .expect("raw lifecycle opening");
            transaction
                .execute(
                    "UPDATE rustx_store SET next_event_sequence=?1 WHERE id=1",
                    [1i64],
                )
                .expect("bump event sequence");
            transaction.commit().expect("commit raw row");
        }

        // A terminal for S1 resolves the ownership fact by S1's canonical
        // EventId and must reject the embedded-S2 mismatch.
        let message_id = MessageId::new("terminal-s1");
        let event = envelope(
            &conversation_id,
            "terminal-s1-event",
            None,
            RuntimeEvent::SubagentTerminalPublished {
                subagent_id: s1.clone(),
                child_agent_id: AgentId::new("agent-b"),
                message_id: message_id.clone(),
                state: SubagentTerminalState::Succeeded,
            },
        );
        let draft = InboundDraft {
            message_id: Some(message_id),
            source: UserSource::Agent {
                agent_id: AgentId::new("agent-b"),
            },
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock {
                text: "terminal".to_owned(),
            })],
            timestamp: Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
            correlation: Some(format!("subagent-terminal:{s1}")),
        };
        assert!(
            matches!(
                store.accept_inbound_with_event(draft, event),
                Err(ConversationStoreError::InvalidReference(_))
            ),
            "the ownership fact located by S1's canonical EventId belongs to S2 and must be rejected"
        );
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
        assert_eq!(committed.0.sequence, 3);
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
                    RuntimeEvent::ModelRetryScheduled {
                        attempt_number: 1,
                        retry_delay_ms: None,
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
        let (revision, _, _, _) = store
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
            Vec::new(),
            crate::runtime::RuntimeResourceRevision::new(1),
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
            Vec::new(),
            crate::runtime::RuntimeResourceRevision::new(1),
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

    /// Issue #106 intentionally removes canonical System messages and adds
    /// exact System-section/resource fields to `RequestSnapshot`. A version-2
    /// development database is rejected at open; there is no migration or
    /// compatibility decoder for the obsolete representation.
    #[test]
    fn pre_issue_106_schema_version_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-issue-106.sqlite");
        let conversation_id = ConversationId::new("conv-pre-issue-106");
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute("UPDATE rustx_store SET schema_version = 2 WHERE id = 1", [])
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 2,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    /// Issue #108 proposal-state rows are a development-only physical
    /// contract. A version-5 database is rejected explicitly instead of
    /// attempting to interpret its older proposal table or migrate it.
    #[test]
    fn pre_proposal_state_machine_schema_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-proposal-state-machine.sqlite");
        let conversation_id = ConversationId::new("conv-pre-proposal-state-machine");
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute("UPDATE rustx_store SET schema_version = 5 WHERE id = 1", [])
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 5,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }
}
