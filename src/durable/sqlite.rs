//! `SQLite` implementation of the semantic conversation durability contract.
//!
//! One database contains the deliberately separate authority domains:
//! Pending Inbound, the append-only Message Ledger, immutable Surface
//! operations, immutable Request Snapshots, the append-only Event Journal, and
//! the durable publication plane (Issue #108). A narrow transcript ordering
//! spine stores references only; it is not a body store or a second history.
//! The tables share transactions where rustX needs one semantic linearization
//! point, but no table is a serialized `ConversationRecord` or transcript.

use crate::durable::TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};

use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

use sha2::{Digest, Sha256};

use crate::conversation::{SurfaceOp, SurfaceRevision};
use crate::events::interaction::{
    InteractionSubject, interaction_arguments_digest, validate_interaction_settlement,
    validate_interaction_subject,
};
use crate::events::types::{
    EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope, contribution_emission_event_id,
};
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, ContentBlockIndex, ContributionEmission, InboundKind, MessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
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
use crate::runtime::process_death;

use super::inbox::{
    AcceptedInbound, CanonicalMessagePage, ContributionEmissionRecord, ConversationStore,
    ConversationStoreError, DurableConversationHead, EventPage, InboundDraft, LineageSeed,
    ModelTurnStartCommit, ModelTurnStartCommitDisposition, PendingBatch, PendingInboundItem,
    RequestSnapshotPage, SurfaceUserMessageBoundary, SurfaceUserMessageBoundaryPage,
    TRANSCRIPT_PAGE_LIMIT_MAX, TranscriptCommitReceipt, TranscriptCursor, TranscriptEntry,
    TranscriptItem, TranscriptPage,
};

/// Process-wide count of `ConversationStore` open-path entries
/// (Issue #386). Diagnostics-grade: the Session Catalog's display
/// projection exists so listing/pagination/search open zero stores, and
/// this counter is how tests and benchmarks prove it. Every constructor
/// that opens a `SQLite` connection increments it exactly once, in
/// production builds as in tests.
static CONVERSATION_STORE_OPENS: AtomicU64 = AtomicU64::new(0);

// Per-thread count of `ConversationStore` open-path entries (Issue #386).
// The global `conversation_store_open_count` is useless inside the parallel
// lib-test binary — unrelated tests open stores concurrently — so acceptance
// tests measure the per-thread delta instead. This is faithful for the
// native list/search path: `SessionCatalog::list_page`/`summary` and the App
// Server `session/list` handler (which runs synchronously under the
// connection task on a current-thread test runtime) execute their store
// opens — if any — on the calling thread. Async tests must therefore use
// the default current-thread `#[tokio::test]` flavor and must not
// `spawn_blocking` between the two snapshots of the counter.
#[cfg(test)]
thread_local! {
    static CONVERSATION_STORE_OPENS_ON_THREAD: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

/// The number of `SqliteConversationStore` opens this process has performed.
#[must_use]
pub fn conversation_store_open_count() -> u64 {
    CONVERSATION_STORE_OPENS.load(Ordering::Relaxed)
}

/// The number of `SqliteConversationStore` opens performed on the calling
/// thread. See [`CONVERSATION_STORE_OPENS_ON_THREAD`] for why the per-thread
/// delta — not the global count — is the contamination-free measurement.
#[cfg(test)]
#[must_use]
pub(crate) fn conversation_store_opens_on_this_thread() -> u64 {
    CONVERSATION_STORE_OPENS_ON_THREAD.with(std::cell::Cell::get)
}

/// Records one store open on both the process-wide and the per-thread
/// diagnostics counters.
fn count_conversation_store_open() {
    CONVERSATION_STORE_OPENS.fetch_add(1, Ordering::Relaxed);
    #[cfg(test)]
    CONVERSATION_STORE_OPENS_ON_THREAD.with(|opens| opens.set(opens.get() + 1));
}

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
/// store invariants; and a Questionnaire settlement must satisfy the exact
/// requested Questionnaire. No new table or column was needed — the generation
/// proof reuses the retained publication ownership v5 and v6 introduced.
/// Version 8 froze the Issue #110 derived transcript ordering spine: accepted
/// inbound, visible canonical messages, publication audits, and interaction
/// audit facts receive one durable reference position. Bodies remain owned by
/// Pending Inbound, the Message Ledger, the publication plane, or the Event
/// Journal.
///
/// Version 9 froze the Issue #111 durable **answer obligation**: the adoption
/// transaction now commits a [`RuntimeEvent::InboundTurnAdopted`] fact naming
/// the exact adopted batch, and startup recovery decides continuation from
/// that fact alone rather than from canonical shape.
///
/// This is a *semantic* format change with no table change, and it is exactly
/// the kind that must gate open. A v8 database recorded no such fact, so a v9
/// reader would read "this conversation was never adopted anything it still
/// owes an answer for" from a journal that simply predates the vocabulary —
/// and would silently strand precisely the crash states (an adopted turn
/// killed in the attempt-start window) that the obligation exists to rescue.
/// Refusing the file states that honestly instead.
///
/// Version 10 freezes the structured Questionnaire interaction audit
/// vocabulary introduced by Issue #126: the requested subject stores one
/// bounded questionnaire by value, and terminal settlements distinguish a
/// submitted answer set from an explicit decline or owning-attempt
/// cancellation. A v9 journal can contain the obsolete Question payloads,
/// so it must fail at store open rather than being interpreted by a v10
/// reader.
///
/// Version 11 freezes the structured Agent Status generation descriptor
/// introduced by Issue #131: canonical Agent Status context messages carry
/// their UTC generation instant and typed admitted-module membership. A v10
/// Ledger cannot answer Surface visibility for the closed status modules
/// without renderer parsing, so it must fail at store open rather than being
/// interpreted by a v11 reader.
///
/// Version 12 freezes the complete Issue #130 Todo Agent Status durability
/// contract: semantic emission facts and their bounded latest-emission heads
/// are committed with model-turn start, and the conversation-owned
/// `logical_step_sequence` records one unit for each newly committed first
/// request of a logical primary model step. A v11 database has neither this
/// final suppression format nor the request-snapshot emission binding, so it
/// is refused.
///
/// Version 13 freezes Issue #134's per-request retry correlation and failed
/// request usage payload. The journal remains envelope-version 1, but an old
/// store cannot be interpreted under the new request-recovery vocabulary.
///
/// Version 14 freezes Issue #136's typed tool cancellation phase.
/// A version-13 store has only the undifferentiated cancellation status, so it
/// is rejected rather than decoded with an invented or default phase.
///
/// Version 15 freezes Issue #137's one-shot unresolved model-output carryover:
/// the store root owns the pending Publication Stream identity, and each
/// Request Snapshot may freeze the exact request-only bounded representation
/// and canonical insertion anchor. The audit remains the sole body authority;
/// no raw carryover body is stored in the conversation schema.
///
/// Version 16 freezes Issue #140's typed compaction-summary metadata: a
/// canonical compaction summary now carries validated cumulative
/// file-operation metadata on its kind, and the summary text follows the
/// fixed structured Markdown contract with metadata-derived file sections.
/// A v15 Ledger records the obsolete unit-kind summary, which cannot be
/// interpreted as typed metadata, so it is rejected rather than decoded with
/// invented empty metadata.
///
/// Version 17 freezes Issue #144's named-subagent ownership identity: the
/// durable `SubagentOwnershipCommitted` fact carries `(agent,
/// definition_digest)` instead of a profile name, so an already-committed
/// child stays bound to the definition it actually started with even after a
/// configuration publication redefines that agent name.
///
/// Version 18 freezes Issue #146's workspace authority facts: a committed
/// child records its exact workspace snapshot, including the committed base
/// and runtime-created ref when isolated. A terminal event may additionally
/// carry the runtime-observed preserved workspace handoff, so recovery cannot
/// make retained child work undiscoverable. A v17 database has no way to
/// distinguish the workspace authority, so it is rejected rather than decoded
/// with invented shared-workspace defaults.
///
/// Version 19 freezes Issue #83's native Workflow execution facts. A version
/// 18 journal cannot decode the Workflow lifecycle vocabulary, and an older
/// runtime must not open a store after those facts have been appended, so it
/// is rejected rather than interpreted through a compatibility path.
///
/// Version 20 replaces Issue #146's conflated subagent workspace path with
/// Issue #187's explicit logical-child scope and physical-worktree ownership
/// facts. A v19 journal can contain the obsolete shape, so it is rejected
/// rather than decoded with an invented repository-relative authority.
///
/// Version 22 freezes Issue #190's retained-workspace disposal resource
/// lifecycle: a durable disposal intent precedes physical mutation, typed
/// settlement facts distinguish an authorized partial worktree removal from a
/// final disposed resource, and terminal settlement facts preserve unresolved
/// physical ownership durably. A v21 journal has no durable authority for
/// recovery of the unresolved resource state, so it is rejected rather than
/// guessed.
///
/// A v3/v4/v5/v6/v7/v8/v9/v10/v11/v12/v13/v15/v15/v16/v17/v18/v19/v21/v21 database
/// must fail at store open; there is no migration or compatibility path.
/// Version 23 preserves Denied in detached terminal facts (Issue #206).
/// Version 24 adds Workflow block/node instance lifecycle facts (Issue #217).
/// Older stores are rejected; there is no compatibility decoding.
/// Version 26 adds typed native deadline interruption to interaction
/// outcomes and durable approval settlement, without fabricating user intent.
/// Version 28 separates proven final Workflow candidates from last-proven
/// `PhysicalSettlement` recovery guards. Older stores lack this source-content
/// disposal authority and are rejected without migration.
/// Version 29 adds Review audit and required Questionnaire invocation identity.
/// Version 30 adds bounded Loop iteration settlement and explicit exit facts.
///
/// Version 31 freezes Issue #242's provider-independent typed Questionnaire
/// interaction audit representation. A requested Questionnaire subject now
/// carries the canonical `InteractionRequester` identity and, for each
/// question, a typed `AnswerSpecification` (`Text`, `Number`, `Integer`,
/// `Boolean`, `SingleChoice`, `MultiChoice`) in place of the obsolete
/// choice-only `options` / `multi_select` pair. A settled submission carries
/// typed scalar answers and addresses a choice by its zero-based option
/// **index** rather than by the authored label the old
/// `single_option`/`multiple_option` answers echoed back. Finite `Number`
/// values are stored in their canonical binary64 encoding.
///
/// This is a semantic format change with no table change, and it is exactly
/// the kind that must gate open: a v31 journal can contain the obsolete
/// Questionnaire payload, which the typed vocabulary cannot interpret — an
/// authored label is not an option index, and a v31 request records no
/// requester identity at all. Refusing the file states that honestly rather
/// than decoding an old audit fact under invented semantics.
///
/// Version 32 freezes Issue #258's effective child execution-profile identity
/// into the durable subagent ownership fact. `SubagentOwnershipCommitted` now
/// carries `profile_digest` beside `definition_digest`, because a named role
/// is only a child's *default* profile once an authorized invocation override
/// can replace its tools, Skills, or extensions: two children of one role can
/// share a definition digest and execute materially different contracts.
///
/// A v31 journal records ownership facts with no profile identity at all, and
/// there is nothing a v33 reader could honestly make of them — recomputing the
/// value from the current role definition or the current resource generation
/// is exactly the derivation the durable fact exists to prevent. So the file
/// is refused at open rather than decoded with an invented or reconstructed
/// identity.
///
/// The Runtime Client protocol version and the
/// [`EVENT_SCHEMA_VERSION`](crate::events::EVENT_SCHEMA_VERSION) envelope
/// framing are independent version domains and are unchanged by this: the
/// envelope shape did not move, only the semantic vocabulary stored inside
/// `events.event_json`.
/// Version 33 originally required rollback journaling for non-creating management reads
/// and the separated local product workspace allocation contract.
/// Version 34 adds native revisioned Goal state and atomic Goal/inbound accounting.
/// Version 36 records Session upload facts and request-time path projection.
/// Version 35 requires the fixed Journal presentation indexes for bounded Trace
/// seeks. Older development stores are rejected, never lazily repaired.
/// Version 37 adds native compare-and-set revisions to Pending Inbound.
/// Version 39 introduced the Tool occurrence index.
/// Version 40 makes Tool-result occurrence ownership canonical; prior shapes are refused.
/// Version 41 adds generation evidence to persisted request terminal events.
/// Version 42 retains immutable completed-response provenance in lineage bootstrap.
/// Version 46 stores complete executable Agent authority privately; ownership
/// events contain only an opaque Agent identity reference.
/// Older development stores are rejected; secrets never enter execution facts.
/// Version 47 retains finite Workflow physical-settlement evidence independently
/// of terminal publication. Older development stores lack the required proof.
/// Version 48 retains native turn-reading provenance and the semantic read-mutation epoch.
/// Version 49 separates immutable inherited outcomes from live execution state.
/// Version 50 retains the required independent task title in private Agent authority.
/// Version 51 stores immutable checkpoint statistics and local completion correlation
/// with the canonical Ledger summary, and general Model Profiles in request snapshots
/// and Session model configuration. Older development stores are refused.
/// Version 52 requires identity-bearing message fact lookup and a qualifying
/// context-reading expression index over immutable Journal evidence.
/// Version 55 indexes reverse immutable Turn membership for exact validation.
/// Version 56 combines native child history/provenance with frozen shared
/// Session workspace ownership and removes Candidate/worktree lifecycle facts.
/// Version 57 requires the immutable inherited transcript watermark in bootstrap identity.
/// Version 58 adds transaction-owned content byte segments and exact versioned
/// publication frontier evidence for bounded observation range reads.
/// Earlier development stores are rejected without migration.
/// Immutable context projection plus finite bounded observation layouts.
/// Version 62 gives request fields explicit inline-header or original-span storage.
/// Version 67 stores child lookup paths rather than duplicate Source Fact payloads.
/// Version 68 binds canonical binary `ContentRefs` to frozen fixed-chunk digests.
/// Version 69 requires explicit native artifact settlement evidence.
/// Version 70 removes the redundant serialized active history from the Surface head.
/// Version 71 stores authenticated Source pages with a bounded binary codec.
/// Version 72 excludes unrelated NULL-key rows from scoped event indexes.
/// Version 73 derives state field previews lazily from retained geometry and bytes.
/// Version 74 removes independent original-capture Source witness seats.
pub const SQLITE_SCHEMA_VERSION: i64 = 74;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactionFaultOperation {
    /// Fail before immutable checkpoint content insertion.
    BeforeContentInsert,
    /// Fail after immutable checkpoint content has staged.
    AfterContentInsert,
    /// Fail after the historical projection activation has staged.
    AfterActivationInsert,
    /// Fail after the Projection Head transition has staged.
    AfterHeadTransition,
    /// Fail before the Event Journal insert.
    BeforeCompletionInsert,
    /// Fail after the Event Journal fact has staged.
    AfterCompletionInsert,
    AfterReceiptInsert,
    #[cfg(test)]
    AfterCommit,
    #[cfg(test)]
    AfterCommitOneHotReadFailure,
    #[cfg(test)]
    AfterCommitTwoHotReadFailures,
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
    AfterCompletionInsert,
    /// Fail after the contribution emission fact has staged.
    AfterContributionEventInsert,
    /// Fail after the latest-emission head has staged.
    AfterContributionHeadUpsert,
}

#[cfg(test)]
pub(crate) struct HiddenContextAuditFixture {
    pub store: Arc<SqliteConversationStore>,
    pub before: crate::durable::observation::ObservationReadCut,
    pub context: TranscriptCommitReceipt,
    pub audit: crate::durable::inbox::PublicationAuditReceipt,
}

/// The native durable conversation authority for one conversation.
pub struct SqliteConversationStore {
    conversation_id: ConversationId,
    conn: Arc<ConnectionOwner>,
    read_cancel: Option<Arc<AtomicBool>>,
    #[cfg(test)]
    read_progress_entered: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    journal_observer: Mutex<Option<(Arc<dyn super::presentation::JournalObserver>, u64)>>,
    lifecycle: Option<Arc<crate::runtime::local_storage::ConversationAccess>>,
    #[cfg(test)]
    pub(crate) fail_accept_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    head_read_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    // Park only after SQLite has granted the writer, before reading pending rows.
    #[cfg(test)]
    pending_mutation_transaction_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    pending_adoption_transaction_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    pub(crate) fail_adopt_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_select_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) checkpoint_read_counts: Arc<[AtomicUsize; 3]>,
    #[cfg(test)]
    fail_projection_history_remaining: AtomicUsize,
    #[cfg(test)]
    projection_commit_return_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    pub(crate) fail_event_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) fail_terminal_event_remaining: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) terminal_event_attempts: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) request_snapshot_page_reads: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) request_snapshot_body_reads: Arc<AtomicUsize>,
    #[cfg(test)]
    pub(crate) contribution_head_lookup_reads: Arc<AtomicUsize>,
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

/// A connection lease owns SQL, not the condition mutex. Cancellation can
/// therefore wake a waiting presentation reader without waiting for SQL.
struct ConnectionOwner {
    available: Mutex<Option<Connection>>,
    wake: Condvar,
    poisoned: AtomicBool,
    manifest: Arc<manifest::Owner>,
    append_validation: Arc<append_validation::Owner>,
}
impl ConnectionOwner {
    #[cfg(test)]
    fn lock(&self) -> Result<ConnectionLease<'_>, ConversationStoreError> {
        self.acquire(None)
    }
    fn acquire(
        &self,
        cancelled: Option<&AtomicBool>,
    ) -> Result<ConnectionLease<'_>, ConversationStoreError> {
        let mut slot = self
            .available
            .lock()
            .map_err(|_| storage("connection poisoned"))?;
        loop {
            if self.poisoned.load(Ordering::Acquire) {
                return Err(storage("connection poisoned"));
            }
            if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                return Err(storage("presentation read cancelled"));
            }
            if let Some(connection) = slot.take() {
                return Ok(ConnectionLease {
                    connection: Some(connection),
                    owner: self,
                });
            }
            slot = blocking_wait(|| self.wake.wait(slot))
                .map_err(|_| storage("connection poisoned"))?;
        }
    }
}

/// Run a wait for another thread's progress — the connection owner's
/// release, or another SQLite writer's lock — without parking the Tokio
/// worker that called into this synchronous store. On a multi-thread runtime
/// the worker's core is handed to another thread first, so protocol
/// admission, transport and drain supervision keep running while a durable
/// write waits. Elsewhere (current-thread runtimes, plain threads) the wait
/// simply runs here.
fn blocking_wait<T>(wait: impl FnOnce() -> T) -> T {
    if tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(wait)
    } else {
        wait()
    }
}

/// The store's writer budget for a database another connection has locked.
const BUSY_TIMEOUT_MS: i32 = 5000;

/// SQLite's own busy-timeout schedule (`sqliteDefaultBusyCallback`) under
/// [`BUSY_TIMEOUT_MS`]: the same delays and the same total budget as
/// `busy_timeout`, but each sleep is a [`blocking_wait`]. `count` is the
/// number of earlier invocations for this lock attempt.
fn writer_busy(count: i32) -> bool {
    let Some(delay) = writer_busy_delay(count) else {
        return false;
    };
    #[cfg(test)]
    busy_probe::invoked(count);
    blocking_wait(|| std::thread::sleep(delay));
    true
}

/// The sleep before retry `count`, or `None` once the budget is spent.
fn writer_busy_delay(count: i32) -> Option<std::time::Duration> {
    const DELAYS: [i32; 12] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100];
    const TOTALS: [i32; 12] = [0, 1, 3, 8, 18, 33, 53, 78, 103, 128, 178, 228];
    let (delay, prior) = usize::try_from(count)
        .ok()
        .and_then(|index| Some((*DELAYS.get(index)?, TOTALS[index])))
        .unwrap_or_else(|| {
            (
                100,
                228_i32.saturating_add(100_i32.saturating_mul(count.saturating_sub(11))),
            )
        });
    let delay = u64::try_from(delay.min(BUSY_TIMEOUT_MS.saturating_sub(prior))).ok()?;
    (delay > 0).then(|| std::time::Duration::from_millis(delay))
}

/// Thread-local observation of [`writer_busy`] for deterministic contention
/// tests: SQLite invokes the handler on the waiting writer's own thread.
#[cfg(test)]
pub(crate) mod busy_probe {
    use std::cell::RefCell;
    type Probe = Box<dyn FnMut(i32)>;
    thread_local! {
        static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
    }
    /// Observe this thread's writer busy waits until the returned guard drops.
    pub(crate) fn observe(probe: impl FnMut(i32) + 'static) -> Guard {
        PROBE.with(|slot| *slot.borrow_mut() = Some(Box::new(probe)));
        Guard
    }
    pub(crate) struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            PROBE.with(|slot| slot.borrow_mut().take());
        }
    }
    pub(super) fn invoked(count: i32) {
        PROBE.with(|slot| {
            if let Some(probe) = slot.borrow_mut().as_mut() {
                probe(count);
            }
        });
    }
}

/// Install the store's writer busy policy ([`writer_busy`]) on `connection`.
fn writer_busy_policy(connection: &Connection) -> rusqlite::Result<()> {
    connection.busy_handler(Some(writer_busy))
}
struct ConnectionLease<'a> {
    connection: Option<Connection>,
    owner: &'a ConnectionOwner,
}
impl std::ops::Deref for ConnectionLease<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection.as_ref().expect("leased connection")
    }
}
impl std::ops::DerefMut for ConnectionLease<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection.as_mut().expect("leased connection")
    }
}
impl Drop for ConnectionLease<'_> {
    fn drop(&mut self) {
        // Preserve the original connection mutex's fail-closed panic semantics.
        if std::thread::panicking() {
            self.owner.poisoned.store(true, Ordering::Release);
        }
        *self.owner.available.lock().expect("connection slot") = self.connection.take();
        self.owner.wake.notify_all();
    }
}

/// Stages only successfully committed Journal receipts before releasing the
/// store lock. Staging cannot advance a Runtime Client cursor. The native owner
/// subsequently publishes the exact receipt after installing its semantic state.
/// Rollbacks and read-only guards produce no receipt.
struct StoreGuard<'a> {
    connection: ConnectionLease<'a>,
    presentation: bool,
    changes: u64,
    observer: &'a Mutex<Option<(Arc<dyn super::presentation::JournalObserver>, u64)>>,
}
impl std::ops::Deref for StoreGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}
impl std::ops::DerefMut for StoreGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}
fn journal_prefix(connection: &Connection) -> Result<u64, ConversationStoreError> {
    let through: i64 = connection
        .query_row(
            "SELECT next_event_sequence FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    sequence_from_i64(through)
}
impl Drop for StoreGuard<'_> {
    fn drop(&mut self) {
        if self.presentation {
            self.connection
                .progress_handler(0, None::<fn() -> bool>)
                .expect("restore SQLite progress policy");
            writer_busy_policy(&self.connection).expect("restore SQLite busy policy");
        }
        if self.connection.total_changes() == self.changes || !self.connection.is_autocommit() {
            return;
        }
        let mut subscription = self.observer.lock().expect("Journal observer lock");
        if let Some((observer, previous)) = subscription.as_mut() {
            match journal_prefix(&self.connection) {
                Ok(through) if through > *previous => {
                    *previous = through;
                    observer.committed(Some(through));
                }
                Err(_) => observer.committed(None),
                _ => {}
            }
        }
    }
}

pub(crate) fn journal_attempt_source(
    sequence: u64,
    attempt: &AttemptId,
) -> Result<crate::runtime_client::response::AttemptIdentitySource, ConversationStoreError> {
    observation_identity::journal(sequence, attempt)
}

impl SqliteConversationStore {
    /// Select projection eligibility at the message boundary inside the same read transaction.
    pub(crate) fn read_message_lineage_cut(
        &self,
        revision: SurfaceRevision,
        message: &MessageId,
        include: bool,
    ) -> Result<super::reading::LineageReadCut, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        lineage::read_at(
            &transaction,
            &self.conversation_id,
            revision,
            None,
            Some((message, include)),
        )
    }

    /// Bound a management traversal while unrelated execution may append events.
    pub(crate) fn event_high_watermark(&self) -> Result<u64, ConversationStoreError> {
        self.lock()?
            .query_row("SELECT coalesce(max(sequence),0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(|e| storage(format!("event high watermark: {e}")))
    }

    fn ownership_transition(
        &self,
        event: &RuntimeEvent,
    ) -> Result<Option<crate::runtime::local_storage::OwnershipMutation>, ConversationStoreError>
    {
        if matches!(
            event,
            RuntimeEvent::AgentActivationAdmission { .. }
                | RuntimeEvent::SubagentOwnershipCommitted { .. }
                | RuntimeEvent::SubagentTerminalPublished { .. }
                | RuntimeEvent::SubagentPhysicalSettlementProven { .. }
                | RuntimeEvent::SubagentTerminalSettled { .. }
        ) {
            self.lifecycle
                .as_ref()
                .map(|access| {
                    access
                        .ownership_mutation()
                        .map_err(|e| storage(e.to_string()))
                })
                .transpose()
        } else {
            Ok(None)
        }
    }

    pub(crate) fn with_lifecycle(
        mut self,
        guard: Arc<crate::runtime::local_storage::ConversationAccess>,
    ) -> Self {
        self.lifecycle = Some(guard);
        self
    }

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
        count_conversation_store_open();
        #[cfg(test)]
        crate::local_runtime::static_effects::observe(
            crate::local_runtime::static_effects::Effect::State,
        );
        let mut connection = Connection::open(path)
            .map_err(|error| storage(format!("open {}: {error}", path.display())))?;
        // Refuse obsolete development state before changing journal settings.
        reject_legacy_schema(&connection)?;
        if has_table(&connection, "rustx_store")? {
            register_integrity_function(&connection)?;
            validate_existing_schema(&connection)?;
            validate_existing_identity(&connection, &conversation_id)?;
        }
        configure_connection(&mut connection, false)?;
        Self::from_connection(conversation_id, connection)
    }

    /// Opens an existing bound store for management without initializing state.
    ///
    /// # Errors
    /// Missing, unbound, incompatible and wrong-identity stores are rejected.
    pub fn open_existing(
        conversation_id: ConversationId,
        path: &Path,
    ) -> Result<Self, ConversationStoreError> {
        Self::open_bound_existing(
            conversation_id,
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
    }

    /// Startup recovery is an explicit writer operation, never a management read.
    /// It may recover WAL or convert a compatible rollback journal, but cannot
    /// create, initialize, or migrate a store.
    pub(crate) fn recover_existing(
        conversation_id: ConversationId,
        path: &Path,
        guard: &Arc<crate::runtime::local_storage::ProductController>,
    ) -> Result<Self, ConversationStoreError> {
        let path = guard
            .confined(path)
            .map_err(|error| storage(error.to_string()))?;
        let access = Arc::new(
            crate::runtime::local_storage::ConversationAccess::existing_for_controller(
                guard.clone(),
                path.parent().ok_or_else(|| storage("missing allocation"))?,
            )
            .map_err(|e| storage(e.to_string()))?,
        );
        let store = Self::open_bound_existing(
            conversation_id,
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        configure_connection(&mut *store.lock()?, false)?;
        Ok(store.with_lifecycle(access))
    }

    fn open_bound_existing(
        conversation_id: ConversationId,
        path: &Path,
        flags: rusqlite::OpenFlags,
    ) -> Result<Self, ConversationStoreError> {
        use std::io::Read as _;
        count_conversation_store_open();
        // Reject unsupported file formats before SQLite access. WAL readers may
        // create/rebuild physical sidecars; that is not application-state repair.
        let mut header = [0; 20];
        std::fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut header))
            .map_err(|error| storage(format!("read existing database header: {error}")))?;
        if &header[..16] != b"SQLite format 3\0" || !matches!(&header[18..20], [1, 1] | [2, 2]) {
            return Err(storage(
                "existing-only access requires a supported SQLite file format",
            ));
        }
        let connection = Connection::open_with_flags(path, flags)
            .map_err(|error| storage(format!("open existing {}: {error}", path.display())))?;
        let mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|error| storage(format!("read journal mode: {error}")))?;
        if mode != "wal"
            && !(mode == "delete" && flags.contains(rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE))
        {
            return Err(storage("existing-only management requires WAL storage"));
        }
        writer_busy_policy(&connection)
            .map_err(|error| storage(format!("configure SQLite busy policy: {error}")))?;
        register_integrity_function(&connection)?;
        validate_existing_schema(&connection)?;
        validate_existing_identity(&connection, &conversation_id)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")
            .map_err(|error| storage(format!("configure existing SQLite: {error}")))?;
        Self::from_validated_connection(conversation_id, connection)
    }

    /// Creates an in-memory store for tests and headless ephemeral runs.
    ///
    /// # Errors
    ///
    /// Returns an error if the in-memory `SQLite` connection cannot be
    /// configured or initialized.
    pub fn in_memory(conversation_id: ConversationId) -> Result<Self, ConversationStoreError> {
        count_conversation_store_open();
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
            create_schema(&mut connection)?;
        }
        bind_identity(&mut connection, &conversation_id)?;
        Self::from_validated_connection(conversation_id, connection)
    }

    fn from_validated_connection(
        conversation_id: ConversationId,
        connection: Connection,
    ) -> Result<Self, ConversationStoreError> {
        let append_validation = append_validation::Owner::new(&connection)?;
        let rollback = Arc::clone(&append_validation);
        connection
            .rollback_hook(Some(move || rollback.invalidate()))
            .map_err(|error| storage(format!("install canonical rollback observer: {error}")))?;
        let manifest = manifest::Owner::install(&connection, Arc::clone(&append_validation))?;
        Ok(Self::from_connection_owner(
            conversation_id,
            Arc::new(ConnectionOwner {
                available: Mutex::new(Some(connection)),
                manifest,
                append_validation,
                wake: Condvar::new(),
                poisoned: AtomicBool::new(false),
            }),
        ))
    }

    fn from_connection_owner(conversation_id: ConversationId, conn: Arc<ConnectionOwner>) -> Self {
        Self {
            conversation_id,
            conn,
            read_cancel: None,
            #[cfg(test)]
            read_progress_entered: Mutex::new(None),
            journal_observer: Mutex::new(None),
            lifecycle: None,
            #[cfg(test)]
            head_read_probe: Mutex::new(None),
            #[cfg(test)]
            pending_mutation_transaction_probe: Mutex::new(None),
            #[cfg(test)]
            pending_adoption_transaction_probe: Mutex::new(None),
            #[cfg(test)]
            fail_accept_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            fail_adopt_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            fail_select_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            checkpoint_read_counts: Arc::new(std::array::from_fn(|_| AtomicUsize::new(0))),
            #[cfg(test)]
            fail_projection_history_remaining: AtomicUsize::new(0),
            #[cfg(test)]
            projection_commit_return_probe: Mutex::new(None),
            #[cfg(test)]
            fail_event_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            fail_terminal_event_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            terminal_event_attempts: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            request_snapshot_page_reads: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            request_snapshot_body_reads: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            contribution_head_lookup_reads: Arc::new(AtomicUsize::new(0)),
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
        }
    }

    fn lock(&self) -> Result<StoreGuard<'_>, ConversationStoreError> {
        #[cfg(debug_assertions)]
        if let Some(cancel) = &self.read_cancel
            && std::env::var_os("RUSTX_TEST_PRESENTATION_READ_BLOCK").is_some()
        {
            let mut slot = self.conn.available.lock().expect("connection slot");
            eprintln!("rustx presentation read blocked at connection admission");
            while !cancel.load(Ordering::Acquire) {
                slot = self.conn.wake.wait(slot).expect("connection slot");
            }
        }
        let connection = self.conn.acquire(self.read_cancel.as_deref())?;
        if let Some(cancel) = &self.read_cancel {
            connection
                .busy_timeout(std::time::Duration::ZERO)
                .map_err(|e| storage(e.to_string()))?;
            let cancel = cancel.clone();
            #[cfg(test)]
            let mut entered = self.read_progress_entered.lock().unwrap().take();
            connection
                .progress_handler(
                    1000,
                    Some(move || {
                        #[cfg(test)]
                        if let Some(entered) = entered.take() {
                            let _ = entered.send(());
                        }
                        cancel.load(Ordering::Acquire)
                    }),
                )
                .map_err(|error| {
                    writer_busy_policy(&connection).expect("restore SQLite busy policy");
                    storage(format!("install SQLite cancellation: {error}"))
                })?;
        }
        let changes = connection.total_changes();
        Ok(StoreGuard {
            connection,
            presentation: self.read_cancel.is_some(),
            changes,
            observer: &self.journal_observer,
        })
    }

    #[cfg(test)]
    pub(crate) fn trace_read_work() -> (usize, usize) {
        observation_trace::read_work()
    }

    #[cfg(test)]
    pub(crate) fn content_read_work() -> (usize, usize) {
        content::last_read_work()
    }

    #[cfg(test)]
    pub(crate) fn request_source_read_work() -> observation_request::composed::Work {
        observation_request::composed::take_work()
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
    pub(crate) fn hidden_context_audit_fixture() -> HiddenContextAuditFixture {
        tests::hidden_context_audit_fixture()
    }

    #[cfg(test)]
    pub(crate) fn canonical_scalar_sources_match(
        &self,
        expected: &crate::durable::inbox::CanonicalObservationBatch,
    ) -> Result<bool, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, &expected.read_cut)?;
        let actual = complete_canonical_scalars(&connection, &expected.read_cut)?;
        Ok(actual.human_activity_at == expected.human_activity_at
            && actual.has_opening_user == expected.has_opening_user
            && actual.opening_preview == expected.opening_preview
            && actual.todos == expected.todos)
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

    /// Returns how many bounded latest-emission head reads the test exercised.
    #[cfg(test)]
    pub(crate) fn contribution_head_lookup_reads(&self) -> usize {
        self.contribution_head_lookup_reads.load(Ordering::SeqCst)
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

    /// Park after durable commit and Journal staging, before returning the receipt.
    #[cfg(test)]
    pub(crate) fn arm_projection_commit_return_probe(&self, probe: impl FnOnce() + Send + 'static) {
        *self
            .projection_commit_return_probe
            .lock()
            .expect("projection return probe") = Some(Box::new(probe));
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

    /// Advances the concrete Todo reminder progress coordinate for one fresh
    /// logical primary request. Overflow retries carry a different request
    /// identity but do not represent new semantic progress, so they reuse the
    /// current sequence. This value is durable only as part of the enclosing
    /// model-turn-start transaction.
    fn advance_logical_step_progress_tx(
        transaction: &rusqlite::Transaction<'_>,
        snapshot: &RequestSnapshot,
    ) -> Result<u64, ConversationStoreError> {
        let current = read_logical_step_progress_tx(transaction)?;
        if snapshot.identity.retry_number != 0 {
            return Ok(current);
        }
        let next = current
            .checked_add(1)
            .ok_or(ConversationStoreError::SequenceExhausted)?;
        transaction
            .execute(
                "UPDATE rustx_store SET logical_step_sequence=?1 WHERE id=1",
                [seq_to_i64(next)?],
            )
            .map_err(|error| storage(format!("update Todo progress sequence: {error}")))?;
        Ok(next)
    }

    /// Inserts the fresh model-turn start facts into `transaction`: the
    /// request-scoped canonical context, the frozen snapshot, the
    /// `ModelRequestStarted` event, and the sequence binding (Issue #12,
    /// M9b).
    #[allow(clippy::too_many_lines)] // One atomic start transition, contribution settlement and native reference validation.
    fn insert_fresh_start_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        context: &[MessageBlock],
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
        selection: &manifest::Selection,
        admission_epoch: u64,
    ) -> Result<crate::durable::ModelTurnStartCommit, ConversationStoreError> {
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
        validate_canonical_frontier(transaction, snapshot.canonical_frontier)?;
        validate_accepted_continuation_source(transaction, &self.conversation_id, snapshot)?;
        let warm = projection::validate_warm_selection(
            transaction,
            &self.conversation_id,
            snapshot,
            &self.conn.manifest,
            admission_epoch,
        )?;
        if !warm {
            projection::validate_request_selection(transaction, &self.conversation_id, snapshot)?;
        }
        // Retry belongs to the Attempt's original adoption, independent of the
        // selected model context, media transformations or later steering.
        // next_event_sequence is the inclusive committed J frontier in this
        // same transaction, before the new RequestStarted row is appended.
        let through = journal_prefix(transaction)?;
        let retry_message_id =
            reading::attempt_prompt(transaction, &snapshot.identity.attempt_id, through)?;
        let json = encode_snapshot(snapshot)?;
        let manifest_digest = stored_snapshot_digest(&json, selection.root);
        transaction
            .execute(
                "INSERT INTO request_snapshots(request_id,canonical_frontier,snapshot_json,manifest_digest,started_sequence,retry_message_id,retry_ledger_position,previous_request_id,input_root,input_root_digest,input_count) VALUES(?1,?2,?3,?4,NULL,?5,(SELECT position FROM message_ledger WHERE message_id=?5),(SELECT request_id FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence IS NOT NULL ORDER BY started_sequence DESC LIMIT 1),?6,?7,?8)",
                params![
                    snapshot.request_id.as_str(),
                    seq_to_i64(snapshot.canonical_frontier.0)?,
                    json,
                    manifest_digest,
                    retry_message_id.as_ref().map(MessageId::as_str),
                    selection.root.map(|r|r.id),
                    selection.root.as_ref().map(|r|r.digest.as_slice()),
                    selection.root.map_or(0,|r|r.count),
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
            tool_occurrence: None,
            observation: None,
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
        // Validate the original stored R and its immutable C dependencies once,
        // after insertion (including any native SQLite trigger effects).
        validate_fresh_input_binding(transaction, snapshot, &json, selection.root)?;
        let input = if warm && self.conn.manifest.epoch.load(Ordering::Acquire) == admission_epoch {
            None
        } else {
            Some(reconstruct_request_start_reference(
                transaction,
                &event,
                Some(&json),
            )?)
        };
        let validated_epoch = self.conn.manifest.epoch.load(Ordering::Acquire);
        if let Some(input) = &input {
            for association in &snapshot.tool_associations {
                let source = &association.source;
                if source.assistant.conversation_id != self.conversation_id {
                    return Err(ConversationStoreError::InvalidReference(
                        "foreign request Tool source".into(),
                    ));
                }
                let original = load_message(transaction, &source.assistant.message_id)?;
                if !source.assistant.matches(&original) {
                    return Err(ConversationStoreError::InvalidReference(
                        "changed request Tool source".into(),
                    ));
                }
                let MessageBlock::Assistant(assistant) = original else {
                    return Err(ConversationStoreError::InvalidReference(
                        "request Tool source is not Assistant".into(),
                    ));
                };
                let Some(crate::message::types::AssistantContentBlock::ToolCall(physical)) =
                    assistant.content.get(source.block.get() as usize)
                else {
                    return Err(ConversationStoreError::InvalidReference(
                        "request Tool source occurrence is missing".into(),
                    ));
                };
                let crate::model::projected::ModelMessageView::Assistant(projected) =
                    input.messages[association.call.message.item_index as usize].view()
                else {
                    unreachable!("frozen association validated");
                };
                let crate::message::types::AssistantContentBlock::ToolCall(call) =
                    &projected[association.call.block.0 as usize]
                else {
                    unreachable!("frozen association validated");
                };
                if call.tool_id != physical.tool_id {
                    return Err(ConversationStoreError::InvalidReference(
                        "request Tool source names a different physical Tool".into(),
                    ));
                }
            }
        }
        let persisted = persist_event_tx_internal(
            transaction,
            &self.conversation_id,
            event,
            false,
            Some(ValidatedRequestStart {
                transaction,
                snapshot,
            }),
        )?;
        #[cfg(test)]
        if self.consume_request_start_fault(RequestStartFaultOperation::AfterCompletionInsert) {
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
        let source_input = input.as_ref().map_or_else(
            || snapshot.input.messages(),
            |r| {
                crate::model::SelectedInputManifest::from_messages(
                    &self.conversation_id,
                    &r.messages,
                )
                .messages()
            },
        );
        index_request_read_layouts(
            transaction,
            &self.conversation_id,
            snapshot,
            &source_input,
            selection,
            retry_message_id.as_ref(),
            context,
            &persisted.event,
        );
        // Todo progress is a conversation-owned semantic coordinate, not a
        // Surface append count. Advance it only for the first request of a
        // logical primary step and do so after this start's request context
        // and ModelRequestStarted fact have staged. The enclosing transaction
        // makes the resulting value the exact origin of any Todo emission
        // committed below; a failed or cancelled start rolls it back.
        Self::advance_logical_step_progress_tx(transaction, snapshot)?;
        // contribution emissions are semantic observations of the logical
        // step, not one emission per transport attempt. A transient or
        // overflow retry may carry the same status metadata in its snapshot
        // when the canonical status message remains on the frozen Surface,
        // but only the initial request owns the durable emission facts.
        let contribution_emissions = if snapshot.identity.retry_number == 0 {
            self.insert_contribution_emissions_tx(transaction, snapshot, timestamp)?
        } else {
            Vec::new()
        };
        consume_pending_carryover_tx(transaction, snapshot)?;
        let result = request_start_observation_tx(
            transaction,
            &self.conversation_id,
            context,
            snapshot,
            persisted.event,
            contribution_emissions,
        )?;
        validate_fresh_input_binding(transaction, snapshot, &json, selection.root)?;
        if self.conn.manifest.epoch.load(Ordering::Acquire) != validated_epoch {
            reconstruct_request_start_reference(transaction, &result.started, Some(&json))?;
        }
        Ok(result)
    }

    fn insert_contribution_emissions_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
        let logical_step_origin = read_logical_step_progress_tx(transaction)?;
        let mut persisted_emissions = Vec::new();
        for contribution in &snapshot.contributions {
            for emission in &contribution.emissions {
                let emission_event = RuntimeEventEnvelope {
                    tool_occurrence: None,
                    observation: None,
                    schema_version: EVENT_SCHEMA_VERSION,
                    event_id: contribution_emission_event_id(
                        &snapshot.request_id,
                        &contribution.producer,
                        emission,
                    ),
                    sequence: 0,
                    conversation_id: self.conversation_id.clone(),
                    attempt_id: Some(snapshot.identity.attempt_id.clone()),
                    turn_id: Some(snapshot.identity.turn.clone()),
                    timestamp,
                    event: RuntimeEvent::ContextContributionEmitted {
                        producer: contribution.producer.clone(),
                        request_id: snapshot.request_id.clone(),
                        message_id: contribution.message_id.clone(),
                        emission: emission.clone(),
                        logical_step_origin,
                    },
                };
                let persisted =
                    persist_event_tx_deferred(transaction, &self.conversation_id, emission_event)?;
                #[cfg(test)]
                if self.consume_request_start_fault(
                    RequestStartFaultOperation::AfterContributionEventInsert,
                ) {
                    return Err(storage(
                        "fault injected: after contribution emission event insert",
                    ));
                }
                upsert_contribution_head_tx(transaction, &persisted.event)?;
                #[cfg(test)]
                if self.consume_request_start_fault(
                    RequestStartFaultOperation::AfterContributionHeadUpsert,
                ) {
                    return Err(storage(
                        "fault injected: after contribution emission head upsert",
                    ));
                }
                persisted_emissions.push(persisted.event);
            }
        }
        Ok(persisted_emissions)
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

impl SqliteConversationStore {
    fn append_event_internal(
        &self,
        event: RuntimeEventEnvelope,
        authority: Option<&crate::runtime::subagent::DurableAgentAuthority>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        let _ownership = self.ownership_transition(&event.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("event transaction: {error}")))?;
        // Recovery may retry after losing a commit acknowledgement. This
        // monotonic resource proof has one canonical event identity; return the
        // already committed receipt (including its original timestamp) rather
        // than stranding an obligation behind the unique event index.
        if matches!(
            event.event,
            RuntimeEvent::SubagentPhysicalSettlementProven { .. }
                | RuntimeEvent::AgentActivationAdmission {
                    phase: crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                        physical_settlement_proven: true
                    },
                    ..
                }
        ) && let Some(existing) = find_event_by_id(&transaction, &event.event_id)?
        {
            let mut retry = event;
            retry.timestamp = existing.timestamp;
            if !same_event_ignoring_sequence(&existing, &retry) {
                return Err(ConversationStoreError::InvalidReference(
                    "physical settlement proof retry conflicts with its committed identity".into(),
                ));
            }
            return Ok(existing);
        }
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
        persist_agent_authority_tx(&transaction, &event.event, authority)?;
        process_death::reach_event("before:event", &event.event);
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("event commit: {error}")))?;
        process_death::reach_event("after:event", &persisted.event.event);
        Ok(persisted.event)
    }
}

impl SqliteConversationStore {
    fn mutate_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
        text: Option<&str>,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, ConversationStoreError> {
        use crate::durable::inbox::PendingMutationOutcome as Outcome;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("pending mutation: {error}")))?;
        #[cfg(test)]
        if let Some(probe) = self
            .pending_mutation_transaction_probe
            .lock()
            .unwrap()
            .take()
        {
            probe();
        }
        let revision: Option<u64> = transaction
            .query_row(
                "SELECT revision FROM pending_inbound WHERE sequence=?1",
                [expected.sequence.get()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        let Some(revision) = revision else {
            return Ok(Outcome::NotPending);
        };
        if revision != expected.revision {
            return Ok(Outcome::Conflict);
        }
        let Some(mut item) = load_pending_exact(&transaction, expected.sequence)? else {
            return Ok(Outcome::NotPending);
        };
        // Producer-correlated runtime work is not user-authored queue content.
        if item.message.source != UserSource::Human
            || item.message.kind != InboundKind::Message
            || item.correlation.is_some()
        {
            return Ok(Outcome::InvalidItem);
        }
        if let Some(text) = text {
            if text.trim().is_empty()
                || !matches!(item.message.content.as_slice(), [UserContentBlock::Text(_)])
            {
                return Ok(Outcome::InvalidItem);
            }
            rewrite_pending_body(&transaction, &mut item, expected, text)?;
        } else {
            let _ = optional_display_layout(&transaction, || {
                content::clear_pending(&transaction, &item.sequence.get().to_string())
            });
            transaction
                .execute(
                    "DELETE FROM pending_inbound WHERE sequence=?1",
                    [seq_to_i64(expected.sequence.get())?],
                )
                .map_err(|error| storage(format!("pending remove: {error}")))?;
            let _ = optional_display_layout(&transaction, || {
                transaction.execute("DELETE FROM observation_headers WHERE reference_kind='message' AND reference_id=?1",[item.message_id.as_str()]).map_err(|error|storage(error.to_string()))?;
                Ok(())
            });
            transaction.execute("DELETE FROM transcript_order WHERE reference_kind='message' AND reference_id=?1", [item.message_id.as_str()])
                .map_err(|error| storage(format!("pending transcript remove: {error}")))?;
            transaction.execute("UPDATE rustx_store SET retained_transcript_count=retained_transcript_count-1,pending_observation_count=pending_observation_count-1 WHERE id=1", []).map_err(|error|storage(error.to_string()))?;
        }
        transaction.execute("UPDATE rustx_store SET reading_mutation_revision=reading_mutation_revision+1 WHERE id=1", [])
            .map_err(|error| storage(format!("reading mutation: {error}")))?;
        let read_cut = reading::cut(&transaction, &self.conversation_id)?;
        let pending_observation = observation_receipt::pending(
            &transaction,
            &self.conversation_id,
            text.map(|_| expected.sequence.get()),
            text.is_none()
                .then_some((expected.sequence.get(), item.transcript_cursor)),
            None,
        );
        transaction
            .commit()
            .map_err(|error| storage(format!("pending mutation commit: {error}")))?;
        Ok(Outcome::Applied {
            read_cut,
            pending_observation,
        })
    }
}

impl ConversationStore for SqliteConversationStore {
    fn observation_trace_facts(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        query: &crate::durable::observation::TraceFactQuery,
    ) -> Result<Vec<crate::durable::observation::TraceFactHeader>, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        observation_trace::facts(&transaction, cut, query)
    }
    fn observation_request_accepted_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
    ) -> Result<
        Option<crate::runtime_client::content::CanonicalContentLocator>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        observation_request::validate_start(&transaction, source_sequence, cut.read.journal)?;
        let position:Option<u64>=transaction.query_row("SELECT observation_accepted_ledger FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.flatten();
        Ok(position
            .filter(|position| *position <= cut.ledger_through)
            .map(|position| {
                crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
                    position,
                }
            }))
    }
    fn observation_request_summary_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::durable::observation::RequestReadSummary>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_trace::summary(&connection, source_sequence, through)
    }
    fn observation_request_predecessor_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::TraceRequestPredecessorHeader>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_trace::predecessor(&connection, source_sequence, through)
    }
    fn observation_request_retry_failure_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::model::error::ModelErrorKind>, ConversationStoreError> {
        let connection = self.lock()?;
        observation_trace::retry_failure(&connection, source_sequence, through)
    }
    fn observation_trace_canonical_summary(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        position: u64,
    ) -> Result<crate::durable::observation::TraceCanonicalSummary, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        observation_trace::canonical(&transaction, cut, position)
    }
    fn observation_trace_tool_result(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<Option<crate::durable::observation::TraceCanonicalSummary>, ConversationStoreError>
    {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        observation_trace::tool_result(&transaction, cut, occurrence)
    }
    fn observation_trace_tool_proposal(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<crate::durable::observation::TraceToolProposalSummary, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        observation_trace::proposal(&transaction, cut, occurrence)
    }
    fn observation_trace_tool_definition(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<Option<crate::runtime_client::trace::TraceFieldSource>, ConversationStoreError>
    {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        observation_trace::tool_definition(&transaction, cut, occurrence)
    }
    fn observation_trace_adoption(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::TraceAdoptionPage, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        observation_trace::adoption_page(&transaction, cut, source_sequence, offset, limit)
    }
    fn observation_request_context_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::TraceRequestContextPage, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        observation_trace::context(&transaction, cut, source_sequence, offset, limit)
    }
    fn request_summary_header_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<crate::durable::observation::RequestObservationHeader, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::header_at(&connection, source_sequence, through)
    }
    fn request_field_header_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
    ) -> Result<crate::durable::observation::RequestObservationField, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::field_header_at(&connection, source_sequence, through, path)
    }
    fn request_field_page_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::page_at(&connection, source_sequence, through, path, offset, limit)
    }
    fn request_value_read_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::read_at(
            &connection,
            source_sequence,
            through,
            path,
            offset,
            max_bytes,
        )
    }
    fn request_outcome_header_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::RequestObservationOutcome>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_request::outcome_at(&connection, source_sequence, through)
    }
    fn request_outcome_read_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::outcome_read_at(
            &connection,
            source_sequence,
            through,
            path,
            offset,
            max_bytes,
        )
    }
    fn request_previous_identity(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<Option<RequestId>, ConversationStoreError> {
        let connection = self.lock()?;
        let previous:Option<String>=connection.query_row("SELECT previous_request_id FROM request_snapshots WHERE request_id=?1 AND started_sequence<=?2",params![request.as_str(),through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.ok_or_else(||ConversationStoreError::InvalidReference("request outside native read cut".into()))?;
        Ok(previous.map(RequestId::new))
    }
    fn request_outcome_header(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::RequestObservationOutcome>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_request::outcome(&connection, request, through)
    }
    fn request_outcome_read(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::outcome_read(&connection, request, through, path, offset, max_bytes)
    }
    fn request_field_header(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
    ) -> Result<crate::durable::observation::RequestObservationField, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::field_header(&connection, request, through, path)
    }
    fn request_identity_at(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<RequestId>, ConversationStoreError> {
        let connection = self.lock()?;
        let request:Option<String>=connection.query_row("SELECT request_id FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1 AND started_sequence<=?2",params![sequence,through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        Ok(request.map(RequestId::new))
    }
    fn request_summary_header(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<crate::durable::observation::RequestObservationHeader, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::header(&connection, request, through)
    }
    fn request_field_page(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::page(&connection, request, through, path, offset, limit)
    }
    fn request_value_read(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::read(&connection, request, through, path, offset, max_bytes)
    }
    fn validate_content_source_cut(
        &self,
        reference: &crate::runtime_client::content::ContentRef,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<(), ConversationStoreError> {
        let connection = self.lock()?;
        content::validate_source_cut(&connection, &self.conversation_id, reference, cut)
    }
    fn observation_inherited_attempt_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        kind: crate::runtime_client::response::InheritedAttemptIdentityKind,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        observation_identity::read(&connection, cut, kind, ordinal, offset, max_bytes)
    }
    #[cfg(test)]
    fn observation_inherited_native_oracle(
        &self,
        messages: &[MessageId],
    ) -> Result<crate::durable::observation::InheritedObservation, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::observation_native_oracle(&connection, messages)
    }
    fn observation_inherited_models(
        &self,
        response_ordinal: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::InheritedModelPage, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::observation_models(&connection, response_ordinal, offset, limit)
    }
    fn observation_inherited_model_read(
        &self,
        response_ordinal: u64,
        model_ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::observation_model_read(
            &connection,
            response_ordinal,
            model_ordinal,
            offset,
            max_bytes,
        )
    }
    fn observation_failure_value_read(
        &self,
        attempt: &AttemptId,
        through: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let chunk = observation_execution::failure_value(
            &transaction,
            attempt,
            through,
            offset,
            max_bytes,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(chunk)
    }
    fn response_identity_source(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript_cursor: u64,
    ) -> Result<Option<crate::durable::observation::ResponseIdentitySource>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if transcript_cursor > cut.read.transcript {
            return Err(ConversationStoreError::InvalidReference(
                "response source beyond native transcript cut".into(),
            ));
        }
        let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_member_holes_sequence_idx WHERE observation_members_required=1 AND observation_members IS NULL AND sequence<=?1)",[cut.read.journal],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        if missing {
            return Err(storage("native response owner header unavailable"));
        }
        let position:Option<u64>=connection.query_row("SELECT position FROM message_ledger INDEXED BY message_ledger_native_transcript_idx WHERE native_transcript=?1 AND native_transcript IS NOT NULL AND position<=?2",params![transcript_cursor,cut.ledger_through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        let Some(mut position) = position else {
            let sequence:Option<u64>=connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_transcript_idx WHERE observation_transcript=?1 AND observation_transcript IS NOT NULL AND sequence<=?2 AND observation_members IS NOT NULL",params![transcript_cursor,cut.read.journal],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
            return Ok(sequence.map(|sequence| {
                crate::durable::observation::ResponseIdentitySource::Attempt { sequence }
            }));
        };
        let proposal:Option<u64>=connection.query_row("SELECT observation_ledger FROM canonical_tool_calls INDEXED BY canonical_tool_calls_result_ledger_idx WHERE result_ledger_position=?1 AND result_ledger_position IS NOT NULL",[position],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        if let Some(proposal) = proposal {
            position = proposal;
        }
        let anchor: Option<Option<u64>> = connection.query_row("SELECT observation_member_anchor FROM events INDEXED BY events_observation_assistant_ledger_idx WHERE observation_assistant_ledger=?1 AND observation_assistant_ledger IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![position,cut.read.journal],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        if let Some(anchor) = anchor.flatten() {
            let sequence:u64=connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_member_anchor_idx WHERE observation_member_anchor=?1 AND observation_members IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![anchor,cut.read.journal],|row|row.get(0)).map_err(|_|storage("native response member header unavailable"))?;
            return Ok(Some(
                crate::durable::observation::ResponseIdentitySource::Attempt { sequence },
            ));
        }
        let ordinal:Option<u64>=connection.query_row("SELECT ordinal FROM inherited_response_provenance WHERE closing_ledger_position=?1",[position],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        Ok(ordinal.map(
            |ordinal| crate::durable::observation::ResponseIdentitySource::Inherited { ordinal },
        ))
    }
    fn inherited_response_summary(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        ordinal: u64,
    ) -> Result<Option<crate::durable::observation::InheritedResponseHeader>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        let bytes:Option<Vec<u8>>=connection.query_row("SELECT display_provenance FROM inherited_response_provenance WHERE ordinal=?1 AND closing_ledger_position<=?2",params![ordinal,cut.ledger_through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        bytes
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string())))
            .transpose()
    }
    fn response_identity_fields(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source: &crate::durable::observation::ResponseIdentitySource,
    ) -> Result<Vec<crate::durable::observation::ResponseIdentityField>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        response_identity_fields(&connection, cut, source)
    }
    fn response_identity_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source: &crate::durable::observation::ResponseIdentitySource,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        let (key, field) = response_identity_key(&connection, cut, source, path)?;
        content::utf8_field_read(
            &connection,
            "response_identity",
            &key,
            field,
            offset,
            max_bytes,
        )
    }
    fn observation_execution_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
    ) -> Result<crate::durable::observation::AttemptExecutionSummary, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "execution outside source cut".into(),
            ));
        }
        observation_execution::summary_selected(&connection, sequence, cut.read.journal)
    }
    fn observation_members_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
    ) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "member outside source cut".into(),
            ));
        }
        observation_members::at_sequence(&connection, sequence)
    }
    fn observation_attempt_summary(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<crate::durable::observation::AttemptExecutionSummary, ConversationStoreError> {
        let connection = self.lock()?;
        observation_execution::summary(&connection, attempt, through)
    }
    fn todo_observation_head(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<crate::durable::inbox::NativeTodoObservation>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        observation_state::todo_head(&connection, 1, cut.ledger_through)
    }

    fn observation_state_source(
        &self,
        domain: &str,
        cut: &super::observation::ObservationReadCut,
    ) -> Result<Option<String>, ConversationStoreError> {
        let connection = self.lock()?;
        observation_state::source(&connection, domain, cut)
    }
    fn observation_state_header(
        &self,
        domain: &str,
        source_id: &str,
        cut: &super::observation::ObservationReadCut,
        path: &[String],
    ) -> Result<super::observation::RequestObservationField, ConversationStoreError> {
        let connection = self.lock()?;
        observation_state::header(&connection, domain, source_id, cut, path)
    }
    fn observation_state_page(
        &self,
        domain: &str,
        source_id: &str,
        cut: &super::observation::ObservationReadCut,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<super::observation::RequestObservationPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation_state::page(&connection, domain, source_id, cut, path, offset, limit)
    }
    fn observation_state_value_read(
        &self,
        domain: &str,
        source_id: &str,
        cut: &super::observation::ObservationReadCut,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_state::read(&connection, domain, source_id, cut, path, offset, max_bytes)
    }
    fn observation_models_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AttemptModelNamePage, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "native owner outside cut".into(),
            ));
        }
        observation_execution::models_at(&connection, sequence, cut.read.journal, offset, limit)
    }
    fn observation_model_read_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "native owner outside cut".into(),
            ));
        }
        observation_execution::model_value_at(
            &connection,
            sequence,
            cut.read.journal,
            ordinal,
            offset,
            max_bytes,
        )
    }
    fn observation_failure_read_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "native owner outside cut".into(),
            ));
        }
        observation_execution::failure_at(&connection, sequence, offset, max_bytes)
    }
    fn observation_models_page(
        &self,
        attempt: &AttemptId,
        through: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AttemptModelNamePage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let page = observation_execution::models(&transaction, attempt, through, offset, limit)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(page)
    }
    fn observation_model_value_read(
        &self,
        attempt: &AttemptId,
        through: u64,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let chunk = observation_execution::model_value(
            &transaction,
            attempt,
            through,
            ordinal,
            offset,
            max_bytes,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(chunk)
    }
    fn observation_request_retry_locator_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<
        Option<crate::runtime_client::content::CanonicalContentLocator>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_request::validate_start(&connection, source_sequence, through)?;
        let position:Option<u64>=connection.query_row("SELECT retry_ledger_position FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        Ok(position.map(|position| {
            crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position }
        }))
    }
    fn observation_request_retry_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<MessageId>, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::validate_start(&connection, source_sequence, through)?;
        let position:Option<u64>=connection.query_row("SELECT retry_ledger_position FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        let Some(position) = position else {
            return Ok(None);
        };
        let bytes:Vec<u8>=connection.query_row("SELECT header_json FROM observation_headers INDEXED BY observation_headers_ledger_idx WHERE ledger_position=?1 AND reference_kind='message'",[position],|row|row.get(0)).map_err(|_|storage("native retry message header unavailable"))?;
        let header: crate::durable::observation::MessageHeader = serde_json::from_slice(&bytes)
            .map_err(|_| storage("native retry message header corrupt"))?;
        Ok(header.identity)
    }
    fn observation_request_retry_locator(
        &self,
        request_id: &RequestId,
    ) -> Result<
        Option<crate::runtime_client::content::CanonicalContentLocator>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        let position: Option<u64> = connection
            .query_row(
                "SELECT retry_ledger_position FROM request_snapshots WHERE request_id=?1",
                [request_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(position.map(|position| {
            crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position }
        }))
    }
    fn observation_request_retry(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<MessageId>, ConversationStoreError> {
        let connection = self.lock()?;
        let id: Option<String> = connection
            .query_row(
                "SELECT CASE WHEN length(CAST(retry_message_id AS BLOB))<=256 THEN retry_message_id ELSE NULL END FROM request_snapshots WHERE request_id=?1",
                [request_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(id.map(MessageId::new))
    }
    fn observation_member_counts(
        &self,
        message_id: &MessageId,
        through: u64,
    ) -> Result<Option<crate::durable::observation::AttemptMemberCounts>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_members::message_members(&connection, message_id, through)
    }
    fn observation_attempt_members(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError> {
        let connection = self.lock()?;
        observation_members::members(&connection, attempt, through)
    }

    fn observation_transcript_process(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript: u64,
    ) -> Result<Option<crate::runtime_client::response::TurnProcessView>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        observation_members::transcript_process(&connection, cut, transcript)
    }
    fn audit_block_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AuditBlockHeaderPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation::audit_blocks(
            &connection,
            &self.conversation_id,
            cut,
            transcript,
            offset,
            limit,
        )
    }
    fn publication_settlement_head(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<
        Vec<crate::runtime_client::snapshot::RuntimeClientPublicationSettlementRecord>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation::publication_settlement_head(&connection, &self.conversation_id, cut)
    }
    fn observation_trace_total(&self, through: u64) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        observation_statistics::trace_total(&connection, through)
    }
    fn observation_occupancy(
        &self,
        through: u64,
    ) -> Result<
        Option<crate::runtime_client::occupancy::RuntimeClientContextOccupancy>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_occupancy::read(&connection, through)
    }
    fn observation_occupancy_model_value_read(
        &self,
        sequence: Option<u64>,
        through: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_occupancy::model_read(&connection, sequence, through, offset, max_bytes)
    }
    fn observation_statistics(
        &self,
        through: u64,
    ) -> Result<crate::runtime_client::response::ConversationStatistics, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_statistics::read(&connection, through)
    }
    fn observation_agent_statistics(
        &self,
        through: u64,
    ) -> Result<crate::runtime_client::agent_statistics::AgentStatistics, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_statistics::agent_read(&connection, through)
    }
    fn pending_header_page(
        &self,
        cut: &crate::durable::reading::ConversationReadCut,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<crate::durable::observation::PendingHeaderPage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::pending_page(
            &transaction,
            &self.conversation_id,
            cut,
            after_sequence,
            limit,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn pending_value_read(
        &self,
        cut: &crate::durable::reading::ConversationReadCut,
        sequence: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::pending_value(
            &transaction,
            &self.conversation_id,
            cut,
            sequence,
            offset,
            max_bytes,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn read_window_headers(
        &self,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::durable::observation::ObservationWindow, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::window(&transaction, &self.conversation_id, at, limit)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }

    fn event_transcript_cursor_at_sequence(
        &self,
        sequence: u64,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        let connection = self.lock()?;
        let cursor: Option<u64> = connection
            .query_row(
                "SELECT observation_transcript FROM events WHERE sequence=?1",
                [sequence],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?
            .flatten();
        Ok(cursor.map(TranscriptCursor::new))
    }
    fn observation_event_kind(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<String>, ConversationStoreError> {
        let connection = self.lock()?;
        observation_event::kind(&connection, sequence, through)
    }
    fn observation_event_witness(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<crate::durable::observation::ObservationEventWitness>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_event::witness(&connection, sequence, through)
    }
    fn event_value_page(
        &self,
        sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation_event::page(&connection, sequence, through, path, offset, limit)
    }
    fn event_value_read(
        &self,
        sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_event::read(&connection, sequence, through, path, offset, max_bytes)
    }
    fn latest_observation_events(
        &self,
        through: u64,
        kinds: &[String],
        attempt: Option<&AttemptId>,
        limit: usize,
    ) -> Result<Vec<crate::durable::observation::ObservationEventHeader>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::latest_events(&connection, through, kinds, attempt, limit)
    }
    fn tool_call_occurrence_at(
        &self,
        call_id: &crate::runtime::identity::ToolCallId,
        attempt: &AttemptId,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<crate::message::types::ToolCallOccurrenceRef>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::tool_occurrence(&connection, &self.conversation_id, call_id, attempt, cut)
    }

    fn tool_association_headers(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        message_id: &MessageId,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::ToolAssociationHeaderPage, ConversationStoreError>
    {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::tools(
            &transaction,
            &self.conversation_id,
            cut,
            message_id,
            offset,
            limit,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }

    fn tool_association_headers_by_locator(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::ToolAssociationHeaderPage, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::tools_by_locator(
            &connection,
            &self.conversation_id,
            cut,
            locator,
            offset,
            limit,
        )
    }
    fn transcript_tool_headers(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::NativeToolObservationPage, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_tools::page(
            &connection,
            &self.conversation_id,
            cut,
            locator,
            offset,
            limit,
        )
    }

    fn ordinary_user_message(
        &self,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
    ) -> Result<bool, ConversationStoreError> {
        let connection = self.lock()?;
        match locator {
            crate::runtime_client::content::CanonicalContentLocator::ByMessageId { message_id } => {
                ordinary_user_by_id(&connection, message_id)
            }
            crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
                position,
            } => connection
                .query_row(
                    "SELECT ordinary_user FROM message_ledger WHERE position=?1",
                    [position],
                    |row| row.get(0),
                )
                .optional()
                .map(|value| value.unwrap_or(false))
                .map_err(|error| storage(error.to_string())),
        }
    }
    fn message_ledger_position(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<u64>, ConversationStoreError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [message_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))
    }
    fn message_header(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<crate::durable::observation::MessageHeader>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::message(&connection, message_id)
    }
    fn message_block_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        message_id: &MessageId,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageBlockHeaderPage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::blocks(
            &transaction,
            &self.conversation_id,
            cut,
            message_id,
            offset,
            limit,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }

    fn message_block_header_page_by_locator(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageBlockHeaderPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation::blocks_by_locator(
            &connection,
            &self.conversation_id,
            cut,
            locator,
            offset,
            limit,
        )
    }
    fn observation_tool_trace_sequence(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        ledger_position: u64,
        block_index: crate::message::types::ContentBlockIndex,
    ) -> Result<Option<u64>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if ledger_position == 0 || ledger_position > cut.ledger_through {
            return Err(ConversationStoreError::InvalidReference(
                "tool trace source beyond captured cut".into(),
            ));
        }
        connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_tool_lifecycle_idx WHERE observation_tool_ledger=?1 AND observation_tool_block=?2 AND observation_tool_phase=1 AND sequence<=?3 ORDER BY sequence DESC LIMIT 1",params![ledger_position,block_index.get(),seq_to_i64(cut.read.journal)?],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))
    }

    fn observation_read_cut(
        &self,
    ) -> Result<crate::durable::observation::ObservationReadCut, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::cut(&transaction, &self.conversation_id)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn message_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageHeaderPage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result =
            observation::messages(&transaction, &self.conversation_id, cut, offset, limit)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn transcript_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<crate::durable::observation::TranscriptHeaderPage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result =
            observation::transcript(&transaction, &self.conversation_id, cut, before, limit)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }

    fn compaction_resource_cut(
        &self,
    ) -> Result<crate::context::resources::ResourceReadCut, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        compaction_resources::cut(&transaction, &self.conversation_id)
    }

    fn compaction_message_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::LedgerFrontier,
        ids: Option<&[MessageId]>,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::resources::ResourcePage<MessageBlock>, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        compaction_resources::messages(&transaction, &self.conversation_id, cut, after, ids, limits)
    }

    fn compaction_dependency(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        source: &crate::model::projected::CanonicalMessageRef,
        body_required: bool,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::checkpoint::CanonicalDependency, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        compaction_resources::dependency(
            &transaction,
            &self.conversation_id,
            cut,
            source,
            body_required,
            limits,
        )
    }
    fn compaction_trajectory_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::LedgerFrontier,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<
        crate::context::resources::ResourcePage<crate::context::resources::TrajectoryMessage>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        compaction_resources::trajectory(&transaction, &self.conversation_id, cut, after, limits)
    }
    fn compaction_event_page(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        after: crate::context::resources::JournalFrontier,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::context::resources::ResourcePage<RuntimeEventEnvelope>, ConversationStoreError>
    {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        compaction_resources::events(&transaction, &self.conversation_id, cut, after, limits)
    }
    fn transcript_attempts(
        &self,
        messages: &[MessageId],
        through: u64,
    ) -> Result<Vec<AttemptId>, ConversationStoreError> {
        let messages: std::collections::BTreeSet<_> = messages.iter().collect();
        let connection = self.lock()?;
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = connection
            .prepare_cached(TRANSCRIPT_ATTEMPTS_SQL)
            .map_err(|error| storage(error.to_string()))?;
        let mut attempts = std::collections::BTreeSet::new();
        for message in messages {
            let rows = statement
                .query_map(params![message.as_str(), seq_to_i64(through)?], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| storage(error.to_string()))?;
            for row in rows {
                attempts.insert(AttemptId::new(
                    row.map_err(|error| storage(error.to_string()))?,
                ));
            }
        }
        Ok(attempts.into_iter().collect())
    }

    fn inherited_responses_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<super::response::CompletedResponseProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::selected_responses(&connection, messages)
    }

    fn load_inherited_turns(
        &self,
    ) -> Result<Vec<super::reading::TurnReadingProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        reading::inherited_turns(&connection)
    }
    fn inherited_turns_for(
        &self,
        messages: &[MessageId],
    ) -> Result<Vec<super::reading::TurnReadingProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::selected_turns(&connection, messages)
    }

    fn read_projection_lineage_cut(
        &self,
        ledger: crate::context::resources::LedgerFrontier,
        activation: crate::context::checkpoint::ProjectionActivationId,
    ) -> Result<super::reading::LineageReadCut, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        lineage::read_at(
            &transaction,
            &self.conversation_id,
            SurfaceRevision::new(ledger.0),
            Some(activation),
            None,
        )
    }

    fn read_lineage_cut(
        &self,
        revision: SurfaceRevision,
    ) -> Result<super::reading::LineageReadCut, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        lineage::read(&transaction, &self.conversation_id, revision)
    }
    fn conversation_read_cut(
        &self,
    ) -> Result<super::reading::ConversationReadCut, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        reading::cut(&transaction, &self.conversation_id)
    }

    fn observation_turns(
        &self,
        offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::runtime_client::turn::ObservedTurnPage, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation_turn::page(&transaction, &self.conversation_id, offset, limit)
    }
    fn observation_file_delivery(
        &self,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        index: u64,
    ) -> Result<crate::durable::observation::NativeFileDelivery, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation_delivery::selected(&transaction, &self.conversation_id, locator, index)
    }
    fn observation_deliveries(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::runtime_client::delivery::RuntimeClientDeliveryHead, ConversationStoreError>
    {
        content::validate_canonical_locator(locator)?;
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        let position = content::canonical_position(&transaction, locator)?;
        if position > cut.ledger_through {
            return Err(ConversationStoreError::InvalidReference(
                "delivery beyond captured canonical cut".into(),
            ));
        }
        observation_delivery::head(&transaction, position, offset, limit)
    }
    fn observation_delivery_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
        index: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        content::validate_canonical_locator(locator)?;
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation::validate(&transaction, &self.conversation_id, cut)?;
        let position = content::canonical_position(&transaction, locator)?;
        if position > cut.ledger_through {
            return Err(ConversationStoreError::InvalidReference(
                "delivery beyond captured canonical cut".into(),
            ));
        }
        observation_delivery::read(&transaction, position, index, offset, max_bytes)
    }

    fn attempt_prompt(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<Option<MessageId>, ConversationStoreError> {
        reading::attempt_prompt(&*self.lock()?, attempt, through)
    }
    fn conversation_turns(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<super::reading::ConversationTurnPage, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        reading::turns(&transaction, &self.conversation_id, offset, limit)
    }

    fn conversation_window(
        &self,
        at: &super::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<super::reading::DurableConversationWindow, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        reading::window(&transaction, &self.conversation_id, at, limit)
    }
    fn load_agent_authority(
        &self,
        agent_id: &crate::runtime::identity::AgentId,
    ) -> Result<crate::runtime::subagent::DurableAgentAuthority, ConversationStoreError> {
        let connection = self.lock()?;
        let encoded: Option<String> = connection
            .query_row(
                "SELECT authority_json FROM agent_authorities WHERE agent_id=?1",
                [agent_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage("read private Agent authority"))?;
        let encoded = encoded.ok_or_else(|| storage("missing admitted Agent authority"))?;
        let mut private: PrivateAgentAuthority = serde_json::from_str(&encoded)
            .map_err(|_| storage("decode private Agent authority"))?;
        let credentials = crate::credentials::CredentialSnapshot::new(private.credentials);
        private
            .authority
            .resolved
            .model
            .restore_admitted_credentials(&credentials)
            .map_err(|_| storage("restore private Agent provider credentials"))?;
        for binding in private
            .authority
            .resolved
            .materialization
            .sources
            .values_mut()
        {
            binding.credentials.capture(credentials.clone());
        }
        Ok(private.authority)
    }

    fn append_agent_admission(
        &self,
        event: RuntimeEventEnvelope,
        authority: &crate::runtime::subagent::DurableAgentAuthority,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        self.append_event_internal(event, Some(authority))
    }

    fn presentation_reader(&self) -> Arc<dyn ConversationStore> {
        let mut reader =
            Self::from_connection_owner(self.conversation_id.clone(), self.conn.clone());
        reader.read_cancel = Some(Arc::new(AtomicBool::new(false)));
        reader.lifecycle.clone_from(&self.lifecycle);
        Arc::new(reader)
    }

    #[cfg(test)]
    fn park_presentation_read_until_cancelled(&self, entered: tokio::sync::oneshot::Sender<()>) {
        let mut slot = self.conn.available.lock().unwrap();
        entered.send(()).unwrap();
        while !self.read_cancel.as_ref().unwrap().load(Ordering::Acquire) {
            slot = self.conn.wake.wait(slot).unwrap();
        }
    }

    fn close_presentation_reader(&self) {
        if let Some(cancel) = &self.read_cancel {
            let _slot = self.conn.available.lock().expect("connection slot");
            cancel.store(true, Ordering::Release);
            self.conn.wake.notify_all();
        }
    }

    fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    fn load_goal(&self) -> Result<Option<crate::goal::GoalSnapshot>, ConversationStoreError> {
        read_goal(&*self.lock()?)
    }

    fn goal_observation_view(
        &self,
    ) -> Result<crate::runtime_client::goal::RuntimeClientGoalView, ConversationStoreError> {
        let connection = self.lock()?;
        let bytes: Option<Option<Vec<u8>>> = connection
            .query_row(
                "SELECT observation_header FROM goal_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        match bytes {
            None => Ok(crate::runtime_client::goal::RuntimeClientGoalView { current: None }),
            Some(None) => Err(storage("native Goal display header unavailable")),
            Some(Some(bytes)) => {
                serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
            }
        }
    }
    fn write_goal(
        &self,
        write: crate::goal::GoalWrite,
    ) -> Result<
        (
            crate::goal::GoalResult,
            Option<Arc<crate::durable::inbox::GoalObservationReceipt>>,
        ),
        ConversationStoreError,
    > {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("Goal transaction: {error}")))?;
        let previous = read_goal(&tx)?;
        let change = write.change();
        let result = crate::goal::transition(previous.as_ref(), write);
        let mut receipt = None;
        if let Ok(goal) = &result {
            save_goal(&tx, goal)?;
            #[cfg(test)]
            if Self::consume(&self.fail_event_remaining) {
                return Err(storage("fault injected: Goal journal commit"));
            }
            let event = persist_event_tx(
                &tx,
                &self.conversation_id,
                crate::goal::journal_envelope(
                    &self.conversation_id,
                    crate::goal::GoalFact::Written {
                        previous: previous.map(|g| g.reference),
                        current: goal.reference.clone(),
                        phase: goal.phase,
                        change,
                    },
                ),
            )?
            .event;
            receipt = Some(Arc::new(crate::durable::inbox::GoalObservationReceipt {
                view: crate::runtime_client::goal::RuntimeClientGoalView::from_native(Some(goal)),
                native: event.observation,
                pending: None,
            }));
            tx.commit()
                .map_err(|error| storage(format!("Goal commit: {error}")))?;
        }
        Ok((result, receipt))
    }

    fn accept_goal_round(
        &self,
        expected: &crate::goal::GoalRef,
        draft: InboundDraft,
    ) -> Result<Option<AcceptedInbound>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("Goal admission transaction: {error}")))?;
        let Some(mut goal) = read_goal(&tx)? else {
            return Ok(None);
        };
        if &goal.reference != expected
            || goal.phase != crate::goal::GoalPhase::Active
            || goal.autonomous_rounds_consumed >= goal.autonomous_round_budget
        {
            return Ok(None);
        }
        let pending: bool = tx
            .query_row("SELECT EXISTS(SELECT 1 FROM pending_inbound)", [], |row| {
                row.get(0)
            })
            .map_err(|error| storage(format!("Goal pending probe: {error}")))?;
        if pending {
            return Ok(None);
        }
        if draft.source != UserSource::Runtime
            || draft.kind != InboundKind::GoalContinuation(expected.clone())
        {
            return Err(ConversationStoreError::InvalidReference(
                "Goal admission requires its exact typed runtime trigger".to_owned(),
            ));
        }
        let mut accepted = accept_inbound_tx(self, &tx, draft)?;
        if accepted.retried {
            return Err(ConversationStoreError::InvalidReference(
                "Goal round identity already accepted".to_owned(),
            ));
        }
        goal.autonomous_rounds_consumed += 1;
        goal.reference.revision = goal
            .reference
            .revision
            .checked_add(1)
            .ok_or(ConversationStoreError::SequenceExhausted)?;
        goal.last_round_message_id = Some(accepted.message_id.clone());
        save_goal(&tx, &goal)?;
        let event = persist_event_tx(
            &tx,
            &self.conversation_id,
            crate::goal::journal_envelope(
                &self.conversation_id,
                crate::goal::GoalFact::RoundAdmitted {
                    previous: expected.clone(),
                    current: goal.reference.clone(),
                    round: goal.autonomous_rounds_consumed,
                    message_id: accepted.message_id.clone(),
                },
            ),
        )?
        .event;
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: Goal accept commit"));
        }
        refresh_pending_receipt(&tx, &self.conversation_id, &mut accepted)?;
        accepted.goal_observation = Some(Arc::new(crate::durable::inbox::GoalObservationReceipt {
            view: crate::runtime_client::goal::RuntimeClientGoalView::from_native(Some(&goal)),
            native: event.observation,
            pending: accepted.pending_observation.clone(),
        }));
        process_death::reach("before:accept_goal_round");
        tx.commit()
            .map_err(|error| storage(format!("Goal admission commit: {error}")))?;
        process_death::reach("after:accept_goal_round");
        Ok(Some(accepted))
    }

    fn accept_inbound(
        &self,
        draft: InboundDraft,
    ) -> Result<AcceptedInbound, ConversationStoreError> {
        reject_unaccounted_goal(&draft)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("accept transaction: {error}")))?;
        let accepted = accept_inbound_tx(self, &transaction, draft)?;
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: accept commit"));
        }
        process_death::reach("before:accept_inbound");
        transaction
            .commit()
            .map_err(|error| storage(format!("accept commit: {error}")))?;
        process_death::reach("after:accept_inbound");
        Ok(accepted)
    }

    fn accept_inbound_with_event(
        &self,
        draft: InboundDraft,
        mut event: RuntimeEventEnvelope,
    ) -> Result<(AcceptedInbound, RuntimeEventEnvelope), ConversationStoreError> {
        reject_unaccounted_goal(&draft)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("accept/event transaction: {error}")))?;
        // Issue #192: this generic compound transition owns exactly the
        // detached background terminal domain. A normal subagent terminal
        // fact has its own semantically closed authority
        // (`accept_subagent_terminal`) and is rejected here, so no
        // `SubagentTerminalPublished` can bypass the Issue #192
        // terminal-publication contract.
        let RuntimeEvent::BackgroundTerminalPublished {
            execution_id,
            message_id: event_message_id,
            ..
        } = &event.event
        else {
            return Err(ConversationStoreError::InvalidReference(
                "inbound/event acceptance owns only a detached background terminal fact; a \
                 subagent terminal publishes through accept_subagent_terminal"
                    .to_owned(),
            ));
        };
        if event.event_id.as_str().is_empty() {
            event.event_id = EventId::new(format!("background-terminal-event:{execution_id}"));
        }
        let mut accepted = accept_inbound_tx(self, &transaction, draft)?;
        if *event_message_id != accepted.message_id {
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
        process_death::reach_event("before:event", &event.event);
        let mut persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        refresh_pending_receipt(&transaction, &self.conversation_id, &mut accepted)?;
        if let Some(native) = &mut persisted.event.observation {
            Arc::make_mut(native)
                .pending
                .clone_from(&accepted.pending_observation);
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("accept/event commit: {error}")))?;
        process_death::reach_event("after:event", &persisted.event.event);
        Ok((accepted, persisted.event))
    }

    #[allow(clippy::too_many_lines)] // one durable linearization boundary
    fn accept_subagent_terminal(
        &self,
        notice: Option<InboundDraft>,
        draft: InboundDraft,
        mut event: RuntimeEventEnvelope,
    ) -> Result<
        (
            Option<AcceptedInbound>,
            AcceptedInbound,
            RuntimeEventEnvelope,
        ),
        ConversationStoreError,
    > {
        let _ownership = self.ownership_transition(&event.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("subagent terminal transaction: {error}")))?;
        // The transition is semantically closed (Issue #192): every input
        // is validated against the durable ownership fact and the canonical
        // identities before anything is accepted, so no semantically
        // unreachable durable state can be created.
        validate_subagent_terminal_publication(&transaction, notice.as_ref(), &draft, &event)?;
        if event.event_id.as_str().is_empty()
            && let RuntimeEvent::SubagentTerminalPublished { subagent_id, .. } = &event.event
        {
            event.event_id = EventId::new(format!("subagent-terminal-event:{subagent_id}"));
        }
        let event_message_id = match &event.event {
            RuntimeEvent::SubagentTerminalPublished { message_id, .. } => message_id.clone(),
            _ => unreachable!("the payload was validated above"),
        };
        // The notice is ordered first, so the terminal report remains the
        // last item of the publication.
        let notice = notice
            .map(|notice| accept_inbound_tx(self, &transaction, notice))
            .transpose()?;
        let mut accepted = accept_inbound_tx(self, &transaction, draft)?;
        if event_message_id != accepted.message_id {
            return Err(ConversationStoreError::InvalidReference(format!(
                "subagent terminal fact references {}, accepted inbound is {}",
                event_message_id, accepted.message_id
            )));
        }
        // The notice/report pair is one atomic publication: a retry must
        // find both sides already accepted, never exactly one.
        if let Some(notice) = &notice
            && notice.retried != accepted.retried
        {
            return Err(ConversationStoreError::InvalidReference(
                "a subagent terminal notice/report pair must retry together".to_owned(),
            ));
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
                .map_err(|error| storage(format!("subagent terminal retry commit: {error}")))?;
            return Ok((notice, accepted, existing));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_accept_remaining) {
            return Err(storage("fault injected: subagent terminal accept commit"));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: subagent terminal journal commit"));
        }
        process_death::reach_event("before:event", &event.event);
        let mut persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        refresh_pending_receipt(&transaction, &self.conversation_id, &mut accepted)?;
        if let Some(native) = &mut persisted.event.observation {
            Arc::make_mut(native)
                .pending
                .clone_from(&accepted.pending_observation);
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("subagent terminal commit: {error}")))?;
        process_death::reach_event("after:event", &persisted.event.event);
        Ok((notice, accepted, persisted.event))
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
        attempt_id: Option<crate::runtime::identity::AttemptId>,
    ) -> Result<crate::durable::inbox::AdoptedBatch, ConversationStoreError> {
        let mut connection = self.lock()?;
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("adopt transaction: {error}")))?;
        #[cfg(test)]
        if let Some(probe) = self
            .pending_adoption_transaction_probe
            .lock()
            .unwrap()
            .take()
        {
            probe();
        }
        ensure_surface_head(&transaction)?;

        let items = load_pending_until(&transaction, watermark)?;
        for item in &items {
            let block = MessageBlock::User(item.message.clone());
            let cursor = validation.append(&transaction, &block, Some(&item.message_id))?;
            if cursor != item.transcript_cursor {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "adopted inbound {} changed its transcript cursor from {:?} to {:?}",
                    item.message_id, item.transcript_cursor, cursor
                )));
            }
        }
        transaction
            .execute(
                "DELETE FROM pending_inbound WHERE sequence <= ?1",
                [seq_to_i64(watermark.get())?],
            )
            .map_err(|error| storage(format!("adopt pending delete: {error}")))?;
        for item in &items {
            // The removed native pending row fences these derived bytes even
            // if cleanup fails; their deletion cannot refuse adoption.
            let _ = optional_display_layout(&transaction, || {
                content::clear_pending(&transaction, &item.sequence.get().to_string())
            });
        }
        if !items.is_empty() {
            transaction.execute("UPDATE rustx_store SET pending_observation_count=pending_observation_count-?1,reading_mutation_revision=reading_mutation_revision+1 WHERE id=1",[items.len() as u64]).map_err(|error|storage(error.to_string()))?;
        }
        let mut adoption = None;
        if !items.is_empty() {
            let fact = crate::durable::inbox::inbound_adoption_event(
                &self.conversation_id,
                attempt_id,
                items.iter().map(|item| item.message_id.clone()).collect(),
            );
            adoption = Some(persist_event_tx(&transaction, &self.conversation_id, fact)?.event);
        }
        #[cfg(test)]
        if self.consume_admission_fault(AdmissionFaultOperation::AdoptPendingBatch)
            || Self::consume(&self.fail_adopt_remaining)
        {
            return Err(storage("fault injected: adopt commit"));
        }
        let read_cut = reading::cut(&transaction, &self.conversation_id)?;
        let canonical_receipts = canonical_observation_batch(
            &transaction,
            &self.conversation_id,
            &items
                .iter()
                .map(|item| (&item.message_id, item.transcript_cursor))
                .collect::<Vec<_>>(),
            adoption
                .as_ref()
                .map(|event| (event.sequence, event.sequence)),
        )?;
        if let Some(event) = &mut adoption {
            event.observation = canonical_receipts.first().and_then(|receipt| {
                observation_receipt::canonical(
                    &transaction,
                    &self.conversation_id,
                    event,
                    Arc::clone(&receipt.observation),
                )
            });
        }
        let pending_observation = observation_receipt::pending(
            &transaction,
            &self.conversation_id,
            None,
            None,
            Some((watermark.get(), items.len() as u64)),
        );
        process_death::reach("before:adopt_pending_batch");
        transaction
            .commit()
            .map_err(|error| storage(format!("adopt commit: {error}")))?;
        validation.committed(&connection);
        process_death::reach("after:adopt_pending_batch");
        Ok(crate::durable::inbox::AdoptedBatch {
            pending_observation,
            canonical_receipts,
            read_cut,
            items,
            adoption,
        })
    }

    fn edit_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
        text: &str,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, ConversationStoreError> {
        self.mutate_pending(expected, Some(text))
    }
    fn remove_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, ConversationStoreError> {
        self.mutate_pending(expected, None)
    }

    fn load_pending(&self) -> Result<Vec<PendingInboundItem>, ConversationStoreError> {
        let connection = self.lock()?;
        load_pending_rows(&connection)
    }

    fn has_accepted_inbound(&self) -> Result<bool, ConversationStoreError> {
        // The durable acceptance watermark. `accept_inbound` advances it in
        // the acceptance commit, adoption never touches it, and lineage
        // seeding initializes it to zero without an acceptance, so a
        // nonzero value is permanent proof that user work was durably
        // accepted into this conversation.
        let connection = self.lock()?;
        let watermark: i64 = connection
            .query_row(
                "SELECT next_inbound_sequence FROM rustx_store WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("read inbound acceptance watermark: {error}")))?;
        Ok(watermark > 0)
    }

    fn is_initialized(&self) -> Result<bool, ConversationStoreError> {
        let connection = self.lock()?;
        let (initialized, identity): (i64, Option<i64>) = connection
            .prepare_cached("SELECT s.bootstrap_initialized,b.id FROM rustx_store s LEFT JOIN bootstrap_identity b ON b.id=s.id WHERE s.id=1")
            .map_err(|error| storage(format!("bootstrap state: {error}")))?
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| storage(format!("bootstrap state: {error}")))?;
        match (initialized, identity) {
            (0, None) => Ok(false),
            (1, Some(1)) => Ok(true),
            _ => Err(storage(
                "missing or inconsistent immutable bootstrap identity",
            )),
        }
    }

    fn initialize(&self, messages: &[MessageBlock]) -> Result<(), ConversationStoreError> {
        let bootstrap: Option<(u64, String)> = self
            .lock()?
            .query_row(
                "SELECT message_count,history_digest FROM bootstrap_identity WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| storage(format!("bootstrap count: {error}")))?;
        let seed = if let Some((count, digest)) = bootstrap {
            if count != messages.len() as u64 || digest != initial_history_digest(messages)? {
                return Err(ConversationStoreError::InitialHistoryMismatch);
            }
            // Runtime reopening supplies canonical bootstrap bodies, not a new
            // Surface. Reconstruct the original immutable seed, never flatten it.
            let history = self.load_surface_history(SurfaceRevision::new(count))?;
            LineageSeed::replayed(messages.to_vec(), history)?
        } else {
            LineageSeed::history(messages.to_vec())
        };
        let mut inherited = Vec::new();
        let mut after = crate::context::checkpoint::ProjectionActivationId(0);
        loop {
            let page = self.projection_timeline(after, 64)?;
            if page.is_empty() {
                break;
            }
            after = page.last().unwrap().activation.activation;
            inherited.extend(
                page.into_iter()
                    .filter(|p| p.activation.destination_installation.is_some()),
            );
        }
        let seed = seed.with_projections(inherited)?;
        self.initialize_lineage(
            &seed
                .with_completed_responses(self.load_inherited_responses()?)?
                .with_turns(self.load_inherited_turns()?)?,
        )
    }

    #[allow(clippy::too_many_lines)] // One atomic bootstrap owns Ledger, Surface and inherited facts.
    fn initialize_lineage(&self, seed: &LineageSeed) -> Result<(), ConversationStoreError> {
        let validated =
            LineageSeed::replayed(seed.canonical().to_vec(), seed.surface_history().to_vec())?
                .with_completed_responses(seed.completed_responses().to_vec())?
                .with_turns(seed.turns().to_vec())?
                .with_projections(seed.projections().to_vec())?
                .with_raw_tool_arguments(seed.raw_tool_arguments().to_vec())?;
        let seed = &validated;
        let messages = seed.canonical();
        // Semantics belong to the Context Engine; persistence has no alternate selection rule.
        let occupancy =
            crate::context::occupancy::inherit(seed.turns(), seed.canonical(), seed.projections());
        let occupancy_json = encode(&occupancy, "bootstrap occupancy")?;
        let history_digest = initial_history_digest(messages)?;
        let occupancy_digest =
            inherited_occupancy_digest(messages.len() as u64, &history_digest, &occupancy_json);
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("initialize transaction: {error}")))?;
        // Fail closed if initialized metadata is missing or damaged. The marker
        // and identity are written in this same transaction, never independently.
        read_inherited_occupancy(&transaction)?;
        let bootstrap: Option<(i64, String, String, String)> = transaction
            .query_row(
                "SELECT message_count,history_digest,occupancy_json,occupancy_digest FROM bootstrap_identity WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| storage(format!("bootstrap probe: {error}")))?;
        if let Some((count, digest, retained_occupancy, retained_digest)) = bootstrap {
            let bootstrap_revision = SurfaceRevision::new(
                u64::try_from(count).map_err(|_| storage("negative bootstrap message count"))?,
            );
            projection::verify_lineage(&transaction, seed.projections())?;
            if replay_surface_history(&transaction, bootstrap_revision)?.1 != seed.surface_history()
            {
                return Err(ConversationStoreError::InitialHistoryMismatch);
            }
            let supplied_count = i64::try_from(messages.len())
                .map_err(|_| storage("initial message count is not representable"))?;
            if retained_occupancy != occupancy_json
                || retained_digest != occupancy_digest
                || count != supplied_count
                || digest != history_digest
                || provenance::responses(&transaction)? != seed.completed_responses()
                || provenance::turns(&transaction)? != seed.turns()
            {
                return Err(ConversationStoreError::InitialHistoryMismatch);
            }
            provenance::verify_seed_membership(&transaction, seed)?;
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
            // The two parts of the seed go to their two durable homes, in two
            // passes rather than one, because they are not the same order:
            // the Ledger is written in canonical commit order, and a
            // compaction summary the source produced *last* is the fact its
            // Surface shows *first*. Fusing the passes would force one order
            // on both and reproduce neither.
            let append_terminals =
                |predecessor: Option<&MessageId>| -> Result<(), ConversationStoreError> {
                    for (index, turn) in seed.turns().iter().enumerate() {
                        if turn.process_message_ids.is_empty()
                            && turn.preceding_message_id.as_ref() == predecessor
                        {
                            append_transcript_reference(
                                &transaction,
                                "inherited_turn",
                                &index.to_string(),
                            )?;
                        }
                    }
                    Ok(())
                };
            append_terminals(None)?;
            for message in messages {
                append_seeded_ledger_message(&transaction, message)?;
                append_terminals(Some(&crate::conversation::message_id_of(message)))?;
            }
            for item in seed.raw_tool_arguments() {
                content::index_raw_arguments(&transaction, item)?;
                let message = messages
                    .iter()
                    .find(|message| message.id() == &item.message_id)
                    .ok_or_else(|| storage("raw Tool canonical source missing"))?;
                let position: u64 = transaction
                    .query_row(
                        "SELECT position FROM message_ledger WHERE message_id=?1",
                        [item.message_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|error| storage(error.to_string()))?;
                let _ = optional_display_layout(&transaction, || {
                    observation::index_message(&transaction, message, Some(position))?;
                    observation_trace::index_proposals(
                        &transaction,
                        &self.conversation_id,
                        position,
                        message,
                    )?;
                    observation_trace::index_canonical(
                        &transaction,
                        &self.conversation_id,
                        position,
                        message,
                    )
                });
            }
            // The seed's Surface history becomes this lineage's own retained
            // operation log, replayed from revision 1. Flattening it into one
            // append per active message would produce the same Surface and a
            // different history, and a later fork or tree of *this* lineage
            // reads the history: its branch points would then be this copy's
            // artefacts rather than the source's real ones.
            let mut active: Vec<MessageId> = Vec::new();
            let mut revision = SurfaceRevision::INITIAL;
            let generation = 0_u64;
            for operation in seed.surface_history() {
                revision = revision.next();
                validate_surface_operation_references(&transaction, operation)?;
                append_surface_op(&transaction, revision, generation, operation)?;
                apply_surface_op(&mut active, operation)?;
            }
            if revision != SurfaceRevision::INITIAL {
                update_surface_head(&transaction, revision, generation)?;
            }
            transaction
                .execute(
                    "INSERT INTO bootstrap_identity(id,message_count,history_digest,turn_count,response_count,occupancy_json,occupancy_digest,provenance_digest,inherited_transcript_through) VALUES(1,?1,?2,?3,?4,?5,?6,?7,(SELECT next_transcript_position FROM rustx_store WHERE id=1))",
                    params![
                        i64::try_from(messages.len()).map_err(|_| storage("initial message count is not representable"))?,
                        history_digest,
                        seq_to_i64(seed.turns().len() as u64)?,
                        seq_to_i64(seed.completed_responses().len() as u64)?,
                        occupancy_json,
                        occupancy_digest,
                        provenance::counts_digest(&history_digest,seq_to_i64(seed.turns().len() as u64)?,seq_to_i64(seed.completed_responses().len() as u64)?)
                    ],
                )
                .map_err(|error| storage(format!("insert bootstrap identity: {error}")))?;
            let _ = optional_display_layout(&transaction, || {
                observation_statistics::seed(&transaction, seed.turns())
            });
            let _ = optional_display_layout(&transaction, || {
                observation_occupancy::inherited(&transaction, occupancy.as_ref())
            });
            provenance::persist(&transaction, seed)?;
            // The original identity and provenance now exist together. Derived
            // readers validate this same transaction's complete bootstrap shape;
            // no external reader sees the marker before COMMIT.
            projection::install_lineage(&transaction, &self.conversation_id, seed.projections())?;
            transaction
                .execute(
                    "UPDATE rustx_store SET bootstrap_initialized=1 WHERE id=1",
                    [],
                )
                .map_err(|error| storage(format!("mark lineage bootstrap: {error}")))?;
            let _ = optional_display_layout(&transaction, || {
                observation_members::seed_members(&transaction, seed)
            });
            let _ = optional_display_layout(&transaction, || {
                observation_turn::seed(&transaction, seed)
            });
            for (ordinal, turn) in seed.turns().iter().enumerate() {
                let _ = optional_display_layout(&transaction, || {
                    reading::index_inherited_turn(&transaction, seq_to_i64(ordinal as u64)?, turn)
                });
            }
            for (ordinal, turn) in seed.turns().iter().enumerate() {
                if turn.process_message_ids.is_empty() {
                    let id = ordinal.to_string();
                    if let Some(cursor) =
                        load_transcript_reference(&transaction, "inherited_turn", &id)?
                    {
                        let _ = optional_display_layout(&transaction, || {
                            observation::index_item(
                                &transaction,
                                "inherited_turn",
                                &id,
                                TranscriptItem::InheritedTurn {
                                    provenance: turn.clone(),
                                },
                                cursor,
                            )
                        });
                    }
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| storage(format!("initialize commit: {error}")))
    }

    fn load_inherited_responses(
        &self,
    ) -> Result<Vec<super::response::CompletedResponseProvenance>, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::responses(&connection)
    }

    fn load_inherited_occupancy(
        &self,
    ) -> Result<Option<crate::context::occupancy::ContextOccupancy>, ConversationStoreError> {
        let connection = self.lock()?;
        read_inherited_occupancy(&connection)
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

    fn unsettled_canonical_tool_calls(
        &self,
        limit: usize,
    ) -> Result<
        Vec<(crate::message::types::ToolCallOccurrenceRef, ToolCallId)>,
        ConversationStoreError,
    > {
        if limit == 0 || limit > 8192 {
            return Err(crate::context::resources::bound(
                "canonical unsettled occurrence read limit",
            ));
        }
        let connection = self.lock()?;
        let mut query = connection.prepare("SELECT CASE WHEN octet_length(assistant_message_id)<=4096 THEN assistant_message_id END, block_index, CASE WHEN octet_length(call_id)<=4096 THEN call_id END FROM canonical_tool_calls WHERE result_message_id IS NULL ORDER BY assistant_message_id,block_index LIMIT ?1").map_err(|e| storage(e.to_string()))?;
        let rows = query
            .query_map([limit], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, u32>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| storage(e.to_string()))?;
        rows.map(|row| {
            let (id, block, wire) = row.map_err(|e| storage(e.to_string()))?;
            Ok((
                crate::message::types::ToolCallOccurrenceRef::new(
                    MessageId::new(id.ok_or_else(|| {
                        crate::context::resources::bound(
                            "canonical occurrence identity exceeds bound",
                        )
                    })?),
                    ContentBlockIndex::new(block),
                ),
                ToolCallId::new(wire.ok_or_else(|| {
                    crate::context::resources::bound("canonical wire correlation exceeds bound")
                })?),
            ))
        })
        .collect()
    }

    fn load_head(&self) -> Result<DurableConversationHead, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        #[cfg(test)]
        if let Some(probe) = self.head_read_probe.lock().unwrap().take() {
            transaction
                .query_row(
                    "SELECT schema_version FROM rustx_store WHERE id=1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            probe();
        }
        load_head(&transaction)
    }

    fn messages_are_human_inputs(&self, ids: &[MessageId]) -> Result<bool, ConversationStoreError> {
        let connection = self.lock()?;
        let mut query = connection
            .prepare_cached(
                "SELECT json_extract(message_json, '$.id'),
                    json_extract(message_json, '$.role') = 'user'
                    AND json_extract(message_json, '$.source') = 'human'
                    AND json_extract(message_json, '$.kind') = 'message'
             FROM message_ledger WHERE message_id = ?1",
            )
            .map_err(|error| storage(format!("Ledger input authorship: {error}")))?;
        for id in ids {
            let (stored_id, human): (String, bool) = query
                .query_row([id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()
                .map_err(|error| storage(format!("Ledger input authorship: {error}")))?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!("message {id} is unavailable"))
                })?;
            if stored_id != id.as_str() {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Ledger row for {id} contains a different message identity"
                )));
            }
            if !human {
                return Ok(false);
            }
        }
        Ok(!ids.is_empty())
    }

    fn load_messages(
        &self,
        ids: &[MessageId],
    ) -> Result<Vec<MessageBlock>, ConversationStoreError> {
        #[cfg(test)]
        self.checkpoint_read_counts[0].fetch_add(ids.len(), Ordering::SeqCst);
        let connection = self.lock()?;
        ids.iter().map(|id| load_message(&connection, id)).collect()
    }

    fn message_transcript_cursor(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        let position: Option<i64> = self.lock()?.query_row(
            "SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1",
            [message_id.as_str()], |row| row.get(0),
        ).optional().map_err(|error| storage(format!("process control position: {error}")))?;
        position
            .map(|value| nonnegative(value, "process control position").map(TranscriptCursor::new))
            .transpose()
    }

    fn event_transcript_cursor(
        &self,
        event_id: &EventId,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        let position: Option<i64> = self.lock()?.query_row(
            "SELECT position FROM transcript_order WHERE reference_kind='attempt_terminal' AND reference_id=?1",
            [event_id.as_str()], |row| row.get(0),
        ).optional().map_err(|error| storage(format!("terminal control position: {error}")))?;
        position
            .map(|value| nonnegative(value, "terminal control position").map(TranscriptCursor::new))
            .transpose()
    }

    fn message_append_revision_by_locator(
        &self,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
    ) -> Result<Option<SurfaceRevision>, ConversationStoreError> {
        let connection = self.lock()?;
        let position = content::canonical_position(&connection, locator)?;
        let revision: Option<u64> = connection
            .query_row(
                "SELECT first_append_revision FROM message_ledger WHERE position=?1",
                [position],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(revision.map(SurfaceRevision::new))
    }
    fn message_append_revision(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<SurfaceRevision>, ConversationStoreError> {
        let revision: Option<i64> = self.lock()?.query_row(
            "SELECT revision FROM surface_ops WHERE json_extract(op_json, '$.op') = 'append' AND json_extract(op_json, '$.message_id') = ?1 ORDER BY revision LIMIT 1",
            [message_id.as_str()], |row| row.get(0),
        ).optional().map_err(|error| storage(format!("message append revision: {error}")))?;
        revision
            .map(|value| nonnegative(value, "message append revision").map(SurfaceRevision::new))
            .transpose()
    }

    fn projected_history(
        &self,
    ) -> Result<super::projection::HistoricalModelInput, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        #[cfg(test)]
        if Self::consume(&self.fail_projection_history_remaining) {
            return Err(storage("fault injected: post-commit hot projection read"));
        }
        projection::selected_history(&transaction, &self.conversation_id, &self.conn.manifest)
    }
    fn observation_projection_provenance_read(
        &self,
        cut: &super::observation::ObservationReadCut,
        activation: u64,
        item: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation_projection::provenance_read(
            &transaction,
            &self.conversation_id,
            cut,
            activation,
            item,
            path,
            offset,
            max_bytes,
        )
    }
    fn observation_projection_head(
        &self,
        through: u64,
    ) -> Result<
        Option<crate::runtime_client::snapshot::RuntimeClientProjectionHeadView>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        observation_projection::read(&transaction, through)
    }
    fn projection_at_journal_cut(
        &self,
        through: u64,
    ) -> Result<Option<super::projection::ProjectionCheckpoint>, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        projection::at_journal_cut(&transaction, through)
    }
    fn compaction_checkpoint_at(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        id: &crate::runtime::identity::CheckpointId,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<super::projection::ProjectionCheckpoint, ConversationStoreError> {
        limits.validate()?;
        if cut.conversation_id != self.conversation_id {
            return Err(ConversationStoreError::InvalidReference(
                "foreign checkpoint resource cut".into(),
            ));
        }
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        compaction_resources::validate(&transaction, &self.conversation_id, cut, limits)?;
        let bytes:Option<u64>=transaction.query_row(
            "SELECT octet_length(c.candidate_json)+octet_length(a.activation_json)+coalesce(octet_length(c.occupancy_json),0) FROM projection_checkpoints c JOIN projection_activations a ON a.checkpoint_id=c.checkpoint_id WHERE c.checkpoint_id=?1 AND a.activation<=?2",
            params![id.as_str(),seq_to_i64(cut.projection_activation.0)?],|row|row.get(0),
        ).optional().map_err(|e|storage(e.to_string()))?;
        let bytes = bytes.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "checkpoint is absent at Resource Read Cut".into(),
            )
        })?;
        if bytes > limits.record_bytes as u64 || bytes > limits.page_bytes as u64 {
            return Err(crate::context::resources::bound(
                "checkpoint record exceeds resource materialization bound",
            ));
        }
        projection::checkpoint(&transaction, id)
    }
    fn compaction_request(
        &self,
        cut: &crate::context::resources::ResourceReadCut,
        request: &crate::runtime::identity::RequestId,
        limits: crate::context::resources::ResourceLimits,
    ) -> Result<crate::model::RequestSnapshot, ConversationStoreError> {
        limits.validate()?;
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        let bytes: Option<u64> = transaction.query_row("SELECT octet_length(snapshot_json) FROM request_snapshots WHERE request_id=?1 AND started_sequence<=?2",params![request.as_str(),seq_to_i64(cut.journal.0)?],|row|row.get(0)).optional().map_err(|e|storage(e.to_string()))?;
        if cut.conversation_id != self.conversation_id {
            return Err(ConversationStoreError::InvalidReference(
                "foreign frozen request Resource Read Cut".into(),
            ));
        }
        let bytes = bytes.ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "request is absent at the Resource Read Cut".into(),
            )
        })?;
        if bytes > limits.record_bytes as u64 || bytes > limits.page_bytes as u64 {
            return Err(crate::context::resources::bound(
                "historical frozen request exceeds materialization bound",
            ));
        }
        load_request_snapshot_bounded(
            &transaction,
            &self.conversation_id,
            request,
            limits.record_bytes.min(limits.page_bytes),
        )
    }

    fn projection_head(
        &self,
    ) -> Result<Option<super::projection::ProjectionCheckpoint>, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        projection::head(&transaction)
    }
    fn projection_checkpoint(
        &self,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<super::projection::ProjectionCheckpoint, ConversationStoreError> {
        projection::checkpoint(&*self.lock()?, id)
    }
    fn projection_timeline(
        &self,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<Vec<super::projection::ProjectionCheckpoint>, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        projection::timeline(&transaction, after, limit)
    }
    fn projection_timeline_at_journal_cut(
        &self,
        through: u64,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<super::projection::ProjectionTimelinePage, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        projection::timeline_at_journal_cut(&transaction, through, after, limit)
    }
    fn projection_checkpoint_at_journal_cut(
        &self,
        through: u64,
        id: &crate::runtime::identity::CheckpointId,
    ) -> Result<super::projection::ProjectionCheckpoint, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        projection::checkpoint_at_journal_cut(&transaction, through, id)
    }
    fn projection_owner(
        &self,
        attempt: Option<&crate::runtime::identity::AttemptId>,
    ) -> Result<super::projection::ProjectionOwner, ConversationStoreError> {
        projection::owner(&*self.lock()?, attempt)
    }
    fn projection_receipt(
        &self,
        operation: &crate::runtime::identity::CompactionOperationId,
        fingerprint: &str,
    ) -> Result<Option<super::projection::ProjectionCommitReceipt>, ConversationStoreError> {
        projection::receipt(&*self.lock()?, operation, fingerprint)
    }
    fn commit_projection(
        &self,
        input: super::projection::ProjectionCommitInput,
    ) -> Result<super::projection::ProjectionCommitReceipt, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| storage(e.to_string()))?;
        let result = projection::commit(&transaction, &self.conversation_id, input, |stage| {
            #[cfg(test)]
            if self.consume_compaction_fault(stage) || Self::consume(&self.fail_event_remaining) {
                return Err(storage("fault injected: checkpoint transaction"));
            }
            #[cfg(not(test))]
            let _ = stage;
            Ok(())
        })?;
        process_death::reach("before:commit_projection");
        transaction.commit().map_err(|e| storage(e.to_string()))?;
        process_death::reach("after:commit_projection");
        // Stage committed Journal facts and release SQLite before returning a receipt.
        drop(connection);
        #[cfg(test)]
        {
            let probe = self
                .projection_commit_return_probe
                .lock()
                .expect("projection return probe")
                .take();
            if let Some(probe) = probe {
                probe();
            }
        }
        #[cfg(test)]
        for (stage, count) in [
            (CompactionFaultOperation::AfterCommitOneHotReadFailure, 1),
            (CompactionFaultOperation::AfterCommitTwoHotReadFailures, 2),
        ] {
            if self.consume_compaction_fault(stage) {
                self.fail_projection_history_remaining
                    .store(count, Ordering::SeqCst);
            }
        }
        #[cfg(test)]
        if self.consume_compaction_fault(CompactionFaultOperation::AfterCommit) {
            return Err(storage("fault injected: lost checkpoint commit receipt"));
        }
        Ok(result)
    }

    fn load_surface_history(
        &self,
        through: SurfaceRevision,
    ) -> Result<Vec<SurfaceOp>, ConversationStoreError> {
        let connection = self.lock()?;
        load_surface_history(&connection, through)
    }

    fn reconstruct_surface(
        &self,
        revision: SurfaceRevision,
    ) -> Result<Vec<MessageId>, ConversationStoreError> {
        #[cfg(test)]
        self.checkpoint_read_counts[1].fetch_add(1, Ordering::SeqCst);
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

    fn surface_boundary_header_page(
        &self,
        through: SurfaceRevision,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::SurfaceBoundaryHeaderPage, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::surface_boundaries(&connection, through, offset, limit)
    }
    fn surface_boundary_message_id(
        &self,
        boundary_revision: SurfaceRevision,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
    ) -> Result<MessageId, ConversationStoreError> {
        let connection = self.lock()?;
        let position = content::canonical_position(&connection, locator)?;
        let id:Option<String>=connection.query_row("SELECT message_id FROM message_ledger WHERE ordinary_user=1 AND first_append_revision<=?1 AND position=?2",params![seq_to_i64(boundary_revision.get())?,position],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        id.map(MessageId::new).ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "Surface boundary locator/revision mismatch".into(),
            )
        })
    }

    fn canonical_boundary_message_id(
        &self,
        boundary_revision: SurfaceRevision,
        locator: &crate::runtime_client::content::CanonicalContentLocator,
    ) -> Result<MessageId, ConversationStoreError> {
        let connection = self.lock()?;
        let position = content::canonical_position(&connection, locator)?;
        let id: Option<String> = connection
            .query_row(
                "SELECT message_id FROM message_ledger WHERE first_append_revision<=?1 AND position=?2",
                params![seq_to_i64(boundary_revision.get())?, position],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        id.map(MessageId::new).ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "canonical boundary locator/revision mismatch".into(),
            )
        })
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
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical event transaction: {error}")))?;
        ensure_surface_head(&transaction)?;

        let transcript_cursor = validation.append(&transaction, message, None)?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical event journal commit"));
        }
        process_death::reach_event("before:canonical", &event.event);
        let mut persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        let mut receipt = canonical_observation_receipt(
            &transaction,
            &self.conversation_id,
            message,
            transcript_cursor,
            persisted.event.tool_occurrence.clone(),
        )?;
        set_canonical_range(
            &mut receipt,
            Some((persisted.event.sequence, persisted.event.sequence)),
        );
        persisted.event.observation = if matches!(message, MessageBlock::Tool(_)) {
            observation_receipt::tool_results(
                &transaction,
                &self.conversation_id,
                std::slice::from_ref(&persisted.event),
                std::slice::from_ref(message),
                Arc::clone(&receipt.observation),
            )
        } else {
            observation_receipt::canonical(
                &transaction,
                &self.conversation_id,
                &persisted.event,
                Arc::clone(&receipt.observation),
            )
        };
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical event commit: {error}")))?;
        validation.committed(&connection);
        process_death::reach_event("after:canonical", &persisted.event.event);
        Ok((persisted.event, receipt))
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
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical event batch transaction: {error}")))?;
        ensure_surface_head(&transaction)?;

        let mut receipts = Vec::with_capacity(messages.len());
        for message in messages {
            receipts.push(validation.append(&transaction, message, None)?);
        }
        let mut persisted = Vec::with_capacity(events.len());
        for event in events {
            #[cfg(test)]
            if Self::consume(&self.fail_event_remaining) {
                return Err(storage(
                    "fault injected: canonical batch event journal commit",
                ));
            }
            persisted.push(
                persist_event_tx_deferred(&transaction, &self.conversation_id, event.clone())?
                    .event,
            );
        }
        let range = persisted
            .first()
            .zip(persisted.last())
            .map(|(first, last)| (first.sequence, last.sequence));
        let receipts = canonical_observation_batch(
            &transaction,
            &self.conversation_id,
            &messages
                .iter()
                .zip(receipts)
                .map(|(message, cursor)| (message.id(), cursor))
                .collect::<Vec<_>>(),
            range,
        )?;
        let shared = receipts.first().and_then(|receipt| {
            observation_receipt::tool_results(
                &transaction,
                &self.conversation_id,
                &persisted,
                messages,
                Arc::clone(&receipt.observation),
            )
        });
        for event in &mut persisted {
            event.observation.clone_from(&shared);
        }
        process_death::reach("before:append_canonical_batch");
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical event batch commit: {error}")))?;
        validation.committed(&connection);
        process_death::reach("after:append_canonical_batch");
        Ok((persisted, receipts))
    }

    #[allow(clippy::too_many_lines)]
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

    fn load_inspection_seed(&self) -> Result<super::inbox::InspectionSeed, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|e| storage(e.to_string()))?;
        let head = load_head(&transaction)?;
        let ids = reconstruct_surface(&transaction, head.revision)?;
        Ok(super::inbox::InspectionSeed {
            messages: ids
                .iter()
                .map(|id| load_message(&transaction, id))
                .collect::<Result<_, _>>()?,
            canonical: load_canonical_rows(&transaction)?,
            transcript: load_transcript_page(&transaction, None, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT)?,
            journal_through: journal_prefix(&transaction)?,
        })
    }

    fn load_transcript_snapshot(
        &self,
        cut: &super::inbox::TranscriptSnapshotCut,
    ) -> Result<TranscriptPage, ConversationStoreError> {
        let connection = self.lock()?;
        let messages = serde_json::to_string(&cut.messages).map_err(|e| storage(e.to_string()))?;
        let publications =
            serde_json::to_string(&cut.publications).map_err(|e| storage(e.to_string()))?;
        // The bootstrap prefix is immutable canonical history. Subsequent
        // canonical membership comes from captured semantic receipts, never
        // the current ledger head. Pending rows are supplied below from C.
        let mut statement = connection
            .prepare(
                "SELECT t.position,t.reference_kind,t.reference_id FROM transcript_order t
             WHERE (t.reference_kind='message' AND EXISTS
               (SELECT 1 FROM message_ledger m WHERE m.message_id=t.reference_id)
               AND (t.position<=?1 OR t.reference_id IN (SELECT value FROM json_each(?2))))
             OR (t.reference_kind='publication_audit' AND
               (t.position<=?1 OR t.reference_id IN (SELECT value FROM json_each(?3))))
             OR (t.reference_kind='inherited_turn' AND t.position<=?1)
             OR (t.reference_kind IN ('interaction_event','attempt_terminal') AND EXISTS
               (SELECT 1 FROM events e WHERE e.event_id=t.reference_id AND e.sequence<=?4))
             ORDER BY t.position DESC LIMIT ?5",
            )
            .map_err(|e| storage(e.to_string()))?;
        let references = statement
            .query_map(
                params![
                    seq_to_i64(cut.bootstrap_through)?,
                    messages,
                    publications,
                    seq_to_i64(cut.journal_through)?,
                    TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT + 1
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|e| storage(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| storage(e.to_string()))?;
        let allowed: BTreeSet<_> = cut.messages.iter().map(MessageId::as_str).collect();
        let mut entries = Vec::new();
        for (position, kind, id) in references {
            let item = load_transcript_item(&connection, &kind, &id)?;
            let mut tool_calls = match &item {
                TranscriptItem::Message {
                    message: MessageBlock::Assistant(assistant),
                } => load_transcript_tools(&connection, assistant)?,
                _ => Vec::new(),
            };
            // Associations are mutable as later Tool results commit. Keep only
            // results whose canonical message belonged to this captured cut.
            for tool in &mut tool_calls {
                let result: Option<(String, i64)> = connection.query_row(
                    "SELECT m.message_id,t.position FROM canonical_tool_calls c
                     JOIN message_ledger m ON m.message_id=c.result_message_id
                     JOIN transcript_order t ON t.reference_kind='message' AND t.reference_id=m.message_id
                     WHERE c.assistant_message_id=?1 AND c.block_index=?2",
                    params![tool.message_id.as_str(), tool.block_index.get()],
                    |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e| storage(e.to_string()))?;
                if result.is_some_and(|(id, pos)| {
                    pos > i64::try_from(cut.bootstrap_through).expect("validated transcript prefix")
                        && !allowed.contains(id.as_str())
                }) {
                    tool.result = None;
                }
            }
            entries.push(TranscriptEntry {
                cursor: TranscriptCursor::new(nonnegative(position, "transcript position")?),
                item,
                tool_calls,
            });
        }
        // A later adoption/edit/removal cannot replace the pending body at C.
        for (cursor, message) in &cut.pending {
            entries.retain(|entry| entry.cursor != *cursor);
            entries.push(TranscriptEntry {
                cursor: *cursor,
                item: TranscriptItem::Message {
                    message: MessageBlock::User(message.clone()),
                },
                tool_calls: Vec::new(),
            });
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.cursor));
        let more = entries.len() > TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT;
        entries.truncate(TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT);
        let next_cursor = more.then(|| entries.last().unwrap().cursor);
        entries.reverse();
        Ok(TranscriptPage {
            inherited_through: inherited_transcript_through(&connection)?,
            entries,
            next_cursor,
        })
    }

    fn load_transcript_page(
        &self,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<TranscriptPage, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        load_transcript_page(&transaction, before, limit)
    }

    fn commit_model_turn_start(
        &self,
        context: &[MessageBlock],
        snapshot: &RequestSnapshot,
        timestamp: DateTime<Utc>,
    ) -> Result<ModelTurnStartCommit, ConversationStoreError> {
        validate_snapshot_identity(snapshot)?;
        snapshot
            .input
            .validate_budget()
            .map_err(ConversationStoreError::InvalidReference)?;
        // The one input validation rule shared by the fresh commit and the
        // idempotent retry (Issue #12, M9b): the exact ordered
        // request-scoped context the caller supplies must equal the frozen
        // `snapshot.request_context_ids` before the store chooses a path or
        // touches durable state. A fresh commit must never be able to append
        // context while persisting a snapshot whose `request_context_ids`
        // disagrees with what it just appended.
        validate_request_context(snapshot, context)?;
        validate_contribution_start(snapshot, context)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("request start transaction: {error}")))?;
        let admission_epoch = self.conn.manifest.capture(&transaction)?;
        let existing: Option<(Option<String>, Option<i64>, String)> = transaction
            .query_row(
                "SELECT CASE WHEN octet_length(snapshot_json)<=67108864 THEN snapshot_json END,started_sequence,CASE WHEN typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64 THEN manifest_digest ELSE '' END FROM request_snapshots WHERE request_id = ?1",
                [snapshot.request_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, read_manifest_digest(row, 2)?)),
            )
            .optional()
            .map_err(|error| storage(format!("request snapshot probe: {error}")))?;
        if let Some((json, started_sequence, integrity)) = existing {
            let json = json.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "frozen request record exceeds byte budget".into(),
                )
            })?;
            if stored_snapshot_digest(
                &json,
                read_stored_snapshot_root(&transaction, &snapshot.request_id)?,
            ) != integrity
            {
                return Err(ConversationStoreError::InvalidReference(
                    "corrupt frozen request manifest".into(),
                ));
            }
            let mut verified = verify_committed_start_tx(
                &transaction,
                &json,
                started_sequence,
                context,
                snapshot,
            )?;
            transaction
                .commit()
                .map_err(|error| storage(format!("request start retry commit: {error}")))?;
            verified.verified_snapshot = Some(Box::new(snapshot.clone()));
            return Ok(verified);
        }
        let projection = projection::current_checkpoint_id(&transaction)?;
        if projection.as_ref() != snapshot.history_cut.checkpoint_id.as_ref() {
            return Err(ConversationStoreError::InvalidReference(
                "actual request would start from a stale Projection Head".into(),
            ));
        }
        if snapshot.history_cut.base_frontier > snapshot.history_cut.ledger {
            return Err(ConversationStoreError::InvalidReference(
                "invalid Request History Cut".into(),
            ));
        }
        if snapshot.identity.retry_number != 0 {
            verify_frozen_contributions_tx(&transaction, snapshot)?;
        }
        // The request-scoped canonical context commits first, inside the
        // same transaction as the snapshot and the start fact: a failure
        // anywhere below rolls all of it back, so request-scoped context
        // can never become canonical without its request starting.
        let selection = manifest::persist(&transaction, &snapshot.input, &self.conn.manifest)?;
        let mut persisted = self.insert_fresh_start_tx(
            &transaction,
            context,
            snapshot,
            timestamp,
            &selection,
            admission_epoch,
        )?;
        process_death::reach("before:commit_model_turn_start");
        transaction
            .commit()
            .map_err(|error| storage(format!("request start commit: {error}")))?;
        process_death::reach("after:commit_model_turn_start");
        selection.commit(&self.conn.manifest);
        persisted.verified_snapshot = Some(Box::new(snapshot.clone()));
        Ok(persisted)
    }

    fn latest_contribution_emission(
        &self,
        producer: &crate::runtime::identity::ContextContributorIdentity,
        key: &str,
    ) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError> {
        #[cfg(test)]
        self.contribution_head_lookup_reads
            .fetch_add(1, Ordering::SeqCst);
        let connection = self.lock()?;
        read_contribution_head(&connection, producer, key)
    }

    fn load_pending_unresolved_output_stream_id(
        &self,
    ) -> Result<Option<PublicationStreamId>, ConversationStoreError> {
        let connection = self.lock()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT pending_unresolved_output_stream_id FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("pending carryover lookup: {error}")))?;
        Ok(value.map(PublicationStreamId::new))
    }

    fn commit_attempt_terminal_with_carryover(
        &self,
        event: RuntimeEventEnvelope,
        pending_source: Option<PublicationStreamId>,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        if !is_terminal(&event.event)
            || !matches!(
                event.event,
                RuntimeEvent::AttemptCompleted { .. }
                    | RuntimeEvent::AttemptCancelled { .. }
                    | RuntimeEvent::AttemptTimedOut { .. }
                    | RuntimeEvent::AttemptLimitExceeded { .. }
                    | RuntimeEvent::AttemptFailed { .. }
            )
        {
            return Err(ConversationStoreError::InvalidReference(
                "carryover terminal transition accepts only attempt terminal events".to_owned(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("attempt terminal transaction: {error}")))?;
        #[cfg(test)]
        {
            self.terminal_event_attempts.fetch_add(1, Ordering::SeqCst);
            if Self::consume(&self.fail_terminal_event_remaining) {
                return Err(storage("fault injected: terminal event commit"));
            }
            if Self::consume(&self.fail_event_remaining) {
                return Err(storage("fault injected: event commit"));
            }
        }
        validate_pending_carryover_source_tx(&transaction, &event, pending_source.as_ref())?;
        process_death::reach_event("before:attempt_terminal", &event.event);
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        let source = pending_source.as_ref().map(PublicationStreamId::as_str);
        transaction
            .execute(
                "UPDATE rustx_store SET pending_unresolved_output_stream_id=?1 WHERE id=1",
                [source],
            )
            .map_err(|error| storage(format!("install pending carryover source: {error}")))?;
        process_death::reach("before:commit_attempt_terminal");
        transaction
            .commit()
            .map_err(|error| storage(format!("attempt terminal commit: {error}")))?;
        process_death::reach("after:commit_attempt_terminal");
        Ok(persisted.event)
    }

    fn current_logical_step_progress(&self) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        let value: i64 = connection
            .query_row(
                "SELECT logical_step_sequence FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(format!("Todo progress lookup: {error}")))?;
        nonnegative(value, "Todo progress sequence")
    }

    fn load_request_snapshot(
        &self,
        request_id: &RequestId,
    ) -> Result<RequestSnapshot, ConversationStoreError> {
        #[cfg(test)]
        self.request_snapshot_body_reads
            .fetch_add(1, Ordering::SeqCst);
        {
            let connection = self.lock()?;
            load_request_snapshot(&connection, &self.conversation_id, request_id)
        }
    }

    fn reconstruct_model_request(
        &self,
        request_id: &crate::runtime::identity::RequestId,
    ) -> Result<crate::model::types::ModelRequest, ConversationStoreError> {
        let connection = self.lock()?;
        let snapshot = load_request_snapshot(&connection, &self.conversation_id, request_id)?;
        snapshot
            .reconstruct_with(|reference| {
                if reference.conversation_id != self.conversation_id {
                    return Err(
                        crate::model::snapshot::RequestReconstructionError::Conversation(
                            "foreign frozen Original scope".to_owned(),
                        ),
                    );
                }
                load_message(&connection, &reference.message_id).map_err(|error| {
                    crate::model::snapshot::RequestReconstructionError::Conversation(
                        error.to_string(),
                    )
                })
            })
            .map_err(|error| ConversationStoreError::InvalidReference(error.to_string()))
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
        if requires_specialized_transition(&event.event) {
            return Err(ConversationStoreError::InvalidReference(
                "this event kind must use its specialized durable transition".to_owned(),
            ));
        }
        self.append_event_internal(event, None)
    }

    fn commit_subagent_terminal(
        &self,
        event: RuntimeEventEnvelope,
    ) -> Result<RuntimeEventEnvelope, ConversationStoreError> {
        if !matches!(
            &event.event,
            RuntimeEvent::SubagentTerminalSettled {
                state: crate::events::types::SubagentTerminalState::Failed
                    | crate::events::types::SubagentTerminalState::Cancelled
                    | crate::events::types::SubagentTerminalState::Interrupted,
                ..
            }
        ) {
            return Err(ConversationStoreError::InvalidReference(
                "a direct Workflow child terminal transition accepts only non-success states"
                    .to_owned(),
            ));
        }
        let _ownership = self.ownership_transition(&event.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("Workflow child terminal transaction: {error}")))?;
        if let Some(existing) = find_event_by_id(&transaction, &event.event_id)? {
            if same_event_ignoring_sequence(&existing, &event) {
                transaction.commit().map_err(|error| {
                    storage(format!("Workflow child terminal retry commit: {error}"))
                })?;
                return Ok(existing);
            }
            return Err(ConversationStoreError::TerminalViolation(
                "Workflow child terminal retry conflicts with its durable fact".to_owned(),
            ));
        }
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: Workflow child terminal commit"));
        }
        process_death::reach_event("before:workflow_child_terminal", &event.event);
        let persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        transaction
            .commit()
            .map_err(|error| storage(format!("Workflow child terminal commit: {error}")))?;
        process_death::reach_event("after:workflow_child_terminal", &persisted.event.event);
        Ok(persisted.event)
    }

    fn commit_workflow_agent_terminal(
        &self,
        terminal: RuntimeEventEnvelope,
        output: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, RuntimeEventEnvelope), ConversationStoreError> {
        let valid_pair = matches!(
            (&terminal.event, &output.event),
            (
                RuntimeEvent::SubagentTerminalSettled {
                    subagent_id: terminal_subagent,
                    state: crate::events::types::SubagentTerminalState::Succeeded,
                    physical_settlement_proven: true,
                    ..
                },
                RuntimeEvent::WorkflowAgentOutputCommitted {
                    subagent_id: output_subagent,
                    ..
                }
            ) if terminal_subagent == output_subagent
        );
        if !valid_pair {
            return Err(ConversationStoreError::InvalidReference(
                "the Workflow terminal transaction requires one matching physically settled successful child terminal and output fact".to_owned(),
            ));
        }
        if terminal.conversation_id != output.conversation_id
            || terminal.timestamp != output.timestamp
        {
            return Err(ConversationStoreError::InvalidReference(
                "the Workflow terminal facts must share conversation and timestamp identity"
                    .to_owned(),
            ));
        }

        let _ownership = self.ownership_transition(&terminal.event)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("Workflow terminal transaction: {error}")))?;
        validate_event_reference(&transaction, &output)?;
        let existing_terminal = find_event_by_id(&transaction, &terminal.event_id)?;
        let existing_output = find_event_by_id(&transaction, &output.event_id)?;
        match (existing_terminal, existing_output) {
            (Some(existing_terminal), Some(existing_output)) => {
                if !same_event_ignoring_sequence(&existing_terminal, &terminal)
                    || !same_event_ignoring_sequence(&existing_output, &output)
                {
                    return Err(ConversationStoreError::InvalidReference(
                        "Workflow terminal retry conflicts with its durable facts".to_owned(),
                    ));
                }
                transaction
                    .commit()
                    .map_err(|error| storage(format!("Workflow terminal retry commit: {error}")))?;
                Ok((existing_terminal, existing_output))
            }
            (Some(_), None) | (None, Some(_)) => Err(ConversationStoreError::TerminalViolation(
                "Workflow terminal facts are only valid as an atomic pair".to_owned(),
            )),
            (None, None) => {
                let mut persisted_terminal =
                    persist_event_tx_deferred(&transaction, &self.conversation_id, terminal)?;
                let mut persisted_output =
                    persist_event_tx_deferred(&transaction, &self.conversation_id, output)?;
                #[cfg(test)]
                if Self::consume(&self.fail_event_remaining) {
                    return Err(storage("fault injected: Workflow terminal commit"));
                }
                let shared = observation_receipt::workflow_terminal(
                    &transaction,
                    &self.conversation_id,
                    &persisted_terminal.event,
                    &persisted_output.event,
                );
                persisted_terminal.event.observation.clone_from(&shared);
                persisted_output.event.observation = shared;
                process_death::reach_event(
                    "before:workflow_terminal",
                    &persisted_output.event.event,
                );
                transaction
                    .commit()
                    .map_err(|error| storage(format!("Workflow terminal commit: {error}")))?;
                process_death::reach_event(
                    "after:workflow_terminal",
                    &persisted_output.event.event,
                );
                Ok((persisted_terminal.event, persisted_output.event))
            }
        }
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
        process_death::reach_event("before:interaction", &persisted.event.event);
        transaction
            .commit()
            .map_err(|error| storage(format!("interaction audit commit: {error}")))?;
        process_death::reach_event("after:interaction", &persisted.event.event);
        Ok((persisted.event, transcript_cursor))
    }

    fn observe_journal(
        &self,
        observer: Arc<dyn super::presentation::JournalObserver>,
    ) -> Result<u64, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(error.to_string()))?;
        let through = journal_prefix(&transaction)?;
        // A newly installed Runtime owns recovery of already committed history.
        // Previous process-local publication holes cannot survive this epoch.
        observation_readiness::recover_journal(&transaction)?;
        observation_readiness::mark(&transaction, 1, through)?;
        transaction
            .execute(
                "UPDATE event_observation_readiness SET direct=0 WHERE direct=1",
                [],
            )
            .map_err(|error| storage(error.to_string()))?;
        observation_readiness::recover_transcript(&transaction)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        *self.journal_observer.lock().expect("Journal observer lock") = Some((observer, through));
        Ok(through)
    }

    fn mark_observation_ready(&self, first: u64, last: u64) -> Result<(), ConversationStoreError> {
        if first == 0 || first > last {
            return Err(ConversationStoreError::InvalidReference(
                "invalid observation readiness range".into(),
            ));
        }
        {
            let mut connection = self.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| storage(error.to_string()))?;
            observation_readiness::mark(&transaction, first, last)?;
            transaction
                .commit()
                .map_err(|error| storage(error.to_string()))?;
        }
        // Never wake under the storage or observer-registration lock, and never
        // before the tiny derived layout transaction physically commits.
        let observer = self
            .journal_observer
            .lock()
            .expect("Journal observer lock")
            .as_ref()
            .map(|(observer, _)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer.ready(last);
        }
        Ok(())
    }
    fn mark_observation_direct(&self, first: u64, last: u64) -> Result<(), ConversationStoreError> {
        if first == 0 || first > last {
            return Err(ConversationStoreError::InvalidReference(
                "invalid observation readiness range".into(),
            ));
        }
        {
            let mut connection = self.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| storage(error.to_string()))?;
            observation_readiness::mark_direct(&transaction, first, last)?;
            transaction
                .commit()
                .map_err(|error| storage(error.to_string()))?;
        }
        // Never wake under the storage or observer-registration lock, and never
        // before the tiny derived layout transaction physically commits.
        let observer = self
            .journal_observer
            .lock()
            .expect("Journal observer lock")
            .as_ref()
            .map(|(observer, _)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer.ready(last);
        }
        Ok(())
    }
    fn observation_ready_frontier(
        &self,
        after: u64,
        through: u64,
    ) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        observation_readiness::frontier(&connection, after, through)
    }

    fn mark_observation_transcript_ready(&self, cursor: u64) -> Result<(), ConversationStoreError> {
        {
            let mut connection = self.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| storage(error.to_string()))?;
            observation_readiness::mark_transcript(&transaction, cursor)?;
            transaction
                .commit()
                .map_err(|error| storage(error.to_string()))?;
        }
        let observer = self
            .journal_observer
            .lock()
            .expect("Journal observer lock")
            .as_ref()
            .map(|(observer, _)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer.ready(0);
        }
        Ok(())
    }
    fn mark_canonical_transcript_ready(
        &self,
        first_ledger: u64,
        last_ledger: u64,
    ) -> Result<(), ConversationStoreError> {
        {
            let mut connection = self.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| storage(error.to_string()))?;
            observation_readiness::mark_canonical(&transaction, first_ledger, last_ledger)?;
            transaction
                .commit()
                .map_err(|error| storage(error.to_string()))?;
        }
        let observer = self
            .journal_observer
            .lock()
            .expect("Journal observer lock")
            .as_ref()
            .map(|(observer, _)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer.ready(0);
        }
        Ok(())
    }
    fn observation_transcript_ready_frontier(
        &self,
        after: u64,
        through: u64,
    ) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        observation_readiness::transcript_frontier(&connection, after, through)
    }

    fn transcript_source_ready(
        &self,
    ) -> Result<crate::durable::observation::TranscriptSourceReady, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let read_cut = observation::cut(&transaction, &self.conversation_id)?;
        let prefix =
            observation_readiness::transcript_frontier(&transaction, 0, read_cut.read.transcript)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(crate::durable::observation::TranscriptSourceReady { prefix, read_cut })
    }

    fn presentation_frontier(&self) -> Result<u64, ConversationStoreError> {
        let value: i64 = self
            .lock()?
            .query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| {
                row.get(0)
            })
            .map_err(|_| storage("presentation frontier failed"))?;
        sequence_from_i64(value)
    }

    fn read_presentation_events(
        &self,
        query: &super::presentation::FactQuery,
    ) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
        let statements = presentation_statements(query)?;
        let connection = self.lock()?;
        let mut events: Vec<RuntimeEventEnvelope> = Vec::new();
        for (sql, params) in statements {
            let mut statement = connection
                .prepare(&sql)
                .map_err(|_| storage("presentation query preparation failed"))?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(params), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|_| storage("presentation query failed"))?;
            for row in rows {
                #[cfg(test)]
                observation_statistics::original_read();
                events.push(decode(
                    &row.map_err(|_| storage("presentation row failed"))?,
                    "presentation fact",
                )?);
            }
        }
        events.sort_by_key(|event| event.sequence);
        if !query.ascending {
            events.reverse();
        }
        events.truncate(query.limit);
        Ok(events)
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

    fn live_block_header_page(
        &self,
        source: &crate::runtime_client::content::ContentSource,
        through_revision: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::runtime_client::content::LiveBlockHeaderPage, ConversationStoreError> {
        let connection = self.lock()?;
        content::live_block_header_page(
            &connection,
            &self.conversation_id,
            source,
            through_revision,
            offset,
            limit,
        )
    }

    fn live_tool_header_page(
        &self,
        source: &crate::runtime_client::content::ContentSource,
        through_revision: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::runtime_client::content::LiveBlockHeaderPage, ConversationStoreError> {
        let connection = self.lock()?;
        content::live_tool_header_page(
            &connection,
            &self.conversation_id,
            source,
            through_revision,
            offset,
            limit,
        )
    }

    fn content_descriptor_at(
        &self,
        source: &crate::runtime_client::content::ContentSource,
        representation: crate::runtime_client::content::ContentRepresentation,
        revision: u64,
    ) -> Result<Option<crate::runtime_client::content::ContentRef>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = content::descriptor_at(
            &transaction,
            &self.conversation_id,
            source,
            representation,
            revision,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn content_descriptor(
        &self,
        source: &crate::runtime_client::content::ContentSource,
        representation: crate::runtime_client::content::ContentRepresentation,
    ) -> Result<Option<crate::runtime_client::content::ContentRef>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result =
            content::descriptor(&transaction, &self.conversation_id, source, representation)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }

    fn content_read(
        &self,
        reference: &crate::runtime_client::content::ContentRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::runtime_client::content::ContentChunk, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = content::read(
            &transaction,
            &self.conversation_id,
            reference,
            offset,
            max_bytes,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
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
        let (snapshot, _, _) =
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
        process_death::reach("before:open_publication_stream");
        transaction
            .commit()
            .map_err(|error| storage(format!("publication open commit: {error}")))?;
        process_death::reach("after:open_publication_stream");
        Ok(())
    }

    fn stage_publication_frames(
        &self,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError> {
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
        let (stream, receipts) = stage_frames_tx(&transaction, frames, &self.conversation_id)?;
        if stream.terminal_sequence.is_some() {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication stream {} already reached its terminal boundary",
                stream.start.stream_id
            )));
        }
        process_death::reach("before:stage_publication_frames");
        transaction
            .commit()
            .map_err(|error| storage(format!("publication staging commit: {error}")))?;
        process_death::reach("after:stage_publication_frames");
        Ok(receipts)
    }

    fn commit_publication_terminal(
        &self,
        stream_id: &PublicationStreamId,
        frames: &[PublicationFrame],
    ) -> Result<Vec<crate::publication::PublicationFrameReceipt>, ConversationStoreError> {
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
        let (snapshot, _, completed_sequence) = require_started_request_tx(
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
            &snapshot,
            completed_sequence,
        )? {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "publication terminal for {stream_id} has no durable provider outcome for request {}",
                stream.start.request_id
            )));
        }
        let (stream, receipts) = stage_frames_tx(&transaction, frames, &self.conversation_id)?;
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
        process_death::reach("before:commit_publication_terminal");
        transaction
            .commit()
            .map_err(|error| storage(format!("publication terminal commit: {error}")))?;
        process_death::reach("after:commit_publication_terminal");
        Ok(receipts)
    }

    // Keep native acceptance, event/staging and COMMIT in one auditable boundary.
    #[allow(clippy::too_many_lines)]
    fn commit_canonical_publication(
        &self,
        stream_id: &PublicationStreamId,
        message: &MessageBlock,
        event: RuntimeEventEnvelope,
    ) -> Result<(RuntimeEventEnvelope, TranscriptCommitReceipt), ConversationStoreError> {
        let mut connection = self.lock()?;
        let mut validation = append_validation::Batch::begin(&self.conn.append_validation);
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("canonical publication transaction: {error}")))?;
        let stream = require_publication_stream(&transaction, stream_id)?;
        let (snapshot, _, completed_sequence) = require_started_request_tx(
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
            &snapshot,
            completed_sequence,
        )? {
            return Err(ConversationStoreError::PublicationViolation(format!(
                "canonical acceptance of publication stream {stream_id} has no durable provider outcome for request {}",
                stream.start.request_id
            )));
        }
        ensure_surface_head(&transaction)?;

        let transcript_cursor =
            validation
                .append(&transaction, message, None)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "canonical publication must receive a transcript cursor".to_owned(),
                    )
                })?;
        #[cfg(test)]
        if Self::consume(&self.fail_event_remaining) {
            return Err(storage("fault injected: canonical publication commit"));
        }
        let mut persisted = persist_event_tx(&transaction, &self.conversation_id, event)?;
        content::settle_canonical(&transaction, stream_id, message)?;
        rebind_published_read_layouts(&transaction, message, &persisted.event)?;
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
        let mut receipt = canonical_observation_receipt(
            &transaction,
            &self.conversation_id,
            message,
            Some(transcript_cursor),
            persisted.event.tool_occurrence.clone(),
        )?;
        process_death::reach("before:commit_canonical_publication");
        let sequence = persisted.event.sequence;
        set_canonical_range(&mut receipt, Some((sequence, sequence)));
        persisted.event.observation = observation_receipt::canonical(
            &transaction,
            &self.conversation_id,
            &persisted.event,
            Arc::clone(&receipt.observation),
        );
        transaction
            .commit()
            .map_err(|error| storage(format!("canonical publication commit: {error}")))?;
        validation.committed(&connection);
        process_death::reach("after:commit_canonical_publication");
        Ok((persisted.event, receipt))
    }

    fn terminalize_publication_audit(
        &self,
        stream_id: &PublicationStreamId,
        timestamp: DateTime<Utc>,
    ) -> Result<
        (
            PublicationAudit,
            crate::durable::inbox::PublicationAuditReceipt,
        ),
        ConversationStoreError,
    > {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| storage(format!("publication audit transaction: {error}")))?;
        #[cfg(test)]
        if Self::consume(&self.fail_publication_audit_remaining) {
            return Err(storage("fault injected: publication audit commit"));
        }
        let stream = require_publication_stream(&transaction, stream_id)?;
        let (snapshot, _, completed_sequence) = require_started_request_tx(
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
                &snapshot,
                completed_sequence,
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
        let _ =
            optional_display_layout(&transaction, || content::settle_audit(&transaction, &audit));
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
        let receipt = audit_observation_receipt(
            &transaction,
            &self.conversation_id,
            &audit,
            transcript_cursor,
        )?;
        process_death::reach("before:terminalize_publication_audit");
        transaction
            .commit()
            .map_err(|error| storage(format!("publication audit commit: {error}")))?;
        process_death::reach("after:terminalize_publication_audit");
        Ok((audit, receipt))
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
    transaction: &Connection,
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
    transaction: &Connection,
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

fn rewrite_pending_body(
    transaction: &Transaction<'_>,
    item: &mut PendingInboundItem,
    expected: &crate::durable::inbox::PendingInboundRef,
    text: &str,
) -> Result<(), ConversationStoreError> {
    item.message.content = vec![UserContentBlock::Text(crate::message::content::TextBlock {
        text: text.to_owned(),
    })];
    let revision = item
        .revision
        .checked_add(1)
        .filter(|value| i64::try_from(*value).is_ok())
        .ok_or(ConversationStoreError::SequenceExhausted)?;
    transaction
        .execute(
            "UPDATE pending_inbound SET message_json=?1, revision=?2 WHERE sequence=?3",
            params![
                encode(&item.message, "pending edit")?,
                revision,
                seq_to_i64(expected.sequence.get())?
            ],
        )
        .map_err(|error| storage(format!("pending edit: {error}")))?;
    let _ = optional_display_layout(transaction, || {
        content::index_pending(
            transaction,
            &item.message,
            expected.sequence.get(),
            revision,
        )?;
        observation::index_message(transaction, &MessageBlock::User(item.message.clone()), None)
    });
    let _ = observation::index_pending_header(
        transaction,
        expected.sequence.get(),
        &item.message_id,
        None,
    );
    Ok(())
}

fn request_start_observation_tx(
    transaction: &Transaction<'_>,
    conversation: &ConversationId,
    context: &[MessageBlock],
    snapshot: &RequestSnapshot,
    started: RuntimeEventEnvelope,
    contribution_emissions: Vec<RuntimeEventEnvelope>,
) -> Result<ModelTurnStartCommit, ConversationStoreError> {
    let context_journal_range = Some((
        started.sequence,
        contribution_emissions
            .last()
            .map_or(started.sequence, |event| event.sequence),
    ));
    let context_receipts = canonical_observation_batch(
        transaction,
        conversation,
        &context
            .iter()
            .map(|message| (message.id(), None))
            .collect::<Vec<_>>(),
        context_journal_range,
    )?;
    let mut started = started;
    let status_sources = observation_receipt::status_sources(transaction, conversation, snapshot);
    let shared = observation_receipt::request_start(
        transaction,
        conversation,
        &started,
        &contribution_emissions,
        context_receipts
            .first()
            .map(|receipt| Arc::clone(&receipt.observation)),
        snapshot,
        &status_sources,
    );
    started.observation.clone_from(&shared);
    let mut contribution_emissions = contribution_emissions;
    for emission in &mut contribution_emissions {
        emission.observation.clone_from(&shared);
    }
    Ok(ModelTurnStartCommit {
        verified_snapshot: None,
        status_sources,
        context_receipts,
        started,
        contribution_emissions,
        disposition: ModelTurnStartCommitDisposition::NewlyCommitted,
    })
}

fn validate_fresh_input_binding(
    connection: &Connection,
    snapshot: &RequestSnapshot,
    json: &str,
    root: Option<manifest::NodeRef>,
) -> Result<(), ConversationStoreError> {
    let actual: String = connection
        .query_row(
            "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
            [snapshot.request_id.as_str()],
            |r| r.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if actual != json || read_stored_snapshot_root(connection, &snapshot.request_id)? != root {
        return Err(ConversationStoreError::InvalidReference(
            "fresh request stored Snapshot/root differs from exact admitted values".into(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Native start inputs plus its transaction-local optional fragment session.
fn index_request_read_layouts(
    transaction: &Transaction<'_>,
    scope: &ConversationId,
    snapshot: &RequestSnapshot,
    input: &crate::model::SelectedModelInput,
    selection: &manifest::Selection,
    retry_message_id: Option<&MessageId>,
    context: &[MessageBlock],
    event: &RuntimeEventEnvelope,
) {
    let _ = optional_display_layout(transaction, || {
        if let Some(retry) = retry_message_id {
            content::index_utf8_field(
                transaction,
                "response_identity",
                &format!("request-start:{}", event.sequence),
                1,
                retry.as_str(),
            )?;
        }
        Ok(())
    });
    let _ = optional_display_layout(transaction, || {
        observation_trace::request_core(transaction, event, snapshot)
    });
    let _ = optional_display_layout(transaction, || {
        transaction
            .execute(
                "UPDATE request_snapshots SET observation_source_id=?2 WHERE request_id=?1",
                params![snapshot.request_id.as_str(), event.sequence.to_string()],
            )
            .map_err(|error| storage(error.to_string()))?;
        observation_request_summary::index(transaction, snapshot)?;
        observation_request::index_selection(
            transaction,
            event.sequence,
            scope,
            input,
            snapshot,
            selection,
        )
    });
    let _ = optional_display_layout(transaction, || {
        observation_trace::index_context(transaction, event.sequence, snapshot, context)
    });
    // Catalog identity evidence is independent of optional request-body layouts.
    let _ = optional_display_layout(transaction, || {
        observation_trace::index_catalog(transaction, event.sequence, snapshot)
    });
}

fn rebind_published_read_layouts(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let _ = optional_display_layout(transaction, || {
        observation_trace::settle_fields(transaction, event)
    });
    rebind_canonical_display(transaction, message)
}

fn rebind_canonical_display(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    // Rebind deferred Tool arguments to their retained original raw encoding at C.
    let position: i64 = transaction
        .query_row(
            "SELECT position FROM message_ledger WHERE message_id=?1",
            [message.id().as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let _ = optional_display_layout(transaction, || {
        observation::index_message(
            transaction,
            message,
            Some(nonnegative(position, "header ledger position")?),
        )
    });
    Ok(())
}

fn audit_observation_receipt(
    connection: &Connection,
    conversation: &ConversationId,
    audit: &PublicationAudit,
    cursor: TranscriptCursor,
) -> Result<crate::durable::inbox::PublicationAuditReceipt, ConversationStoreError> {
    let read_cut = observation::cut(connection, conversation)?;
    let summary = observation::audit_summary(audit);
    let _ = optional_display_layout(connection, || {
        observation::index_audit(connection, audit, cursor)?;
        connection.execute("UPDATE observation_headers SET journal_position=?1 WHERE reference_kind='publication_audit' AND reference_id=?2",params![read_cut.read.journal,audit.stream_id.as_str()]).map_err(|error|storage(error.to_string()))?;
        Ok(())
    });
    let bytes:Option<Vec<u8>>=connection.query_row("SELECT header_json FROM observation_headers WHERE reference_kind='publication_audit' AND reference_id=?1",[audit.stream_id.as_str()],|row|row.get(0)).optional().ok().flatten();
    let header = bytes
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| crate::durable::observation::TranscriptHeader {
            tool_calls: Vec::new(),
            publication_settlement: Some(
                crate::runtime_client::snapshot::RuntimeClientPublicationSettlementRecord {
                    settlement: summary.clone().into(),
                    transcript_cursor: cursor.into(),
                    read_cut: read_cut.clone(),
                },
            ),
            cursor,
            reference_kind: "publication_audit".into(),
            reference_id: format!("transcript:{}", cursor.get()),
            message: None,
            display: None,
            content: Vec::new(),
            collections: Vec::new(),
            deferred: true,
        });
    Ok(crate::durable::inbox::PublicationAuditReceipt {
        ready_through: observation_readiness::frontier(connection, 0, read_cut.read.journal).ok(),
        ready_t_through: observation_readiness::transcript_frontier(
            connection,
            0,
            read_cut.read.transcript,
        )
        .ok(),
        summary,
        transcript_cursor: cursor,
        header,
        read_cut,
    })
}

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
/// returns the prior stream and finite per-frame committed source evidence.
fn stage_frames_tx(
    transaction: &Transaction<'_>,
    frames: &[PublicationFrame],
    conversation_id: &ConversationId,
) -> Result<
    (
        PublicationStreamRecord,
        Vec<crate::publication::PublicationFrameReceipt>,
    ),
    ConversationStoreError,
> {
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

    let mut receipts = Vec::with_capacity(frames.len());
    for frame in frames {
        let receipt = stage_frame_tx(transaction, conversation_id, &stream.start, frame)?;
        receipts.push(receipt);
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
    Ok((stream, receipts))
}

fn stage_frame_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    start: &PublicationStreamStart,
    frame: &PublicationFrame,
) -> Result<crate::publication::PublicationFrameReceipt, ConversationStoreError> {
    let stream_id = &start.stream_id;
    let failed:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM publication_frames INDEXED BY publication_frames_unindexed_idx WHERE stream_id=?1 AND observation_indexed=0)",[stream_id.as_str()],|row|row.get(0)).unwrap_or(true);
    let receipt = (!failed)
        .then(|| {
            optional_display_layout(transaction, || {
                content::index_frame(transaction, frame)?;
                content::frame_receipt(transaction, conversation_id, start, frame)
            })
        })
        .flatten()
        .unwrap_or_else(|| crate::publication::PublicationFrameReceipt {
            unavailable: true,
            stream_id: frame.stream_id.clone(),
            sequence: frame.sequence,
            content: Vec::new(),
            suffix_bytes: None,
            block_ordinal: None,
            block_total: 0,
            tool_ordinal: None,
            tool_total: 0,
        });
    transaction
            .execute(
                "INSERT INTO publication_frames(stream_id,sequence,frame_json,observation_indexed) VALUES(?1,?2,?3,?4)",
                params![
                    stream_id.as_str(),
                    seq_to_i64(frame.sequence)?,
                    encode(frame, "publication frame")?,
                    !receipt.unavailable
                ],
            )
            .map_err(|error| storage(format!("stage publication frame: {error}")))?;
    Ok(receipt)
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
    snapshot: &RequestSnapshot,
    completed_sequence: Option<u64>,
) -> Result<bool, ConversationStoreError> {
    // The caller has already validated this exact immutable row and its start
    // in this transaction, before generation/envelope checks. Reuse its P marker.
    let request_id = &snapshot.request_id;
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
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One owner validates execution envelopes and exact canonical references.
fn record_tool_proposal_dependency(
    transaction: &Transaction<'_>,
    call_id: &ToolCallId,
    attempt_id: Option<&AttemptId>,
    turn_id: Option<&TurnId>,
    expected_tool_id: Option<&ToolId>,
    dependency: &str,
    allow_unowned_canonical: bool,
    canonical_owner: Option<&MessageId>,
) -> Result<(), ConversationStoreError> {
    // Detached authorization facts intentionally outlive an attempt and carry
    // no envelope generation. Resolve those facts through the current durable
    // Surface: an active canonical Assistant is the only valid owner, and
    // ambiguous publication owners are rejected. This keeps a historical
    // audited/canonical reuse from becoming a bare-call-id alias.
    let detached_active_messages =
        if canonical_owner.is_none() && attempt_id.is_none() && turn_id.is_none() {
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
        let owner_tool_id = if let Some(owner) = canonical_owner {
            let message = load_message_tx(transaction, owner)?;
            match message {
                MessageBlock::Assistant(assistant) => {
                    assistant.content.iter().find_map(|block| match block {
                        AssistantContentBlock::ToolCall(call) if call.id == *call_id => {
                            Some(call.tool_id.clone())
                        }
                        _ => None,
                    })
                }
                _ => None,
            }
        } else {
            canonical_surface_tool_id(transaction, call_id)?
        };
        if let Some(owner_tool_id) = owner_tool_id {
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
                canonical_owner.is_none_or(|owner| candidate_message == owner.as_str())
                    && attempt_id.is_none_or(|attempt| candidate_attempt == attempt.as_str())
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

fn native_approval_key(id: &crate::tools::types::ToolInvocationId) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(serde_json::to_vec(id).expect("invocation identity"));
    format!("native-approval:{digest:x}")
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
/// Every Agent Approval reaches this with a publication owner: the Agent Loop
/// commits an Assistant message through `commit_canonical_publication` and
/// refuses to commit one at all without an open stream, so a canonical
/// `ToolCall` without a frozen `(attempt, turn, message_id)` owner cannot occur
/// on the approval path. There is deliberately no lenient fallback for a state
/// the runtime cannot produce and no database can already contain.
/// Workflow subjects instead pin exact prepared invocation facts in this
/// interaction transaction, without consulting observational native events.
fn validate_approval_subject_against_canonical(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    interaction_id: &InteractionId,
    attempt_id: &AttemptId,
    turn_id: &TurnId,
    subject: &InteractionSubject,
) -> Result<(), ConversationStoreError> {
    let InteractionSubject::Approval {
        invocation_id,
        tool_id,
        tool_name,
        arguments_digest,
        ..
    } = subject
    else {
        return Ok(());
    };
    let call_id = match invocation_id {
        crate::tools::types::ToolInvocationId::Agent { call_id } => call_id,
        crate::tools::types::ToolInvocationId::Workflow { node } => {
            if node.block.run.attempt_id != *attempt_id
                || node.block.run.conversation_id != *conversation_id
            {
                return Err(ConversationStoreError::InvalidReference(
                    "approval attempt differs from native invocation owner".into(),
                ));
            }
            // The interaction-request transaction is the exact preparation audit
            // boundary. Native execution events are observations, not authority.
            // The coordinator supplies this immutable subject only after preflight.
            let key = native_approval_key(invocation_id);
            let exists: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM lifecycle_state WHERE lifecycle_key=?1)",
                    [&key],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            if exists {
                return Err(ConversationStoreError::InvalidReference(
                    "native invocation already has a pinned approval subject".into(),
                ));
            }
            return Ok(());
        }
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

fn reject_unaccounted_goal(draft: &InboundDraft) -> Result<(), ConversationStoreError> {
    if matches!(draft.kind, InboundKind::GoalContinuation(_)) {
        return Err(ConversationStoreError::InvalidReference(
            "Goal continuation must cross accept_goal_round with atomic accounting".to_owned(),
        ));
    }
    Ok(())
}

fn read_goal(
    connection: &Connection,
) -> Result<Option<crate::goal::GoalSnapshot>, ConversationStoreError> {
    let json: Option<String> = connection
        .query_row(
            "SELECT snapshot_json FROM goal_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("read Goal: {error}")))?;
    json.map(|json| {
        serde_json::from_str(&json).map_err(|error| storage(format!("decode Goal: {error}")))
    })
    .transpose()
}

fn save_goal(
    tx: &Transaction<'_>,
    goal: &crate::goal::GoalSnapshot,
) -> Result<(), ConversationStoreError> {
    let header = crate::runtime_client::goal::RuntimeClientGoalView::from_native(Some(goal));
    let bytes = crate::runtime_client::checkpoint::bounded_bytes(&header, 16384);
    tx.execute("INSERT INTO goal_state(singleton, snapshot_json,observation_header) VALUES(1, ?1,?2) ON CONFLICT(singleton) DO UPDATE SET snapshot_json = excluded.snapshot_json,observation_header=excluded.observation_header,observation_source_id=NULL,observation_sequence=NULL",
        params![encode(goal, "Goal")?,bytes]).map_err(|error| storage(format!("write Goal: {error}")))?;
    let _ = optional_display_layout(tx, || observation_state::goal(tx, goal));
    Ok(())
}

fn refresh_pending_receipt(
    connection: &Connection,
    conversation: &ConversationId,
    accepted: &mut AcceptedInbound,
) -> Result<(), ConversationStoreError> {
    accepted.read_cut = reading::cut(connection, conversation)?;
    accepted.pending_observation = observation_receipt::pending(
        connection,
        conversation,
        Some(accepted.sequence.get()),
        None,
        None,
    );
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
                goal_observation: None,
                pending_observation: observation_receipt::pending(
                    transaction,
                    &store.conversation_id,
                    Some(sequence_from_i64(sequence)?),
                    None,
                    None,
                ),
                read_cut: reading::cut(transaction, &store.conversation_id)?,
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
            "UPDATE rustx_store SET next_inbound_sequence = ?1,pending_observation_count=pending_observation_count+1 WHERE id = 1",
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
    let _ = optional_display_layout(transaction, || {
        content::index_pending(transaction, &message, sequence, 0)?;
        observation::index_message(transaction, &MessageBlock::User(message.clone()), None)
    });
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
    let _ = observation::index_pending_header(
        transaction,
        sequence,
        &message_id,
        transcript_cursor.map(TranscriptCursor::get),
    );
    Ok(AcceptedInbound {
        goal_observation: None,
        pending_observation: observation_receipt::pending(
            transaction,
            &store.conversation_id,
            Some(sequence),
            None,
            None,
        ),
        read_cut: reading::cut(transaction, &store.conversation_id)?,
        sequence: InboundSequence::new(sequence),
        message_id,
        message,
        transcript_cursor,
        retried: false,
    })
}

fn response_identity_key(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    source: &crate::durable::observation::ResponseIdentitySource,
    path: &[String],
) -> Result<(String, u32), ConversationStoreError> {
    let field = match path {
        [field] if field == "closing_message_id" => 0,
        [field] if field == "retry_message_id" => 1,
        [origin, field] if origin == "origin" && field == "closing_message_id" => 2,
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "unknown response identity source field".into(),
            ));
        }
    };
    match source {
        crate::durable::observation::ResponseIdentitySource::Attempt { sequence } => {
            if *sequence > cut.read.journal {
                return Err(ConversationStoreError::InvalidReference(
                    "response outside source cut".into(),
                ));
            }
            let summary = observation_execution::summary_at_sequence(connection, *sequence)?
                .ok_or_else(|| storage("native response execution header unavailable"))?;
            if field == 1 {
                Ok((
                    format!(
                        "request-start:{}",
                        summary
                            .last_request_sequence
                            .ok_or_else(|| storage("response request identity unavailable"))?
                    ),
                    1,
                ))
            } else {
                Ok((
                    format!(
                        "journal:{}",
                        summary
                            .closing_identity_sequence
                            .ok_or_else(|| storage("response closing identity unavailable"))?
                    ),
                    0,
                ))
            }
        }
        crate::durable::observation::ResponseIdentitySource::Inherited { ordinal } => {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM inherited_response_provenance WHERE ordinal=?1 AND closing_ledger_position<=?2)",
                    params![ordinal,cut.ledger_through],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            if !exists {
                return Err(ConversationStoreError::InvalidReference(
                    "inherited response identity unavailable".into(),
                ));
            }
            Ok((format!("inherited:{ordinal}"), field))
        }
    }
}
fn response_identity_fields(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    source: &crate::durable::observation::ResponseIdentitySource,
) -> Result<Vec<crate::durable::observation::ResponseIdentityField>, ConversationStoreError> {
    let mut fields = Vec::new();
    for path in [
        vec!["closing_message_id".into()],
        vec!["retry_message_id".into()],
        vec!["origin".into(), "closing_message_id".into()],
    ] {
        if let Ok((key, field)) = response_identity_key(connection, cut, source, &path)
            && let Some(readable_end) =
                content::utf8_field_end(connection, "response_identity", &key, field)?
        {
            fields.push(crate::durable::observation::ResponseIdentityField { path, readable_end });
        }
    }
    Ok(fields)
}

fn canonical_observation_receipt(
    connection: &Connection,
    conversation: &ConversationId,
    message: &MessageBlock,
    transcript_cursor: Option<TranscriptCursor>,
    _tool_occurrence: Option<crate::events::types::ToolObservationOccurrence>,
) -> Result<TranscriptCommitReceipt, ConversationStoreError> {
    canonical_observation_receipt_for_id(
        connection,
        conversation,
        message.id(),
        transcript_cursor,
        None,
    )
}
fn canonical_observation_receipt_for_id(
    connection: &Connection,
    conversation: &ConversationId,
    message_id: &MessageId,
    transcript_cursor: Option<TranscriptCursor>,
    _tool_occurrence: Option<crate::events::types::ToolObservationOccurrence>,
) -> Result<TranscriptCommitReceipt, ConversationStoreError> {
    let mut receipts = canonical_observation_batch(
        connection,
        conversation,
        &[(message_id, transcript_cursor)],
        None,
    )?;
    Ok(receipts.remove(0))
}
fn set_canonical_range(receipt: &mut TranscriptCommitReceipt, range: Option<(u64, u64)>) {
    Arc::make_mut(&mut receipt.observation).journal_range = range;
}
fn canonical_observation_batch(
    connection: &Connection,
    conversation: &ConversationId,
    items: &[(&MessageId, Option<TranscriptCursor>)],
    journal_range: Option<(u64, u64)>,
) -> Result<Vec<TranscriptCommitReceipt>, ConversationStoreError> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let through = journal_prefix(connection)?;
    let mut source_unavailable = false;
    let mut positions = Vec::with_capacity(items.len());
    for (id, _) in items {
        let position: u64 = connection
            .query_row(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        source_unavailable |= connection.execute("UPDATE observation_headers SET journal_position=?1,header_json=CAST(json_set(header_json,'$.transcript_cursor',transcript_position) AS BLOB) WHERE ledger_position=?2",params![through,position]).is_err();
        positions.push(position);
    }
    let first_position = *positions.first().expect("nonempty canonical batch");
    let last_position = *positions.last().expect("nonempty canonical batch");
    let read_cut = observation::cut(connection, conversation)?;
    let head = canonical_display_head(connection, conversation, &read_cut, &mut source_unavailable);
    let scalars = complete_canonical_scalars(connection, &read_cut).unwrap_or_else(|_| {
        source_unavailable = true;
        CanonicalSourceScalars::default()
    });
    let batch = Arc::new(crate::durable::inbox::CanonicalObservationBatch {
        ready_through: observation_readiness::frontier(connection, 0, through).ok(),
        ready_t_through: observation_readiness::transcript_frontier(
            connection,
            0,
            read_cut.read.transcript,
        )
        .ok(),
        source_unavailable,
        todos: scalars.todos,
        read_cut,
        journal_range,
        first_position,
        last_position,
        count: items.len() as u64,
        first_transcript: items.iter().filter_map(|(_, cursor)| *cursor).min(),
        last_transcript: items.iter().filter_map(|(_, cursor)| *cursor).max(),
        transcript_count: items.iter().filter(|(_, cursor)| cursor.is_some()).count() as u64,
        head,
        human_activity_at: scalars.human_activity_at,
        has_opening_user: scalars.has_opening_user,
        opening_preview: scalars.opening_preview,
    });
    Ok(items
        .iter()
        .zip(positions)
        .enumerate()
        .map(
            |(ordinal, ((_, transcript_cursor), ledger_position))| TranscriptCommitReceipt {
                transcript_cursor: *transcript_cursor,
                ledger_position,
                ordinal: ordinal as u64,
                observation: Arc::clone(&batch),
            },
        )
        .collect())
}

#[derive(Default)]
struct CanonicalSourceScalars {
    human_activity_at: Option<chrono::DateTime<Utc>>,
    has_opening_user: bool,
    opening_preview: Option<String>,
    todos: Option<crate::durable::inbox::NativeTodoObservation>,
}
fn complete_canonical_scalars(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
) -> Result<CanonicalSourceScalars, ConversationStoreError> {
    let activity: Option<(i64,u32)> = connection.query_row(
        "SELECT human_activity_seconds,human_activity_nanos FROM message_ledger INDEXED BY message_ledger_human_activity_idx WHERE human_activity_at IS NOT NULL AND position<=?1 ORDER BY human_activity_seconds DESC,human_activity_nanos DESC LIMIT 1",
        [cut.ledger_through], |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional().map_err(|error| storage(error.to_string()))?;
    let opening: Option<Option<String>> = connection.query_row(
        "SELECT opening_preview FROM message_ledger INDEXED BY message_ledger_opening_idx WHERE ordinary_user=1 AND position<=?1 ORDER BY position LIMIT 1",
        [cut.ledger_through], |row| row.get(0),
    ).optional().map_err(|error| storage(error.to_string()))?;
    Ok(CanonicalSourceScalars {
        human_activity_at: activity
            .map(|(seconds, nanos)| {
                chrono::DateTime::from_timestamp(seconds, nanos)
                    .ok_or_else(|| storage("native human activity timestamp unavailable"))
            })
            .transpose()?,
        has_opening_user: opening.is_some(),
        opening_preview: opening.flatten(),
        todos: observation_state::todo_head(connection, 0, cut.ledger_through)?,
    })
}
fn canonical_display_head(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &crate::durable::observation::ObservationReadCut,
    source_unavailable: &mut bool,
) -> Vec<crate::durable::observation::MessageHeader> {
    match observation::messages(
        connection,
        conversation,
        cut,
        Some(cut.ledger_through.saturating_sub(8)),
        8,
    ) {
        Ok(page) if !*source_unavailable => page.headers,
        _ => {
            *source_unavailable = true;
            Vec::new()
        }
    }
}

fn append_canonical_messages(
    store: &SqliteConversationStore,
    messages: &[MessageBlock],
) -> Result<Vec<TranscriptCommitReceipt>, ConversationStoreError> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let mut connection = store.lock()?;
    let mut validation = append_validation::Batch::begin(&store.conn.append_validation);
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
                Some(&tool.occurrence.assistant_message_id),
            )?;
        }
        receipts.push(validation.append(&transaction, message, None)?);
    }
    let receipts = canonical_observation_batch(
        &transaction,
        &store.conversation_id,
        &messages
            .iter()
            .zip(receipts)
            .map(|(message, cursor)| (message.id(), cursor))
            .collect::<Vec<_>>(),
        None,
    )?;
    transaction
        .commit()
        .map_err(|error| storage(format!("canonical commit: {error}")))?;
    validation.committed(&connection);
    Ok(receipts)
}

fn ordinary_user_by_id(
    connection: &Connection,
    id: &MessageId,
) -> Result<bool, ConversationStoreError> {
    connection
        .prepare_cached("SELECT ordinary_user FROM message_ledger WHERE message_id=?1")
        .map_err(|error| storage(error.to_string()))?
        .query_row([id.as_str()], |row| row.get(0))
        .optional()
        .map(|value| value.unwrap_or(false))
        .map_err(|error| storage(error.to_string()))
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
) -> Result<ModelTurnStartCommit, ConversationStoreError> {
    let Some(started_sequence) = started_sequence else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} exists without a durable start sequence",
            snapshot.request_id
        )));
    };
    let mut stored: RequestSnapshot = decode(json, "request snapshot")?;
    if !stored.input.is_empty() {
        return Err(ConversationStoreError::InvalidReference(
            "normalized frozen request metadata contains selected input".into(),
        ));
    }
    stored.input = manifest::load(
        transaction,
        read_stored_snapshot_root(transaction, &snapshot.request_id)?,
    )?;
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
    let contribution_emissions =
        verify_contribution_start_tx(transaction, context, &stored, &started)?;
    Ok(ModelTurnStartCommit {
        verified_snapshot: None,
        status_sources: Vec::new(),
        context_receipts: Vec::new(),
        started,
        contribution_emissions,
        disposition: ModelTurnStartCommitDisposition::IdempotentReplay,
    })
}

fn validate_contribution_emission_shape(
    emission: &ContributionEmission,
) -> Result<(), ConversationStoreError> {
    emission
        .validate()
        .map_err(ConversationStoreError::InvalidReference)
}

/// Validates the exact status message/emission binding before a start chooses
/// its fresh or idempotent path. This is the construction boundary that keeps
/// a free-standing emission vector from entering durable storage.
fn validate_contribution_start(
    snapshot: &RequestSnapshot,
    context: &[MessageBlock],
) -> Result<(), ConversationStoreError> {
    let mut ids = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for contribution in &snapshot.contributions {
        if !ids.insert(&contribution.message_id) {
            return Err(ConversationStoreError::InvalidReference(
                "duplicate accepted contribution identity".to_owned(),
            ));
        }
        if !snapshot
            .context_generation
            .contributors
            .iter()
            .any(|generation| generation.identity == contribution.producer)
        {
            return Err(ConversationStoreError::InvalidReference(
                "contribution producer is absent from the accepted generation".to_owned(),
            ));
        }
        for emission in &contribution.emissions {
            validate_contribution_emission_shape(emission)?;
            if !keys.insert((&contribution.producer, &emission.key)) {
                return Err(ConversationStoreError::InvalidReference(
                    "duplicate producer-scoped contribution key".to_owned(),
                ));
            }
        }
        if snapshot.identity.retry_number == 0 {
            let message = context
                .iter()
                .find(|message| message.id() == &contribution.message_id)
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "accepted contribution has no canonical context".to_owned(),
                    )
                })?;
            contribution.validate_message(message)?;
        } else if context
            .iter()
            .any(|message| message.id() == &contribution.message_id)
        {
            return Err(ConversationStoreError::InvalidReference(
                "retry must not recommit an accepted contribution".to_owned(),
            ));
        }
    }
    if snapshot.identity.retry_number == 0
        && context.iter().any(|message| !ids.contains(message.id()))
    {
        return Err(ConversationStoreError::InvalidReference(
            "canonical context has no accepted contribution record".to_owned(),
        ));
    }
    Ok(())
}

fn verify_frozen_contributions_tx(
    transaction: &Transaction<'_>,
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let visible = |id: &MessageId| {
        snapshot.input.iter().any(|item| match item {
            crate::model::projected::FrozenModelInput::Original(reference) => {
                &reference.message_id == id
            }
            crate::model::projected::FrozenModelInput::Derived(derived) => {
                derived
                    .sources
                    .iter()
                    .any(|source| &source.message_id == id)
                    && !derived.transformations.is_empty()
                    && derived.transformations.iter().all(|t| {
                        matches!(
                            t,
                            crate::model::projected::SemanticTransformation::ImageDowngrade
                                | crate::model::projected::SemanticTransformation::UploadRendering
                        )
                    })
            }
            crate::model::projected::FrozenModelInput::RequestOnly(_) => false,
        })
    };
    for contribution in &snapshot.contributions {
        contribution.validate_message(&load_message_tx(transaction, &contribution.message_id)?)?;
        if !visible(&contribution.message_id) {
            return Err(ConversationStoreError::InvalidReference(
                "frozen contribution is absent from exact typed request".to_owned(),
            ));
        }
        // The original snapshot is immutable even if a newer head for this
        // producer/key exists. A retry verifies its own logical-step receipt.
        let mut origin = snapshot.identity.clone();
        origin.retry_number = 0;
        let (original, started, _) = read_request_snapshot_tx(transaction, &origin.request_id())?
            .ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "retry has no original accepted contribution state".to_owned(),
            )
        })?;
        if started.is_none() || !original.contributions.contains(contribution) {
            return Err(ConversationStoreError::InvalidReference(
                "retry changed its frozen accepted contribution".to_owned(),
            ));
        }
        for emission in &contribution.emissions {
            let event_id = contribution_emission_event_id(
                &origin.request_id(),
                &contribution.producer,
                emission,
            );
            let event = find_event_by_id(transaction, &event_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "frozen contribution has no committed receipt".to_owned(),
                )
            })?;
            if !matches!(&event.event, RuntimeEvent::ContextContributionEmitted { producer, message_id, emission: committed, .. } if producer == &contribution.producer && message_id == &contribution.message_id && committed == emission)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "frozen contribution receipt disagrees".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn verify_contribution_start_tx(
    transaction: &Transaction<'_>,
    context: &[MessageBlock],
    snapshot: &RequestSnapshot,
    started: &RuntimeEventEnvelope,
) -> Result<Vec<RuntimeEventEnvelope>, ConversationStoreError> {
    validate_contribution_start(snapshot, context)?;
    if snapshot.identity.retry_number != 0 {
        verify_frozen_contributions_tx(transaction, snapshot)?;
        return Ok(Vec::new());
    }
    let mut events = Vec::new();
    let mut sequence = started.sequence;
    for contribution in &snapshot.contributions {
        for emission in &contribution.emissions {
            let event_id = contribution_emission_event_id(
                &snapshot.request_id,
                &contribution.producer,
                emission,
            );
            let event = find_event_by_id(transaction, &event_id)?.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "committed contribution lacks receipt".to_owned(),
                )
            })?;
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| storage("contribution sequence overflow"))?;
            let RuntimeEvent::ContextContributionEmitted {
                producer,
                request_id,
                message_id,
                emission: committed,
                logical_step_origin,
            } = &event.event
            else {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution receipt has wrong event kind".to_owned(),
                ));
            };
            if producer != &contribution.producer
                || request_id != &snapshot.request_id
                || message_id != &contribution.message_id
                || committed != emission
                || event.sequence != sequence
                || event.conversation_id != started.conversation_id
                || event.attempt_id != started.attempt_id
                || event.turn_id != started.turn_id
                || event.timestamp != started.timestamp
            {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution receipt disagrees with startup".to_owned(),
                ));
            }
            let head = read_contribution_head_tx(transaction, producer, &emission.key)?
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(
                        "contribution receipt has no durable head".to_owned(),
                    )
                })?;
            if head.event_sequence < sequence
                || (head.event_sequence == sequence
                    && (head.fingerprint != emission.fingerprint
                        || head.canonical_message_id != *message_id
                        || head.request_id != *request_id
                        || head.logical_step_origin != *logical_step_origin
                        || head.emitted_at != event.timestamp))
            {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution head disagrees with receipt".to_owned(),
                ));
            }
            events.push(event);
        }
    }
    Ok(events)
}

fn register_integrity_function(connection: &Connection) -> Result<(), ConversationStoreError> {
    use rusqlite::functions::FunctionFlags;
    connection
        .create_scalar_function(
            "rustx_content_digest",
            1,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_INNOCUOUS,
            |context| {
                use sha2::{Digest, Sha256};
                let bytes = context
                    .get_raw(0)
                    .as_str()
                    .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))?;
                Ok(format!("{:x}", Sha256::digest(bytes.as_bytes())))
            },
        )
        .map_err(|e| storage(e.to_string()))
}

fn configure_connection(
    connection: &mut Connection,
    in_memory: bool,
) -> Result<(), ConversationStoreError> {
    register_integrity_function(connection)?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")
        .map_err(|error| storage(format!("configure SQLite: {error}")))?;
    writer_busy_policy(connection)
        .map_err(|error| storage(format!("configure SQLite busy policy: {error}")))?;
    if !in_memory {
        let mode: String = connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(|error| storage(format!("configure SQLite journal: {error}")))?;
        if mode != "wal" {
            return Err(storage(format!("SQLite refused WAL journal mode: {mode}")));
        }
    }
    Ok(())
}

fn validate_existing_identity(
    connection: &Connection,
    conversation_id: &ConversationId,
) -> Result<(), ConversationStoreError> {
    let stored: String = connection
        .query_row(
            "SELECT conversation_id FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("read existing identity: {error}")))?;
    if stored != conversation_id.as_str() {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: ConversationId::parse(stored).map_err(storage)?,
            requested: conversation_id.clone(),
        });
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

// This is an index discriminator, not a second occupancy fact. The Journal
// continues to own the usage payload and the compaction's explicit absence.
const CONTEXT_READING_PREDICATE: &str = "json_extract(event_json, '$.event.type') = 'compaction_completed' OR COALESCE(json_type(event_json, '$.event.usage'), 'null') != 'null'";

#[allow(clippy::too_many_lines)] // Closed indexed scope vocabulary in one place.
/// The actual bounded SQL used by the reader and its query-plan contract tests.
fn presentation_statements(
    query: &super::presentation::FactQuery,
) -> Result<Vec<(String, Vec<rusqlite::types::Value>)>, ConversationStoreError> {
    use super::presentation::FactScope;
    use rusqlite::types::Value;
    if query.limit == 0 || query.limit > 129 || query.kinds.is_empty() || query.kinds.len() > 32 {
        return Err(storage("invalid presentation fact query bound"));
    }
    let mut values = vec![
        Value::Integer(seq_to_i64(query.through)?),
        Value::Integer(seq_to_i64(
            query.before.unwrap_or(query.through.saturating_add(1)),
        )?),
    ];
    let scope = match &query.scope {
        FactScope::All => String::new(),
        FactScope::ContextReading => format!(" AND ({CONTEXT_READING_PREDICATE}) = 1"),
        FactScope::Compaction(operation) => {
            values.push(Value::Text(operation.to_string()));
            format!(
                " AND json_extract(event_json, '$.event.operation_id') = ?{}",
                values.len()
            )
        }
        FactScope::Step(attempt, turn) => {
            values.push(Value::Text(attempt.to_string()));
            values.push(Value::Text(turn.to_string()));
            " AND attempt_id = ?3 AND turn_id = ?4".to_owned()
        }
        FactScope::Attempt(attempt) => {
            values.push(Value::Text(attempt.to_string()));
            " AND attempt_id = ?3".to_owned()
        }
        FactScope::ToolCall {
            call_id,
            attempt,
            turn,
        } => {
            values.push(Value::Text(call_id.clone()));
            values.push(
                attempt
                    .as_ref()
                    .map_or(Value::Null, |id| Value::Text(id.to_string())),
            );
            values.push(
                turn.as_ref()
                    .map_or(Value::Null, |id| Value::Text(id.to_string())),
            );
            " AND json_extract(event_json, '$.event.tool_call_id') = ?3 AND attempt_id IS ?4 AND turn_id IS ?5".to_owned()
        }
        other => {
            let (field, value) = match other {
                FactScope::Request(value) => ("request_id", value),
                FactScope::Compaction(_) => unreachable!("compaction scope is handled separately"),
                FactScope::Execution(value) => ("execution_id", value),
                FactScope::Subagent(value) => ("subagent_id", value),
                FactScope::Workflow(value) => ("run_id", value),
                FactScope::Interaction(value) => ("interaction_id", value),
                FactScope::Message(value) => ("message_id", value),
                FactScope::All
                | FactScope::ContextReading
                | FactScope::Step(..)
                | FactScope::Attempt(..)
                | FactScope::ToolCall { .. } => unreachable!(),
            };
            values.push(Value::Text(value.clone()));
            format!(" AND json_extract(event_json, '$.event.{field}') = ?3")
        }
    };
    let index = match query.scope {
        FactScope::All => "events_kind_idx",
        FactScope::ContextReading => "events_context_reading_idx",
        FactScope::Message(..) => "events_message_id_kind_idx",
        FactScope::Attempt(..) => "events_attempt_kind_idx",
        FactScope::Step(..) => "events_step_kind_idx",
        FactScope::Request(..) => "events_request_id_kind_idx",
        FactScope::Compaction(..) => "events_compaction_operation_kind_idx",
        FactScope::ToolCall { .. } => "events_tool_call_scope_kind_idx",
        FactScope::Execution(..) => "events_execution_id_kind_idx",
        FactScope::Subagent(..) => "events_subagent_id_kind_idx",
        FactScope::Workflow(..) => "events_run_id_kind_idx",
        FactScope::Interaction(..) => "events_interaction_id_kind_idx",
    };
    // Index selection is part of the bounded read contract, independent of
    // SQLite statistics and unrelated kind cardinalities.
    let mut statements = Vec::new();
    for kind in &query.kinds {
        let mut params = values.clone();
        params.push(Value::Text((*kind).to_owned()));
        let kind_param = params.len();
        params.push(Value::Integer(seq_to_i64(query.after)?));
        let after_param = params.len();
        params.push(Value::Integer(
            i64::try_from(query.limit).map_err(|_| storage("fact limit"))?,
        ));
        let direction = if query.ascending { "ASC" } else { "DESC" };
        let sql = format!("SELECT event_json FROM events INDEXED BY {index} WHERE sequence <= ?1 AND sequence < ?2{scope}
                AND json_extract(event_json, '$.event.type') = ?{kind_param} AND sequence > ?{after_param}
                ORDER BY sequence {direction} LIMIT ?{}", params.len());
        statements.push((sql, params));
    }
    Ok(statements)
}

#[cfg(test)]
mod scoped_event_index_tests;

// Full index suffixes, including predicates that every matching reader implies.
const PRESENTATION_INDEXES: &[(&str, &str)] = &[
    (
        "events_context_reading_idx",
        "(json_extract(event_json, '$.event.type'), (json_extract(event_json, '$.event.type') = 'compaction_completed' OR COALESCE(json_type(event_json, '$.event.usage'), 'null') != 'null'), sequence) WHERE (json_extract(event_json, '$.event.type') = 'compaction_completed' OR COALESCE(json_type(event_json, '$.event.usage'), 'null') != 'null') = 1",
    ),
    (
        "events_message_id_kind_idx",
        "(json_extract(event_json, '$.event.message_id'), json_extract(event_json, '$.event.type'), sequence) WHERE json_extract(event_json, '$.event.message_id') IS NOT NULL",
    ),
    (
        "events_kind_idx",
        "(json_extract(event_json, '$.event.type'), sequence)",
    ),
    (
        "events_attempt_kind_idx",
        "(attempt_id, json_extract(event_json, '$.event.type'), sequence)",
    ),
    (
        "events_step_kind_idx",
        "(attempt_id, turn_id, json_extract(event_json, '$.event.type'), sequence) WHERE attempt_id IS NOT NULL AND turn_id IS NOT NULL",
    ),
    (
        "events_compaction_operation_kind_idx",
        "(json_extract(event_json, '$.event.operation_id'), json_extract(event_json, '$.event.type'), sequence) WHERE json_extract(event_json, '$.event.operation_id') IS NOT NULL",
    ),
    (
        "events_request_id_kind_idx",
        "(json_extract(event_json, '$.event.request_id'), json_extract(event_json, '$.event.type'), sequence) WHERE json_extract(event_json, '$.event.request_id') IS NOT NULL",
    ),
    (
        "events_tool_call_scope_kind_idx",
        "(json_extract(event_json, '$.event.tool_call_id'), attempt_id, turn_id, json_extract(event_json, '$.event.type'), sequence) WHERE json_extract(event_json, '$.event.tool_call_id') IS NOT NULL",
    ),
    (
        "events_execution_id_kind_idx",
        "(json_extract(event_json, '$.event.execution_id'), json_extract(event_json, '$.event.type'), sequence) WHERE json_extract(event_json, '$.event.execution_id') IS NOT NULL",
    ),
    (
        "events_subagent_id_kind_idx",
        "(json_extract(event_json, '$.event.subagent_id'), json_extract(event_json, '$.event.type'), sequence) WHERE json_extract(event_json, '$.event.subagent_id') IS NOT NULL",
    ),
    (
        "events_run_id_kind_idx",
        "(json_extract(event_json, '$.event.run_id'), json_extract(event_json, '$.event.type'), sequence) WHERE json_extract(event_json, '$.event.run_id') IS NOT NULL",
    ),
    (
        "events_interaction_id_kind_idx",
        "(json_extract(event_json, '$.event.interaction_id'), json_extract(event_json, '$.event.type'), sequence) WHERE json_extract(event_json, '$.event.interaction_id') IS NOT NULL",
    ),
];

#[allow(clippy::too_many_lines)] // One fixed development schema; no migrations.
fn create_schema(connection: &mut Connection) -> Result<(), ConversationStoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage(format!("create schema transaction: {error}")))?;
    let connection = &transaction;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS rustx_store (
                id INTEGER PRIMARY KEY CHECK(id=1),
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL,
                next_inbound_sequence INTEGER NOT NULL CHECK(next_inbound_sequence >= 0),
                next_event_sequence INTEGER NOT NULL CHECK(next_event_sequence >= 0),
                next_transcript_position INTEGER NOT NULL CHECK(next_transcript_position >= 0),
                retained_transcript_count INTEGER NOT NULL DEFAULT 0 CHECK(retained_transcript_count >= 0),
                pending_observation_count INTEGER NOT NULL DEFAULT 0 CHECK(pending_observation_count >= 0),
                observation_statistics BLOB CHECK(observation_statistics IS NULL OR length(observation_statistics)<=16384),
                reading_mutation_revision INTEGER NOT NULL DEFAULT 0 CHECK(reading_mutation_revision >= 0),
                logical_step_sequence INTEGER NOT NULL CHECK(logical_step_sequence >= 0),
                pending_unresolved_output_stream_id TEXT,
                bootstrap_initialized INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_initialized IN (0,1))
            );
            CREATE TABLE IF NOT EXISTS observation_block_headers (
                ledger_position INTEGER NOT NULL REFERENCES message_ledger(position),
                block_index INTEGER NOT NULL CHECK(block_index>=0),
                header_json BLOB NOT NULL CHECK(length(header_json)<=16384),
                message_id TEXT NOT NULL REFERENCES message_ledger(message_id),
                PRIMARY KEY(ledger_position,block_index),UNIQUE(message_id,block_index)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS audit_observation_blocks (
                transcript_position INTEGER NOT NULL REFERENCES transcript_order(position) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),header_json BLOB NOT NULL CHECK(length(header_json)<=16384),
                PRIMARY KEY(transcript_position,ordinal)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS observation_headers (
                header_json BLOB NOT NULL CHECK(length(header_json)<=16384),
                tool_result_json BLOB CHECK(tool_result_json IS NULL OR length(tool_result_json)<=16384),
                transcript_position INTEGER UNIQUE,ledger_position INTEGER,
                journal_position INTEGER NOT NULL DEFAULT 0 CHECK(journal_position>=0),
                reference_kind TEXT NOT NULL, reference_id TEXT NOT NULL,
                PRIMARY KEY(reference_kind,reference_id)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS observation_headers_ledger_idx ON observation_headers(ledger_position) WHERE ledger_position IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS observation_headers_transcript_idx ON observation_headers(transcript_position) WHERE transcript_position IS NOT NULL;
            CREATE TABLE IF NOT EXISTS surface_observation_boundaries (
                ordinal INTEGER PRIMARY KEY,ledger_position INTEGER NOT NULL UNIQUE,surface_revision INTEGER NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS content_segments (
                source_kind TEXT NOT NULL, source_id TEXT NOT NULL, block_index INTEGER NOT NULL,
                representation TEXT NOT NULL, segment INTEGER NOT NULL, bytes BLOB NOT NULL CHECK(length(bytes)<=65536),
                PRIMARY KEY(source_kind,source_id,block_index,representation,segment)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS content_frontiers (
                source_kind TEXT NOT NULL, source_id TEXT NOT NULL, block_index INTEGER NOT NULL,
                representation TEXT NOT NULL, revision INTEGER NOT NULL, readable_end INTEGER NOT NULL,
                PRIMARY KEY(source_kind,source_id,block_index,representation,revision)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS publication_metadata_segments (
                stream_id TEXT NOT NULL,block_index INTEGER NOT NULL,revision INTEGER NOT NULL,
                segment INTEGER NOT NULL,bytes BLOB NOT NULL CHECK(length(bytes)<=65536),
                PRIMARY KEY(stream_id,block_index,revision,segment)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS publication_observation_blocks (
                stream_id TEXT NOT NULL, block_index INTEGER NOT NULL, opened_revision INTEGER NOT NULL,
                creation_ordinal INTEGER NOT NULL,tool_ordinal INTEGER,kind TEXT NOT NULL, PRIMARY KEY(stream_id,block_index),UNIQUE(stream_id,creation_ordinal),UNIQUE(stream_id,tool_ordinal)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS publication_observation_counts (
                stream_id TEXT NOT NULL, revision INTEGER NOT NULL, total INTEGER NOT NULL,tool_total INTEGER NOT NULL,
                PRIMARY KEY(stream_id,revision)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS goal_state (
                observation_header BLOB CHECK(observation_header IS NULL OR length(observation_header)<=16384),
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                observation_source_id TEXT, observation_sequence INTEGER,
                snapshot_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS observation_block_ledger_kind_idx ON observation_block_headers(ledger_position,json_extract(header_json,'$.kind'),block_index);
            CREATE INDEX IF NOT EXISTS observation_block_kind_idx ON observation_block_headers(message_id,json_extract(header_json,'$.kind'),block_index);
            CREATE TABLE IF NOT EXISTS observation_models (
                owner_anchor INTEGER NOT NULL REFERENCES events(sequence),
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),
                first_sequence INTEGER NOT NULL REFERENCES events(sequence) ON DELETE CASCADE,
                request_alias TEXT CHECK(request_alias IS NULL OR length(CAST(request_alias AS BLOB))<=512),
                request_encoded_bytes INTEGER NOT NULL CHECK(request_encoded_bytes>=2),
                model_hash BLOB NOT NULL CHECK(length(model_hash)=32),
                encoded_bytes INTEGER NOT NULL CHECK(encoded_bytes>=0),
                attempt_id TEXT NOT NULL,
                PRIMARY KEY(attempt_id,ordinal),
                UNIQUE(attempt_id,model_hash)
            );
            CREATE INDEX IF NOT EXISTS observation_models_owner_idx ON observation_models(owner_anchor,ordinal);
            CREATE TABLE IF NOT EXISTS pending_inbound (
                sequence INTEGER PRIMARY KEY,
                observation_header BLOB CHECK(observation_header IS NULL OR length(observation_header)<=16384),
                observation_transcript INTEGER,
                revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
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
                observation_trace_summary BLOB CHECK(observation_trace_summary IS NULL OR length(observation_trace_summary)<=16384),
                observation_identity_digest BLOB CHECK(observation_identity_digest IS NULL OR length(observation_identity_digest)=32),
                observation_delivery_success INTEGER CHECK(observation_delivery_success IS NULL OR observation_delivery_success IN (0,1)),
                observation_delivery_total INTEGER CHECK(observation_delivery_total IS NULL OR observation_delivery_total>=0),
                native_transcript INTEGER,
                human_activity_at TEXT,
                human_activity_seconds INTEGER,
                human_activity_nanos INTEGER,
                opening_preview TEXT,
                ordinary_user INTEGER NOT NULL DEFAULT 0 CHECK(ordinary_user IN (0,1)),
                native_todo INTEGER NOT NULL DEFAULT 0 CHECK(native_todo IN (0,1)),
                boundary_indexed INTEGER NOT NULL DEFAULT 0 CHECK(boundary_indexed IN (0,1)),
                first_append_revision INTEGER,
                position INTEGER PRIMARY KEY,
                message_id TEXT NOT NULL UNIQUE,
                message_json TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS message_ledger_identity_position_idx ON message_ledger(position,message_id);
            CREATE UNIQUE INDEX IF NOT EXISTS message_ledger_native_transcript_idx ON message_ledger(native_transcript) WHERE native_transcript IS NOT NULL;
            CREATE TABLE IF NOT EXISTS canonical_tool_calls (
                observation_trace_proposal BLOB CHECK(observation_trace_proposal IS NULL OR length(observation_trace_proposal)<=16384),
                observation_tool_identity_digest BLOB CHECK(observation_tool_identity_digest IS NULL OR length(observation_tool_identity_digest)=32),
                observation_definition_ready INTEGER NOT NULL DEFAULT 0 CHECK(observation_definition_ready IN (0,1)),
                observation_definition_start INTEGER REFERENCES events(sequence),
                observation_definition_commit INTEGER REFERENCES events(sequence),
                observation_definition_ordinal INTEGER CHECK(observation_definition_ordinal IS NULL OR observation_definition_ordinal>=0),
                observation_ledger INTEGER NOT NULL,observation_tool_ordinal INTEGER NOT NULL,observation_call_id TEXT,observation_tool_id TEXT,observation_name TEXT NOT NULL, result_ledger_position INTEGER,
                block_index INTEGER NOT NULL CHECK(block_index >= 0),
                assistant_message_id TEXT NOT NULL REFERENCES message_ledger(message_id),
                call_id TEXT NOT NULL,
                tool_id TEXT NOT NULL,
                attempt_id TEXT,
                commit_sequence INTEGER,
                result_message_id TEXT UNIQUE REFERENCES message_ledger(message_id),
                PRIMARY KEY(assistant_message_id,block_index),
                UNIQUE(assistant_message_id,call_id)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS canonical_tool_calls_observation_idx ON canonical_tool_calls(observation_ledger,block_index);
            CREATE UNIQUE INDEX IF NOT EXISTS canonical_tool_calls_result_ledger_idx ON canonical_tool_calls(result_ledger_position) WHERE result_ledger_position IS NOT NULL;
            CREATE INDEX IF NOT EXISTS canonical_tool_calls_scope_idx ON canonical_tool_calls(attempt_id,call_id,commit_sequence,assistant_message_id,block_index);
            CREATE INDEX IF NOT EXISTS canonical_tool_calls_pending
                ON canonical_tool_calls(assistant_message_id,block_index) WHERE result_message_id IS NULL;
            CREATE TABLE IF NOT EXISTS transcript_order (
                position INTEGER PRIMARY KEY,
                reference_kind TEXT NOT NULL,
                reference_id TEXT NOT NULL,
                observation_readiness_missing INTEGER NOT NULL DEFAULT 1 CHECK(observation_readiness_missing IN (0,1)),
                UNIQUE(reference_kind,reference_id)
            );
            CREATE INDEX IF NOT EXISTS transcript_observation_readiness_missing_idx ON transcript_order(position) WHERE observation_readiness_missing=1;
            CREATE TABLE IF NOT EXISTS transcript_observation_readiness (
                position INTEGER PRIMARY KEY REFERENCES transcript_order(position) ON DELETE CASCADE,
                ready INTEGER NOT NULL CHECK(ready IN (0,1))
            );
            CREATE INDEX IF NOT EXISTS transcript_observation_readiness_holes_idx ON transcript_observation_readiness(position) WHERE ready=0;
            CREATE TABLE IF NOT EXISTS bootstrap_identity (
                occupancy_observation BLOB CHECK(occupancy_observation IS NULL OR length(occupancy_observation)<=16384),
                id INTEGER PRIMARY KEY CHECK(id=1),
                message_count INTEGER NOT NULL,
                history_digest TEXT NOT NULL,
                occupancy_json TEXT NOT NULL,
                occupancy_digest TEXT NOT NULL,
                turn_count INTEGER NOT NULL CHECK(turn_count>=0),
                response_count INTEGER NOT NULL CHECK(response_count>=0),
                provenance_digest TEXT NOT NULL,
                inherited_transcript_through INTEGER NOT NULL CHECK(inherited_transcript_through >= 0),
                projection_count INTEGER NOT NULL DEFAULT 0 CHECK(projection_count>=0),
                projection_digest TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS inherited_turn_provenance (
                observation_turn_display_witness BLOB CHECK(observation_turn_display_witness IS NULL OR length(observation_turn_display_witness)=32),
                observation_json_witness BLOB CHECK(observation_json_witness IS NULL OR length(observation_json_witness)=32),
                observation_turn_display BLOB CHECK(observation_turn_display IS NULL OR length(observation_turn_display)<=16384),
                display_provenance BLOB CHECK(display_provenance IS NULL OR length(display_provenance)<=16384),
                observation_members BLOB CHECK(observation_members IS NULL OR length(observation_members)<=16384),
                observation_json BLOB CHECK(observation_json IS NULL OR length(observation_json)<=16384),
                ordinal INTEGER PRIMARY KEY CHECK(ordinal>=0),
                payload_digest TEXT NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS inherited_turn_identity_idx ON inherited_turn_provenance(json_extract(payload,'$.id.conversation_id'),json_extract(payload,'$.id.attempt_id'));
            CREATE TABLE IF NOT EXISTS inherited_response_provenance (
                closing_ledger_position INTEGER NOT NULL UNIQUE,
                display_provenance BLOB CHECK(display_provenance IS NULL OR length(display_provenance)<=16384),
                ordinal INTEGER PRIMARY KEY CHECK(ordinal>=0),
                payload TEXT NOT NULL,
                payload_digest TEXT NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS inherited_response_closing_idx ON inherited_response_provenance(json_extract(payload,'$.closing_message_id'));
            CREATE INDEX IF NOT EXISTS inherited_response_owner_idx ON inherited_response_provenance(json_extract(payload,'$.origin.conversation_id'),json_extract(payload,'$.origin.attempt_id'),ordinal);
            CREATE TABLE IF NOT EXISTS inherited_message_membership (
                observation_identity_digest BLOB CHECK(observation_identity_digest IS NULL OR length(observation_identity_digest)=32),
                observation_membership_witness BLOB CHECK(observation_membership_witness IS NULL OR length(observation_membership_witness)=32),
                assistant_ledger_position INTEGER NOT NULL UNIQUE,
                turn_ordinal INTEGER REFERENCES inherited_turn_provenance(ordinal),
                response_ordinal INTEGER REFERENCES inherited_response_provenance(ordinal),
                membership_digest TEXT NOT NULL,
                assistant_message_id TEXT PRIMARY KEY REFERENCES message_ledger(message_id),
                FOREIGN KEY(assistant_ledger_position,assistant_message_id) REFERENCES message_ledger(position,message_id)
            );
            CREATE INDEX IF NOT EXISTS inherited_membership_turn_idx ON inherited_message_membership(turn_ordinal,assistant_message_id);
            CREATE TABLE IF NOT EXISTS surface_ops (
                revision INTEGER PRIMARY KEY,
                compaction_generation INTEGER NOT NULL,
                op_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS surface_head (
                id INTEGER PRIMARY KEY CHECK(id=1),
                revision INTEGER NOT NULL,
                compaction_generation INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projection_checkpoints (
                checkpoint_id TEXT PRIMARY KEY,
                candidate_fingerprint TEXT NOT NULL,
                candidate_json TEXT NOT NULL,
                occupancy_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS projection_activations (
                activation INTEGER PRIMARY KEY CHECK(activation>0),
                checkpoint_id TEXT NOT NULL REFERENCES projection_checkpoints(checkpoint_id),
                ledger_frontier INTEGER NOT NULL CHECK(ledger_frontier>=0),
                visible_journal INTEGER NOT NULL CHECK(visible_journal>=0),
                installation_id TEXT,
                header_digest TEXT NOT NULL,
                activation_digest TEXT NOT NULL,
                observation_header_witness BLOB CHECK(observation_header_witness IS NULL OR length(observation_header_witness)=32),
                observation_header BLOB CHECK(observation_header IS NULL OR length(observation_header)<=16384),
                activation_json TEXT NOT NULL,
                CHECK((installation_id IS NULL AND visible_journal>0) OR (installation_id IS NOT NULL AND visible_journal=0))
            );
            CREATE INDEX IF NOT EXISTS projection_ledger_idx ON projection_activations(ledger_frontier,activation);
            CREATE UNIQUE INDEX IF NOT EXISTS projection_checkpoint_idx ON projection_activations(checkpoint_id);
            CREATE TABLE IF NOT EXISTS projection_head (
                id INTEGER PRIMARY KEY CHECK(id=1),
                activation INTEGER NOT NULL REFERENCES projection_activations(activation)
            );
            CREATE TABLE IF NOT EXISTS compaction_operations (
                observation_receipt_witness BLOB CHECK(observation_receipt_witness IS NULL OR length(observation_receipt_witness)=32),
                observation_receipt BLOB CHECK(observation_receipt IS NULL OR length(observation_receipt)<=65536),
                operation_id TEXT PRIMARY KEY,
                candidate_fingerprint TEXT NOT NULL,
                receipt_digest TEXT NOT NULL,
                receipt_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS request_snapshots (
                observation_step_anchor INTEGER, observation_retry_number INTEGER, observation_context_total INTEGER, observation_accepted_ledger INTEGER,
                observation_summary BLOB CHECK(observation_summary IS NULL OR length(observation_summary)<=16384),
                retry_ledger_position INTEGER,
                observation_terminal_sequence INTEGER,
                observation_terminal_header BLOB CHECK(observation_terminal_header IS NULL OR length(observation_terminal_header)<=16384),
                observation_source_id TEXT UNIQUE CHECK(observation_source_id IS NULL OR (length(observation_source_id)<=20 AND CAST(observation_source_id AS INTEGER)=started_sequence)),
                canonical_frontier INTEGER NOT NULL,
                input_root INTEGER REFERENCES native_input_nodes(id),
                input_root_digest BLOB,
                input_count INTEGER NOT NULL CHECK(input_count BETWEEN 0 AND 8192),
                started_sequence INTEGER,
                completed_sequence INTEGER,
                manifest_digest TEXT NOT NULL CHECK(typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64),
                previous_request_id TEXT,
                request_id TEXT PRIMARY KEY,
                retry_message_id TEXT,
                snapshot_json TEXT NOT NULL,
                CHECK((input_root IS NULL AND input_root_digest IS NULL AND input_count=0) OR
                    (input_root IS NOT NULL AND typeof(input_root_digest)='blob' AND length(input_root_digest)=32 AND input_count>0))
            );
            CREATE UNIQUE INDEX IF NOT EXISTS request_trace_retry_idx ON request_snapshots(observation_step_anchor,observation_retry_number) WHERE observation_step_anchor IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS request_snapshots_started_idx ON request_snapshots(started_sequence) WHERE started_sequence IS NOT NULL;
            CREATE TABLE IF NOT EXISTS state_observation_sources (
                domain TEXT NOT NULL,source_id TEXT NOT NULL,ledger_position INTEGER,journal_sequence INTEGER,
                todo_header BLOB CHECK(todo_header IS NULL OR length(todo_header)<=16384),
                PRIMARY KEY(domain,source_id)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS state_observation_ledger_idx ON state_observation_sources(domain,ledger_position,source_id);
            CREATE INDEX IF NOT EXISTS state_observation_journal_idx ON state_observation_sources(domain,journal_sequence,source_id);
            CREATE TABLE IF NOT EXISTS request_trace_catalogs (
                source_sequence INTEGER PRIMARY KEY REFERENCES events(sequence),
                tool_count INTEGER NOT NULL CHECK(tool_count>=0),
                provisional_message_id TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS request_trace_catalog_identity_idx ON request_trace_catalogs(provisional_message_id,source_sequence);
            CREATE TABLE IF NOT EXISTS request_trace_definitions (
                source_sequence INTEGER NOT NULL REFERENCES request_trace_catalogs(source_sequence) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),
                tool_identity_digest BLOB NOT NULL CHECK(length(tool_identity_digest)=32),
                tool_id TEXT NOT NULL,
                PRIMARY KEY(source_sequence,ordinal)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS request_trace_definition_digest_idx ON request_trace_definitions(source_sequence,tool_identity_digest,ordinal);
            CREATE INDEX IF NOT EXISTS request_trace_definition_identity_idx ON request_trace_definitions(source_sequence,tool_id,ordinal);
            CREATE TABLE IF NOT EXISTS trace_adoption_members (
                source_sequence INTEGER NOT NULL REFERENCES events(sequence) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),
                ledger_position INTEGER NOT NULL REFERENCES message_ledger(position),
                identity_digest BLOB NOT NULL CHECK(length(identity_digest)=32),
                PRIMARY KEY(source_sequence,ordinal)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS request_trace_contexts (
                source_sequence INTEGER NOT NULL REFERENCES events(sequence),
                ordinal INTEGER NOT NULL CHECK(ordinal>=0),
                header_json BLOB NOT NULL CHECK(length(header_json)<=16384),
                PRIMARY KEY(source_sequence,ordinal)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS contribution_emission_heads (
                producer TEXT NOT NULL,
                semantic_key TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                emitted_at TEXT NOT NULL,
                request_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                logical_step_origin INTEGER NOT NULL CHECK(logical_step_origin >= 0),
                event_sequence INTEGER NOT NULL CHECK(event_sequence >= 0),
                PRIMARY KEY(producer, semantic_key)
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
                observation_indexed INTEGER NOT NULL CHECK(observation_indexed IN (0,1)),
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
            CREATE TABLE IF NOT EXISTS agent_authorities (
                agent_id TEXT PRIMARY KEY,
                authority_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS events (
                observation_trace_fact BLOB CHECK(observation_trace_fact IS NULL OR length(observation_trace_fact)<=16384),
                observation_trace_owner INTEGER,
                observation_trace_fact_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_trace_fact_required IN (0,1)),
                observation_kind TEXT CHECK(observation_kind IS NULL OR length(observation_kind)<=128),
                observation_unavailable INTEGER NOT NULL DEFAULT 0 CHECK(observation_unavailable IN (0,1)),
                observation_transcript INTEGER,
                observation_witness BLOB CHECK(observation_witness IS NULL OR length(observation_witness)<=16384),
                observation_assistant_ledger INTEGER,
                observation_tool_ledger INTEGER,observation_tool_block INTEGER,
                observation_tool_phase INTEGER CHECK(observation_tool_phase IS NULL OR observation_tool_phase IN (1,2)),
                observation_turn_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_turn_required IN (0,1)),
                observation_turn_ordinal INTEGER, observation_turn_anchor INTEGER,
                observation_turn_header BLOB CHECK(observation_turn_header IS NULL OR length(observation_turn_header)<=16384),
                observation_turn_prompt BLOB CHECK(observation_turn_prompt IS NULL OR length(observation_turn_prompt)<=512),
                observation_turn_prompt_positive INTEGER NOT NULL DEFAULT 0 CHECK(observation_turn_prompt_positive IN (0,1)),
                observation_execution_owner INTEGER,
                observation_execution_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_execution_required IN(0,1)),
                observation_execution BLOB CHECK(observation_execution IS NULL OR length(observation_execution)<=16384),
                observation_members BLOB CHECK(observation_members IS NULL OR length(observation_members)<=16384),
                event_timestamp TEXT NOT NULL,
                observation_member_anchor INTEGER,
                observation_statistics BLOB CHECK(observation_statistics IS NULL OR length(observation_statistics)<=16384),
                observation_occupancy BLOB CHECK(observation_occupancy IS NULL OR length(observation_occupancy)<=16384),
                observation_readiness_missing INTEGER NOT NULL DEFAULT 1 CHECK(observation_readiness_missing IN (0,1)),
                observation_statistics_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_statistics_required IN (0,1)),
                observation_members_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_members_required IN (0,1)),
                observation_trace_required INTEGER NOT NULL DEFAULT 0 CHECK(observation_trace_required IN (0,1)),
                observation_trace_total INTEGER CHECK(observation_trace_total IS NULL OR observation_trace_total>=0),
                sequence INTEGER PRIMARY KEY,
                event_id TEXT NOT NULL UNIQUE,
                schema_version INTEGER NOT NULL,
                conversation_id TEXT NOT NULL,
                attempt_id TEXT,
                turn_id TEXT,
                observation_json BLOB CHECK(observation_json IS NULL OR length(observation_json)<=16384),
                event_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS events_trace_kind_idx ON events(observation_kind,sequence) WHERE observation_kind IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_trace_owner_idx ON events(observation_trace_owner,observation_kind,sequence) WHERE observation_trace_owner IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_trace_holes_idx ON events(sequence) WHERE observation_trace_fact_required=1 AND observation_trace_fact IS NULL;
            CREATE INDEX IF NOT EXISTS events_workflow_coordinates_idx ON events(json_extract(event_json,'$.event.run_id.conversation_id'),json_extract(event_json,'$.event.run_id.attempt_id'),json_extract(event_json,'$.event.run_id.invocation'),json_extract(event_json,'$.event.type'),sequence) WHERE json_extract(event_json,'$.event.run_id.conversation_id') IS NOT NULL;
            CREATE TABLE IF NOT EXISTS event_observation_readiness (
                sequence INTEGER PRIMARY KEY REFERENCES events(sequence) ON DELETE CASCADE,
                ready INTEGER NOT NULL CHECK(ready IN (0,1)),
                direct INTEGER NOT NULL DEFAULT 0 CHECK(direct IN (0,1))
            );
            CREATE INDEX IF NOT EXISTS events_observation_readiness_missing_idx ON events(sequence) WHERE observation_readiness_missing=1;
            CREATE INDEX IF NOT EXISTS event_observation_readiness_holes_idx ON event_observation_readiness(sequence) WHERE ready=0;
            CREATE INDEX IF NOT EXISTS events_observation_assistant_ledger_idx ON events(observation_assistant_ledger,sequence) WHERE observation_assistant_ledger IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_trace_holes_idx ON events(sequence) WHERE observation_trace_required=1 AND observation_trace_total IS NULL;
            CREATE INDEX IF NOT EXISTS events_observation_trace_total_idx ON events(sequence) WHERE observation_trace_total IS NOT NULL;
            CREATE INDEX IF NOT EXISTS transcript_order_audit_position_idx ON transcript_order(position) WHERE reference_kind='publication_audit';
            CREATE INDEX IF NOT EXISTS events_observation_statistics_holes_idx ON events(sequence) WHERE observation_statistics_required=1 AND observation_statistics IS NULL;
            CREATE INDEX IF NOT EXISTS events_observation_statistics_idx ON events(sequence) WHERE observation_statistics IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_tool_lifecycle_idx ON events(observation_tool_ledger,observation_tool_block,observation_tool_phase,sequence) WHERE observation_tool_phase IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_tool_occurrence_idx ON events(observation_tool_ledger,observation_tool_block,sequence) WHERE observation_tool_ledger IS NOT NULL AND json_extract(event_json,'$.event.type')='tool_execution_started';
            CREATE INDEX IF NOT EXISTS events_observation_tool_idx ON events(json_extract(event_json,'$.event.tool_call_id'),attempt_id,json_extract(event_json,'$.event.type'),sequence) WHERE json_extract(event_json,'$.event.tool_call_id') IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_members_idx ON events(attempt_id,sequence) WHERE observation_members IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_member_holes_idx ON events(attempt_id,sequence) WHERE observation_members_required=1 AND observation_members IS NULL;
            CREATE INDEX IF NOT EXISTS events_observation_member_holes_sequence_idx ON events(sequence) WHERE observation_members_required=1 AND observation_members IS NULL;
            CREATE INDEX IF NOT EXISTS events_observation_member_anchor_idx ON events(observation_member_anchor,sequence) WHERE observation_members IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_unavailable_idx ON events(sequence) WHERE observation_unavailable=1;
            CREATE UNIQUE INDEX IF NOT EXISTS events_observation_transcript_idx ON events(observation_transcript) WHERE observation_transcript IS NOT NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS events_observation_turn_ordinal_idx ON events(observation_turn_ordinal) WHERE observation_turn_ordinal IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_turn_anchor_idx ON events(observation_turn_anchor,sequence) WHERE observation_turn_header IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_observation_turn_holes_idx ON events(sequence) WHERE observation_turn_required=1 AND observation_turn_header IS NULL;
            CREATE INDEX IF NOT EXISTS events_observation_turn_prompt_idx ON events(attempt_id,sequence) WHERE observation_turn_prompt_positive=1;
            CREATE INDEX IF NOT EXISTS events_observation_turn_prompt_holes_idx ON events(sequence) WHERE observation_kind='inbound_turn_adopted' AND observation_turn_prompt IS NULL;
            CREATE INDEX IF NOT EXISTS events_execution_owner_idx ON events(observation_execution_owner,sequence) WHERE observation_execution IS NOT NULL;
            CREATE INDEX IF NOT EXISTS events_execution_holes_idx ON events(sequence) WHERE observation_execution_required=1 AND observation_execution IS NULL;
            CREATE INDEX IF NOT EXISTS events_observation_execution_idx ON events(attempt_id,sequence) WHERE observation_execution IS NOT NULL;
            CREATE TABLE IF NOT EXISTS lifecycle_state (
                lifecycle_key TEXT PRIMARY KEY,
                terminal_event_id TEXT
            );
            CREATE INDEX IF NOT EXISTS pending_inbound_sequence_idx ON pending_inbound(sequence);
            CREATE INDEX IF NOT EXISTS message_ledger_id_idx ON message_ledger(message_id);
            CREATE INDEX IF NOT EXISTS message_ledger_append_position_idx ON message_ledger(position,first_append_revision) WHERE first_append_revision IS NOT NULL;
            CREATE INDEX IF NOT EXISTS message_ledger_todo_idx ON message_ledger(position) WHERE native_todo=1;
            CREATE INDEX IF NOT EXISTS message_ledger_opening_idx ON message_ledger(position) WHERE ordinary_user=1;
            CREATE INDEX IF NOT EXISTS message_ledger_human_activity_idx ON message_ledger(human_activity_seconds,human_activity_nanos,position) WHERE human_activity_at IS NOT NULL;
            CREATE INDEX IF NOT EXISTS message_ledger_boundary_holes_idx ON message_ledger(first_append_revision) WHERE ordinary_user=1 AND boundary_indexed=0 AND first_append_revision IS NOT NULL;
            CREATE INDEX IF NOT EXISTS transcript_order_reference_idx ON transcript_order(reference_kind, reference_id);
            CREATE INDEX IF NOT EXISTS surface_ops_revision_idx ON surface_ops(revision);

            CREATE INDEX IF NOT EXISTS events_attempt_idx ON events(attempt_id, sequence) WHERE attempt_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS request_snapshots_canonical_frontier_idx ON request_snapshots(canonical_frontier);
            CREATE INDEX IF NOT EXISTS contribution_emission_heads_lookup_idx ON contribution_emission_heads(producer, semantic_key);
            CREATE INDEX IF NOT EXISTS publication_frames_stream_idx ON publication_frames(stream_id, sequence);
            CREATE INDEX IF NOT EXISTS publication_frames_unindexed_idx ON publication_frames(stream_id,sequence) WHERE observation_indexed=0;
            CREATE INDEX IF NOT EXISTS publication_streams_settlement_idx ON publication_streams(settlement);
            CREATE INDEX IF NOT EXISTS publication_streams_message_settlement_idx ON publication_streams(message_id,settlement);",
        )
        .map_err(|error| storage(format!("create schema: {error}")))?;
    content::create_binary_schema(connection)?;
    manifest::create_schema(connection)?;
    observation_request::composed::create_schema(connection)?;
    observation_request::state_preview::create_schema(connection)?;
    connection
        .execute_batch(&projection::completion_index_sql())
        .map_err(|error| storage(error.to_string()))?;
    for (name, definition) in PRESENTATION_INDEXES {
        connection
            .execute_batch(&format!("CREATE INDEX {name} ON events{definition};"))
            .map_err(|error| storage(format!("create presentation index: {error}")))?;
    }
    connection
        .execute(
            "INSERT OR IGNORE INTO rustx_store(id,schema_version,conversation_id,next_inbound_sequence,next_event_sequence,next_transcript_position,logical_step_sequence,pending_unresolved_output_stream_id) VALUES(1,?1,'',0,0,0,0,NULL)",
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
    transaction
        .commit()
        .map_err(|error| storage(format!("create schema commit: {error}")))
}

#[allow(clippy::too_many_lines)] // One required-shape table, one place.
fn verify_schema_shape(connection: &Connection) -> Result<(), ConversationStoreError> {
    content::verify_binary_schema(connection)?;
    provenance::verify_turn_header_shape(connection)?;
    projection::verify_completion_index(connection)?;
    // Bootstrap metadata stays compact; provenance payloads have their own rows.
    // Refuse obsolete inline arrays and require the native keyed representation.
    let bootstrap_columns = connection
        .prepare("PRAGMA table_info(bootstrap_identity)")
        .map_err(|error| storage(error.to_string()))?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    if bootstrap_columns
        != [
            "occupancy_observation",
            "id",
            "message_count",
            "history_digest",
            "occupancy_json",
            "occupancy_digest",
            "turn_count",
            "response_count",
            "provenance_digest",
            "inherited_transcript_through",
            "projection_count",
            "projection_digest",
        ]
    {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid bootstrap lookup field order".into(),
        ));
    }

    for (name, sql) in [
        (
            "projection_ledger_idx",
            "CREATE INDEX projection_ledger_idx ON projection_activations(ledger_frontier,activation)",
        ),
        (
            "projection_checkpoint_idx",
            "CREATE UNIQUE INDEX projection_checkpoint_idx ON projection_activations(checkpoint_id)",
        ),
    ] {
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| storage(e.to_string()))?;
        if actual.as_deref() != Some(sql) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid projection index {name}"
            )));
        }
    }
    for (name, expected) in [
        (
            "events_attempt_idx",
            "CREATE INDEX events_attempt_idx ON events(attempt_id, sequence) WHERE attempt_id IS NOT NULL",
        ),
        (
            "events_workflow_coordinates_idx",
            "CREATE INDEX events_workflow_coordinates_idx ON events(json_extract(event_json,'$.event.run_id.conversation_id'),json_extract(event_json,'$.event.run_id.attempt_id'),json_extract(event_json,'$.event.run_id.invocation'),json_extract(event_json,'$.event.type'),sequence) WHERE json_extract(event_json,'$.event.run_id.conversation_id') IS NOT NULL",
        ),
        (
            "events_observation_tool_idx",
            "CREATE INDEX events_observation_tool_idx ON events(json_extract(event_json,'$.event.tool_call_id'),attempt_id,json_extract(event_json,'$.event.type'),sequence) WHERE json_extract(event_json,'$.event.tool_call_id') IS NOT NULL",
        ),
    ] {
        let actual: Option<String> = connection.query_row("SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1 AND tbl_name='events'", [name], |row| row.get(0)).optional().map_err(|error| storage(error.to_string()))?;
        if actual.as_deref() != Some(expected) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid scoped event index {name}"
            )));
        }
    }
    for (name, definition) in PRESENTATION_INDEXES {
        let sql: Option<String> = connection.query_row("SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1 AND tbl_name='events'", [name], |row| row.get(0)).optional().map_err(|error| storage(error.to_string()))?;
        if sql.as_deref() != Some(format!("CREATE INDEX {name} ON events{definition}").as_str()) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid presentation index {name}"
            )));
        }
    }
    for sql in [
        "CREATE UNIQUE INDEX inherited_turn_identity_idx ON inherited_turn_provenance(json_extract(payload,'$.id.conversation_id'),json_extract(payload,'$.id.attempt_id'))",
        "CREATE UNIQUE INDEX inherited_response_closing_idx ON inherited_response_provenance(json_extract(payload,'$.closing_message_id'))",
        "CREATE UNIQUE INDEX message_ledger_identity_position_idx ON message_ledger(position,message_id)",
        "CREATE UNIQUE INDEX message_ledger_native_transcript_idx ON message_ledger(native_transcript) WHERE native_transcript IS NOT NULL",
    ] {
        let name = sql.split_whitespace().nth(3).expect("literal index name");
        let retained: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        if retained.as_deref() != Some(sql) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing or invalid inherited index {name}"
            )));
        }
    }
    for (name, sql) in [
        (
            "inherited_response_owner_idx",
            "CREATE INDEX inherited_response_owner_idx ON inherited_response_provenance(json_extract(payload,'$.origin.conversation_id'),json_extract(payload,'$.origin.attempt_id'),ordinal)",
        ),
        (
            "observation_models_owner_idx",
            "CREATE INDEX observation_models_owner_idx ON observation_models(owner_anchor,ordinal)",
        ),
        (
            "events_observation_turn_ordinal_idx",
            "CREATE UNIQUE INDEX events_observation_turn_ordinal_idx ON events(observation_turn_ordinal) WHERE observation_turn_ordinal IS NOT NULL",
        ),
        (
            "events_observation_turn_anchor_idx",
            "CREATE INDEX events_observation_turn_anchor_idx ON events(observation_turn_anchor,sequence) WHERE observation_turn_header IS NOT NULL",
        ),
        (
            "events_observation_turn_holes_idx",
            "CREATE INDEX events_observation_turn_holes_idx ON events(sequence) WHERE observation_turn_required=1 AND observation_turn_header IS NULL",
        ),
        (
            "events_observation_turn_prompt_idx",
            "CREATE INDEX events_observation_turn_prompt_idx ON events(attempt_id,sequence) WHERE observation_turn_prompt_positive=1",
        ),
        (
            "events_observation_turn_prompt_holes_idx",
            "CREATE INDEX events_observation_turn_prompt_holes_idx ON events(sequence) WHERE observation_kind='inbound_turn_adopted' AND observation_turn_prompt IS NULL",
        ),
        (
            "observation_block_ledger_kind_idx",
            "CREATE INDEX observation_block_ledger_kind_idx ON observation_block_headers(ledger_position,json_extract(header_json,'$.kind'),block_index)",
        ),
        (
            "events_observation_transcript_idx",
            "CREATE UNIQUE INDEX events_observation_transcript_idx ON events(observation_transcript) WHERE observation_transcript IS NOT NULL",
        ),
    ] {
        let retained: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        if retained.as_deref() != Some(sql) {
            return Err(ConversationStoreError::IncompatibleSchema(
                "native owner index unavailable".into(),
            ));
        }
    }
    let reverse_index: Option<String> = connection
        .query_row("SELECT sql FROM sqlite_schema WHERE type='index' AND name='inherited_membership_turn_idx'", [], |row| row.get(0))
        .optional()
        .map_err(|error| storage(error.to_string()))?;
    if reverse_index.as_deref()
        != Some(
            "CREATE INDEX inherited_membership_turn_idx ON inherited_message_membership(turn_ordinal,assistant_message_id)",
        )
    {
        return Err(ConversationStoreError::IncompatibleSchema(
            "missing or invalid inherited reverse membership index".into(),
        ));
    }
    for table in ["inherited_turn_provenance", "inherited_response_provenance"] {
        let primary: (String, i64) = connection
            .query_row(
                &format!("SELECT type,pk FROM pragma_table_info('{table}') WHERE name='ordinal'"),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| storage(error.to_string()))?;
        if primary != ("INTEGER".into(), 1) {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "invalid inherited ordinal primary key in {table}"
            )));
        }
    }
    let foreign = connection
        .prepare("PRAGMA foreign_key_list(inherited_message_membership)")
        .map_err(|error| storage(error.to_string()))?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| storage(error.to_string()))?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|error| storage(error.to_string()))?;
    let expected = BTreeSet::from([
        (
            "message_ledger".into(),
            "assistant_ledger_position".into(),
            "position".into(),
        ),
        (
            "message_ledger".into(),
            "assistant_message_id".into(),
            "message_id".into(),
        ),
        (
            "inherited_turn_provenance".into(),
            "turn_ordinal".into(),
            "ordinal".into(),
        ),
        (
            "inherited_response_provenance".into(),
            "response_ordinal".into(),
            "ordinal".into(),
        ),
    ]);
    if foreign != expected {
        return Err(ConversationStoreError::IncompatibleSchema(
            "invalid inherited membership foreign keys".into(),
        ));
    }
    let paired: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_list('inherited_message_membership') a JOIN pragma_foreign_key_list('inherited_message_membership') b ON a.id=b.id WHERE a.\"table\"='message_ledger' AND a.\"from\"='assistant_ledger_position' AND a.\"to\"='position' AND b.\"from\"='assistant_message_id' AND b.\"to\"='message_id' AND a.seq=0 AND b.seq=1)",
        [], |row| row.get(0),
    ).map_err(|error| storage(error.to_string()))?;
    if !paired {
        return Err(ConversationStoreError::IncompatibleSchema(
            "inherited canonical identity is not paired".into(),
        ));
    }
    verify_unique_columns(connection, "message_ledger", &["position", "message_id"])?;
    verify_unique_column(
        connection,
        "inherited_message_membership",
        "assistant_ledger_position",
    )?;
    verify_witness_field_order(connection)?;
    let required = [
        (
            "projection_checkpoints",
            &[
                "checkpoint_id",
                "candidate_fingerprint",
                "candidate_json",
                "occupancy_json",
            ] as &[&str],
        ),
        (
            "projection_activations",
            &[
                "activation",
                "checkpoint_id",
                "ledger_frontier",
                "visible_journal",
                "installation_id",
                "header_digest",
                "activation_digest",
                "observation_header_witness",
                "observation_header",
                "activation_json",
            ],
        ),
        ("projection_head", &["id", "activation"]),
        (
            "compaction_operations",
            &[
                "observation_receipt_witness",
                "observation_receipt",
                "operation_id",
                "candidate_fingerprint",
                "receipt_digest",
                "receipt_json",
            ],
        ),
        (
            "rustx_store",
            &[
                "schema_version",
                "conversation_id",
                "next_inbound_sequence",
                "next_event_sequence",
                "next_transcript_position",
                "retained_transcript_count",
                "pending_observation_count",
                "reading_mutation_revision",
                "logical_step_sequence",
                "pending_unresolved_output_stream_id",
                "bootstrap_initialized",
            ] as &[&str],
        ),
        (
            "pending_inbound",
            &[
                "sequence",
                "observation_header",
                "observation_transcript",
                "revision",
                "message_id",
                "message_json",
                "correlation",
            ],
        ),
        (
            "inbound_correlation",
            &["correlation", "sequence", "message_id"],
        ),
        (
            "observation_headers",
            &[
                "reference_kind",
                "reference_id",
                "ledger_position",
                "header_json",
            ],
        ),
        (
            "content_segments",
            &[
                "source_kind",
                "source_id",
                "block_index",
                "representation",
                "segment",
                "bytes",
            ],
        ),
        (
            "content_frontiers",
            &[
                "source_kind",
                "source_id",
                "block_index",
                "representation",
                "revision",
                "readable_end",
            ],
        ),
        (
            "message_ledger",
            &[
                "observation_trace_summary",
                "observation_identity_digest",
                "observation_delivery_success",
                "observation_delivery_total",
                "position",
                "native_transcript",
                "human_activity_at",
                "human_activity_seconds",
                "human_activity_nanos",
                "opening_preview",
                "ordinary_user",
                "message_id",
                "message_json",
            ],
        ),
        (
            "transcript_order",
            &["position", "reference_kind", "reference_id"],
        ),
        (
            "canonical_tool_calls",
            &[
                "observation_trace_proposal",
                "observation_tool_identity_digest",
                "observation_definition_ready",
                "observation_definition_start",
                "observation_definition_commit",
                "observation_definition_ordinal",
                "assistant_message_id",
                "block_index",
                "call_id",
                "tool_id",
                "result_message_id",
            ],
        ),
        (
            "bootstrap_identity",
            &[
                "occupancy_observation",
                "message_count",
                "history_digest",
                "turn_count",
                "response_count",
                "provenance_digest",
                "occupancy_json",
                "occupancy_digest",
                "inherited_transcript_through",
                "projection_count",
                "projection_digest",
            ],
        ),
        (
            "inherited_turn_provenance",
            &[
                "observation_turn_display",
                "observation_turn_display_witness",
                "observation_json_witness",
                "ordinal",
                "payload",
                "payload_digest",
            ],
        ),
        (
            "inherited_response_provenance",
            &["ordinal", "payload", "payload_digest"],
        ),
        (
            "inherited_message_membership",
            &[
                "observation_identity_digest",
                "observation_membership_witness",
                "assistant_message_id",
                "assistant_ledger_position",
                "turn_ordinal",
                "response_ordinal",
                "membership_digest",
            ],
        ),
        (
            "surface_ops",
            &["revision", "compaction_generation", "op_json"],
        ),
        ("surface_head", &["revision", "compaction_generation"]),
        (
            "request_snapshots",
            &[
                "observation_step_anchor",
                "observation_retry_number",
                "observation_context_total",
                "observation_accepted_ledger",
                "observation_summary",
                "observation_source_id",
                "request_id",
                "canonical_frontier",
                "manifest_digest",
                "input_root",
                "input_root_digest",
                "input_count",
                "started_sequence",
                "completed_sequence",
                "snapshot_json",
            ],
        ),
        (
            "contribution_emission_heads",
            &[
                "producer",
                "semantic_key",
                "fingerprint",
                "emitted_at",
                "request_id",
                "message_id",
                "logical_step_origin",
                "event_sequence",
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
            &["stream_id", "sequence", "frame_json", "observation_indexed"],
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
                "observation_turn_required",
                "observation_turn_ordinal",
                "observation_turn_anchor",
                "observation_turn_header",
                "observation_turn_prompt",
                "observation_turn_prompt_positive",
                "observation_trace_fact",
                "observation_trace_owner",
                "observation_trace_fact_required",
                "observation_execution_owner",
                "observation_execution_required",
                "observation_kind",
                "observation_occupancy",
                "observation_statistics_required",
                "observation_members_required",
                "observation_member_anchor",
                "sequence",
                "event_id",
                "schema_version",
                "conversation_id",
                "attempt_id",
                "turn_id",
                "event_json",
                "observation_unavailable",
                "observation_json",
            ],
        ),
        (
            "request_trace_catalogs",
            &["source_sequence", "provisional_message_id", "tool_count"],
        ),
        (
            "request_trace_definitions",
            &[
                "source_sequence",
                "ordinal",
                "tool_identity_digest",
                "tool_id",
            ],
        ),
        (
            "trace_adoption_members",
            &[
                "source_sequence",
                "ordinal",
                "ledger_position",
                "identity_digest",
            ],
        ),
        (
            "request_trace_contexts",
            &["source_sequence", "ordinal", "header_json"],
        ),
        (
            "state_observation_fields",
            &[
                "source_id",
                "path_json",
                "parent_path",
                "ordinal",
                "source_kind",
                "preview_nodes",
                "preview_kind",
                "preview_key",
            ],
        ),
        (
            "observation_models",
            &[
                "request_alias",
                "request_encoded_bytes",
                "first_sequence",
                "owner_anchor",
                "encoded_bytes",
            ],
        ),
        ("agent_authorities", &["agent_id", "authority_json"]),
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
        if table == "surface_head" && columns.iter().any(|column| column == "active_json") {
            return Err(ConversationStoreError::IncompatibleSchema(
                "surface_head retains retired active_json layout".into(),
            ));
        }
        for column in required_columns {
            if !columns.iter().any(|candidate| candidate == column) {
                return Err(ConversationStoreError::IncompatibleSchema(format!(
                    "table {table} is missing required column {column}"
                )));
            }
        }
    }
    for (table, column, target, target_column) in [
        (
            "projection_activations",
            "checkpoint_id",
            "projection_checkpoints",
            "checkpoint_id",
        ),
        (
            "projection_head",
            "activation",
            "projection_activations",
            "activation",
        ),
    ] {
        let mut statement = connection
            .prepare(&format!("PRAGMA foreign_key_list({table})"))
            .map_err(|e| storage(e.to_string()))?;
        let keys = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| storage(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| storage(e.to_string()))?;
        if !keys
            .iter()
            .any(|(t, c, tc)| t == target && c == column && tc == target_column)
        {
            return Err(ConversationStoreError::IncompatibleSchema(format!(
                "missing projection dependency constraint {table}.{column}"
            )));
        }
    }
    for (table, column) in [
        ("pending_inbound", "message_id"),
        ("inbound_correlation", "message_id"),
        ("message_ledger", "message_id"),
        ("canonical_tool_calls", "result_message_id"),
        ("inherited_message_membership", "assistant_message_id"),
        ("agent_authorities", "agent_id"),
        ("events", "event_id"),
        ("request_snapshots", "request_id"),
        ("projection_checkpoints", "checkpoint_id"),
        ("compaction_operations", "operation_id"),
        ("publication_streams", "stream_id"),
        ("publication_audits", "stream_id"),
    ] {
        verify_unique_column(connection, table, column)?;
    }
    verify_unique_columns(
        connection,
        "canonical_tool_calls",
        &["assistant_message_id", "block_index"],
    )?;
    verify_unique_columns(
        connection,
        "canonical_tool_calls",
        &["assistant_message_id", "call_id"],
    )?;
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
    verify_unique_columns(
        connection,
        "contribution_emission_heads",
        &["producer", "semantic_key"],
    )?;
    manifest::verify_schema(connection)?;
    content::verify_schema(connection)?;
    observation::verify_schema(connection)?;
    observation_trace::verify_schema(connection)?;
    observation_request::verify_schema(connection)?;
    observation_request::state_preview::verify_schema(connection)?;
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
            stored: ConversationId::parse(stored).map_err(storage)?,
            requested: conversation_id.clone(),
        });
    }
    transaction
        .commit()
        .map_err(|error| storage(format!("bind conversation commit: {error}")))?;
    Ok(())
}

fn ensure_surface_head(transaction: &Transaction<'_>) -> Result<(), ConversationStoreError> {
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM surface_head WHERE id=1)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if exists {
        return Ok(());
    }
    let ledger: i64 = transaction
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if ledger != 0 {
        return Err(ConversationStoreError::InvalidReference(
            "canonical frontier metadata missing".into(),
        ));
    }
    transaction
        .execute(
            "INSERT INTO surface_head(id,revision,compaction_generation) VALUES(1,0,0)",
            [],
        )
        .map_err(|e| storage(e.to_string()))?;
    Ok(())
}

fn read_surface_head(
    connection: &Connection,
) -> Result<Option<(i64, i64)>, ConversationStoreError> {
    connection
        .query_row(
            "SELECT revision,compaction_generation FROM surface_head WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| storage(format!("load Surface head: {error}")))
}

fn head_frontier(
    connection: &Connection,
) -> Result<(i64, Option<super::projection::ProjectionCheckpoint>), ConversationStoreError> {
    let ledger: i64 = connection
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    let revision: Option<i64> = connection
        .query_row("SELECT revision FROM surface_head WHERE id=1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| storage(e.to_string()))?;
    if revision.is_none_or(|revision| revision != ledger) && ledger != 0 {
        return Err(ConversationStoreError::InvalidReference(
            "canonical frontier metadata is missing or inconsistent".into(),
        ));
    }
    let checkpoint = projection::head(connection)?;
    let base = checkpoint
        .as_ref()
        .map_or(0, |checkpoint| checkpoint.candidate.base_frontier.0);
    if base > nonnegative(ledger, "canonical frontier")? {
        return Err(ConversationStoreError::InvalidReference(
            "checkpoint base exceeds canonical frontier".into(),
        ));
    }
    Ok((ledger, checkpoint))
}

fn load_head(connection: &Connection) -> Result<DurableConversationHead, ConversationStoreError> {
    let (ledger, checkpoint) = head_frontier(connection)?;
    materialize_head(connection, ledger, checkpoint)
}

const CANONICAL_SUFFIX_IDS: &str = "SELECT CASE WHEN octet_length(message_id)<=4096 THEN message_id END FROM message_ledger WHERE position>?1 AND position<=?2 ORDER BY position LIMIT 8193";

fn materialize_head(
    connection: &Connection,
    ledger: i64,
    checkpoint: Option<super::projection::ProjectionCheckpoint>,
) -> Result<DurableConversationHead, ConversationStoreError> {
    use crate::context::checkpoint::CheckpointMessage;
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    let base = checkpoint
        .as_ref()
        .map_or(0, |checkpoint| checkpoint.candidate.base_frontier.0);
    if let Some(checkpoint) = &checkpoint {
        for item in &checkpoint.candidate.items {
            if let CheckpointMessage::Original(reference) = &item.message {
                if !ledger_message_exists(connection, &reference.message_id)? {
                    return Err(ConversationStoreError::InvalidReference(
                        "missing checkpoint Original dependency".into(),
                    ));
                }
                if seen.insert(reference.message_id.clone()) {
                    ids.push(reference.message_id.clone());
                }
            }
        }
    }
    let mut statement = connection
        .prepare(CANONICAL_SUFFIX_IDS)
        .map_err(|e| storage(e.to_string()))?;
    let rows = statement
        .query_map(params![seq_to_i64(base)?, ledger], |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(|e| storage(e.to_string()))?;
    for row in rows {
        if ids.len() >= 8192 {
            return Err(crate::context::resources::bound(
                "canonical working set exceeds bounded projection size",
            ));
        }
        let id = MessageId::new(row.map_err(|e| storage(e.to_string()))?.ok_or_else(|| {
            crate::context::resources::bound("canonical identity record exceeds bound")
        })?);
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    Ok(DurableConversationHead {
        revision: SurfaceRevision::new(nonnegative(ledger, "canonical frontier")?),
        compaction_generation: checkpoint
            .map_or(0, |checkpoint| checkpoint.activation.activation.0),
        active_message_ids: ids,
    })
}

/// Appending needs the frontier, not an owned copy of every active identity.
/// Without a checkpoint the ledger UNIQUE identity constraint supplies the
/// uniqueness proof. Still visit the bounded suffix and validate each borrowed
/// UTF-8 value: this preserves corruption/resource checks without allocating an
/// identity vector and tree. This is a bounded scan, not constant-time context.
fn append_head(connection: &Connection) -> Result<(SurfaceRevision, u64), ConversationStoreError> {
    let (ledger, checkpoint) = head_frontier(connection)?;
    if checkpoint.is_some() {
        // Original dependencies, duplicate suppression and suffix bounds retain
        // exactly the same checkpoint validation as full historical readers.
        let head = materialize_head(connection, ledger, checkpoint)?;
        return Ok((head.revision, head.compaction_generation));
    }
    validate_append_identities(connection, ledger)?;
    Ok((
        SurfaceRevision::new(nonnegative(ledger, "canonical frontier")?),
        0,
    ))
}
fn validate_append_identities(
    connection: &Connection,
    ledger: i64,
) -> Result<usize, ConversationStoreError> {
    let mut statement = connection
        .prepare(CANONICAL_SUFFIX_IDS)
        .map_err(|e| storage(e.to_string()))?;
    let mut rows = statement
        .query(params![0, ledger])
        .map_err(|e| storage(e.to_string()))?;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(|e| storage(e.to_string()))? {
        if count >= 8192 {
            return Err(crate::context::resources::bound(
                "canonical working set exceeds bounded projection size",
            ));
        }
        let value = row.get_ref(0).map_err(|e| storage(e.to_string()))?;
        if value == rusqlite::types::ValueRef::Null {
            return Err(crate::context::resources::bound(
                "canonical identity record exceeds bound",
            ));
        }
        value.as_str().map_err(|e| storage(e.to_string()))?;
        count += 1;
    }
    Ok(count)
}

fn update_surface_head(
    transaction: &Transaction<'_>,
    revision: SurfaceRevision,
    generation: u64,
) -> Result<(), ConversationStoreError> {
    transaction
        .execute(
            "UPDATE surface_head SET revision=?1,compaction_generation=?2 WHERE id=1",
            params![seq_to_i64(revision.get())?, seq_to_i64(generation)?],
        )
        .map_err(|error| storage(format!("update Surface head: {error}")))?;
    Ok(())
}

/// Validates explicit canonical ownership; this index contains no independent
/// relationship evidence. Both ordinary commits and lineage seeds derive it from
/// the canonical Tool message itself, atomically with its Ledger append.
fn associate_canonical_tool_result(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    let MessageBlock::Tool(tool) = message else {
        return Ok(());
    };
    let owner = &tool.occurrence;
    let association: Option<(String, String, Option<String>)> = transaction
        .query_row(
            "SELECT call_id,tool_id,result_message_id FROM canonical_tool_calls
         WHERE assistant_message_id=?1 AND block_index=?2",
            params![owner.assistant_message_id.as_str(), owner.block_index.get()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage(format!("canonical Tool occurrence: {error}")))?;
    let Some((call_id, tool_id, previous)) = association else {
        return Err(ConversationStoreError::InvalidReference(
            "Tool result occurrence does not name a canonical ToolCall block".into(),
        ));
    };
    if call_id != tool.tool_call_id.as_str() || tool_id != tool.tool_id.as_str() {
        return Err(ConversationStoreError::InvalidReference(
            "Tool result correlation differs from its canonical occurrence".into(),
        ));
    }
    if previous.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "canonical Tool occurrence already has a result".into(),
        ));
    }
    transaction.execute("UPDATE canonical_tool_calls SET result_message_id=?1,result_ledger_position=(SELECT position FROM message_ledger WHERE message_id=?1) WHERE assistant_message_id=?2 AND block_index=?3", params![tool.id.as_str(), owner.assistant_message_id.as_str(), owner.block_index.get()])
        .map_err(|error| storage(format!("canonical Tool result index: {error}")))?;
    Ok(())
}

fn append_message_and_surface(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    append_message_with_head(transaction, message, None, append_head(transaction)?)
}

/// Appends a destination-owned immutable canonical fact during lineage bootstrap.
/// Typed projections and historical activations are installed independently in
/// the same bootstrap transaction, without replaying source execution events.
fn append_seeded_ledger_message(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    append_message_ledger(transaction, message, None).map(|_| ())
}

fn append_message_with_head(
    transaction: &Transaction<'_>,
    message: &MessageBlock,
    allowed_pending_message_id: Option<&MessageId>,
    (head_revision, generation): (SurfaceRevision, u64),
) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
    let id = crate::conversation::message_id_of(message);
    let transcript_cursor =
        append_message_ledger(transaction, message, allowed_pending_message_id)?;
    let revision = head_revision.next();
    append_surface_op(
        transaction,
        revision,
        generation,
        &SurfaceOp::Append { message_id: id },
    )?;
    update_surface_head(transaction, revision, generation)?;
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
    let (ordinary_user, human_activity_at, opening_preview) = match message {
        MessageBlock::User(user) if user.kind == InboundKind::Message => (
            true,
            matches!(user.source, UserSource::Human)
                .then_some(user.timestamp)
                .flatten(),
            crate::local_runtime::session::preview_of(user),
        ),
        _ => (false, None, None),
    };
    transaction
        .execute(
            "INSERT INTO message_ledger(position,message_id,message_json,ordinary_user,human_activity_at,human_activity_seconds,human_activity_nanos,opening_preview,native_todo) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![position, id.as_str(), encode(message, "canonical message")?,ordinary_user,human_activity_at.map(|time|time.to_rfc3339()),human_activity_at.map(|time|time.timestamp()),human_activity_at.map(|time|time.timestamp_subsec_nanos()),opening_preview,observation_state::todo_value(message).is_some()],
        )
        .map_err(|error| map_insert_error(&error, &id))?;
    let _ = optional_display_layout(transaction, || {
        content::index_message(transaction, message)?;
        observation::index_message(
            transaction,
            message,
            Some(nonnegative(position, "header ledger position")?),
        )
    });
    if let MessageBlock::Assistant(assistant) = message {
        let mut tool_ordinal = 0_u64;
        for (index, block) in assistant.content.iter().enumerate() {
            if let AssistantContentBlock::ToolCall(call) = block {
                let block_index =
                    u32::try_from(index).map_err(|_| storage("Tool block index overflow"))?;
                transaction
                    .execute(
                        "INSERT INTO canonical_tool_calls(assistant_message_id,block_index,call_id,tool_id,observation_ledger,observation_call_id,observation_tool_id,observation_tool_ordinal,observation_name)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                        params![assistant.id.as_str(), block_index, call.id.as_str(), call.tool_id.as_str(),position,(call.id.as_str().len()<=256).then_some(call.id.as_str()),(call.tool_id.as_str().len()<=256).then_some(call.tool_id.as_str()),tool_ordinal, observation_tools::name_preview(&call.name)],
                    )
                    .map_err(|error| storage(format!("canonical Tool occurrence: {error}")))?;
                tool_ordinal = tool_ordinal
                    .checked_add(1)
                    .ok_or_else(|| storage("Tool ordinal overflow"))?;
            }
        }
    }
    associate_canonical_tool_result(transaction, message)?;
    let _ = optional_display_layout(transaction, || {
        let conversation: String = transaction
            .query_row(
                "SELECT conversation_id FROM rustx_store WHERE id=1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        let conversation = ConversationId::parse(conversation).map_err(storage)?;
        let position = nonnegative(position, "native Trace C position")?;
        observation_trace::index_proposals(transaction, &conversation, position, message)?;
        observation_trace::index_canonical(transaction, &conversation, position, message)
    });
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
/// from the normal transcript. Canonical conversation messages,
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
        if reference_kind == "message" {
            transaction
                .execute(
                    "UPDATE message_ledger SET native_transcript=?1 WHERE message_id=?2",
                    params![position, reference_id],
                )
                .map_err(|error| storage(error.to_string()))?;
        }
        let _=transaction.execute("UPDATE observation_headers SET transcript_position=?1,header_json=CASE WHEN reference_kind='message' THEN CAST(json_set(header_json,'$.transcript_cursor',?1) AS BLOB) ELSE header_json END WHERE reference_kind=?2 AND reference_id=?3",params![position,reference_kind,reference_id]);
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
    let _ = optional_display_layout(transaction, || {
        transaction
            .execute(
                "INSERT INTO transcript_observation_readiness(position,ready) VALUES(?1,0)",
                [position],
            )
            .map_err(|error| storage(error.to_string()))?;
        transaction
            .execute(
                "UPDATE transcript_order SET observation_readiness_missing=0 WHERE position=?1",
                [position],
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(())
    });
    transaction
        .execute(
            "UPDATE rustx_store SET next_transcript_position=?1,retained_transcript_count=retained_transcript_count+1 WHERE id=1",
            [position],
        )
        .map_err(|error| storage(format!("update transcript position: {error}")))?;
    if reference_kind == "message" {
        transaction
            .execute(
                "UPDATE message_ledger SET native_transcript=?1 WHERE message_id=?2",
                params![position, reference_id],
            )
            .map_err(|error| storage(error.to_string()))?;
    }
    let _=transaction.execute("UPDATE observation_headers SET transcript_position=?1,header_json=CASE WHEN reference_kind='message' THEN CAST(json_set(header_json,'$.transcript_cursor',?1) AS BLOB) ELSE header_json END WHERE reference_kind=?2 AND reference_id=?3",params![position,reference_kind,reference_id]);
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
    let SurfaceOp::Append { message_id } = operation;
    {
        transaction.execute("UPDATE message_ledger SET first_append_revision=COALESCE(first_append_revision,?1) WHERE message_id=?2",params![revision.get(),message_id.as_str()]).map_err(|error|storage(error.to_string()))?;
        // Existing native Surface commit owns first appearance. This index
        // never rewrites Surface membership, Compaction or canonical history.
        let _ = optional_display_layout(transaction, || {
            let boundary: Option<u64> = transaction
                .query_row(
                    "SELECT position FROM message_ledger WHERE message_id=?1 AND ordinary_user=1",
                    [message_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage(error.to_string()))?;
            if let Some(position) = boundary {
                transaction.execute("INSERT OR IGNORE INTO surface_observation_boundaries SELECT COALESCE((SELECT MAX(ordinal)+1 FROM surface_observation_boundaries),0),?1,?2",params![position,seq_to_i64(revision.get())?]).map_err(|error|storage(error.to_string()))?;
                transaction
                    .execute(
                        "UPDATE message_ledger SET boundary_indexed=1 WHERE position=?1",
                        [position],
                    )
                    .map_err(|error| storage(error.to_string()))?;
            }
            Ok(())
        });
    }
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

/// Both lookups use existing unique Ledger identity and Journal sequence keys.
fn load_message(
    connection: &Connection,
    message_id: &MessageId,
) -> Result<MessageBlock, ConversationStoreError> {
    let json: String = connection
        .prepare_cached("SELECT message_json FROM message_ledger WHERE message_id=?1")
        .map_err(|error| storage(format!("Ledger lookup prepare: {error}")))?
        .query_row([message_id.as_str()], |row| row.get(0))
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
            "SELECT p.sequence,p.message_id,p.message_json,p.correlation,t.position,p.revision
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
                revision: row.get::<_, u64>(5)?,
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

fn load_pending_exact(
    connection: &Connection,
    sequence: InboundSequence,
) -> Result<Option<PendingInboundItem>, ConversationStoreError> {
    type PendingIdentityRow = (String, String, Option<String>, Option<u64>, u64);
    let row:Option<PendingIdentityRow>=connection.query_row("SELECT p.message_id,p.message_json,p.correlation,t.position,p.revision FROM pending_inbound p LEFT JOIN transcript_order t ON t.reference_kind='message' AND t.reference_id=p.message_id WHERE p.sequence=?1",[sequence.get()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(|error|storage(error.to_string()))?;
    row.map(|(id, json, correlation, cursor, revision)| {
        let message: UserMessageBlock =
            serde_json::from_str(&json).map_err(|error| storage(error.to_string()))?;
        if message.id.as_str() != id
            || (!matches!(message.kind, InboundKind::Context(_))) != cursor.is_some()
        {
            return Err(ConversationStoreError::InvalidReference(
                "pending source identity or transcript mismatch".into(),
            ));
        }
        Ok(PendingInboundItem {
            revision,
            sequence,
            message_id: MessageId::new(id),
            message,
            transcript_cursor: cursor.map(TranscriptCursor::new),
            correlation,
        })
    })
    .transpose()
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

#[cfg(test)]
thread_local! {
    static REQUEST_INPUT_RECONSTRUCTIONS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}
#[cfg(test)]
pub(crate) fn take_request_input_reconstructions() -> (usize, usize) {
    REQUEST_INPUT_RECONSTRUCTIONS.with(|count| count.replace((0, 0)))
}

fn reconstruct_snapshot_input(
    connection: &Connection,
    conversation: &ConversationId,
    snapshot: &RequestSnapshot,
) -> Result<ModelRequest, ConversationStoreError> {
    #[cfg(test)]
    REQUEST_INPUT_RECONSTRUCTIONS.with(|count| {
        let (requests, originals) = count.get();
        count.set((requests + 1, originals));
    });
    snapshot
        .reconstruct_with(|source| {
            if &source.conversation_id != conversation {
                return Err(
                    crate::model::snapshot::RequestReconstructionError::Conversation(
                        "frozen source scope mismatch".into(),
                    ),
                );
            }
            #[cfg(test)]
            REQUEST_INPUT_RECONSTRUCTIONS.with(|count| {
                let (requests, originals) = count.get();
                count.set((requests, originals + 1));
            });
            load_message(connection, &source.message_id).map_err(|e| {
                crate::model::snapshot::RequestReconstructionError::Conversation(e.to_string())
            })
        })
        .map_err(|e| ConversationStoreError::InvalidReference(e.to_string()))
}

fn validate_accepted_continuation_source(
    connection: &Connection,
    conversation: &ConversationId,
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let Some(source) = &snapshot.continuation_source else {
        return if snapshot.continuation.is_none() {
            Ok(())
        } else {
            Err(ConversationStoreError::InvalidReference(
                "continuation has no accepted-output source".into(),
            ))
        };
    };
    if snapshot.continuation.is_none()
        || source.request_id == snapshot.request_id
        || &source.assistant.conversation_id != conversation
    {
        return Err(ConversationStoreError::InvalidReference(
            "invalid actual continuation source ownership".into(),
        ));
    }
    let previous = load_request_snapshot(connection, conversation, &source.request_id)?;
    if previous.provisional_message_id != source.assistant.message_id {
        return Err(ConversationStoreError::InvalidReference(
            "continuation source is not the prior request's accepted output".into(),
        ));
    }
    let output = load_message(connection, &source.assistant.message_id)?;
    let input = reconstruct_snapshot_input(connection, conversation, &previous)?;
    let expected =
        crate::runtime::continuation::AcceptedOutputContinuationSource::from_accepted_output(
            previous.request_id,
            conversation,
            &input.messages,
            &output,
        )
        .map_err(ConversationStoreError::InvalidReference)?;
    if source != &expected {
        return Err(ConversationStoreError::InvalidReference(
            "continuation accepted-output or covered-input integrity mismatch".into(),
        ));
    }
    Ok(())
}

fn load_request_snapshot(
    connection: &Connection,
    conversation: &ConversationId,
    request_id: &RequestId,
) -> Result<RequestSnapshot, ConversationStoreError> {
    load_request_snapshot_bounded(connection, conversation, request_id, 64 * 1024 * 1024)
}

fn load_request_snapshot_bounded(
    connection: &Connection,
    conversation: &ConversationId,
    request_id: &RequestId,
    max_bytes: usize,
) -> Result<RequestSnapshot, ConversationStoreError> {
    // The normalized [] cannot exceed the eventual flat input array. Enforce
    // both metadata budgets in SQL before rusqlite allocates its String.
    let metadata_limit = max_bytes.min(64 * 1024 * 1024);
    #[cfg(test)]
    REQUEST_SNAPSHOT_READ_WORK.with(|work| {
        let (queries, bytes) = work.get();
        work.set((queries + 1, bytes));
    });
    let (stored_canonical_frontier, json, started_sequence, digest, native_oversized): (i64, Option<String>, Option<i64>, String, bool) = connection
        .query_row("SELECT canonical_frontier,CASE WHEN octet_length(snapshot_json)<=?2 THEN snapshot_json END,started_sequence,CASE WHEN typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64 THEN manifest_digest ELSE '' END,octet_length(snapshot_json)>67108864 FROM request_snapshots WHERE request_id=?1", params![request_id.as_str(),metadata_limit], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, read_manifest_digest(row, 3)?,row.get(4)?)))
        .optional().map_err(|error| storage(format!("request snapshot lookup: {error}")))?
        .ok_or_else(|| ConversationStoreError::RequestNotFound(request_id.clone()))?;
    let json = json.ok_or_else(|| {
        if native_oversized {
            ConversationStoreError::InvalidReference(
                "frozen request record exceeds the byte budget".into(),
            )
        } else {
            crate::context::resources::bound(
                "historical frozen request exceeds materialization bound",
            )
        }
    })?;
    #[cfg(test)]
    REQUEST_SNAPSHOT_READ_WORK.with(|work| {
        let (queries, bytes) = work.get();
        work.set((queries, bytes + json.len()));
    });
    // Normalization retained the two-byte empty input array. Charge the
    // actual flat sequence in its place before hydrating any selected leaf.
    let input_budget = max_bytes
        .checked_sub(json.len().saturating_sub(2))
        .ok_or_else(|| {
            crate::context::resources::bound(
                "historical frozen request exceeds materialization bound",
            )
        })?;
    let root = read_stored_snapshot_root(connection, request_id)?;
    if stored_snapshot_digest(&json, root) != digest {
        return Err(ConversationStoreError::InvalidReference(
            "frozen request manifest integrity mismatch".into(),
        ));
    }
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
    let mut snapshot: RequestSnapshot = decode(&json, "request snapshot")?;
    validate_snapshot_identity(&snapshot)?;
    if stored_canonical_frontier != seq_to_i64(snapshot.canonical_frontier.0)? {
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
    if event.conversation_id != *conversation {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} start event belongs to a foreign conversation"
        )));
    }
    validate_request_start_metadata(&snapshot, &event)?;
    if !snapshot.input.is_empty() {
        return Err(ConversationStoreError::InvalidReference(
            "normalized frozen request metadata contains selected input".into(),
        ));
    }
    snapshot.input = manifest::load_bounded(connection, root, input_budget)?;
    Ok(snapshot)
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

/// One indexed range per Assistant, then indexed result-message seeks.
/// No provider-id query or JSON filtering of the historical Ledger.
fn load_transcript_tools(
    connection: &Connection,
    assistant: &crate::message::types::AssistantMessageBlock,
) -> Result<Vec<crate::durable::inbox::TranscriptTool>, ConversationStoreError> {
    let mut tool_calls = Vec::new();
    let mut statement = connection
        .prepare(TRANSCRIPT_TOOLS_SQL)
        .map_err(|error| storage(format!("transcript Tool associations: {error}")))?;
    let rows = statement
        .query_map([assistant.id.as_str()], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|error| storage(format!("transcript Tool rows: {error}")))?;
    for row in rows {
        let (block_index, result_json) =
            row.map_err(|error| storage(format!("transcript Tool row: {error}")))?;
        let Some(AssistantContentBlock::ToolCall(call)) =
            assistant.content.get(block_index as usize)
        else {
            return Err(storage(
                "canonical Tool occurrence references a non-Tool block",
            ));
        };
        let result = result_json
            .map(|json| decode::<MessageBlock>(&json, "transcript Tool result"))
            .transpose()?
            .map(|message| match message {
                MessageBlock::Tool(tool)
                    if tool.occurrence.assistant_message_id == assistant.id
                        && tool.occurrence.block_index.get() == block_index
                        && tool.tool_call_id == call.id
                        && tool.tool_id == call.tool_id =>
                {
                    Ok(tool.result)
                }
                _ => Err(storage(
                    "canonical Tool occurrence references a foreign result",
                )),
            })
            .transpose()?;
        tool_calls.push(crate::durable::inbox::TranscriptTool {
            message_id: assistant.id.clone(),
            block_index: crate::message::types::ContentBlockIndex::new(block_index),
            call: call.clone(),
            result,
        });
    }
    Ok(tool_calls)
}

const TRANSCRIPT_TOOLS_SQL: &str = "SELECT c.block_index,l.message_json FROM canonical_tool_calls c
    LEFT JOIN message_ledger l ON l.message_id=c.result_message_id
    WHERE c.assistant_message_id=?1 ORDER BY c.block_index";

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
            inherited_through: inherited_transcript_through(connection)?,
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
            let mut tool_calls = Vec::new();
            if let TranscriptItem::Message {
                message: MessageBlock::Assistant(assistant),
            } = &item
            {
                tool_calls = load_transcript_tools(connection, assistant)?;
            }
            Ok(TranscriptEntry {
                tool_calls,
                cursor,
                item,
            })
        })
        .collect::<Result<Vec<_>, ConversationStoreError>>()?;
    Ok(TranscriptPage {
        inherited_through: inherited_transcript_through(connection)?,
        entries,
        next_cursor,
    })
}

const INHERITED_TRANSCRIPT_THROUGH_SQL: &str =
    "SELECT COALESCE((SELECT inherited_transcript_through FROM bootstrap_identity WHERE id=1),0)";

/// The seed owns canonical Ledger positions, not remapped message spelling or
/// origin Conversation IDs. Context messages have no transcript row, while
/// terminal-only inherited turns do: both are covered by this native boundary.
/// Bootstrap persists its final transcript watermark atomically with the seed;
/// local appends and repeated bounded reads never recompute the inherited prefix.
fn inherited_transcript_through(
    connection: &Connection,
) -> Result<TranscriptCursor, ConversationStoreError> {
    let position: i64 = connection
        .prepare_cached(INHERITED_TRANSCRIPT_THROUGH_SQL)
        .map_err(|error| storage(format!("inherited transcript boundary: {error}")))?
        .query_row([], |row| row.get(0))
        .map_err(|error| storage(format!("inherited transcript boundary: {error}")))?;
    Ok(TranscriptCursor::new(nonnegative(
        position,
        "inherited transcript boundary",
    )?))
}

/// Resolves one transcript reference without copying its body into the
/// ordering table.
fn load_transcript_item(
    connection: &Connection,
    reference_kind: &str,
    reference_id: &str,
) -> Result<TranscriptItem, ConversationStoreError> {
    match reference_kind {
        "inherited_turn" => Ok(TranscriptItem::InheritedTurn {
            provenance: provenance::turn_at(
                connection,
                reference_id
                    .parse::<i64>()
                    .map_err(|error| storage(error.to_string()))?,
            )?,
        }),

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
        "interaction_event" | "attempt_terminal" => {
            load_transcript_event(connection, reference_kind, reference_id)
        }
        other => Err(ConversationStoreError::InvalidReference(format!(
            "unknown transcript reference kind {other}"
        ))),
    }
}

fn load_transcript_event(
    connection: &Connection,
    reference_kind: &str,
    reference_id: &str,
) -> Result<TranscriptItem, ConversationStoreError> {
    let json: String = connection
        .query_row(
            "SELECT event_json FROM events WHERE event_id=?1",
            [reference_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("transcript event lookup: {error}")))?
        .ok_or_else(|| {
            ConversationStoreError::InvalidReference(format!(
                "transcript event reference {reference_id} has no durable event"
            ))
        })?;
    let event: RuntimeEventEnvelope = decode(&json, "transcript event")?;
    if event.event_id.as_str() != reference_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "transcript event reference {reference_id} contains a different event"
        )));
    }
    match (reference_kind, &event.event) {
        (
            "attempt_terminal",
            RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. },
        ) => Ok(TranscriptItem::AttemptTerminal { event }),
        ("interaction_event", RuntimeEvent::InteractionRequested { .. }) => {
            Ok(TranscriptItem::InteractionRequested { event })
        }
        ("interaction_event", RuntimeEvent::InteractionSettled { .. }) => {
            Ok(TranscriptItem::InteractionSettled { event })
        }
        _ => Err(ConversationStoreError::InvalidReference(format!(
            "transcript event reference {reference_id} is not a matching transcript event"
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
    let Some((head_revision, _)) = read_surface_head(connection)? else {
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
    for row in rows {
        let (stored_revision, stored_generation, json) =
            row.map_err(|error| storage(format!("Surface history row: {error}")))?;
        if u64::try_from(stored_revision).ok() != Some(expected_revision) {
            return Err(ConversationStoreError::InvalidReference(
                "Surface operation revisions are not contiguous from revision 1".to_owned(),
            ));
        }
        let _ = nonnegative(stored_generation, "canonical append metadata")?;
        let operation: SurfaceOp = decode(&json, "Surface operation")?;
        validate_surface_operation_references_from_ledger(&ledger, &operation)?;
        let appended_message_id = match &operation {
            SurfaceOp::Append { message_id } => Some(message_id.clone()),
        };
        apply_surface_op(&mut active, &operation)?;
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
    let SurfaceOp::Append { message_id } = operation;
    let references = [message_id];
    for message_id in references {
        if !ledger.contains_key(message_id) {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Surface operation references missing Ledger message {message_id}"
            )));
        }
    }

    Ok(())
}

fn reconstruct_surface(
    connection: &Connection,
    revision: SurfaceRevision,
) -> Result<Vec<MessageId>, ConversationStoreError> {
    Ok(replay_surface_history(connection, revision)?.0)
}

/// Reads the retained operations through `revision`, validating the same
/// contiguity, generation, and reference invariants a reconstruction does,
/// and returns both the Surface they denote and the operations themselves.
///
/// The operations are the half a lineage copy needs: they are what a later
/// fork or tree of the copy reads when it looks for its own branch points.
fn load_surface_history(
    connection: &Connection,
    through: SurfaceRevision,
) -> Result<Vec<SurfaceOp>, ConversationStoreError> {
    Ok(replay_surface_history(connection, through)?.1)
}

fn replay_surface_history(
    connection: &Connection,
    revision: SurfaceRevision,
) -> Result<(Vec<MessageId>, Vec<SurfaceOp>), ConversationStoreError> {
    let Some((head_revision, _)) = read_surface_head(connection)? else {
        if revision == SurfaceRevision::INITIAL {
            return Ok((Vec::new(), Vec::new()));
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
    let mut history = Vec::new();
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
    for row in rows {
        let (stored_revision, stored_generation, json) =
            row.map_err(|error| storage(format!("Surface history row: {error}")))?;
        if u64::try_from(stored_revision).ok() != Some(expected_revision) {
            return Err(ConversationStoreError::InvalidReference(
                "Surface operation revisions are not contiguous from revision 1".to_owned(),
            ));
        }
        let _ = nonnegative(stored_generation, "canonical append metadata")?;
        let operation: SurfaceOp = decode(&json, "Surface operation")?;
        validate_surface_operation_references(connection, &operation)?;
        apply_surface_op(&mut active, &operation)?;
        history.push(operation);
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
    Ok((active, history))
}

fn validate_surface_operation_references(
    connection: &Connection,
    operation: &SurfaceOp,
) -> Result<(), ConversationStoreError> {
    let SurfaceOp::Append { message_id } = operation;
    let references = [message_id];
    for message_id in references {
        if !ledger_message_exists(connection, message_id)? {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Surface operation references missing Ledger message {message_id}"
            )));
        }
    }

    Ok(())
}

fn validate_canonical_frontier(
    connection: &Connection,
    frontier: crate::context::resources::LedgerFrontier,
) -> Result<(), ConversationStoreError> {
    let head: i64 = connection
        .query_row(
            "SELECT coalesce(max(position),0) FROM message_ledger",
            [],
            |row| row.get(0),
        )
        .map_err(|e| storage(e.to_string()))?;
    if frontier.0 > nonnegative(head, "canonical frontier")? {
        return Err(ConversationStoreError::InvalidReference(
            "snapshot canonical frontier is beyond durable history".into(),
        ));
    }
    Ok(())
}

/// The one definition of what a Surface operation means lives in
/// `conversation::surface`; this is the durable domain's error wrapper for
/// it, so a reconstruction here and a seed validation there can never
/// disagree about the Surface an operation log denotes.
fn apply_surface_op(
    active: &mut Vec<MessageId>,
    operation: &SurfaceOp,
) -> Result<(), ConversationStoreError> {
    crate::conversation::apply_surface_op(active, operation)
        .map_err(ConversationStoreError::InvalidReference)
}

struct PersistedEvent {
    event: RuntimeEventEnvelope,
    transcript_cursor: Option<TranscriptCursor>,
}

/// Private executable authority and every captured credential share one row.
/// This type is never included in `RuntimeEvent`, projections or archives.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateAgentAuthority {
    authority: crate::runtime::subagent::DurableAgentAuthority,
    credentials: Vec<(String, String)>,
}

fn persist_agent_authority_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEvent,
    authority: Option<&crate::runtime::subagent::DurableAgentAuthority>,
) -> Result<(), ConversationStoreError> {
    match (event, authority) {
        (
            RuntimeEvent::SubagentOwnershipCommitted {
                child_agent_id,
                admitted_authority: Some(reference),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                ..
            },
            Some(authority),
        ) if reference == child_agent_id => {
            let mut authority = authority.clone();
            let mut credentials = Vec::new();
            authority
                .resolved
                .model
                .export_process_credentials(&mut credentials);
            for binding in authority.resolved.materialization.sources.values_mut() {
                binding
                    .credentials
                    .export_process_credentials(&mut credentials);
            }
            let encoded = serde_json::to_string(&PrivateAgentAuthority {
                authority,
                credentials,
            })
            .map_err(|_| storage("encode private Agent authority"))?;
            transaction
                .execute(
                    "INSERT INTO agent_authorities(agent_id,authority_json) VALUES(?1,?2)",
                    params![child_agent_id.as_str(), encoded],
                )
                .map_err(|_| storage("commit private Agent authority"))?;
            Ok(())
        }
        (
            RuntimeEvent::SubagentOwnershipCommitted {
                admitted_authority: Some(_),
                ..
            },
            _,
        )
        | (_, Some(_)) => Err(ConversationStoreError::InvalidReference(
            "Agent admission requires matching private authority and public identity".to_owned(),
        )),
        (_, None) => Ok(()),
    }
}

fn persist_event_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    event: RuntimeEventEnvelope,
) -> Result<PersistedEvent, ConversationStoreError> {
    persist_event_tx_internal(transaction, conversation_id, event, true, None)
}
fn persist_event_tx_deferred(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    event: RuntimeEventEnvelope,
) -> Result<PersistedEvent, ConversationStoreError> {
    persist_event_tx_internal(transaction, conversation_id, event, false, None)
}
// Fresh-start-only proof, borrowed from the exact transaction that has just
// validated its stored Snapshot and reconstructed every immutable dependency.
// Generic Journal insertions always perform their own reference validation.
struct ValidatedRequestStart<'a, 'connection> {
    transaction: &'a Transaction<'connection>,
    snapshot: &'a RequestSnapshot,
}

#[allow(clippy::too_many_lines)] // One atomic event, lifecycle and private admission transaction.
fn persist_event_tx_internal(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    mut event: RuntimeEventEnvelope,
    prepare_observation: bool,
    validated_start: Option<ValidatedRequestStart<'_, '_>>,
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
    validate_agent_admission_reference(transaction, &event)?;
    if let Some(proof) = validated_start {
        if !std::ptr::eq(proof.transaction, transaction) {
            return Err(ConversationStoreError::InvalidReference(
                "fresh request validation belongs to a different transaction".into(),
            ));
        }
        validate_request_start_metadata(proof.snapshot, &event)?;
    } else {
        validate_event_reference(transaction, &event)?;
    }
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
            "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp,observation_trace_required,observation_statistics_required,observation_kind,observation_members_required,observation_turn_required,observation_trace_fact_required,observation_execution_required,observation_unavailable) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,1)",
            params![
                sequence,
                event.event_id.as_str(),
                i64::from(event.schema_version),
                event.conversation_id.as_str(),
                event.attempt_id.as_ref().map(ToString::to_string),
                event.turn_id.as_ref().map(ToString::to_string),
                json,
                event.timestamp.to_rfc3339(),
                observation_statistics::is_trace_anchor(&event.event),
                observation_statistics::requires_statistics(&event),
                observation_event::native_kind(&event.event).ok(),
                observation_members::requires_members(&event),
                observation_turn::required(&event),
                observation_trace::required(&event),
                observation_execution::required(&event),
            ],
        )
        .map_err(|error| storage(format!("insert runtime event: {error}")))?;
    // Missing auxiliary evidence is the fail-closed initial state. Independent
    // savepoints preserve own lifecycle facts if a different layout fails.
    // Readiness has its own recoverable missing-evidence flag. A newly
    // installed Runtime may repair it without clearing unrelated layout holes.
    let _ = optional_display_layout(transaction, || {
        observation_readiness::insert(transaction, &event)
    });
    let statistics_available = optional_display_layout(transaction, || {
        observation_statistics::event(transaction, &event)?;
        observation_members::member_event(transaction, &event)
    })
    .is_some();
    if let RuntimeEvent::AssistantMessageCommitted { message_id } = &event.event {
        transaction.execute("UPDATE canonical_tool_calls SET attempt_id=?1,commit_sequence=?2 WHERE assistant_message_id=?3",params![event.attempt_id.as_ref().map(crate::runtime::identity::AttemptId::as_str),sequence,message_id.as_str()]).map_err(|error|storage(error.to_string()))?;
    }

    // Caller-supplied ephemeral evidence is never trusted. Native canonical
    // ownership is resolved inside this exact append transaction.
    let assistant_available = optional_display_layout(transaction, || {
        if let RuntimeEvent::AssistantMessageCommitted { message_id } = &event.event {
            transaction.execute("UPDATE events SET observation_assistant_ledger=(SELECT position FROM message_ledger WHERE message_id=?2) WHERE sequence=?1",params![sequence,message_id.as_str()]).map_err(|error|storage(error.to_string()))?;
        }
        Ok(())
    }).is_some();
    event.tool_occurrence = observation::tool_occurrence_receipt(transaction, &event)?;
    let tool_available = optional_display_layout(transaction, || {
    if let Some(occurrence) = &event.tool_occurrence {
        let phase = match &event.event {
            RuntimeEvent::ToolExecutionStarted { .. } => Some(1),
            RuntimeEvent::ToolExecutionCompleted { .. }
            | RuntimeEvent::ToolExecutionFailed { .. } => Some(2),
            _ => None,
        };
        transaction.execute("UPDATE events SET observation_tool_ledger=?1,observation_tool_block=?2,observation_tool_phase=?4 WHERE sequence=?3",params![occurrence.ledger_position,occurrence.block_index.get(),sequence,phase]).map_err(|error|storage(error.to_string()))?;
    }
        Ok(())
    }).is_some();
    let execution_available = optional_display_layout(transaction, || {
        observation_execution::event(transaction, &event)
    })
    .is_some();
    let state_available = optional_display_layout(transaction, || {
        observation_state::event(transaction, &event)
    })
    .is_some();
    let request_available = optional_display_layout(transaction, || {
        observation_occupancy::event(transaction, &event)?;
        observation_request::terminal(transaction, &event)
    })
    .is_some();
    let witness_available = observation_event::index_optional(transaction, &event);
    let trace_available = optional_display_layout(transaction, || {
        observation_trace::index_event(transaction, &event)
    })
    .is_some();
    let observation_json = crate::runtime_client::checkpoint::bounded_bytes(
        &event,
        crate::durable::observation::HEADER_ENCODED_MAX,
    );
    let json_available = optional_display_layout(transaction, || {
        transaction
            .execute(
                "UPDATE events SET observation_json=?2 WHERE sequence=?1",
                params![sequence, observation_json],
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(())
    })
    .is_some();
    transaction
        .execute(
            "UPDATE rustx_store SET next_event_sequence=?1 WHERE id=1",
            [sequence],
        )
        .map_err(|error| storage(format!("update event sequence: {error}")))?;
    let transcript_cursor = if matches!(
        &event.event,
        RuntimeEvent::AttemptCancelled { .. }
            | RuntimeEvent::AttemptFailed { .. }
            | RuntimeEvent::AttemptTimedOut { .. }
            | RuntimeEvent::AttemptLimitExceeded { .. }
    ) {
        Some(append_transcript_reference(
            transaction,
            "attempt_terminal",
            event.event_id.as_str(),
        )?)
    } else if matches!(
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
    let transcript_available = enrich_event_transcript(transaction, &event, transcript_cursor);
    let turn_available =
        optional_display_layout(transaction, || observation_turn::event(transaction, &event))
            .is_some();
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
    if statistics_available
        && assistant_available
        && tool_available
        && execution_available
        && state_available
        && request_available
        && witness_available
        && trace_available
        && json_available
        && transcript_available
        && turn_available
    {
        // The marker clear is auxiliary too: its failure leaves the original
        // native J/C/T committed and this row explicitly unavailable.
        let _ = optional_display_layout(transaction, || {
            transaction
                .execute(
                    "UPDATE events SET observation_unavailable=0 WHERE sequence=?1",
                    [sequence],
                )
                .map_err(|error| storage(error.to_string()))?;
            Ok(())
        });
    }
    event.observation = prepare_observation
        .then(|| observation_receipt::single(transaction, conversation_id, &event))
        .flatten();
    Ok(PersistedEvent {
        event,
        transcript_cursor,
    })
}

/// Independent derived T coordinates, witness and display header cannot refuse
/// the original transcript seat or erase another successfully captured layout.
fn enrich_event_transcript(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
    cursor: Option<TranscriptCursor>,
) -> bool {
    let Some(cursor) = cursor else {
        return true;
    };
    let coordinates = optional_display_layout(transaction, || {
        transaction
            .execute(
                "UPDATE events SET observation_transcript=?1 WHERE sequence=?2",
                params![cursor.get(), event.sequence],
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(())
    })
    .is_some();
    let witness = optional_display_layout(transaction, || {
        observation_event::attach_transcript(transaction, event.sequence, cursor)
    })
    .is_some();
    let kind = if matches!(
        event.event,
        RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
    ) {
        "interaction_event"
    } else {
        "attempt_terminal"
    };
    let header = optional_display_layout(transaction, || {
        observation::index_native_event(transaction, kind, event, cursor)
    })
    .is_some();
    coordinates && witness && header
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

// These expression indexes cover the original native Journal JSON, rather
// than optional observation headers. Two starts retain duplicate detection;
// one winner per terminal kind retains earliest native sequence ordering.
const REQUEST_START_PROBE_SQL: &str =
    "SELECT event_json FROM events INDEXED BY events_request_id_kind_idx
    WHERE json_extract(event_json,'$.event.request_id')=?1
      AND json_extract(event_json,'$.event.type')='model_request_started'
    ORDER BY sequence LIMIT 2";
const REQUEST_OUTCOME_PROBE_SQL: &str = "SELECT event_json FROM (
    SELECT event_json,sequence FROM (
        SELECT event_json,sequence FROM events INDEXED BY events_request_id_kind_idx
        WHERE json_extract(event_json,'$.event.request_id')=?1
          AND json_extract(event_json,'$.event.type')='model_request_completed'
        ORDER BY sequence LIMIT 1)
    UNION ALL
    SELECT event_json,sequence FROM (
        SELECT event_json,sequence FROM events INDEXED BY events_request_id_kind_idx
        WHERE json_extract(event_json,'$.event.request_id')=?1
          AND json_extract(event_json,'$.event.type')='model_request_failed'
        ORDER BY sequence LIMIT 1))
    ORDER BY sequence LIMIT 1";

#[cfg(test)]
thread_local! {
    static REQUEST_PROBE_WORK: std::cell::Cell<(usize,usize)> = const { std::cell::Cell::new((0,0)) };
}
#[cfg(test)]
fn record_request_probe_work(json: &str) {
    REQUEST_PROBE_WORK.with(|work| {
        let (rows, bytes) = work.get();
        work.set((rows + 1, bytes + json.len()));
    });
}

fn find_request_start_event(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let mut statement = transaction
        .prepare_cached(REQUEST_START_PROBE_SQL)
        .map_err(|error| storage(format!("request-start event probe: {error}")))?;
    let rows = statement
        .query_map([request_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(|error| storage(format!("request-start event query: {error}")))?;
    let mut found = None;
    for row in rows {
        let json = row.map_err(|error| storage(format!("request-start event row: {error}")))?;
        #[cfg(test)]
        record_request_probe_work(&json);
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
/// Scalar request identity and configuration, with no selected-input hydration.
/// Full request disclosure must explicitly load the separately bound native root.
#[derive(Debug)]
struct StoredRequestMetadata(RequestSnapshot);
impl std::ops::Deref for StoredRequestMetadata {
    type Target = RequestSnapshot;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
type RequestSnapshotRow = (StoredRequestMetadata, Option<u64>, Option<u64>);
type StartedRequestRow = (StoredRequestMetadata, u64, Option<u64>);
type StoredSnapshotRow = (i64, Option<String>, Option<i64>, Option<i64>, String);

#[cfg(test)]
thread_local! {
    static REQUEST_SNAPSHOT_READ_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
thread_local! {
    static MANIFEST_DIGEST_READ_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

// Every caller selects a fixed-size text digest or an empty rejection value.
fn read_manifest_digest(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    let digest: String = row.get(index)?;
    #[cfg(test)]
    MANIFEST_DIGEST_READ_WORK.with(|work| {
        let (rows, bytes) = work.get();
        work.set((rows + 1, bytes + digest.len()));
    });
    Ok(digest)
}

fn read_request_snapshot_tx(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RequestSnapshotRow>, ConversationStoreError> {
    #[cfg(test)]
    REQUEST_SNAPSHOT_READ_WORK.with(|work| {
        let (queries, bytes) = work.get();
        work.set((queries + 1, bytes));
    });
    let row: Option<StoredSnapshotRow> = transaction
        .query_row(
            "SELECT canonical_frontier,CASE WHEN octet_length(snapshot_json)<=67108864 THEN snapshot_json END,started_sequence,completed_sequence,CASE WHEN typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64 THEN manifest_digest ELSE '' END
             FROM request_snapshots WHERE request_id=?1",
            [request_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, read_manifest_digest(row, 4)?)),
        )
        .optional()
        .map_err(|error| storage(format!("request snapshot transaction lookup: {error}")))?;
    let Some((stored_canonical_frontier, json, started_sequence, completed_sequence, digest)) = row
    else {
        return Ok(None);
    };
    let json = json.ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "frozen request record exceeds the byte budget".into(),
        )
    })?;
    #[cfg(test)]
    REQUEST_SNAPSHOT_READ_WORK.with(|work| {
        let (queries, bytes) = work.get();
        work.set((queries, bytes + json.len()));
    });
    let root = read_stored_snapshot_root(transaction, request_id)?;
    if stored_snapshot_digest(&json, root) != digest {
        return Err(ConversationStoreError::InvalidReference(
            "frozen request manifest integrity mismatch".into(),
        ));
    }
    let snapshot: RequestSnapshot = decode(&json, "request snapshot")?;
    validate_snapshot_identity(&snapshot)?;
    if !snapshot.input.is_empty() {
        return Err(ConversationStoreError::InvalidReference(
            "normalized frozen request metadata contains selected input".into(),
        ));
    }
    if snapshot.request_id != *request_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot row {request_id} contains a different RequestId"
        )));
    }
    if stored_canonical_frontier != seq_to_i64(snapshot.canonical_frontier.0)? {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} Surface column disagrees with its frozen snapshot"
        )));
    }
    Ok(Some((
        StoredRequestMetadata(snapshot),
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
) -> Result<StartedRequestRow, ConversationStoreError> {
    let Some(row) = read_request_snapshot_tx(transaction, request_id)? else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {request_id} does not exist"
        )));
    };
    validate_started_request_row_tx(transaction, conversation_id, request_id, row)
}

/// Reuses a row decoded in this same transaction, preserving all original start
/// checks. Callers retain their operation-specific missing-row diagnostic.
fn validate_started_request_row_tx(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    request_id: &RequestId,
    (snapshot, started_sequence, completed_sequence): RequestSnapshotRow,
) -> Result<StartedRequestRow, ConversationStoreError> {
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
    Ok((snapshot, started_sequence, completed_sequence))
}

/// Finds any already durable provider terminal for one request. Both success
/// and failure are terminal outcomes: a contradictory second outcome is never
/// accepted, while only a successful outcome establishes P.
fn find_request_outcome_event(
    transaction: &Transaction<'_>,
    request_id: &RequestId,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let mut statement = transaction
        .prepare_cached(REQUEST_OUTCOME_PROBE_SQL)
        .map_err(|error| storage(format!("request outcome probe: {error}")))?;
    let rows = statement
        .query_map([request_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(|error| storage(format!("request outcome query: {error}")))?;
    for row in rows {
        let json = row.map_err(|error| storage(format!("request outcome row: {error}")))?;
        #[cfg(test)]
        record_request_probe_work(&json);
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
        RuntimeEvent::ToolExecutionDeadlineFired { .. } => "ToolExecutionDeadlineFired",
        RuntimeEvent::ToolExecutionCancellationRequested { .. } => {
            "ToolExecutionCancellationRequested"
        }
        RuntimeEvent::ToolExecutionSettlementObserved { .. } => "ToolExecutionSettlementObserved",
        RuntimeEvent::ToolExecutionSettlementControlFailed { .. } => {
            "ToolExecutionSettlementControlFailed"
        }
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

/// Compares a retried event with its durable copy. Event Journal sequence is
/// store-allocated and therefore intentionally excluded; every other field
/// is part of the producer's frozen terminal transition identity.
fn same_event_ignoring_sequence(
    existing: &RuntimeEventEnvelope,
    requested: &RuntimeEventEnvelope,
) -> bool {
    existing.schema_version == requested.schema_version
        && existing.event_id == requested.event_id
        && existing.conversation_id == requested.conversation_id
        && existing.attempt_id == requested.attempt_id
        && existing.turn_id == requested.turn_id
        && existing.timestamp == requested.timestamp
        && existing.event == requested.event
}

fn find_event_at_sequence(
    transaction: &Transaction<'_>,
    sequence: u64,
) -> Result<Option<RuntimeEventEnvelope>, ConversationStoreError> {
    let sequence = seq_to_i64(sequence)?;
    let Some(json) = transaction
        .query_row(
            "SELECT event_json FROM events WHERE sequence=?1",
            [sequence],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| storage(format!("event sequence probe: {error}")))?
    else {
        return Ok(None);
    };
    Ok(Some(decode(&json, "event sequence")?))
}

fn read_logical_step_progress_tx(
    transaction: &Transaction<'_>,
) -> Result<u64, ConversationStoreError> {
    let value: i64 = transaction
        .query_row(
            "SELECT logical_step_sequence FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("Todo progress transaction lookup: {error}")))?;
    nonnegative(value, "Todo progress sequence")
}

struct ContributionHeadRow {
    producer: String,
    key: String,
    fingerprint: String,
    emitted_at: String,
    request_id: String,
    message_id: String,
    logical_step_origin: i64,
    event_sequence: i64,
}

fn read_contribution_head_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContributionHeadRow> {
    Ok(ContributionHeadRow {
        producer: row.get(0)?,
        key: row.get(1)?,
        fingerprint: row.get(2)?,
        emitted_at: row.get(3)?,
        request_id: row.get(4)?,
        message_id: row.get(5)?,
        logical_step_origin: row.get(6)?,
        event_sequence: row.get(7)?,
    })
}

fn head_record_from_row(
    row: ContributionHeadRow,
) -> Result<ContributionEmissionRecord, ConversationStoreError> {
    let emitted_at = DateTime::parse_from_rfc3339(&row.emitted_at)
        .map_err(|error| {
            ConversationStoreError::InvalidReference(format!(
                "invalid Agent Status head timestamp {:?}: {error}",
                row.emitted_at
            ))
        })?
        .with_timezone(&Utc);
    Ok(ContributionEmissionRecord {
        producer: decode(&row.producer, "contribution producer")?,
        key: row.key,
        fingerprint: row.fingerprint,
        emitted_at,
        request_id: RequestId::new(row.request_id),
        canonical_message_id: MessageId::new(row.message_id),
        logical_step_origin: nonnegative(
            row.logical_step_origin,
            "Agent Status logical-step origin",
        )?,
        event_sequence: sequence_from_i64(row.event_sequence)?,
    })
}

fn read_contribution_head(
    connection: &Connection,
    producer: &crate::runtime::identity::ContextContributorIdentity,
    key: &str,
) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError> {
    connection
        .query_row(
            "SELECT producer,semantic_key,fingerprint,emitted_at,request_id,message_id,logical_step_origin,event_sequence
             FROM contribution_emission_heads
             WHERE producer=?1 AND semantic_key=?2",
            params![producer.durable_key(), key],
            read_contribution_head_row,
        )
        .optional()
        .map_err(|error| storage(format!("Agent Status head lookup: {error}")))?
        .map(head_record_from_row)
        .transpose()
}

fn read_contribution_head_tx(
    transaction: &Transaction<'_>,
    producer: &crate::runtime::identity::ContextContributorIdentity,
    key: &str,
) -> Result<Option<ContributionEmissionRecord>, ConversationStoreError> {
    transaction
        .query_row(
            "SELECT producer,semantic_key,fingerprint,emitted_at,request_id,message_id,logical_step_origin,event_sequence
             FROM contribution_emission_heads
             WHERE producer=?1 AND semantic_key=?2",
            params![producer.durable_key(), key],
            read_contribution_head_row,
        )
        .optional()
        .map_err(|error| storage(format!("Agent Status head lookup: {error}")))?
        .map(head_record_from_row)
        .transpose()
}

fn upsert_contribution_head_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::ContextContributionEmitted {
        producer,
        request_id,
        message_id,
        emission,
        logical_step_origin,
    } = &event.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "latest-emission head update requires an contribution emission fact".to_owned(),
        ));
    };
    let existing = read_contribution_head_tx(transaction, producer, &emission.key)?;
    if let Some(existing) = existing {
        if existing.event_sequence > event.sequence {
            return Err(ConversationStoreError::InvalidReference(format!(
                "Agent Status head for {}:{} is newer than event {}",
                producer.durable_key(),
                emission.key,
                event.sequence
            )));
        }
        if existing.event_sequence == event.sequence {
            if existing.fingerprint == emission.fingerprint
                && existing.request_id == *request_id
                && existing.canonical_message_id == *message_id
                && existing.logical_step_origin == *logical_step_origin
                && existing.emitted_at == event.timestamp
            {
                return Ok(());
            }
            return Err(ConversationStoreError::InvalidReference(
                "Agent Status latest-emission head conflicts at one event sequence".to_owned(),
            ));
        }
        if existing.logical_step_origin > *logical_step_origin {
            return Err(ConversationStoreError::InvalidReference(
                "Agent Status latest-emission head regresses the committed logical-step origin"
                    .to_owned(),
            ));
        }
        transaction
            .execute(
                "UPDATE contribution_emission_heads
                 SET fingerprint=?1,emitted_at=?2,request_id=?3,message_id=?4,logical_step_origin=?5,event_sequence=?6
                 WHERE producer=?7 AND semantic_key=?8",
                params![
                    emission.fingerprint,
                    event.timestamp.to_rfc3339(),
                    request_id.as_str(),
                    message_id.as_str(),
                    seq_to_i64(*logical_step_origin)?,
                    seq_to_i64(event.sequence)?,
                    producer.durable_key(),
                    emission.key,
                ],
            )
            .map_err(|error| storage(format!("update Agent Status head: {error}")))?;
    } else {
        transaction
            .execute(
                "INSERT INTO contribution_emission_heads
                 (producer,semantic_key,fingerprint,emitted_at,request_id,message_id,logical_step_origin,event_sequence)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    producer.durable_key(),
                    emission.key,
                    emission.fingerprint,
                    event.timestamp.to_rfc3339(),
                    request_id.as_str(),
                    message_id.as_str(),
                    seq_to_i64(*logical_step_origin)?,
                    seq_to_i64(event.sequence)?,
                ],
            )
            .map_err(|error| storage(format!("insert Agent Status head: {error}")))?;
    }
    Ok(())
}

/// Resolves the authoritative child identity and workspace binding from the one durable
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
fn find_subagent_ownership(
    transaction: &Transaction<'_>,
    subagent_id: &crate::runtime::identity::SubagentId,
) -> Result<
    (
        AgentId,
        std::path::PathBuf,
        crate::events::types::SubagentOwnershipKind,
        String,
        crate::runtime::subagent::AgentActivationOrigin,
    ),
    ConversationStoreError,
> {
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
            ownership,
            workspace,
            agent,
            origin,
            ..
        } if embedded == *subagent_id => Ok((child_agent_id, workspace, ownership, agent, origin)),
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

/// The complete legal-state validation of one normal subagent terminal
/// publication (Issue #192), run before anything is accepted.
///
/// The transition is semantically closed: it binds the
/// `SubagentTerminalPublished` fact, the terminal report draft, and — on
/// success — the runtime-authored terminal notice draft to each other and
/// to the committed ownership fact, so a caller cannot create durable
/// state no legal sequence of runtime decisions could reach:
///
/// - the event identity, the report message identity, and both producer
///   correlations are the canonical functions of the one `SubagentId`;
/// - the event names the committed child agent of a `Normal` ownership;
/// - a successful report is authored by that child agent, every other
///   terminal by the runtime;
/// - exactly a success carries the runtime-authored terminal notice, whose
///   identity, correlation, provenance, and exact canonical text are
///   recomputed from durable facts.
#[allow(clippy::too_many_lines)] // one semantically closed validation boundary
fn validate_subagent_terminal_publication(
    transaction: &Transaction<'_>,
    notice: Option<&InboundDraft>,
    report: &InboundDraft,
    event: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let RuntimeEvent::SubagentTerminalPublished {
        subagent_id,
        child_agent_id,
        message_id,
        state,
        ..
    } = &event.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "subagent terminal acceptance requires a subagent terminal fact".to_owned(),
        ));
    };
    if event.attempt_id.is_some() || event.turn_id.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "subagent terminal publication must not carry an attempt or turn envelope".to_owned(),
        ));
    }
    let canonical_event_id = crate::runtime::subagent::terminal_event_id(subagent_id);
    if !event.event_id.as_str().is_empty() && event.event_id != canonical_event_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent terminal event identity {} does not match the canonical identity {canonical_event_id}",
            event.event_id
        )));
    }
    let (committed_child_agent_id, _workspace, ownership, agent, _) =
        find_subagent_ownership(transaction, subagent_id)?;
    if ownership != crate::events::types::SubagentOwnershipKind::Normal {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} Workflow ownership cannot use parent terminal publication"
        )));
    }
    if committed_child_agent_id != *child_agent_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal claims child agent {child_agent_id}, but durable ownership committed {committed_child_agent_id}"
        )));
    }
    let canonical_report_id = crate::runtime::subagent::terminal_message_id(subagent_id);
    if *message_id != canonical_report_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal references {message_id}, not its canonical report message {canonical_report_id}"
        )));
    }
    if report.message_id.as_ref() != Some(&canonical_report_id) {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal report identity is not the canonical {canonical_report_id}"
        )));
    }
    if report.correlation.as_deref()
        != Some(crate::runtime::subagent::terminal_correlation(subagent_id).as_str())
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal report does not carry the canonical producer correlation"
        )));
    }
    if report.kind != InboundKind::Message {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal report has an ineligible inbound kind"
        )));
    }
    let success = matches!(
        state,
        crate::events::types::SubagentTerminalState::Succeeded
    );
    let provenance_ok = if success {
        report.source
            == UserSource::Agent {
                agent_id: child_agent_id.clone(),
            }
    } else {
        report.source == UserSource::Runtime
    };
    if !provenance_ok {
        return Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} terminal report provenance does not match its terminal state"
        )));
    }
    match (success, notice) {
        (true, Some(notice)) => {
            if notice.source != UserSource::Runtime || notice.kind != InboundKind::Message {
                return Err(ConversationStoreError::InvalidReference(
                    "a subagent terminal notice is runtime-authored only".to_owned(),
                ));
            }
            let canonical_notice_id =
                crate::runtime::subagent::terminal_notice_message_id(subagent_id);
            if notice.message_id.as_ref() != Some(&canonical_notice_id) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal notice identity is not the canonical {canonical_notice_id}"
                )));
            }
            if notice.correlation.as_deref()
                != Some(crate::runtime::subagent::terminal_notice_correlation(subagent_id).as_str())
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal notice does not carry the canonical producer correlation"
                )));
            }
            let expected =
                crate::runtime::subagent::terminal_notice_text(child_agent_id, subagent_id, &agent);
            if notice.content != [UserContentBlock::Text(TextBlock { text: expected })] {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal notice is not the canonical runtime-authored terminal metadata"
                )));
            }
            Ok(())
        }
        (true, None) => Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} successful terminal requires the runtime-authored terminal notice"
        ))),
        (false, Some(_)) => Err(ConversationStoreError::InvalidReference(format!(
            "subagent {subagent_id} non-success terminal must not carry a successful-terminal notice"
        ))),
        (false, None) => Ok(()),
    }
}

fn validate_agent_admission_reference(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    use crate::events::types::AgentActivationAdmissionPhase;
    let (agent_id, activation_id, origin, phase) = match &envelope.event {
        RuntimeEvent::AgentActivationAdmission {
            agent_id,
            activation_id,
            origin,
            phase,
        } => (agent_id, activation_id, origin, Some(phase)),
        RuntimeEvent::SubagentOwnershipCommitted {
            child_agent_id,
            subagent_id,
            origin,
            admitted_authority: None,
            ownership: crate::events::types::SubagentOwnershipKind::Normal,
            ..
        } => (child_agent_id, subagent_id, origin, None),
        _ => return Ok(()),
    };
    let invalid = || {
        ConversationStoreError::InvalidReference(
            "Agent activation admission must reference its durable Agent and exact reservation"
                .to_owned(),
        )
    };
    if activation_id
        .conversation_ordinal(&envelope.conversation_id)
        .is_none()
    {
        return Err(invalid());
    }
    if let Some(phase) = phase
        && envelope.event_id != crate::runtime::subagent::admission_event_id(activation_id, phase)
    {
        return Err(invalid());
    }
    let admitted: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_authorities WHERE agent_id=?1)",
            [agent_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("Agent admission authority: {error}")))?;
    if !admitted {
        return Err(invalid());
    }
    if matches!(phase, Some(AgentActivationAdmissionPhase::Reserved)) {
        return Ok(());
    }
    let reserved_id = crate::runtime::subagent::admission_event_id(
        activation_id,
        &AgentActivationAdmissionPhase::Reserved,
    );
    let reserved = find_event_by_id(transaction, &reserved_id)?.ok_or_else(invalid)?;
    if !matches!(reserved.event, RuntimeEvent::AgentActivationAdmission {
        agent_id: owner, activation_id: target, origin: admitted_origin,
        phase: AgentActivationAdmissionPhase::Reserved,
    } if owner == *agent_id && target == *activation_id && admitted_origin == *origin)
    {
        return Err(invalid());
    }
    Ok(())
}

fn reconstruct_request_start_reference(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
    expected_json: Option<&str>,
) -> Result<ModelRequest, ConversationStoreError> {
    let RuntimeEvent::ModelRequestStarted { request_id, .. } = &envelope.event else {
        return Err(ConversationStoreError::InvalidReference(
            "request reference validation requires a ModelRequestStarted fact".into(),
        ));
    };
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
    if expected_json.is_some_and(|expected| expected != json) {
        return Err(ConversationStoreError::InvalidReference(
            "fresh request stored Snapshot differs from its exact admitted bytes".into(),
        ));
    }
    let snapshot: RequestSnapshot = decode(&json, "request-start snapshot")?;
    validate_snapshot_identity(&snapshot)?;
    if snapshot.request_id != *request_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request-start snapshot identity does not match {request_id}"
        )));
    }
    validate_request_start_metadata(&snapshot, envelope)?;
    let root = read_stored_snapshot_root(transaction, request_id)?;
    let digest: String = transaction
        .query_row(
            "SELECT CASE WHEN typeof(manifest_digest)='text' AND octet_length(manifest_digest)=64 THEN manifest_digest ELSE '' END FROM request_snapshots WHERE request_id=?1",
            [request_id.as_str()],
            |row| read_manifest_digest(row, 0),
        )
        .map_err(|error| storage(format!("request reference integrity: {error}")))?;
    if stored_snapshot_digest(&json, root) != digest || !snapshot.input.is_empty() {
        return Err(ConversationStoreError::InvalidReference(
            "frozen request manifest integrity mismatch".into(),
        ));
    }
    let mut snapshot = snapshot;
    snapshot.input = manifest::load(transaction, root)?;
    reconstruct_snapshot_input(transaction, &envelope.conversation_id, &snapshot)
}

#[allow(clippy::too_many_lines)] // Closed validation of every durable event family.
fn validate_event_reference(
    transaction: &Transaction<'_>,
    envelope: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    match &envelope.event {
        RuntimeEvent::ContextContributionEmitted {
            producer,
            request_id,
            message_id,
            emission,
            logical_step_origin,
        } => {
            validate_contribution_emission_shape(emission)?;
            let Some((snapshot, Some(started_sequence), _)) =
                read_request_snapshot_tx(transaction, request_id)?
            else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "contribution emission references request {request_id}, which has not started"
                )));
            };
            let started =
                find_event_at_sequence(transaction, started_sequence)?.ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "request {request_id} has no event at its recorded start sequence"
                    ))
                })?;
            validate_request_start_metadata(&snapshot, &started)?;
            let contribution = snapshot
                .contributions
                .iter()
                .find(|candidate| {
                    candidate.message_id == *message_id && candidate.producer == *producer
                })
                .ok_or_else(|| {
                    ConversationStoreError::InvalidReference(format!(
                        "emission for request {request_id} has no accepted contribution"
                    ))
                })?;
            if snapshot.identity.retry_number != 0 {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution emission must belong to the first request of its logical primary step"
                        .to_owned(),
                ));
            }
            let current_logical_step_progress = read_logical_step_progress_tx(transaction)?;
            if *logical_step_origin != current_logical_step_progress {
                return Err(ConversationStoreError::InvalidReference(
                    "contribution emission logical-step origin is not the store-owned start boundary"
                        .to_owned(),
                ));
            }
            if envelope.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
                || envelope.turn_id.as_ref() != Some(&snapshot.identity.turn)
                || envelope.event_id
                    != contribution_emission_event_id(request_id, producer, emission)
                || envelope.timestamp != started.timestamp
                || !contribution
                    .emissions
                    .iter()
                    .any(|candidate| candidate == emission)
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "contribution emission for request {request_id} does not match its prepared start"
                )));
            }
            let message = load_message_tx(transaction, message_id)?;
            contribution.validate_message(&message)?;
        }
        // A retry schedule is correlated by the failed request identity, not
        // by event position. It is valid only after that exact actual request
        // started and durably failed, and its ordinal must be the next shared
        // actual-request generation of the same logical turn.
        RuntimeEvent::ModelRetryScheduled {
            failed_request_id,
            retry_number,
            ..
        } => {
            let (snapshot, _, _) = require_started_request_tx(
                transaction,
                &envelope.conversation_id,
                failed_request_id,
            )?;
            if envelope.attempt_id.as_ref() != Some(&snapshot.identity.attempt_id)
                || envelope.turn_id.as_ref() != Some(&snapshot.identity.turn)
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "model retry schedule for {failed_request_id} has a foreign attempt or turn envelope"
                )));
            }
            let expected_retry_number =
                snapshot
                    .identity
                    .retry_number
                    .checked_add(1)
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "model request retry ordinal is exhausted".to_owned(),
                        )
                    })?;
            if *retry_number != expected_retry_number {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "retry schedule for {failed_request_id} names ordinal {retry_number}, expected {expected_retry_number}"
                )));
            }
            let Some(outcome) = find_request_outcome_event(transaction, failed_request_id)? else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "retry schedule for {failed_request_id} has no provider outcome"
                )));
            };
            if !matches!(
                outcome.event,
                RuntimeEvent::ModelRequestFailed {
                    request_id,
                    ..
                } if request_id == *failed_request_id
            ) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "retry schedule for {failed_request_id} does not follow a failed provider outcome"
                )));
            }
        }
        // A provider outcome names an actual started request. Without this
        // the P marker could be recorded against a request that never had a
        // durable start fact.
        RuntimeEvent::ModelRequestCompleted { request_id, .. }
        | RuntimeEvent::ModelRequestFailed { request_id, .. } => {
            let Some(row) = read_request_snapshot_tx(transaction, request_id)? else {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "model request outcome references request {request_id}, which never started"
                )));
            };
            let (snapshot, _, _) = validate_started_request_row_tx(
                transaction,
                &envelope.conversation_id,
                request_id,
                row,
            )?;
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
        | RuntimeEvent::ToolExecutionDeadlineFired {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionCancellationRequested {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionSettlementObserved {
            tool_call_id,
            tool_id,
            ..
        }
        | RuntimeEvent::ToolExecutionSettlementControlFailed {
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
                None,
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
                None,
            )?;
        }
        RuntimeEvent::SubagentOwnershipCommitted {
            subagent_id,
            origin,
            workspace,
            ownership,
            ..
        } => {
            match (ownership, origin) {
                (crate::events::types::SubagentOwnershipKind::Workflow,
                 crate::runtime::subagent::AgentActivationOrigin::Workflow { node_id })
                    if node_id.block.run.conversation_id == envelope.conversation_id => {}
                (crate::events::types::SubagentOwnershipKind::Normal,
                 crate::runtime::subagent::AgentActivationOrigin::CreationTool { .. }
                 | crate::runtime::subagent::AgentActivationOrigin::MessageTool { .. }
                 | crate::runtime::subagent::AgentActivationOrigin::ClientControl) => {}
                _ => return Err(ConversationStoreError::InvalidReference(
                    "subagent ownership kind and origin must agree and name the admitting Workflow Conversation".into(),
                )),
            }
            if !workspace.is_absolute() {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} has a non-absolute execution workspace binding"
                )));
            }
            if let Some(tool_call_id) = origin.tool_call_id() {
                record_tool_proposal_dependency(
                    transaction,
                    tool_call_id,
                    envelope.attempt_id.as_ref(),
                    envelope.turn_id.as_ref(),
                    None,
                    runtime_event_dependency_name(&envelope.event),
                    true,
                    None,
                )?;
            }
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
            let workflow_node = match subject {
                InteractionSubject::Review { review } => Some(review.instance.as_ref()),
                InteractionSubject::Questionnaire {
                    invocation_id: crate::tools::types::ToolInvocationId::Workflow { node },
                    ..
                } => Some(node.as_ref()),
                _ => None,
            };
            if workflow_node.is_some_and(|node| {
                node.block.run.conversation_id != envelope.conversation_id
                    || &node.block.run.attempt_id != attempt_id
            }) {
                return Err(ConversationStoreError::InvalidReference(
                    "interaction Workflow correlation differs from its owning envelope".into(),
                ));
            }
            validate_approval_subject_against_canonical(
                transaction,
                &envelope.conversation_id,
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
                Some(&tool.occurrence.assistant_message_id),
            )?;
        }
        RuntimeEvent::CompactionCompleted {
            checkpoint_id,
            activation_id,
            generation,
            ..
        } => {
            let activation = projection::completion_activation(transaction, checkpoint_id)?;
            if activation.activation != *activation_id || *generation != activation_id.0 {
                return Err(ConversationStoreError::InvalidReference(
                    "compaction completion differs from atomic projection activation".into(),
                ));
            }
        }
        RuntimeEvent::ModelRequestStarted { .. } => {
            let _ = reconstruct_request_start_reference(transaction, envelope, None)?;
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
            let expected_event_id = crate::runtime::subagent::terminal_event_id(subagent_id);
            if envelope.event_id != expected_event_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent terminal event identity {} does not match the canonical identity {expected_event_id}",
                    envelope.event_id
                )));
            }
            if envelope.attempt_id.is_some() || envelope.turn_id.is_some() {
                return Err(ConversationStoreError::InvalidReference(
                    "subagent terminal publication must not carry an attempt or turn envelope"
                        .to_owned(),
                ));
            }
            if *message_id != crate::runtime::subagent::terminal_message_id(subagent_id) {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} terminal references {message_id}, not its canonical report message {}",
                    crate::runtime::subagent::terminal_message_id(subagent_id)
                )));
            }
            let (committed_child_agent_id, _workspace, ownership, agent, _) =
                find_subagent_ownership(transaction, subagent_id)?;
            if ownership != crate::events::types::SubagentOwnershipKind::Normal {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} Workflow ownership cannot use parent terminal publication"
                )));
            }
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
            // A successful publication commits with its runtime-authored
            // terminal notice (Issue #192): the exact canonical notice of
            // this subagent and this named agent must already be durable in
            // the same transaction.
            if *state == crate::events::types::SubagentTerminalState::Succeeded {
                let notice = load_user_notification_tx(
                    transaction,
                    &crate::runtime::subagent::terminal_notice_message_id(subagent_id),
                    "subagent terminal",
                )?;
                let expected_notice = crate::runtime::subagent::terminal_notice_text(
                    child_agent_id,
                    subagent_id,
                    &agent,
                );
                let notice_text_ok = notice.source == UserSource::Runtime
                    && notice.kind == InboundKind::Message
                    && notice.content
                        == vec![UserContentBlock::Text(TextBlock {
                            text: expected_notice,
                        })];
                if !notice_text_ok {
                    return Err(ConversationStoreError::InvalidReference(format!(
                        "subagent {subagent_id} successful terminal lacks its canonical runtime-authored terminal notice"
                    )));
                }
            }
        }
        RuntimeEvent::SubagentPhysicalSettlementProven {
            subagent_id,
            child_agent_id,
        } => {
            if envelope.event_id
                != crate::runtime::subagent::physical_settlement_event_id(subagent_id)
                || envelope.attempt_id.is_some()
                || envelope.turn_id.is_some()
            {
                return Err(ConversationStoreError::InvalidReference(
                    "invalid physical settlement proof identity".into(),
                ));
            }
            let (owner, _, ownership, _, _) = find_subagent_ownership(transaction, subagent_id)?;
            let terminal_id = match ownership {
                crate::events::types::SubagentOwnershipKind::Normal => {
                    crate::runtime::subagent::terminal_event_id(subagent_id)
                }
                crate::events::types::SubagentOwnershipKind::Workflow => {
                    crate::runtime::subagent::terminal_settlement_event_id(subagent_id)
                }
            };
            let terminal: Option<String> = transaction
                .query_row(
                    "SELECT event_json FROM events WHERE event_id=?1",
                    [terminal_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| {
                    storage(format!("physical settlement terminal lookup: {error}"))
                })?;
            let terminal = terminal.ok_or_else(|| {
                ConversationStoreError::InvalidReference(
                    "physical settlement without terminal publication".into(),
                )
            })?;
            let terminal: RuntimeEventEnvelope = decode(&terminal, "physical settlement terminal")?;
            if owner != *child_agent_id
                || !matches!(
                    terminal.event,
                    RuntimeEvent::SubagentTerminalPublished {
                        physical_settlement_proven: false,
                        ..
                    } | RuntimeEvent::SubagentTerminalSettled {
                        physical_settlement_proven: false,
                        ..
                    }
                )
            {
                return Err(ConversationStoreError::InvalidReference(
                    "physical settlement does not name an unresolved terminal".into(),
                ));
            }
        }
        RuntimeEvent::SubagentTerminalSettled {
            subagent_id,
            child_agent_id,
            ..
        } => {
            let expected_event_id =
                crate::runtime::subagent::terminal_settlement_event_id(subagent_id);
            if envelope.event_id != expected_event_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Workflow subagent terminal event identity {} does not match the canonical identity {expected_event_id}",
                    envelope.event_id
                )));
            }
            let (committed_child_agent_id, _workspace, ownership, _agent, _) =
                find_subagent_ownership(transaction, subagent_id)?;
            if ownership != crate::events::types::SubagentOwnershipKind::Workflow {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} normal ownership cannot use Workflow terminal settlement"
                )));
            }
            if committed_child_agent_id != *child_agent_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Workflow subagent {subagent_id} terminal claims child agent {child_agent_id}, but durable ownership committed {committed_child_agent_id}"
                )));
            }
        }
        RuntimeEvent::WorkflowAgentOutputCommitted {
            subagent_id,
            workflow_id,
            run_id,
            node_id,
            output,
            ..
        } => {
            let expected_event_id = crate::runtime::subagent::workflow_output_event_id(subagent_id);
            if envelope.event_id != expected_event_id {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "Workflow output event identity {} does not match the canonical identity {expected_event_id}",
                    envelope.event_id
                )));
            }
            if node_id.node.is_empty()
                || node_id.node.len() > 64
                || node_id.block.definition.blocks.len()
                    > 2 * crate::runtime::workflow::MAX_BLOCK_DEPTH
                || node_id
                    .block
                    .definition
                    .blocks
                    .iter()
                    .any(|key| key.is_empty() || key.len() > 64)
                || node_id.block.invocations.len() != node_id.block.definition.blocks.len() / 2 + 1
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Workflow output event has an invalid node identity".to_owned(),
                ));
            }
            if &node_id.block.run != run_id || &node_id.block.definition.workflow_id != workflow_id
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Workflow output run and definition disagree with its node identity".to_owned(),
                ));
            }
            let encoded = serde_json::to_vec(output)
                .map_err(|error| storage(format!("Workflow output event encoding: {error}")))?;
            if encoded.len() > crate::runtime::subagent::MAX_RESULT_CONTENT_BYTES {
                return Err(ConversationStoreError::InvalidReference(
                    "Workflow output event exceeds the bounded value size".to_owned(),
                ));
            }
            let (_, _, ownership, _, origin) = find_subagent_ownership(transaction, subagent_id)?;
            if ownership != crate::events::types::SubagentOwnershipKind::Workflow {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "subagent {subagent_id} normal ownership cannot commit a Workflow output"
                )));
            }
            if !matches!(origin, crate::runtime::subagent::AgentActivationOrigin::Workflow { node_id: owned } if owned.as_ref() == node_id)
            {
                return Err(ConversationStoreError::InvalidReference(
                    "Workflow output node differs from the committed child ownership origin"
                        .to_owned(),
                ));
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

/// Whether an event has a transcript-visible or otherwise compound durable
/// owner that must return through a purpose-specific transition.
fn requires_specialized_transition(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::Goal {
            fact: crate::goal::GoalFact::Written { .. }
                | crate::goal::GoalFact::RoundAdmitted { .. }
        } | RuntimeEvent::InboundTurnAdopted { .. }
            | RuntimeEvent::AssistantMessageCommitted { .. }
            | RuntimeEvent::ToolMessageCommitted { .. }
            | RuntimeEvent::CompactionCompleted { .. }
            | RuntimeEvent::ModelRequestStarted { .. }
            | RuntimeEvent::ContextContributionEmitted { .. }
            | RuntimeEvent::BackgroundTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalPublished { .. }
            | RuntimeEvent::SubagentTerminalSettled { .. }
            | RuntimeEvent::WorkflowAgentOutputCommitted { .. }
            | RuntimeEvent::InteractionRequested { .. }
            | RuntimeEvent::InteractionSettled { .. }
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
    validate_carryover_snapshot(snapshot)?;
    Ok(())
}

fn validate_carryover_snapshot(snapshot: &RequestSnapshot) -> Result<(), ConversationStoreError> {
    match (
        &snapshot.unresolved_output_carryover_source,
        &snapshot.unresolved_output_carryover,
        &snapshot.unresolved_output_carryover_anchor,
    ) {
        (None, None, None) => Ok(()),
        (Some(source), representation, Some(_)) => {
            if let Some(representation) = representation
                && representation.source_stream_id != *source
            {
                return Err(ConversationStoreError::InvalidReference(format!(
                    "request snapshot {} carryover representation disagrees with its source",
                    snapshot.request_id
                )));
            }
            Ok(())
        }
        _ => Err(ConversationStoreError::InvalidReference(format!(
            "request snapshot {} has an incomplete frozen carryover tuple",
            snapshot.request_id
        ))),
    }
}

fn consume_pending_carryover_tx(
    transaction: &Transaction<'_>,
    snapshot: &RequestSnapshot,
) -> Result<(), ConversationStoreError> {
    let Some(source) = snapshot.unresolved_output_carryover_source.as_ref() else {
        return Ok(());
    };
    if snapshot.identity.retry_number != 0 {
        return Ok(());
    }
    let pending: Option<String> = transaction
        .query_row(
            "SELECT pending_unresolved_output_stream_id FROM rustx_store WHERE id=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage(format!("pending carryover consume probe: {error}")))?;
    if pending.as_deref() != Some(source.as_str()) {
        return Err(ConversationStoreError::InvalidReference(format!(
            "request {} attempted to consume pending carryover {}, but the durable pending source is {:?}",
            snapshot.request_id, source, pending
        )));
    }
    transaction
        .execute(
            "UPDATE rustx_store SET pending_unresolved_output_stream_id=NULL WHERE id=1",
            [],
        )
        .map_err(|error| storage(format!("consume pending carryover: {error}")))?;
    Ok(())
}

fn validate_pending_carryover_source_tx(
    transaction: &Transaction<'_>,
    event: &RuntimeEventEnvelope,
    source: Option<&PublicationStreamId>,
) -> Result<(), ConversationStoreError> {
    let Some(source) = source else {
        return Ok(());
    };
    let Some(attempt_id) = event.attempt_id.as_ref() else {
        return Err(ConversationStoreError::InvalidReference(
            "pending carryover requires an attempt-scoped terminal event".to_owned(),
        ));
    };
    let Some(audit_json): Option<String> = transaction
        .query_row(
            "SELECT audit_json FROM publication_audits WHERE stream_id=?1",
            [source.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage(format!("pending carryover audit probe: {error}")))?
    else {
        return Err(ConversationStoreError::InvalidReference(format!(
            "pending carryover source {source} has no durable Publication Audit"
        )));
    };
    let audit: PublicationAudit = decode(&audit_json, "pending carryover audit")?;
    if audit.stream_id != *source || audit.attempt_id != *attempt_id {
        return Err(ConversationStoreError::InvalidReference(format!(
            "pending carryover source {source} does not belong to terminal attempt {attempt_id}"
        )));
    }
    if !matches!(
        audit.kind,
        crate::publication::PublicationAuditKind::Incomplete
            | crate::publication::PublicationAuditKind::Unaccepted
    ) || crate::publication::render_unresolved_output_carryover(&audit).is_none()
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "pending carryover source {source} is not a meaningful unresolved Publication Audit"
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
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM message_ledger WHERE message_id=?1)")
        .and_then(|mut statement| statement.query_row([message_id.as_str()], |row| row.get(0)))
        .map_err(|error| storage(format!("event Ledger reference: {error}")))
}

#[allow(clippy::too_many_lines)] // Closed mapping from durable facts to exact terminal domains.
fn lifecycle_keys(event: &RuntimeEventEnvelope) -> Vec<(String, bool)> {
    match &event.event {
        RuntimeEvent::CompactionStarted { operation_id } => {
            return vec![(format!("compaction:{operation_id}"), false)];
        }
        RuntimeEvent::CompactionCompleted { operation_id, .. }
        | RuntimeEvent::CompactionFailed { operation_id, .. } => {
            return vec![(format!("compaction:{operation_id}"), true)];
        }
        _ => {}
    }
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
    if let RuntimeEvent::AgentActivationAdmission {
        activation_id,
        phase,
        ..
    } = &event.event
    {
        return vec![(
            format!("agent-admission:{activation_id}"),
            matches!(
                phase,
                crate::events::types::AgentActivationAdmissionPhase::RolledBack {
                    physical_settlement_proven: true
                }
            ),
        )];
    }
    if let RuntimeEvent::SubagentOwnershipCommitted {
        subagent_id,
        admitted_authority,
        ownership,
        ..
    } = &event.event
    {
        let mut keys = vec![(format!("subagent:{subagent_id}"), false)];
        if admitted_authority.is_none()
            && *ownership == crate::events::types::SubagentOwnershipKind::Normal
        {
            keys.push((format!("agent-admission:{subagent_id}"), true));
        }
        return keys;
    }
    if let RuntimeEvent::SubagentTerminalPublished { subagent_id, .. } = &event.event {
        return vec![(format!("subagent:{subagent_id}"), true)];
    }
    if let RuntimeEvent::SubagentTerminalSettled { subagent_id, .. } = &event.event {
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
    if let RuntimeEvent::InteractionRequested {
        interaction_id,
        subject,
    } = &event.event
    {
        let mut keys = vec![(format!("interaction:{interaction_id}"), false)];
        if let InteractionSubject::Approval {
            invocation_id: id @ crate::tools::types::ToolInvocationId::Workflow { .. },
            ..
        } = subject
        {
            keys.push((native_approval_key(id), true));
        }
        return keys;
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
            | RuntimeEvent::SubagentTerminalSettled { .. }
            | RuntimeEvent::InteractionSettled { .. }
    )
}

/// Seed irrelevant text-Assistant history for read-cost regressions without
/// reconstructing and validating the whole growing Surface on every append.
/// Native Ledger, Surface and Journal writers still own every persisted fact.
#[cfg(test)]
pub(crate) fn append_assistant_history_fixture(
    store: &SqliteConversationStore,
    history: &[(
        crate::message::types::AssistantMessageBlock,
        RuntimeEventEnvelope,
    )],
) -> Result<(), ConversationStoreError> {
    let mut connection = store.lock()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage(format!("Assistant history fixture: {error}")))?;
    ensure_surface_head(&transaction)?;
    let mut head = load_head(&transaction)?;
    for (assistant, event) in history {
        if !assistant
            .content
            .iter()
            .all(|block| matches!(block, AssistantContentBlock::Text(_)))
        {
            return Err(storage("the Assistant history fixture accepts only text"));
        }
        let message = MessageBlock::Assistant(assistant.clone());
        validate_canonical_event_for_message(&message, &event.event)?;
        append_message_ledger(&transaction, &message, None)?;
        head.active_message_ids.push(assistant.id.clone());
        head.revision = head.revision.next();
        append_surface_op(
            &transaction,
            head.revision,
            head.compaction_generation,
            &SurfaceOp::Append {
                message_id: assistant.id.clone(),
            },
        )?;
        persist_event_tx(&transaction, store.conversation_id(), event.clone())?;
    }
    update_surface_head(&transaction, head.revision, head.compaction_generation)?;
    // Verify the same final durable invariants as ordinary appends, once.
    load_head(&transaction)?;
    transaction
        .commit()
        .map_err(|error| storage(format!("Assistant history fixture commit: {error}")))
}

const TRANSCRIPT_ATTEMPTS_SQL: &str = "SELECT attempt_id FROM events INDEXED BY events_message_id_kind_idx WHERE json_extract(event_json,'$.event.message_id')=?1 AND json_extract(event_json,'$.event.type')='assistant_message_committed' AND sequence<=?2";
const INHERITED_OCCUPANCY_SQL: &str = "SELECT s.bootstrap_initialized,b.message_count,b.history_digest,b.occupancy_json,b.occupancy_digest FROM rustx_store s LEFT JOIN bootstrap_identity b ON b.id=s.id WHERE s.id=1";

fn inherited_occupancy_digest(count: u64, history_digest: &str, occupancy: &str) -> String {
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(b"rustx-inherited-occupancy-v1\n");
    hasher.update(count.to_be_bytes());
    hasher.update(history_digest.as_bytes());
    hasher.update(occupancy.as_bytes());
    let mut digest = String::new();
    for byte in hasher.finalize() {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

fn read_inherited_occupancy(
    connection: &Connection,
) -> Result<Option<crate::context::occupancy::ContextOccupancy>, ConversationStoreError> {
    let (initialized, count, history, json, digest): (
        i64,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(INHERITED_OCCUPANCY_SQL, [], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(|error| storage(format!("bootstrap occupancy: {error}")))?;
    let (count, history, json, digest) = match (initialized, count, history, json, digest) {
        (0, None, None, None, None) => return Ok(None),
        (1, Some(count), Some(history), Some(json), Some(digest)) if count >= 0 => (
            u64::try_from(count).map_err(|_| storage("negative bootstrap message count"))?,
            history,
            json,
            digest,
        ),
        _ => {
            return Err(storage(
                "missing or inconsistent immutable bootstrap metadata",
            ));
        }
    };
    if digest != inherited_occupancy_digest(count, &history, &json) {
        return Err(storage("inconsistent immutable bootstrap occupancy"));
    }
    let reading: Option<crate::context::occupancy::ContextOccupancy> =
        decode(&json, "bootstrap occupancy")?;
    if let Some(reading) = &reading {
        reading.validate()?;
    }
    Ok(reading)
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

/// Binds normalized metadata to the exact immutable selected-input root.
/// This is an internal storage integrity digest, not a replacement for the
/// original flat provider-semantic or canonical-body SHA identities.
fn stored_snapshot_digest(json: &str, root: Option<manifest::NodeRef>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"rustx-normalized-request-v1\0");
    digest.update((json.len() as u64).to_le_bytes());
    digest.update(json.as_bytes());
    if let Some(root) = root {
        digest.update([1]);
        digest.update(root.id.to_le_bytes());
        digest.update(root.digest);
        digest.update(root.count.to_le_bytes());
        digest.update([root.height]);
    } else {
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

fn read_stored_snapshot_root(
    connection: &Connection,
    request: &RequestId,
) -> Result<Option<manifest::NodeRef>, ConversationStoreError> {
    type RootRow = (Option<u64>, Option<Vec<u8>>, u64, Option<u8>, bool);
    let row: Option<RootRow> = connection.prepare_cached(
        "SELECT r.input_root,CASE WHEN length(r.input_root_digest)=32 THEN r.input_root_digest END,r.input_count,n.height,r.input_root_digest IS NULL
         FROM request_snapshots r LEFT JOIN native_input_nodes n ON n.id=r.input_root WHERE r.request_id=?1",
    ).map_err(|error| storage(format!("request input root lookup: {error}")))?
        .query_row([request.as_str()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        .optional().map_err(|error| storage(format!("request input root row: {error}")))?;
    let row = row.ok_or_else(|| ConversationStoreError::RequestNotFound(request.clone()))?;
    match row {
        (None, None, 0, None, true) => Ok(None),
        (Some(id), Some(digest), count, Some(height), false)
            if id > 0
                && (1..=manifest::MAX_ITEMS).contains(&count)
                && (1..=manifest::MAX_HEIGHT).contains(&height) =>
        {
            let root = manifest::NodeRef {
                id,
                digest: digest.try_into().map_err(|_| {
                    ConversationStoreError::InvalidReference(
                        "native request input root digest".into(),
                    )
                })?,
                count,
                height,
            };
            // One root and its direct children establish the finite native
            // geometry. No selected leaf body is needed for scalar callers.
            manifest::node(connection, &root)?;
            Ok(Some(root))
        }
        _ => Err(ConversationStoreError::InvalidReference(
            "native request input root has an invalid closed shape".into(),
        )),
    }
}

fn encode_snapshot(snapshot: &RequestSnapshot) -> Result<String, ConversationStoreError> {
    struct SnapshotBytes(Vec<u8>);
    impl std::io::Write for SnapshotBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self
                .0
                .len()
                .checked_add(bytes.len())
                .is_none_or(|size| size > 64 * 1024 * 1024)
            {
                return Err(std::io::Error::other(
                    "frozen request exceeds storage byte budget",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // The selected sequence has its own immutable native root. Cloning this
    // handle is O(1); metadata never serializes or copies historical leaves.
    let mut metadata = snapshot.clone();
    metadata.input.clear();
    let mut writer = SnapshotBytes(Vec::new());
    serde_json::to_writer(&mut writer, &metadata)
        .map_err(|error| storage(format!("request snapshot encoding: {error}")))?;
    String::from_utf8(writer.0).map_err(|error| storage(error.to_string()))
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

fn verify_witness_field_order(connection: &Connection) -> Result<(), ConversationStoreError> {
    const REQUEST_WITNESS: &[&str] = &[
        "input_root",
        "input_root_digest",
        "input_count",
        "canonical_frontier",
        "manifest_digest",
        "started_sequence",
        "completed_sequence",
    ];
    for (table, original, fields) in [
        (
            "request_snapshots",
            "manifest_digest",
            &[
                "input_root",
                "input_root_digest",
                "input_count",
                "started_sequence",
                "completed_sequence",
            ] as &[&str],
        ),
        ("request_snapshots", "previous_request_id", REQUEST_WITNESS),
        ("request_snapshots", "request_id", REQUEST_WITNESS),
        ("request_snapshots", "snapshot_json", REQUEST_WITNESS),
        (
            "request_trace_catalogs",
            "provisional_message_id",
            &["tool_count"] as &[&str],
        ),
        (
            "canonical_tool_calls",
            "assistant_message_id",
            &[
                "observation_tool_identity_digest",
                "observation_definition_ready",
                "observation_definition_start",
                "observation_definition_commit",
                "observation_definition_ordinal",
            ] as &[&str],
        ),
        (
            "message_ledger",
            "message_id",
            &[
                "observation_identity_digest",
                "observation_delivery_success",
                "observation_delivery_total",
            ] as &[&str],
        ),
        (
            "inherited_message_membership",
            "assistant_message_id",
            &[
                "observation_identity_digest",
                "observation_membership_witness",
                "assistant_ledger_position",
                "turn_ordinal",
                "response_ordinal",
                "membership_digest",
            ] as &[&str],
        ),
    ] {
        let original_position: usize = connection
            .query_row(
                "SELECT cid FROM pragma_table_info(?1) WHERE name=?2",
                params![table, original],
                |row| row.get(0),
            )
            .map_err(|_| {
                ConversationStoreError::IncompatibleSchema(
                    "missing original witness identity column".into(),
                )
            })?;
        for field in fields {
            let position: usize = connection
                .query_row(
                    "SELECT cid FROM pragma_table_info(?1) WHERE name=?2",
                    params![table, field],
                    |row| row.get(0),
                )
                .map_err(|_| {
                    ConversationStoreError::IncompatibleSchema(
                        "missing finite read witness column".into(),
                    )
                })?;
            if position >= original_position {
                return Err(ConversationStoreError::IncompatibleSchema(
                    "finite read witness follows original identity body".into(),
                ));
            }
        }
    }
    Ok(())
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

#[cfg(test)]
mod pending_transaction_tests;

#[cfg(test)]
mod tests {
    mod append_head_tests {
        use super::*;
        include!("sqlite/append_head_tests.rs");
    }
    include!("sqlite/observation_trace_tests.rs");
    include!("sqlite/observation_inspection_tests.rs");
    include!("sqlite/observation_occupancy_tests.rs");
    include!("sqlite/observation_statistics_tests.rs");
    include!("sqlite/observation_member_tests.rs");
    include!("sqlite/observation_owner_tests.rs");
    include!("sqlite/observation_selected_source_tests.rs");
    include!("sqlite/observation_occupancy_origin_tests.rs");
    include!("sqlite/observation_projection_tests.rs");
    mod observation_event_probe_tests {
        use super::*;
        include!("sqlite/observation_event_probe_tests.rs");
    }
    mod initialization_state_tests {
        use super::*;
        include!("sqlite/initialization_state_tests.rs");
    }

    mod manifest_store_tests {
        use super::*;
        include!("sqlite/manifest_store_tests.rs");
    }

    mod request_fragment_tests {
        use super::*;
        include!("sqlite/request_fragment_tests.rs");
    }
    mod request_header_capture_tests {
        use super::*;
        include!("sqlite/request_header_capture_tests.rs");
    }
    mod request_span_tests {
        use super::*;
        include!("sqlite/request_span_tests.rs");
    }
    mod request_span_integrity_tests {
        use super::*;
        include!("sqlite/request_span_integrity_tests.rs");
    }
    mod request_start_validation_tests {
        use super::*;
        include!("sqlite/request_start_validation_tests.rs");
    }
    mod request_probe_tests {
        use super::*;
        include!("sqlite/request_probe_tests.rs");
    }
    mod request_snapshot_validation_tests {
        use super::*;
        include!("sqlite/request_snapshot_validation_tests.rs");
    }
    mod request_retry_tests {
        use super::*;
        include!("sqlite/request_retry_tests.rs");
    }
    mod indexed_fact_tests {
        use super::*;
        include!("sqlite/indexed_fact_tests.rs");
    }
    use super::*;

    use crate::context::assembly::ContextGeneration;
    use crate::conversation::ConversationState;
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope, SubagentTerminalState};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AgentStatusGenerationMetadata, AgentStatusModuleId, AssistantContentBlock,
        AssistantMessageBlock, ContextKind, ContributionEmission, InboundKind, MessageBlock,
        ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::catalog::ModelCapabilities;
    use crate::model::catalog::ModelCompat;
    use crate::model::finish::ModelFinishReason;
    use crate::model::invocation::{ModelInvocationConfig, RequestParams};
    use crate::model::snapshot::{ContributionStart, RequestIdentity, RequestSnapshot};
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::identity::{AttemptId, EventId, TurnId};
    use crate::runtime::types::CancellationReason;
    use crate::tools::types::{
        ToolCall, ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus,
    };
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
        SqliteConversationStore::in_memory(ConversationId::new(
            "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
        ))
        .unwrap()
    }

    #[test]
    fn compaction_resource_paging_never_observes_future_lifecycle_or_ledger_growth() {
        use crate::context::resources::{
            CompactionResources, JournalFrontier, LedgerFrontier, ResourceLimits,
        };
        let store = store();
        let conversation = store.conversation_id().clone();
        let attempt = AttemptId::new("resource-attempt");
        store
            .append_event(envelope(
                &conversation,
                "resource-start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        store
            .append_canonical(&user_message("before", "immutable before cut"))
            .unwrap();
        let mut resources = CompactionResources::capture(
            &store,
            ResourceLimits {
                page_items: 1,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        let frozen = resources.cut().clone();
        // The first append commits before capture; the terminal and next message
        // commit after capture, before the delayed page query. SQL's <= cut is
        // the read linearization boundary, not query time or hot lifecycle state.
        store
            .append_event(envelope(
                &conversation,
                "resource-terminal",
                Some(attempt.clone()),
                RuntimeEvent::AttemptCompleted {
                    attempt_id: attempt,
                    finish_reason: ModelFinishReason::Stop,
                },
            ))
            .unwrap();
        store
            .append_canonical(&user_message("after", "future ledger material"))
            .unwrap();
        let events = resources.events(JournalFrontier(0)).unwrap();
        assert_eq!(events.rows.len(), 1);
        assert!(matches!(
            events.rows[0].1.event,
            RuntimeEvent::AttemptStarted { .. }
        ));
        assert!(resources.events(frozen.journal).unwrap().rows.is_empty());
        let originals = resources.messages(LedgerFrontier(0)).unwrap();
        assert_eq!(originals.rows[0].1.id(), &MessageId::new("before"));
        assert!(resources.messages(frozen.ledger).unwrap().rows.is_empty());
        assert!(resources.originals(&[MessageId::new("after")]).is_err());
    }

    #[test]
    fn compaction_resource_memory_bounds_include_all_pages_and_oversized_single_records() {
        use crate::context::resources::{CompactionResources, LedgerFrontier, ResourceLimits};
        let store = store();
        store
            .append_canonical(&user_message("one", "first"))
            .unwrap();
        store
            .append_canonical(&user_message("two", "second"))
            .unwrap();
        let mut resources = CompactionResources::capture(
            &store,
            ResourceLimits {
                page_items: 1,
                total_items: 1,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        let first = resources.messages(LedgerFrontier(0)).unwrap();
        assert!(
            resources
                .messages(LedgerFrontier(first.rows[0].0))
                .unwrap_err()
                .to_string()
                .contains("cumulative")
        );
        let mut tiny = CompactionResources::capture(
            &store,
            ResourceLimits {
                record_bytes: 8,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        assert!(
            tiny.messages(LedgerFrontier(0))
                .unwrap_err()
                .to_string()
                .contains("byte bound")
        );
        let mut repeated = CompactionResources::capture(
            &store,
            ResourceLimits {
                total_bytes: 256,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        repeated.originals(&[MessageId::new("one")]).unwrap();
        assert!(
            repeated
                .originals(&[MessageId::new("one")])
                .unwrap_err()
                .to_string()
                .contains("byte bound")
        );
    }

    #[test]
    fn compaction_byte_pages_resume_without_decoding_the_next_record() {
        use crate::context::resources::{CompactionResources, LedgerFrontier, ResourceLimits};
        let store = store();
        let rows = (1..=3)
            .map(|i| user_message(&format!("M{i}"), &"x".repeat(128)))
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&rows[0]).unwrap().len();
        store.initialize(&rows).unwrap();
        let mut resources = CompactionResources::capture(
            &store,
            ResourceLimits {
                record_bytes: bytes + 16,
                page_bytes: bytes + 16,
                total_bytes: bytes * 3 + 64,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        compaction_resources::take_body_decodes();
        let mut after = LedgerFrontier(0);
        for expected in &rows {
            let page = resources.messages(after).unwrap();
            assert_eq!(page.rows.len(), 1);
            assert_eq!(&page.rows[0].1, expected);
            after = LedgerFrontier(page.rows[0].0);
            assert_eq!(
                compaction_resources::take_body_decodes(),
                1,
                "byte lookahead never hydrates the next page's record"
            );
        }
        assert!(resources.messages(after).unwrap().rows.is_empty());
    }

    #[test]
    fn compaction_tool_lifecycle_at_cut_never_reads_later_completion() {
        use crate::context::resources::{CompactionResources, JournalFrontier, ResourceLimits};
        let store = store();
        let call_id = ToolCallId::new("cut-tool-call");
        let tool_id = ToolId::new("cut-tool");
        store
            .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("cut-owner"),
                content: vec![AssistantContentBlock::ToolCall(ToolCall {
                    id: call_id.clone(),
                    tool_id: tool_id.clone(),
                    name: "read".into(),
                    arguments: serde_json::json!({"path":"file.rs"}),
                })],
            }))
            .unwrap();
        store
            .append_event(envelope(
                store.conversation_id(),
                "cut-tool-started",
                None,
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call_id.clone(),
                    tool_id: tool_id.clone(),
                },
            ))
            .unwrap();
        let mut resources = CompactionResources::capture(
            &store,
            ResourceLimits {
                page_items: 1,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        let frozen = resources.cut().clone();
        store
            .append_event(envelope(
                store.conversation_id(),
                "cut-tool-completed",
                None,
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: call_id,
                    tool_id,
                    result: crate::tools::types::ToolExecutionResult {
                        workflow: None,
                        status: ToolExecutionStatus::Success,
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        deliveries: Vec::new(),
                        truncation: None,
                        managed_output: None,
                    },
                },
            ))
            .unwrap();
        let page = resources.events(JournalFrontier(0)).unwrap();
        assert!(matches!(
            page.rows[0].1.event,
            RuntimeEvent::ToolExecutionStarted { .. }
        ));
        assert!(resources.events(frozen.journal).unwrap().rows.is_empty());
        assert!(matches!(
            store.read_events(Some(frozen.journal.0), 1).unwrap().events[0].event,
            RuntimeEvent::ToolExecutionCompleted { .. }
        ));
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

    fn agent_status_message(
        id: &str,
        generated_at: DateTime<Utc>,
        modules: &[AgentStatusModuleId],
    ) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "<system-reminder>bounded status</system-reminder>".to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::Context(ContextKind::AgentStatus(
                AgentStatusGenerationMetadata::new(generated_at, modules.to_vec())
                    .expect("valid Agent Status metadata"),
            )),
            timestamp: None,
        })
    }

    fn assistant_message(id: &str, text: &str) -> MessageBlock {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: vec![AssistantContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
        })
    }

    #[test]
    fn snapshot_transcript_excludes_later_tool_results() {
        use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef};
        let store = store();
        let call = ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-bash"),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        };
        let owner = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("a"),
            content: vec![
                AssistantContentBlock::Text(TextBlock {
                    text: "thinking".into(),
                }),
                AssistantContentBlock::ToolCall(call.clone()),
            ],
        });
        store.initialize(&[owner]).unwrap();
        let valid = ToolMessageBlock {
            id: MessageId::new("result"),
            occurrence: ToolCallOccurrenceRef::new(MessageId::new("a"), ContentBlockIndex::new(1)),
            tool_call_id: call.id,
            tool_id: call.tool_id,
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: vec![],
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        };
        let cut = super::super::inbox::TranscriptSnapshotCut {
            bootstrap_through: store.load_transcript_page(None, 1).unwrap().entries[0]
                .cursor
                .get(),
            journal_through: 0,
            messages: vec![MessageId::new("a")],
            publications: Vec::new(),
            pending: Vec::new(),
        };
        store
            .append_canonical(&MessageBlock::Tool(valid.clone()))
            .unwrap();
        let historical = store.load_transcript_snapshot(&cut).unwrap();
        assert_eq!(historical.entries.len(), 1);
        assert!(
            historical.entries[0].tool_calls[0].result.is_none(),
            "later Tool association cannot enter captured membership"
        );
    }

    #[test]
    fn canonical_tool_results_validate_exact_ownership_and_roll_back() {
        use crate::message::types::{ContentBlockIndex, ToolCallOccurrenceRef};
        let store = store();
        let call = ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-bash"),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        };
        let owner = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("a"),
            content: vec![
                AssistantContentBlock::Text(TextBlock {
                    text: "thinking".into(),
                }),
                AssistantContentBlock::ToolCall(call.clone()),
            ],
        });
        store.initialize(&[owner]).unwrap();
        let valid = ToolMessageBlock {
            id: MessageId::new("result"),
            occurrence: ToolCallOccurrenceRef::new(MessageId::new("a"), ContentBlockIndex::new(1)),
            tool_call_id: call.id,
            tool_id: call.tool_id,
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: vec![],
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        };
        let mut missing = valid.clone();
        missing.occurrence.assistant_message_id = MessageId::new("missing");
        let mut text = valid.clone();
        text.occurrence.block_index = ContentBlockIndex::new(0);
        let mut outside = valid.clone();
        outside.occurrence.block_index = ContentBlockIndex::new(2);
        let mut wrong_call = valid.clone();
        wrong_call.tool_call_id = ToolCallId::new("wrong");
        let mut wrong_tool = valid.clone();
        wrong_tool.tool_id = ToolId::new("wrong");
        for invalid in [missing, text, outside, wrong_call, wrong_tool] {
            assert!(
                store
                    .append_canonical(&MessageBlock::Tool(invalid))
                    .is_err()
            );
            assert_eq!(
                store.load_canonical().unwrap().len(),
                1,
                "failed result leaves no ledger row"
            );
            assert!(
                store.load_transcript_page(None, 1).unwrap().entries[0].tool_calls[0]
                    .result
                    .is_none()
            );
        }
        store
            .append_canonical(&MessageBlock::Tool(valid.clone()))
            .unwrap();
        let mut duplicate = valid.clone();
        duplicate.id = MessageId::new("duplicate");
        assert!(
            store
                .append_canonical(&MessageBlock::Tool(duplicate))
                .is_err()
        );
        // Reusing one result MessageId for a second exact occurrence is also invalid.
        let second = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("b"),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: valid.tool_call_id.clone(),
                tool_id: valid.tool_id.clone(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })],
        });
        store.append_canonical(&second).unwrap();
        let mut reused = valid;
        reused.occurrence =
            ToolCallOccurrenceRef::new(MessageId::new("b"), ContentBlockIndex::new(0));
        assert!(store.append_canonical(&MessageBlock::Tool(reused)).is_err());
        assert_eq!(store.load_canonical().unwrap().len(), 3);
        assert!(
            store.load_transcript_page(None, 1).unwrap().entries[0].tool_calls[0]
                .result
                .is_none()
        );
    }

    #[test]
    fn transcript_tool_reads_seek_occurrences_and_result_message_indexes() {
        let store = store();
        let connection = store.conn.lock().unwrap();
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {TRANSCRIPT_TOOLS_SQL}"))
            .unwrap()
            .query_map(["assistant"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            plan.contains("SEARCH c USING INDEX sqlite_autoindex_canonical_tool_calls_2"),
            "{plan}"
        );
        assert!(
            plan.contains("SEARCH l USING INDEX sqlite_autoindex_message_ledger_1"),
            "{plan}"
        );
        assert!(
            !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
            "{plan}"
        );
    }

    #[test]
    fn transcript_tool_obsolete_schemas_are_refused_without_backfill() {
        for obsolete in [38, 39] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("old.sqlite");
            let id = ConversationId::new("conv_a2b8feb8-c105-7980-8a72-d8482750c3fc");
            {
                let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
                store.conn.lock().unwrap().execute_batch(&format!("DROP TABLE canonical_tool_calls; UPDATE rustx_store SET schema_version={obsolete} WHERE id=1")).unwrap();
            }
            assert!(matches!(
                SqliteConversationStore::open(id, &path),
                Err(ConversationStoreError::SchemaVersionMismatch {
                    stored,
                    expected: SQLITE_SCHEMA_VERSION
                }) if stored == obsolete
            ));
            let connection = Connection::open(path).unwrap();
            assert_eq!(
                connection
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE name='canonical_tool_calls'",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One retired-history seed and canonical occurrence contract.
    fn transcript_tool_lineage_seed_keeps_retired_reused_call_occurrences() {
        let call = ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool-bash"),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        };
        let assistant = |id| {
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(id),
                content: vec![AssistantContentBlock::ToolCall(call.clone())],
            })
        };
        let result = |id, owner: &str, text: &str| {
            MessageBlock::Tool(ToolMessageBlock {
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    crate::runtime::identity::MessageId::new(owner),
                    crate::message::types::ContentBlockIndex::new(0),
                ),
                id: MessageId::new(id),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Success,
                    content: vec![crate::tools::types::ToolResultContent::Text(
                        crate::message::content::TextBlock { text: text.into() },
                    )],
                    duration_ms: 1,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            })
        };
        let canonical = vec![assistant("a"), result("ra", "a", "old-result")];
        let store = store();
        store.initialize(&canonical).unwrap();
        store
            .commit_projection(
                crate::scripted_suites::support::context::checkpoint_commit_fixture(
                    &store,
                    crate::scripted_suites::support::context::checkpoint_fixture(
                        &store,
                        "P1",
                        2,
                        "retired A",
                    ),
                    None,
                ),
            )
            .unwrap();
        store.append_canonical(&assistant("b")).unwrap();
        let unresolved = crate::runtime_client::snapshot::transcript_page_view(
            store.load_transcript_page(None, 1).unwrap(),
        )
        .unwrap();
        let current = &unresolved.entries[0].tool_calls[0];
        assert_eq!(current.message_id, Some(MessageId::new("b")));
        assert!(matches!(
            current.state,
            crate::runtime_client::snapshot::ForegroundToolState::Assembled { .. }
        ));
        assert!(
            !serde_json::to_string(current)
                .unwrap()
                .contains("old-result")
        );
        store
            .append_canonical(&result("rb", "b", "new-result"))
            .unwrap();
        let page = store.load_transcript_page(None, 10).unwrap();
        let tools: Vec<_> = page
            .entries
            .iter()
            .flat_map(|entry| &entry.tool_calls)
            .collect();
        assert_eq!(tools.len(), 2);
        for (tool, owner, text) in [(tools[0], "a", "old-result"), (tools[1], "b", "new-result")] {
            assert_eq!(tool.message_id, MessageId::new(owner));
            assert_eq!(
                tool.result.as_ref().unwrap().content,
                vec![crate::tools::types::ToolResultContent::Text(
                    crate::message::content::TextBlock { text: text.into() }
                )]
            );
        }
    }

    #[test]
    fn agent_transcript_tools_resolve_native_results_across_page_boundaries() {
        let store = store();
        store.initialize(&[user_message("u", "Run")]).unwrap();
        let calls = ["first", "second"].map(|id| ToolCall {
            id: ToolCallId::new(id),
            tool_id: ToolId::new("tool-bash"),
            name: "bash".into(),
            arguments: serde_json::json!({"command": id}),
        });
        store
            .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("assistant"),
                content: calls
                    .iter()
                    .cloned()
                    .map(AssistantContentBlock::ToolCall)
                    .collect(),
            }))
            .unwrap();
        let before = store.load_transcript_page(None, 1).unwrap();
        assert_eq!(before.entries[0].tool_calls.len(), 2);
        assert!(
            before.entries[0]
                .tool_calls
                .iter()
                .all(|tool| tool.result.is_none())
        );
        let unresolved =
            crate::runtime_client::snapshot::transcript_page_view(before.clone()).unwrap();
        for (index, tool) in unresolved.entries[0].tool_calls.iter().enumerate() {
            assert_eq!(tool.message_id, Some(MessageId::new("assistant")));
            assert_eq!(tool.block_index.get() as usize, index);
            assert!(matches!(
                tool.state,
                crate::runtime_client::snapshot::ForegroundToolState::Assembled { .. }
            ));
        }
        // Physical/result publication order differs from canonical call order.
        for (index, call) in calls.iter().enumerate().rev() {
            store
                .append_canonical(&MessageBlock::Tool(ToolMessageBlock {
                    occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                        crate::runtime::identity::MessageId::new("assistant"),
                        crate::message::types::ContentBlockIndex::new(
                            u32::try_from(index).unwrap(),
                        ),
                    ),
                    id: MessageId::new(format!("result-{index}")),
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    result: ToolExecutionResult {
                        deliveries: Vec::new(),
                        status: if index == 0 {
                            ToolExecutionStatus::Success
                        } else {
                            ToolExecutionStatus::Failed {
                                error: "native failure".into(),
                            }
                        },
                        content: Vec::new(),
                        duration_ms: 1,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    },
                }))
                .unwrap();
        }
        let tail = store.load_transcript_page(None, 2).unwrap();
        let older = store.load_transcript_page(tail.next_cursor, 1).unwrap();
        assert_eq!(older.entries[0].cursor, before.entries[0].cursor);
        let tools = &older.entries[0].tool_calls;
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.call.id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert!(matches!(
            tools[0].result.as_ref().unwrap().status,
            ToolExecutionStatus::Success
        ));
        assert!(matches!(
            tools[1].result.as_ref().unwrap().status,
            ToolExecutionStatus::Failed { .. }
        ));
        let original_results: Vec<_> = tools.iter().map(|tool| tool.result.clone()).collect();
        let projected = crate::runtime_client::snapshot::transcript_page_view(older).unwrap();
        // Canonical result acceptance is positive C evidence. No execution J
        // fact occurred in this fixture, so it cannot settle the lifecycle.
        for (tool, expected) in projected.entries[0].tool_calls.iter().zip(original_results) {
            assert!(matches!(
                tool.state,
                crate::runtime_client::snapshot::ForegroundToolState::Assembled { .. }
            ));
            assert_eq!(tool.committed_result, expected);
            assert!(tool.result_source.is_none());
        }
    }

    #[test]
    fn invalid_agent_status_metadata_is_rejected_by_durable_decode() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let conversation_id = ConversationId::new("conv_45cf2e82-15bc-7eb9-8052-70016ae024aa");
        let message_id = MessageId::new("status-invalid-after-persist");
        let status = MessageBlock::User(UserMessageBlock {
            id: message_id.clone(),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "renderer text is irrelevant".to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::Context(ContextKind::AgentStatus(
                AgentStatusGenerationMetadata::new(
                    Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                    [AgentStatusModuleId::Time, AgentStatusModuleId::Background],
                )
                .expect("valid Agent Status metadata"),
            )),
            timestamp: None,
        });

        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store.initialize(&[]).unwrap();
            store.append_canonical(&status).unwrap();
            let reloaded = store
                .load_messages(std::slice::from_ref(&message_id))
                .unwrap();
            assert_eq!(reloaded, vec![status.clone()]);
        }

        let mut encoded = serde_json::to_value(&status).unwrap();
        encoded["kind"]["context"]["agent_status"]["modules"] = serde_json::json!([]);
        let encoded = serde_json::to_string(&encoded).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE message_ledger SET message_json=?1 WHERE message_id=?2",
                params![encoded, message_id.as_str()],
            )
            .unwrap();
        drop(connection);

        let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
        let error = reopened
            .load_messages(std::slice::from_ref(&message_id))
            .expect_err("invalid canonical metadata must fail closed");
        assert!(matches!(
            error,
            ConversationStoreError::Storage(message)
                if message.contains("decode Ledger message")
        ));
    }

    #[test]
    fn historical_context_alone_cannot_advance_contribution_receipts() {
        let store = store();
        store.initialize(&[user_message("base", "base")]).unwrap();
        let status = agent_status_message(
            "historical-status",
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[AgentStatusModuleId::Todo],
        );
        store.append_canonical(&status).unwrap();
        assert_eq!(store.current_logical_step_progress().unwrap(), 0);
        assert!(
            store
                .read_request_snapshots(None, 10)
                .unwrap()
                .snapshots
                .is_empty()
        );
        assert!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::AgentStatus
                    ),
                    "active_actionable"
                )
                .unwrap()
                .is_none()
        );
    }

    fn envelope(
        conversation_id: &ConversationId,
        event_id: &str,
        attempt_id: Option<AttemptId>,
        event: RuntimeEvent,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
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
    fn wal_management_head_is_one_snapshot_while_another_writer_commits() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let id = ConversationId::generate();
        let writer = Arc::new(SqliteConversationStore::open(id.clone(), &path).unwrap());
        let reader = SqliteConversationStore::open_existing(id, &path).unwrap();
        let initial = reader.load_head().unwrap();
        let observed = Arc::new(AtomicBool::new(false));
        let committed = observed.clone();
        let competing = writer.clone();
        *reader.head_read_probe.lock().unwrap() = Some(Box::new(move || {
            let accepted = competing
                .accept_inbound(draft("during read snapshot"))
                .unwrap();
            competing
                .adopt_pending_batch(accepted.sequence, None)
                .unwrap();
            committed.store(true, Ordering::SeqCst);
        }));
        assert_eq!(reader.load_head().unwrap(), initial);
        assert!(
            observed.load(Ordering::SeqCst),
            "the interleaving gate must execute"
        );
        assert_eq!(reader.load_head().unwrap(), writer.load_head().unwrap());
        assert_eq!(reader.load_head().unwrap().active_message_ids.len(), 1);
    }

    #[test]
    fn acceptance_and_adoption_share_durable_identity() {
        let store = store();
        let accepted = store.accept_inbound(draft("hello")).unwrap();
        let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
        assert_eq!(adopted.items.len(), 1);
        assert!(store.load_pending().unwrap().is_empty());
        assert_eq!(store.load_head().unwrap().active_message_ids.len(), 1);
    }

    /// The Session lifecycle classification fact: durable acceptance is a
    /// monotonic watermark. Adoption moves the accepted work from Pending
    /// Inbound into the canonical Ledger and Surface, but never rewinds the
    /// acceptance itself — once a conversation has accepted user work, it
    /// has accepted user work forever.
    #[test]
    fn acceptance_watermark_is_monotonic_across_adoption() {
        let store = store();
        assert!(
            !store.has_accepted_inbound().unwrap(),
            "a fresh conversation has never accepted work"
        );

        let accepted = store.accept_inbound(draft("work")).unwrap();
        assert!(
            store.has_accepted_inbound().unwrap(),
            "the acceptance commit advances the watermark"
        );

        let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
        assert_eq!(adopted.items.len(), 1);
        assert!(store.load_pending().unwrap().is_empty());
        assert!(
            store.has_accepted_inbound().unwrap(),
            "adoption cannot rewind the acceptance fact"
        );
    }

    /// Seeding a lineage writes canonical and Surface history directly; it
    /// is not an inbound acceptance and must not raise the watermark. This
    /// is what keeps a lineage cut at an empty boundary honest: usage
    /// classification for those destinations is the catalog's provenance
    /// decision, never a store artifact.
    #[test]
    fn lineage_seeding_is_not_an_acceptance() {
        let store = store();
        store
            .initialize_lineage(&crate::durable::LineageSeed::history(vec![user_message(
                "seeded-user",
                "seeded history",
            )]))
            .unwrap();
        assert_eq!(store.load_canonical().unwrap().len(), 1);
        assert!(
            !store.has_accepted_inbound().unwrap(),
            "a seeded lineage carries history without any acceptance"
        );
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
    fn canonical_summary_payload_is_not_a_canonical_message() {
        let store = store();
        store.initialize(&[user_message("a", "A")]).unwrap();
        let input = serde_json::json!({"role":"user","id":"summary","source":"runtime","kind":{"compaction_summary":{}},"content":[]});
        assert!(serde_json::from_value::<MessageBlock>(input).is_err());
        assert_eq!(store.load_canonical().unwrap(), [user_message("a", "A")]);
        assert!(store.read_events(None, 64).unwrap().events.is_empty());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn immutable_projection_activations_survive_compactions_growth_and_restart() {
        use crate::scripted_suites::support::context::{
            checkpoint_commit_fixture, checkpoint_fixture,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let conversation = ConversationId::generate();
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        let history = vec![
            user_message("a", "A"),
            user_message("b", "B"),
            user_message("c", "C"),
        ];
        store.initialize(&history).unwrap();
        let first =
            checkpoint_commit_fixture(&store, checkpoint_fixture(&store, "P1", 1, "first"), None);
        let receipt1 = store.commit_projection(first.clone()).unwrap();
        store.append_canonical(&user_message("d", "D")).unwrap();
        let second =
            checkpoint_commit_fixture(&store, checkpoint_fixture(&store, "P2", 2, "second"), None);
        let receipt2 = store.commit_projection(second).unwrap();
        let mut replayed = store.commit_projection(first).unwrap();
        assert!(receipt1.newly_committed);
        assert!(!replayed.newly_committed);
        replayed.newly_committed = true;
        assert_eq!(replayed, receipt1);
        assert_eq!(
            store.projection_head().unwrap().unwrap(),
            receipt2.checkpoint
        );
        assert_eq!(store.load_canonical().unwrap().len(), 4);
        drop(store);
        let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
        let timeline = reopened
            .projection_timeline(crate::context::checkpoint::ProjectionActivationId(0), 64)
            .unwrap();
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[0], receipt1.checkpoint);
        assert_eq!(timeline[1], receipt2.checkpoint);
        let active = reopened.projected_history().unwrap();
        assert_eq!(
            active
                .messages
                .iter()
                .filter_map(|m| m.canonical_id())
                .cloned()
                .collect::<Vec<_>>(),
            [MessageId::new("c"), MessageId::new("d")]
        );
    }

    #[test]
    fn corrupted_checkpoint_content_manifest_fails_closed() {
        use crate::scripted_suites::support::context::{
            checkpoint_commit_fixture, checkpoint_fixture,
        };
        let store = store();
        store.initialize(&[user_message("a", "A")]).unwrap();
        store
            .commit_projection(checkpoint_commit_fixture(
                &store,
                checkpoint_fixture(&store, "P1", 1, "summary"),
                None,
            ))
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute("UPDATE projection_checkpoints SET candidate_json='{}'", [])
            .unwrap();
        assert!(store.projection_head().is_err());
        assert!(store.projected_history().is_err());
    }

    #[test]
    fn every_checkpoint_transaction_stage_fault_exposes_only_the_old_complete_state() {
        use crate::scripted_suites::support::context::{
            checkpoint_commit_fixture, checkpoint_fixture,
        };
        for fault in [
            CompactionFaultOperation::BeforeContentInsert,
            CompactionFaultOperation::AfterContentInsert,
            CompactionFaultOperation::AfterActivationInsert,
            CompactionFaultOperation::AfterHeadTransition,
            CompactionFaultOperation::BeforeCompletionInsert,
            CompactionFaultOperation::AfterCompletionInsert,
            CompactionFaultOperation::AfterReceiptInsert,
        ] {
            let store = store();
            let history = vec![user_message("a", "A"), user_message("b", "B")];
            store.initialize(&history).unwrap();
            let input = checkpoint_commit_fixture(
                &store,
                checkpoint_fixture(&store, "P1", 2, "summary"),
                None,
            );
            let fingerprint = input.candidate.fingerprint();
            store.arm_compaction_fault_script([fault]);
            assert!(store.commit_projection(input.clone()).is_err());
            assert_eq!(store.load_canonical().unwrap(), history);
            assert!(store.projection_head().unwrap().is_none());
            assert!(
                store
                    .projection_timeline(crate::context::checkpoint::ProjectionActivationId(0), 64)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                store
                    .projection_receipt(&input.operation_id, &fingerprint)
                    .unwrap()
                    .is_none()
            );
            assert!(store.read_events(None, 64).unwrap().events.is_empty());
            let receipt = store.commit_projection(input).unwrap();
            assert_eq!(
                store.projection_head().unwrap().unwrap(),
                receipt.checkpoint
            );
            assert_eq!(store.read_events(None, 64).unwrap().events.len(), 1);
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
        assert!(store.adopt_pending_batch(accepted.sequence, None,).is_err());
        assert_eq!(store.load_pending().unwrap().len(), 1);
        assert!(store.load_canonical().unwrap().is_empty());
        assert_eq!(
            store.load_head().unwrap().revision,
            SurfaceRevision::INITIAL
        );

        let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
        assert_eq!(adopted.items.len(), 1);
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
            .execute("UPDATE surface_head SET revision=2 WHERE id=1", [])
            .unwrap();
        drop(connection);
        assert!(matches!(
            store.load_head(),
            Err(ConversationStoreError::InvalidReference(detail))
                if detail.contains("canonical frontier metadata")
        ));
    }

    /// Freeze once before a test starts a request. Receipt replay uses this exact value.
    fn freeze_test_input(
        store: &SqliteConversationStore,
        snapshot: &mut RequestSnapshot,
        context: &[MessageBlock],
    ) {
        let history = store.projected_history().unwrap();
        let mut messages = history.messages.to_vec();
        messages.extend(crate::model::input::canonical_input(context));
        snapshot.history_cut = history.cut;
        snapshot
            .freeze_input(
                store.conversation_id(),
                &ModelRequest {
                    continuation_source: None,
                    images: crate::model::images::ResolvedImages::new(),
                    invocation: snapshot.invocation.clone(),
                    messages: messages.into(),
                    tools: snapshot.tool_definitions.clone(),
                    effective_system_prompt: snapshot.effective_system_prompt.clone(),
                    continuation: snapshot.continuation.clone(),
                },
            )
            .unwrap();
    }

    /// The frozen request snapshot used by the request-start tests.
    #[test]
    #[allow(clippy::too_many_lines)] // One native checkpoint/start/recovery proof for all four typed pairings.
    fn all_four_projection_tool_pairings_start_exact_requests_without_recovery_execution() {
        use crate::context::checkpoint::{
            CheckpointBlockIndex, CheckpointCallCoordinate, CheckpointItem, CheckpointMessage,
            CheckpointMessageOccurrence, ProjectedCallSource,
        };
        use crate::model::projected::{
            CanonicalMessageRef, DerivedModelMessage, ProjectedMessage, SemanticTransformation,
        };
        let call = crate::tools::types::ToolCall {
            id: ToolCallId::new("wire-call"),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".into(),
            arguments: serde_json::json!({}),
        };
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("physical-assistant"),
            content: vec![AssistantContentBlock::ToolCall(call.clone())],
        });
        let result = MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("physical-result"),
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                assistant.id().clone(),
                ContentBlockIndex::new(0),
            ),
            tool_call_id: call.id,
            tool_id: call.tool_id,
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: vec![crate::tools::types::ToolResultContent::Image(
                    crate::message::content::ImageReference {
                        artifact_id: crate::runtime::identity::ArtifactId::new("frozen-image"),
                        alt: Some("source image".into()),
                    },
                )],
                duration_ms: 1,
                exit_code: None,
                artifacts: vec![],
                deliveries: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        });
        for derived_call in [false, true] {
            for derived_result in [false, true] {
                let store = store();
                store
                    .initialize(&[assistant.clone(), result.clone()])
                    .unwrap();
                let canonical = store.load_canonical().unwrap();
                let mut candidate = crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "typed-pair",
                    2,
                    "unused",
                );
                candidate.summary_metadata = None;
                candidate.items = [&assistant, &result]
                    .into_iter()
                    .enumerate()
                    .map(|(index, message)| {
                        let derived = if index == 0 {
                            derived_call
                        } else {
                            derived_result
                        };
                        CheckpointItem {
                            message: if derived {
                                CheckpointMessage::Derived(Box::new(DerivedModelMessage {
                                    message: ProjectedMessage::from(message),
                                    sources: vec![CanonicalMessageRef::of(
                                        store.conversation_id(),
                                        message,
                                    )],
                                    checkpoint_sources: vec![],
                                    transformations: vec![SemanticTransformation::Summary],
                                }))
                            } else {
                                CheckpointMessage::Original(CanonicalMessageRef::of(
                                    store.conversation_id(),
                                    message,
                                ))
                            },
                            call_sources: if index == 0 && derived {
                                vec![ProjectedCallSource {
                                    block: CheckpointBlockIndex(0),
                                    assistant: CanonicalMessageRef::of(
                                        store.conversation_id(),
                                        &assistant,
                                    ),
                                    canonical_block: ContentBlockIndex::new(0),
                                }]
                            } else {
                                vec![]
                            },
                            result_call: (index == 1 && derived).then(|| {
                                CheckpointCallCoordinate {
                                    message: CheckpointMessageOccurrence {
                                        checkpoint_id: candidate.checkpoint_id.clone(),
                                        item_index: 0,
                                    },
                                    block: CheckpointBlockIndex(0),
                                }
                            }),
                        }
                    })
                    .collect();
                store
                    .commit_projection(
                        crate::scripted_suites::support::context::checkpoint_commit_fixture(
                            &store, candidate, None,
                        ),
                    )
                    .unwrap();
                crate::runtime::recovery::recover(&store, &crate::runtime::types::SystemClock)
                    .unwrap();
                assert_eq!(
                    store.load_canonical().unwrap(),
                    canonical,
                    "projection never fabricates recovery results"
                );
                assert!(store.unsettled_canonical_tool_calls(1).unwrap().is_empty());
                let history = store.projected_history().unwrap();
                let mut snapshot =
                    test_request_snapshot(store.load_head().unwrap().revision, invocation());
                snapshot.history_cut = history.cut.clone();
                let mut request = ModelRequest {
                    messages: history.messages.clone(),
                    images: crate::model::images::ResolvedImages::new(),
                    invocation: snapshot.invocation.clone(),
                    tools: snapshot.tool_definitions.clone(),
                    effective_system_prompt: snapshot.effective_system_prompt.clone(),
                    continuation: None,
                    continuation_source: None,
                };
                let mut projected = request.messages.to_vec();
                crate::model::images::omit_images(&mut projected);
                for (before, after) in history.messages.iter().zip(&mut projected) {
                    if before != after
                        && let Some(source) = before.as_canonical()
                    {
                        after.mark_transformed(
                            source,
                            store.conversation_id(),
                            SemanticTransformation::ImageDowngrade,
                        );
                    }
                }
                request.messages = projected.into();
                assert!(
                    request.messages[1].canonical_id().is_none(),
                    "media changes acquire request-local Derived identity"
                );
                snapshot
                    .freeze_input(store.conversation_id(), &request)
                    .unwrap();
                snapshot.tool_associations = crate::model::occurrence::freeze_associations(
                    &history,
                    &snapshot.request_id,
                    &request,
                    None,
                )
                .unwrap();
                store
                    .commit_model_turn_start(&[], &snapshot, Utc::now())
                    .unwrap();
                assert_eq!(
                    store
                        .reconstruct_model_request(&snapshot.request_id)
                        .unwrap(),
                    request
                );
                assert!(
                    store
                        .read_events(None, 64)
                        .unwrap()
                        .events
                        .iter()
                        .all(|e| !matches!(
                            e.event,
                            RuntimeEvent::ToolExecutionStarted { .. }
                                | RuntimeEvent::ToolExecutionCompleted { .. }
                        ))
                );
            }
        }
    }

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
            crate::context::resources::LedgerFrontier((revision).get()),
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

    /// Builds a fresh model-turn snapshot with an identity that is unique to
    /// the progress test which owns it. The request identity is intentionally
    /// the only progress input: the store, not this helper, assigns the
    /// durable Todo origin at commit time.
    fn progress_request_snapshot(
        attempt_id: &str,
        turn: &str,
        retry_number: u32,
        revision: crate::conversation::SurfaceRevision,
        request_context_ids: Vec<MessageId>,
    ) -> RequestSnapshot {
        RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new(attempt_id),
                turn: TurnId::new(turn),
                retry_number,
            },
            crate::context::resources::LedgerFrontier((revision).get()),
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

    pub(crate) fn hidden_context_audit_fixture() -> HiddenContextAuditFixture {
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936190");
        let store = Arc::new(SqliteConversationStore::in_memory(id).unwrap());
        store.initialize(&[]).unwrap();
        let snapshot = context_start_snapshot(store.load_head().unwrap().revision, Vec::new());
        let time = Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap();
        let started = store.commit_model_turn_start(&[], &snapshot, time).unwrap();
        store
            .mark_observation_ready(started.started.sequence, started.started.sequence)
            .unwrap();
        let start = PublicationStreamStart {
            stream_id: PublicationStreamId::for_request(
                &snapshot.identity.attempt_id,
                &snapshot.provisional_message_id,
            ),
            message_id: snapshot.provisional_message_id.clone(),
            attempt_id: snapshot.identity.attempt_id.clone(),
            turn_id: snapshot.identity.turn.clone(),
            request_id: snapshot.request_id.clone(),
        };
        store.open_publication_stream(&start).unwrap();
        store
            .stage_publication_frames(&[PublicationFrame {
                stream_id: start.stream_id.clone(),
                message_id: start.message_id,
                sequence: 0,
                payload: PublicationPayload::TextSuffix {
                    block_index: ContentBlockIndex::new(0),
                    suffix: "original audit prefix".into(),
                },
            }])
            .unwrap();
        let before = store.observation_read_cut().unwrap();
        let context = store
            .append_canonical(&request_context_message(
                "hidden-before-audit",
                "future context",
            ))
            .unwrap();
        assert!(context.transcript_cursor.is_none());
        let (_, audit) = store
            .terminalize_publication_audit(&start.stream_id, time)
            .unwrap();
        assert_eq!(audit.read_cut.ledger_through, context.ledger_position);
        HiddenContextAuditFixture {
            store,
            before,
            context,
            audit,
        }
    }

    fn status_start_snapshot_for(
        attempt_id: &str,
        turn: &str,
        revision: crate::conversation::SurfaceRevision,
        message_id: MessageId,
        emissions: Vec<ContributionEmission>,
        request_context_ids: Vec<MessageId>,
    ) -> RequestSnapshot {
        let mut snapshot =
            progress_request_snapshot(attempt_id, turn, 0, revision, request_context_ids);
        let producer = crate::runtime::identity::ContextContributorIdentity::Native(
            crate::runtime::identity::NativeContextContributor::AgentStatus,
        );
        snapshot
            .context_generation
            .contributors
            .push(crate::context::ContributorGeneration {
                identity: producer.clone(),
                attestation: None,
            });
        for id in snapshot.request_context_ids.clone() {
            if id != message_id {
                test_bind_contribution(
                    &mut snapshot,
                    id,
                    ContextKind::RuntimeToolObservation,
                    vec![],
                );
            }
        }
        snapshot.contributions.push(ContributionStart {
            message_id,
            producer,
            metadata: ContextKind::AgentStatus(
                AgentStatusGenerationMetadata::new(
                    Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                    vec![AgentStatusModuleId::Todo],
                )
                .unwrap(),
            ),
            presentation: None,
            emissions,
            opportunities: crate::context::ContributionOpportunities::default(),
            post_tool_batch_anchor: None,
        });
        snapshot
    }

    /// A request-start snapshot with a caller-chosen ordered context id set.
    fn context_start_snapshot(
        revision: crate::conversation::SurfaceRevision,
        request_context_ids: Vec<MessageId>,
    ) -> RequestSnapshot {
        let mut snapshot =
            progress_request_snapshot("attempt-1", "1", 0, revision, request_context_ids.clone());
        for message_id in request_context_ids {
            test_bind_contribution(
                &mut snapshot,
                message_id,
                ContextKind::RuntimeToolObservation,
                vec![],
            );
        }
        snapshot
    }

    fn test_bind_contribution(
        snapshot: &mut RequestSnapshot,
        message_id: MessageId,
        metadata: ContextKind,
        emissions: Vec<ContributionEmission>,
    ) {
        let producer = crate::runtime::identity::ContextContributorIdentity::Native(
            metadata.native_contribution_owner().unwrap().0,
        );
        if !snapshot
            .context_generation
            .contributors
            .iter()
            .any(|entry| entry.identity == producer)
        {
            snapshot
                .context_generation
                .contributors
                .push(crate::context::ContributorGeneration {
                    identity: producer.clone(),
                    attestation: None,
                });
        }
        snapshot.contributions.push(ContributionStart {
            message_id,
            producer,
            metadata,
            presentation: None,
            emissions,
            opportunities: crate::context::ContributionOpportunities::default(),
            post_tool_batch_anchor: None,
        });
    }

    fn status_start_snapshot(
        revision: crate::conversation::SurfaceRevision,
        message_id: MessageId,
        emissions: Vec<ContributionEmission>,
    ) -> RequestSnapshot {
        status_start_snapshot_for(
            "attempt-1",
            "1",
            revision,
            message_id.clone(),
            emissions,
            vec![message_id],
        )
    }

    #[test]
    fn request_start_is_atomic_and_reconstructs_from_durable_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let conversation_id = ConversationId::new("conv_cf107b20-dc7b-70e4-83e3-eea701e0da84");
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        let a = user_message("a", "A");
        let b = user_message("b", "B");
        store.initialize(&[a.clone(), b.clone()]).unwrap();
        let revision = store.load_head().unwrap().revision;
        let invocation = invocation();
        let mut snapshot = test_request_snapshot(revision, invocation.clone());
        let expected = ModelRequest {
            continuation_source: None,
            images: crate::model::images::ResolvedImages::new(),
            invocation: invocation.clone(),
            messages: crate::model::input::canonical_input(&[a.clone(), b.clone()]).into(),
            tools: vec![],
            effective_system_prompt: snapshot.effective_system_prompt.clone(),
            continuation: None,
        };
        snapshot.freeze_input(&conversation_id, &expected).unwrap();
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
        assert_eq!(started.started.sequence, 1);
        assert!(matches!(
            started.started.event,
            RuntimeEvent::ModelRequestStarted { ref request_id, .. } if request_id == &snapshot.request_id
        ));
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
                .started
                .sequence,
            1
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One atomicity/replay scenario and its complete durable observations.
    fn contribution_emission_is_atomic_with_start_and_exactly_replayed() {
        let store = store();
        let base = user_message("base", "base");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let revision = store.load_head().unwrap().revision.next();
        let message_id = MessageId::new("status-todo-1");
        let context = agent_status_message(
            message_id.as_str(),
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[AgentStatusModuleId::Todo],
        );
        let emission = ContributionEmission {
            key: "active_actionable".to_owned(),
            fingerprint: "todo-fingerprint-1".to_owned(),
        };
        let mut snapshot =
            status_start_snapshot(revision, message_id.clone(), vec![emission.clone()]);
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap();
        freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));

        let started = store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, timestamp)
            .expect("status and start commit together");
        assert_eq!(started.started.sequence, 1);
        assert_eq!(
            started.disposition,
            ModelTurnStartCommitDisposition::NewlyCommitted
        );
        let events = store.read_events(None, 10).unwrap().events;
        assert_eq!(events.len(), 2);
        assert_eq!(
            started.contribution_emissions,
            vec![events[1].clone()],
            "the typed receipt owns the exact emission fact after the start fact"
        );
        assert_eq!(
            started.events().map(Clone::clone).collect::<Vec<_>>(),
            events,
            "the receipt exposes the durable sequence in order"
        );
        match &events[1].event {
            RuntimeEvent::ContextContributionEmitted {
                request_id,
                message_id: stored_message_id,
                emission: stored_emission,
                logical_step_origin,
                ..
            } => {
                assert_eq!(request_id, &snapshot.request_id);
                assert_eq!(stored_message_id, &message_id);
                assert_eq!(stored_emission, &emission);
                assert_eq!(*logical_step_origin, 1);
            }
            other => panic!("expected contribution emission fact, got {other:?}"),
        }
        let head = store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus,
                ),
                "active_actionable",
            )
            .unwrap()
            .expect("durable latest-emission head");
        assert_eq!(
            head.producer,
            crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::AgentStatus
            )
        );
        assert_eq!(head.key, emission.key);
        assert_eq!(head.fingerprint, emission.fingerprint);
        assert_eq!(head.request_id, snapshot.request_id);
        assert_eq!(head.canonical_message_id, message_id);
        assert_eq!(head.event_sequence, events[1].sequence);

        let retried = store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, timestamp)
            .expect("exact start replay is idempotent");
        assert_eq!(retried.started, started.started);
        assert_eq!(
            retried.contribution_emissions,
            started.contribution_emissions
        );
        assert_eq!(
            retried.disposition,
            ModelTurnStartCommitDisposition::IdempotentReplay
        );
        assert_eq!(store.read_events(None, 10).unwrap().events.len(), 2);

        let mut contradictory = snapshot.clone();
        contradictory
            .contributions
            .first_mut()
            .expect("contribution")
            .emissions[0]
            .fingerprint = "contradictory-fingerprint".to_owned();
        assert!(matches!(
            store.commit_model_turn_start(
                std::slice::from_ref(&context),
                &contradictory,
                timestamp,
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(store.read_events(None, 10).unwrap().events.len(), 2);
        assert_eq!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::AgentStatus
                    ),
                    "active_actionable"
                )
                .unwrap()
                .expect("head remains committed")
                .fingerprint,
            emission.fingerprint
        );
    }

    /// The Todo cooldown origin is assigned inside the same transaction that
    /// installs request-scoped context and the canonical Agent Status. Two
    /// context appends therefore produce one semantic progress unit, and the
    /// committed reminder starts with zero elapsed progress of its own.
    #[test]
    fn todo_origin_is_assigned_at_the_post_context_start_boundary() {
        let store = store();
        let base = user_message("base", "base");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let base_revision = store.load_head().unwrap().revision;
        let runtime_context = request_context_message("runtime-observation", "tool observation");
        let status_message_id = MessageId::new("status-origin");
        let status = agent_status_message(
            status_message_id.as_str(),
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[AgentStatusModuleId::Todo],
        );
        let emission = ContributionEmission {
            key: "active_actionable".to_owned(),
            fingerprint: "origin-fingerprint".to_owned(),
        };
        let mut snapshot = status_start_snapshot_for(
            "origin-attempt",
            "1",
            base_revision.next().next(),
            status_message_id.clone(),
            vec![emission.clone()],
            vec![runtime_context.id().clone(), status_message_id.clone()],
        );
        freeze_test_input(
            &store,
            &mut snapshot,
            &[runtime_context.clone(), status.clone()],
        );

        let started = store
            .commit_model_turn_start(
                &[runtime_context, status],
                &snapshot,
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
            )
            .expect("same-start RuntimeToolObservation and Agent Status commit");

        assert_eq!(store.current_logical_step_progress().unwrap(), 1);
        let head = store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus,
                ),
                "active_actionable",
            )
            .unwrap()
            .expect("latest Todo head");
        assert_eq!(head.logical_step_origin, 1);
        assert_eq!(
            head.logical_step_origin,
            store.current_logical_step_progress().unwrap()
        );
        assert_eq!(head.canonical_message_id, status_message_id);
        assert_eq!(
            head.event_sequence,
            started.contribution_emissions[0].sequence
        );
        assert!(matches!(
            started.contribution_emissions[0].event,
            RuntimeEvent::ContextContributionEmitted {
                logical_step_origin: 1,
                emission: ref committed,
                ..
            } if committed == &emission
        ));

        // An exact replay verifies the same store-assigned origin and does
        // not advance the sequence a second time.
        let replay = store
            .commit_model_turn_start(
                &[
                    request_context_message("runtime-observation", "tool observation"),
                    agent_status_message(
                        "status-origin",
                        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                        &[AgentStatusModuleId::Todo],
                    ),
                ],
                &snapshot,
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
            )
            .expect("exact start replay");
        assert_eq!(
            replay.disposition,
            ModelTurnStartCommitDisposition::IdempotentReplay
        );
        assert_eq!(
            replay.contribution_emissions,
            started.contribution_emissions
        );
        assert_eq!(store.current_logical_step_progress().unwrap(), 1);
    }

    /// Todo's four-unit window is measured from the committed origin, not
    /// from the status preparation snapshot. Later fresh primary starts add
    /// exactly one unit each; an overflow retry, a status-free retry, and
    /// request-scoped context do not create another unit.
    #[test]
    fn logical_step_progress_counts_later_fresh_starts_once_and_overflow_retries_zero() {
        let store = store();
        let base = user_message("base", "base");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let status_message_id = MessageId::new("status-progress");
        let emission = ContributionEmission {
            key: "active_actionable".to_owned(),
            fingerprint: "progress-fingerprint".to_owned(),
        };
        let mut first = status_start_snapshot(
            store.load_head().unwrap().revision.next(),
            status_message_id.clone(),
            vec![emission],
        );
        let status = agent_status_message(
            status_message_id.as_str(),
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[AgentStatusModuleId::Todo],
        );
        freeze_test_input(&store, &mut first, std::slice::from_ref(&status));
        store
            .commit_model_turn_start(
                std::slice::from_ref(&status),
                &first,
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
            )
            .unwrap();
        let origin = store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus,
                ),
                "active_actionable",
            )
            .unwrap()
            .expect("first Todo head")
            .logical_step_origin;
        assert_eq!(origin, 1);

        // The overflow request belongs to the already-started first logical
        // step. It is a real start snapshot, but it cannot consume cooldown
        // progress or move the emission origin.
        let retry_revision = store.load_head().unwrap().revision;
        let mut overflow_retry =
            progress_request_snapshot("attempt-1", "1", 1, retry_revision, Vec::new());
        freeze_test_input(&store, &mut overflow_retry, &[]);
        store
            .commit_model_turn_start(&[], &overflow_retry, Utc::now())
            .expect("overflow retry start");
        assert_eq!(store.current_logical_step_progress().unwrap(), origin);

        for index in 0..3 {
            let revision = store.load_head().unwrap().revision;
            let mut snapshot = progress_request_snapshot(
                &format!("later-attempt-{index}"),
                "1",
                0,
                revision,
                Vec::new(),
            );
            freeze_test_input(&store, &mut snapshot, &[]);
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .expect("later fresh primary start");
        }
        assert_eq!(
            store.current_logical_step_progress().unwrap() - origin,
            3,
            "three later fresh starts are strictly before the inclusive boundary"
        );
        assert_eq!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::AgentStatus
                    ),
                    "active_actionable"
                )
                .unwrap()
                .expect("Todo head remains the first emission")
                .logical_step_origin,
            origin
        );

        let fourth_revision = store.load_head().unwrap().revision;
        let mut fourth =
            progress_request_snapshot("later-attempt-3", "1", 0, fourth_revision, Vec::new());
        freeze_test_input(&store, &mut fourth, &[]);
        store
            .commit_model_turn_start(&[], &fourth, Utc::now())
            .expect("fourth later fresh primary start");
        assert_eq!(store.current_logical_step_progress().unwrap() - origin, 4);
        assert_eq!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::AgentStatus
                    ),
                    "active_actionable"
                )
                .unwrap()
                .expect("Todo head remains unchanged without a status")
                .logical_step_origin,
            origin
        );
    }

    /// Typed projection compaction retires model-visible history, but it is not Todo
    /// progress. In particular, retiring the Agent Status message does not
    /// reset or consume the durable reminder window.
    #[test]
    fn compaction_does_not_reset_or_advance_logical_step_progress() {
        let store = store();
        let base = user_message("base", "base");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let status_id = MessageId::new("status-before-compaction");
        let status = agent_status_message(
            status_id.as_str(),
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[AgentStatusModuleId::Todo],
        );
        let emission = ContributionEmission {
            key: "active_actionable".to_owned(),
            fingerprint: "compaction-fingerprint".to_owned(),
        };
        let mut snapshot = status_start_snapshot(
            store.load_head().unwrap().revision.next(),
            status_id.clone(),
            vec![emission],
        );
        freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&status));
        store
            .commit_model_turn_start(
                std::slice::from_ref(&status),
                &snapshot,
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
            )
            .unwrap();
        let origin = store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus,
                ),
                "active_actionable",
            )
            .unwrap()
            .expect("Todo head")
            .logical_step_origin;
        assert_eq!(origin, 1);

        store
            .commit_projection(
                crate::scripted_suites::support::context::checkpoint_commit_fixture(
                    &store,
                    crate::scripted_suites::support::context::checkpoint_fixture(
                        &store,
                        "summary-after-status",
                        2,
                        "summary",
                    ),
                    None,
                ),
            )
            .expect("compaction commits");

        assert_eq!(store.current_logical_step_progress().unwrap(), origin);
        assert_eq!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::AgentStatus
                    ),
                    "active_actionable"
                )
                .unwrap()
                .expect("durable Todo head survives compaction")
                .logical_step_origin,
            origin
        );
    }

    /// Time and Background are optional presentation modules in one status
    /// generation. Their canonical status membership is not another logical
    /// primary start, so enabling either one cannot move Todo's cadence.
    #[test]
    fn optional_status_modules_do_not_add_logical_step_progress_units() {
        let store = store();
        let base = user_message("base", "base");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let runtime_context = request_context_message("runtime-context", "observation");
        let status_id = MessageId::new("status-all-modules");
        let status = agent_status_message(
            status_id.as_str(),
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[
                AgentStatusModuleId::Time,
                AgentStatusModuleId::Background,
                AgentStatusModuleId::Todo,
            ],
        );
        let mut snapshot = status_start_snapshot_for(
            "all-modules-attempt",
            "1",
            store.load_head().unwrap().revision.next().next(),
            status_id.clone(),
            vec![ContributionEmission {
                key: "active_actionable".to_owned(),
                fingerprint: "all-modules-fingerprint".to_owned(),
            }],
            vec![runtime_context.id().clone(), status_id],
        );
        snapshot.contributions.last_mut().unwrap().metadata =
            ContextKind::AgentStatus(status.agent_status_metadata().unwrap().clone());
        freeze_test_input(
            &store,
            &mut snapshot,
            &[runtime_context.clone(), status.clone()],
        );
        store
            .commit_model_turn_start(
                &[runtime_context, status],
                &snapshot,
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 3, 0).unwrap(),
            )
            .unwrap();

        assert_eq!(
            store.current_logical_step_progress().unwrap(),
            1,
            "all optional status sections and same-start context share one start unit"
        );
        assert_eq!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::AgentStatus
                    ),
                    "active_actionable"
                )
                .unwrap()
                .expect("Todo head")
                .logical_step_origin,
            1
        );
    }

    #[test]
    fn agent_status_start_faults_roll_back_message_emission_and_head_together() {
        for fault in [
            RequestStartFaultOperation::AfterContributionEventInsert,
            RequestStartFaultOperation::AfterContributionHeadUpsert,
        ] {
            let store = store();
            let base = user_message("base", "base");
            store.initialize(std::slice::from_ref(&base)).unwrap();
            let message_id = MessageId::new(format!("status-fault-{fault:?}"));
            let context = agent_status_message(
                message_id.as_str(),
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                &[AgentStatusModuleId::Todo],
            );
            let emission = ContributionEmission {
                key: "active_actionable".to_owned(),
                fingerprint: "fault-fingerprint".to_owned(),
            };
            let snapshot = status_start_snapshot(
                store.load_head().unwrap().revision.next(),
                message_id,
                vec![emission],
            );
            store.arm_request_start_fault_script([fault]);
            assert!(
                store
                    .commit_model_turn_start(
                        std::slice::from_ref(&context),
                        &snapshot,
                        Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
                    )
                    .is_err()
            );

            assert_eq!(
                store.load_head().unwrap().revision,
                crate::conversation::SurfaceRevision::new(1)
            );
            assert_eq!(store.load_canonical().unwrap(), vec![base]);
            assert!(
                store
                    .read_request_snapshots(None, 10)
                    .unwrap()
                    .snapshots
                    .is_empty()
            );
            assert!(store.read_events(None, 10).unwrap().events.is_empty());
            assert_eq!(
                store.current_logical_step_progress().unwrap(),
                0,
                "a failed status start does not advance Todo progress"
            );
            assert!(
                store
                    .latest_contribution_emission(
                        &crate::runtime::identity::ContextContributorIdentity::Native(
                            crate::runtime::identity::NativeContextContributor::AgentStatus
                        ),
                        "active_actionable"
                    )
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn agent_status_start_binds_metadata_to_the_exact_status_message() {
        let store = store();
        let base = user_message("base", "base");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let emission = ContributionEmission {
            key: "active_actionable".to_owned(),
            fingerprint: "fingerprint".to_owned(),
        };
        let expected_id = MessageId::new("expected-status");
        let actual = agent_status_message(
            "actual-status",
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[AgentStatusModuleId::Todo],
        );
        let snapshot = status_start_snapshot(
            store.load_head().unwrap().revision.next(),
            expected_id,
            vec![emission],
        );
        assert!(matches!(
            store.commit_model_turn_start(
                std::slice::from_ref(&actual),
                &snapshot,
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert!(store.read_events(None, 10).unwrap().events.is_empty());
        assert_eq!(store.load_canonical().unwrap(), vec![base]);
    }

    #[test]
    fn contribution_head_resume_uses_one_bounded_lookup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let conversation_id = ConversationId::new("conv_f9113f99-5f0f-7b59-8f83-495e254f63f1");
        let emission = ContributionEmission {
            key: "active_actionable".to_owned(),
            fingerprint: "resume-fingerprint".to_owned(),
        };
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            let base = user_message("base", "base");
            store.initialize(std::slice::from_ref(&base)).unwrap();
            let message_id = MessageId::new("status-resume");
            let context = agent_status_message(
                message_id.as_str(),
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                &[AgentStatusModuleId::Todo],
            );
            let mut snapshot = status_start_snapshot(
                store.load_head().unwrap().revision.next(),
                message_id,
                vec![emission.clone()],
            );
            freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));
            store
                .commit_model_turn_start(
                    std::slice::from_ref(&context),
                    &snapshot,
                    Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
                )
                .unwrap();
        }
        let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
        let found = reopened
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus,
                ),
                &emission.key,
            )
            .unwrap()
            .expect("committed suppression survives reopen");
        assert_eq!(found.fingerprint, emission.fingerprint);
        assert_eq!(found.logical_step_origin, 1);
        assert_eq!(reopened.current_logical_step_progress().unwrap(), 1);
        assert_eq!(reopened.contribution_head_lookup_reads(), 1);
    }

    #[test]
    fn a_version_32_profile_store_is_refused_without_migration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile-v33.sqlite");
        let id = ConversationId::new("conv_bb2149cf-1f8d-7c3c-8c60-73f4acb0a518");
        {
            let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE rustx_store SET schema_version = 32 WHERE id = 1",
                    [],
                )
                .unwrap();
        }
        for result in [
            SqliteConversationStore::open(id.clone(), &path),
            SqliteConversationStore::open_existing(id, &path),
        ] {
            assert!(matches!(
                result,
                Err(ConversationStoreError::SchemaVersionMismatch {
                    stored: 32,
                    expected: SQLITE_SCHEMA_VERSION
                })
            ));
        }
        let connection = rusqlite::Connection::open(&path).unwrap();
        let stored: i64 = connection
            .query_row(
                "SELECT schema_version FROM rustx_store WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, 32,
            "neither runtime nor management opening migrates"
        );
    }

    #[test]
    fn stale_projection_head_rejects_request_start_without_snapshot_or_context_admission() {
        use crate::scripted_suites::support::context::{
            checkpoint_commit_fixture, checkpoint_fixture,
        };
        let root = tempfile::tempdir().unwrap();
        let store = SqliteConversationStore::open(
            ConversationId::generate(),
            &root.path().join("start.sqlite"),
        )
        .unwrap();
        store.initialize(&[user_message("a", "original")]).unwrap();
        store
            .commit_projection(checkpoint_commit_fixture(
                &store,
                checkpoint_fixture(&store, "P1", 1, "first"),
                None,
            ))
            .unwrap();
        let context = request_context_message("ctx-1", "request-scoped context");
        let mut snapshot = context_start_snapshot(
            store.load_head().unwrap().revision.next(),
            vec![MessageId::new("ctx-1")],
        );
        freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));
        store
            .commit_projection(checkpoint_commit_fixture(
                &store,
                checkpoint_fixture(&store, "P2", 1, "second"),
                None,
            ))
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute("UPDATE projection_head SET activation=1", [])
            .unwrap();
        let before = store.load_canonical().unwrap();
        let events = store.read_events(None, 64).unwrap().events;
        let error = store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
            .unwrap_err();
        assert!(error.to_string().contains("Projection Head"), "{error}");
        assert_eq!(store.load_canonical().unwrap(), before);
        let after = store.read_events(None, 64).unwrap().events;
        assert_eq!(after, events);
        assert_eq!(
            after
                .iter()
                .filter(|e| matches!(e.event, RuntimeEvent::ModelRequestStarted { .. }))
                .count(),
            0
        );
        let snapshots: u64 = store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM request_snapshots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(snapshots, 0);
        // Correcting only the injected damage proves the snapshot/context fixture
        // is otherwise valid: the obsolete input is rejected, while freezing
        // current history lets this actual-request identity start exactly once.
        store
            .lock()
            .unwrap()
            .execute("UPDATE projection_head SET activation=2", [])
            .unwrap();
        store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
            .unwrap_err();
        let mut current = snapshot;
        freeze_test_input(&store, &mut current, std::slice::from_ref(&context));
        store
            .commit_model_turn_start(std::slice::from_ref(&context), &current, Utc::now())
            .unwrap();
        assert_eq!(
            store
                .read_events(None, 64)
                .unwrap()
                .events
                .iter()
                .filter(|e| matches!(e.event, RuntimeEvent::ModelRequestStarted { .. }))
                .count(),
            1
        );
        assert_eq!(
            store
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM request_snapshots", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
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
        let mut snapshot =
            context_start_snapshot(base_revision.next(), vec![MessageId::new("ctx-1")]);
        snapshot
            .freeze_input(
                &store.conversation_id,
                &ModelRequest {
                    continuation_source: None,
                    images: crate::model::images::ResolvedImages::new(),
                    invocation: snapshot.invocation.clone(),
                    messages: crate::model::input::canonical_input(&[a.clone(), context.clone()])
                        .into(),
                    tools: snapshot.tool_definitions.clone(),
                    effective_system_prompt: snapshot.effective_system_prompt.clone(),
                    continuation: snapshot.continuation.clone(),
                },
            )
            .unwrap();
        let started = store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
            .expect("start commits");
        assert!(matches!(
            started.started.event,
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
        assert_eq!(
            reconstructed.messages.to_vec(),
            crate::model::input::canonical_input(&[a.clone(), context.clone()])
        );
        // An exact retry is idempotent and returns the original start fact.
        let retried = store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
            .expect("exact retry");
        assert_eq!(retried.started.sequence, started.started.sequence);
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
        let mut snapshot = context_start_snapshot(
            base_revision.next().next(),
            vec![MessageId::new("ctx-1"), MessageId::new("ctx-2")],
        );
        freeze_test_input(&store, &mut snapshot, &[ctx1.clone(), ctx2.clone()]);
        let started = store
            .commit_model_turn_start(&[ctx1.clone(), ctx2.clone()], &snapshot, Utc::now())
            .expect("start commits");

        // The exact ordered retry is idempotent and returns the original fact.
        let retried = store
            .commit_model_turn_start(&[ctx1.clone(), ctx2.clone()], &snapshot, Utc::now())
            .expect("exact ordered retry");
        assert_eq!(retried.started.sequence, started.started.sequence);

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
        let mut exact = context_start_snapshot(
            base_revision.next().next(),
            vec![MessageId::new("ctx-1"), MessageId::new("ctx-2")],
        );
        freeze_test_input(&committed, &mut exact, &[ctx1.clone(), ctx2.clone()]);
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
            RequestStartFaultOperation::AfterCompletionInsert,
        ] {
            let store = store();
            let a = user_message("a", "A");
            store.initialize(std::slice::from_ref(&a)).unwrap();
            let base_revision = store.load_head().unwrap().revision;
            let context = request_context_message("ctx-1", "request-scoped context");
            let mut snapshot =
                context_start_snapshot(base_revision.next(), vec![MessageId::new("ctx-1")]);
            store.arm_request_start_fault_script([fault]);
            freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));
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
            let mut snapshot = RequestSnapshot::new(
                RequestIdentity {
                    attempt_id: AttemptId::new(format!("request-page-attempt-{index}")),
                    turn: TurnId::new("1"),
                    retry_number: 0,
                },
                crate::context::resources::LedgerFrontier((revision).get()),
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
            freeze_test_input(&store, &mut snapshot, &[]);
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
        let execution_id = crate::runtime::identity::ToolExecutionId::new(
            "exec_63093cff-69b8-7efb-82db-20a75f993612",
        );
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
    #[allow(clippy::too_many_lines)] // Full admission, private capture, reopen and tamper boundaries.
    fn agent_authorities_reopen_from_private_capture_without_journal_secrets() {
        use crate::credentials::{CredentialSnapshot, SourceCredentials};
        use crate::model::catalog::{CredentialSource, ResolvedCredential};
        use crate::runtime::identity::SubagentId;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let id = ConversationId::generate();
        let child_id = ConversationId::generate();
        let agent_id = AgentId::new("private-agent");
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        store.initialize(&[]).unwrap();
        let (event, mut authority) =
            crate::local_runtime::session::tests::deletion_tests::admit_agent(envelope(
                &id,
                crate::runtime::subagent::subagent_ownership_event_id(
                    &SubagentId::for_conversation(&id, 1),
                )
                .as_ref(),
                None,
                RuntimeEvent::SubagentOwnershipCommitted {
                    parent_agent_id: AgentId::new("parent-agent"),
                    subagent_id: SubagentId::for_conversation(&id, 1),
                    child_agent_id: agent_id.clone(),
                    child_conversation_id: child_id,
                    admitted_authority: None,
                    origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                        tool_call_id: ToolCallId::new("private-call"),
                    },
                    agent: "explore".into(),
                    definition_digest: "sha256:definition".into(),
                    profile_digest: "sha256:profile".into(),
                    ownership: crate::events::types::SubagentOwnershipKind::Normal,
                    workspace: directory.path().to_path_buf(),
                },
            ));
        authority.resolved.model.primary.binding.credential =
            CredentialSource::Literal("private-literal".into());
        authority.resolved.model.primary.binding.resolved_credential = None;
        let mut summary = authority.resolved.model.primary.clone();
        summary.binding.credential = CredentialSource::Environment("AUTHORED_PROVIDER".into());
        summary.binding.resolved_credential = Some(ResolvedCredential::new("captured-provider"));
        authority.resolved.model.summary =
            crate::model::frozen::FrozenSummaryModel::Explicit(Box::new(summary));
        let mut source: SourceCredentials = serde_json::from_value(serde_json::json!({
            "environment": {"TOKEN":"$AUTHORED_SOURCE"}, "headers":{"Authorization":"$AUTHORED_SOURCE"}
        })).unwrap();
        source.capture(CredentialSnapshot::new([(
            "AUTHORED_SOURCE".into(),
            "captured-source".into(),
        )]));
        authority.resolved.materialization.sources.insert(
            crate::capabilities::ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new(
                "private-source",
            )),
            crate::tools::mcp::McpServerBinding {
                credentials: source,
                transport: crate::tools::mcp::McpTransportConfig::Stdio {
                    program: "test-server".into(),
                    args: vec!["private-mcp-arg".into()],
                    cwd: None,
                    environment: [("TOKEN".into(), "private-mcp-env".into())].into(),
                },
                policy: crate::tools::ToolInvocationPolicy::default(),
            },
        );
        authority
            .resolved
            .environment
            .push(("TOOL_TOKEN".into(), "private-tool-env".into()));
        authority.resolved.instructions = "private-instructions".into();
        authority.resolved.materialization.sources.insert(
            crate::capabilities::ToolSourceId::Mcp(crate::runtime::identity::McpServerId::new("private-http")),
            crate::tools::mcp::McpServerBinding {
                credentials: SourceCredentials::default(),
                transport: crate::tools::mcp::McpTransportConfig::StreamableHttp {
                    endpoint: "https://private-url-user:private-url-password@example.test/api?token=private-url-query".into(),
                    headers: [("Authorization".into(), "private-http-auth".into()), ("X-Private".into(), "private-http-header".into())].into(),
                },
                policy: crate::tools::ToolInvocationPolicy::default(),
            },
        );
        let canaries = [
            "private-literal",
            "captured-provider",
            "captured-source",
            "private-mcp-arg",
            "private-mcp-env",
            "private-tool-env",
            "private-instructions",
            "private-url-user",
            "private-url-password",
            "private-url-query",
            "private-http-auth",
            "private-http-header",
        ];
        let public = serde_json::to_string(&event).unwrap();
        for secret in canaries {
            assert!(!public.contains(secret), "public DTO contains {secret}");
        }
        assert!(
            store.append_event(event.clone()).is_err(),
            "public event alone cannot admit private authority"
        );
        let mut invalid = event.clone();
        invalid.schema_version = 0;
        assert!(store.append_agent_admission(invalid, &authority).is_err());
        assert_eq!(
            store
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM agent_authorities", [], |row| row
                    .get::<_, i64>(0),)
                .unwrap(),
            0,
            "failed public event commit rolls back the private authority row"
        );
        assert!(store.read_events(None, 10).unwrap().events.is_empty());
        store
            .append_agent_admission(event.clone(), &authority)
            .unwrap();
        assert!(
            store.append_agent_admission(event, &authority).is_err(),
            "duplicate admission rolls back"
        );
        let journal: String = store
            .lock()
            .unwrap()
            .query_row("SELECT event_json FROM events", [], |row| row.get(0))
            .unwrap();
        for secret in canaries {
            assert!(
                !journal.contains(secret),
                "Event Journal contains private material"
            );
        }
        drop(store);
        let reopened = SqliteConversationStore::open(id, &path).unwrap();
        let restored = reopened.load_agent_authority(&agent_id).unwrap();
        assert_eq!(
            restored.resolved.environment,
            authority.resolved.environment
        );
        assert_eq!(
            restored.resolved.instructions,
            authority.resolved.instructions
        );
        for (name, binding) in &authority.resolved.materialization.sources {
            assert_eq!(
                restored.resolved.materialization.sources[name].transport,
                binding.transport
            );
        }
        let events = reopened.read_events(None, 10).unwrap().events;
        assert!(
            matches!(&events[0].event, RuntimeEvent::SubagentOwnershipCommitted {
            admitted_authority: Some(reference), ..
        } if reference == &agent_id)
        );
        let authority = restored;
        assert_eq!(
            authority
                .resolved
                .model
                .primary
                .binding
                .resolved_credential
                .as_ref()
                .unwrap()
                .expose(),
            "private-literal"
        );
        let crate::model::frozen::FrozenSummaryModel::Explicit(summary) =
            &authority.resolved.model.summary
        else {
            panic!("summary")
        };
        assert_eq!(
            summary
                .binding
                .resolved_credential
                .as_ref()
                .unwrap()
                .expose(),
            "captured-provider"
        );
        for source in authority
            .resolved
            .materialization
            .sources
            .values()
            .filter(|source| {
                matches!(
                    source.transport,
                    crate::tools::mcp::McpTransportConfig::Stdio { .. }
                )
            })
        {
            let resolved = source.credentials.resolve().unwrap();
            assert_eq!(resolved.environment["TOKEN"].expose(), "captured-source");
            assert_eq!(
                resolved.headers["Authorization"].expose(),
                "captured-source"
            );
        }
        reopened
            .lock()
            .unwrap()
            .execute("DELETE FROM agent_authorities", [])
            .unwrap();
        assert!(
            reopened.load_agent_authority(&agent_id).is_err(),
            "missing private capture must fail closed"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Real compaction, later Tool admission, and client admission.
    fn resumed_agent_origin_survives_creation_call_compaction() {
        use crate::runtime::identity::SubagentId;
        use crate::runtime::subagent::AgentActivationOrigin;
        let store = store();
        let conversation = store.conversation_id().clone();
        let agent = AgentId::new("continuable-agent");
        let child = ConversationId::generate();
        let call_message = |message: &str, call: &str, name: &str| {
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(message),
                content: vec![AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new(call),
                    tool_id: ToolId::new(name),
                    name: name.into(),
                    arguments: serde_json::json!({}),
                })],
            })
        };
        store
            .initialize(&[
                call_message("creation", "create-call", "subagent"),
                MessageBlock::Tool(ToolMessageBlock {
                    id: MessageId::new("creation-result"),
                    occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                        MessageId::new("creation"),
                        ContentBlockIndex::new(0),
                    ),
                    tool_call_id: ToolCallId::new("create-call"),
                    tool_id: ToolId::new("subagent"),
                    result: ToolExecutionResult {
                        deliveries: vec![],
                        status: ToolExecutionStatus::Success,
                        content: vec![],
                        duration_ms: 1,
                        exit_code: None,
                        artifacts: vec![],
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    },
                }),
            ])
            .unwrap();
        let admission = |sequence, origin| {
            let activation = SubagentId::for_conversation(&conversation, sequence);
            envelope(
                &conversation,
                crate::runtime::subagent::subagent_ownership_event_id(&activation).as_ref(),
                None,
                RuntimeEvent::SubagentOwnershipCommitted {
                    parent_agent_id: AgentId::new("parent"),
                    subagent_id: activation,
                    child_agent_id: agent.clone(),
                    child_conversation_id: child.clone(),
                    admitted_authority: None,
                    origin,
                    agent: "explore".into(),
                    definition_digest: "sha256:definition".into(),
                    profile_digest: "sha256:profile".into(),
                    ownership: crate::events::types::SubagentOwnershipKind::Normal,
                    workspace: std::path::PathBuf::from("/shared-workspace"),
                },
            )
        };
        let (creation, authority) =
            crate::local_runtime::session::tests::deletion_tests::admit_agent(admission(
                1,
                AgentActivationOrigin::CreationTool {
                    tool_call_id: ToolCallId::new("create-call"),
                },
            ));
        store.append_agent_admission(creation, &authority).unwrap();
        let settle = |sequence| {
            let (notice, report, event) = success_publication(
                &store,
                &SubagentId::for_conversation(&conversation, sequence),
                &agent,
                "done",
            );
            store
                .accept_subagent_terminal(Some(notice), report, event)
                .unwrap();
        };
        settle(1);
        store
            .commit_projection(
                crate::scripted_suites::support::context::checkpoint_commit_fixture(
                    &store,
                    crate::scripted_suites::support::context::checkpoint_fixture(
                        &store,
                        "creation-checkpoint",
                        2,
                        "creation retired",
                    ),
                    None,
                ),
            )
            .unwrap();
        assert!(
            !store
                .load_head()
                .unwrap()
                .active_message_ids
                .contains(&MessageId::new("creation"))
        );
        store
            .append_canonical(&call_message("followup", "message-call", "send_message"))
            .unwrap();
        let resume = |sequence, origin: AgentActivationOrigin| {
            store
                .append_event(crate::runtime::subagent::admission_event(
                    &conversation,
                    &agent,
                    &SubagentId::for_conversation(&conversation, sequence),
                    &origin,
                    crate::events::types::AgentActivationAdmissionPhase::Reserved,
                    Utc::now(),
                ))
                .unwrap();
            store.append_event(admission(sequence, origin)).unwrap();
        };
        resume(
            2,
            AgentActivationOrigin::MessageTool {
                tool_call_id: ToolCallId::new("message-call"),
            },
        );
        settle(2);
        resume(3, AgentActivationOrigin::ClientControl);
        let origins = store
            .read_events(None, 100)
            .unwrap()
            .events
            .into_iter()
            .filter_map(|event| match event.event {
                RuntimeEvent::SubagentOwnershipCommitted {
                    child_agent_id,
                    child_conversation_id,
                    origin,
                    ..
                } => {
                    assert_eq!(child_agent_id, agent);
                    assert_eq!(child_conversation_id, child);
                    Some(origin)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            origins,
            vec![
                AgentActivationOrigin::CreationTool {
                    tool_call_id: ToolCallId::new("create-call")
                },
                AgentActivationOrigin::MessageTool {
                    tool_call_id: ToolCallId::new("message-call")
                },
                AgentActivationOrigin::ClientControl,
            ]
        );
        assert!(store.load_agent_authority(&agent).is_ok());
    }

    /// Commits the canonical Normal-ownership fact of one subagent child
    /// over a shared workspace (agent name `explore`).
    fn own_normal_subagent(
        store: &SqliteConversationStore,
        subagent_id: &crate::runtime::identity::SubagentId,
        child_agent_id: &AgentId,
    ) {
        own_normal_subagent_with_workspace(
            store,
            subagent_id,
            child_agent_id,
            std::path::PathBuf::from("/shared-workspace"),
        );
    }

    /// Commits the canonical Normal-ownership fact over an explicit frozen
    /// Session workspace binding.
    fn own_normal_subagent_with_workspace(
        store: &SqliteConversationStore,
        subagent_id: &crate::runtime::identity::SubagentId,
        child_agent_id: &AgentId,
        workspace: std::path::PathBuf,
    ) {
        let event = envelope(
            store.conversation_id(),
            crate::runtime::subagent::subagent_ownership_event_id(subagent_id).as_ref(),
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                admitted_authority: None,
                subagent_id: subagent_id.clone(),
                child_agent_id: child_agent_id.clone(),
                child_conversation_id:
                    crate::scripted_suites::common::identity::child_conversation_id(
                        subagent_id.as_str(),
                    ),
                origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: ToolCallId::new("call-sub"),
                },
                agent: "explore".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                profile_digest: "sha256:profile".to_owned(),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                workspace,
            },
        );
        if store.load_agent_authority(child_agent_id).is_ok() {
            let RuntimeEvent::SubagentOwnershipCommitted { origin, .. } = &event.event else {
                unreachable!("ownership fixture")
            };
            store
                .append_event(crate::runtime::subagent::admission_event(
                    store.conversation_id(),
                    child_agent_id,
                    subagent_id,
                    origin,
                    crate::events::types::AgentActivationAdmissionPhase::Reserved,
                    Utc::now(),
                ))
                .unwrap();
            store.append_event(event).expect("resumed ownership fact");
        } else {
            let (event, authority) =
                crate::local_runtime::session::tests::deletion_tests::admit_agent(event);
            store
                .append_agent_admission(event, &authority)
                .expect("ownership fact");
        }
    }

    /// The canonical successful terminal publication triple (notice,
    /// report, event) exactly as the registry builds it (Issue #192).
    fn success_publication(
        store: &SqliteConversationStore,
        subagent_id: &crate::runtime::identity::SubagentId,
        child_agent_id: &AgentId,
        report_text: &str,
    ) -> (InboundDraft, InboundDraft, RuntimeEventEnvelope) {
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
        let (report, event) = crate::runtime::subagent::terminal_publication(
            store.conversation_id(),
            subagent_id,
            child_agent_id,
            SubagentTerminalState::Succeeded,
            vec![UserContentBlock::Text(TextBlock {
                text: report_text.to_owned(),
            })],
            true,
            timestamp,
        );
        let notice = crate::runtime::subagent::terminal_notice(
            child_agent_id,
            subagent_id,
            &crate::runtime::subagent::SubagentName::parse("explore").expect("canonical name"),
            timestamp,
        );
        (notice, report, event)
    }

    /// The canonical non-success terminal publication pair (report, event):
    /// one runtime-authored terminal message and no notice.
    fn non_success_publication(
        store: &SqliteConversationStore,
        subagent_id: &crate::runtime::identity::SubagentId,
        child_agent_id: &AgentId,
        state: SubagentTerminalState,
        text: &str,
    ) -> (InboundDraft, RuntimeEventEnvelope) {
        crate::runtime::subagent::terminal_publication(
            store.conversation_id(),
            subagent_id,
            child_agent_id,
            state,
            vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            true,
            Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
        )
    }

    /// A normal subagent terminal publication commits through
    /// `accept_subagent_terminal` — the one durable authority for
    /// `SubagentTerminalPublished` (Issue #192). The retry of a notice-free
    /// non-success publication is an idempotent no-op, a semantically
    /// different retry of the same child conflicts deterministically, and
    /// the obsolete generic `accept_inbound_with_event` path rejects the
    /// payload so it can never bypass the terminal-publication contract.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn subagent_terminal_publication_is_idempotent_and_terminal_unique() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
        own_normal_subagent(&store, &subagent_id, &child_agent_id);

        // A failed terminal: one runtime-authored message, no notice.
        let (report, event) = non_success_publication(
            &store,
            &subagent_id,
            &child_agent_id,
            SubagentTerminalState::Failed,
            "subagent failed: boom",
        );

        // The obsolete generic path can no longer publish a normal subagent
        // terminal fact — for either a success-shaped or a failure-shaped
        // payload.
        assert!(matches!(
            store.accept_inbound_with_event(report.clone(), event.clone()),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(
            store.load_pending().unwrap().len(),
            0,
            "the rejected generic transition accepted nothing"
        );

        let (notice, accepted, persisted) = store
            .accept_subagent_terminal(None, report.clone(), event.clone())
            .unwrap();
        assert!(
            notice.is_none(),
            "a non-success terminal carries no successful-terminal notice"
        );
        assert!(!accepted.retried);
        assert_eq!(persisted.sequence, 2, "the ownership commit is sequence 1");

        // The notice-free retry is an idempotent no-op.
        let (notice, retried, persisted_retry) = store
            .accept_subagent_terminal(None, report.clone(), event.clone())
            .unwrap();
        assert!(notice.is_none());
        assert!(retried.retried);
        assert_eq!(persisted_retry.sequence, persisted.sequence);

        // A semantically different retry of the same child's terminal —
        // same canonical identities, changed content — is a deterministic
        // conflict, never a second publication.
        let (conflicting_report, _) = non_success_publication(
            &store,
            &subagent_id,
            &child_agent_id,
            SubagentTerminalState::Failed,
            "a different diagnostic",
        );
        assert!(matches!(
            store.accept_subagent_terminal(None, conflicting_report, event),
            Err(ConversationStoreError::CorrelationConflict { .. })
        ));
        assert_eq!(store.load_pending().unwrap().len(), 1, "exactly once");

        // Provenance: a failed terminal must be Runtime-authored — an
        // Agent-authored one is rejected before anything is accepted.
        let other = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
        let other_child = crate::runtime::identity::AgentId::new("agent-other");
        own_normal_subagent(&store, &other, &other_child);
        let (mut wrong_report, wrong_event) = non_success_publication(
            &store,
            &other,
            &other_child,
            SubagentTerminalState::Failed,
            "subagent failed: boom",
        );
        wrong_report.source = UserSource::Agent {
            agent_id: other_child,
        };
        assert!(matches!(
            store.accept_subagent_terminal(None, wrong_report, wrong_event),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(store.load_pending().unwrap().len(), 1);
    }

    /// The Issue #192 pair publication: a successful terminal commits the
    /// runtime-authored terminal notice (the parent-model execution
    /// correlation) and the child-authored report in one transaction —
    /// notice first, report (the terminal result) last, the event
    /// committed once.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn subagent_terminal_pair_publication_is_atomic_idempotent_and_ordered() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
        own_normal_subagent(&store, &subagent_id, &child_agent_id);
        let notice_id = crate::runtime::subagent::terminal_notice_message_id(&subagent_id);
        let report_id = crate::runtime::subagent::terminal_message_id(&subagent_id);
        let (notice, report, event) =
            success_publication(&store, &subagent_id, &child_agent_id, "the final report");

        let (notice, report, persisted) = store
            .accept_subagent_terminal(Some(notice), report, event)
            .unwrap();
        let notice = notice.expect("the notice was accepted");
        assert!(!notice.retried && !report.retried);
        assert!(
            notice.sequence < report.sequence,
            "the notice is ordered strictly before the terminal report"
        );
        let pending = store.load_pending().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].message.id, notice_id);
        assert_eq!(pending[1].message.id, report_id);
        let notice_text = match &pending[0].message.content[0] {
            UserContentBlock::Text(text) => text.text.clone(),
            other => panic!("the notice is text: {other:?}"),
        };
        assert!(
            notice_text.contains(&format!("Agent {child_agent_id} activation {subagent_id}")),
            "the notice separately names the durable Agent and finite activation: {notice_text}"
        );

        // The retry of the identical pair is an idempotent no-op on both
        // sides and never re-commits the event.
        let (notice, report, event) =
            success_publication(&store, &subagent_id, &child_agent_id, "the final report");
        let (notice_retry, report_retry, persisted_retry) = store
            .accept_subagent_terminal(Some(notice), report, event)
            .unwrap();
        assert!(notice_retry.expect("notice").retried);
        assert!(report_retry.retried);
        assert_eq!(persisted_retry.sequence, persisted.sequence);
        assert_eq!(store.load_pending().unwrap().len(), 2);

        // A notice that is not runtime-authored is rejected before anything
        // is accepted.
        let other = crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
        let other_child = crate::runtime::identity::AgentId::new("agent-other");
        own_normal_subagent(&store, &other, &other_child);
        let (mut child_authored_notice, other_report, other_event) =
            success_publication(&store, &other, &other_child, "the other report");
        child_authored_notice.source = UserSource::Agent {
            agent_id: other_child.clone(),
        };
        assert!(matches!(
            store.accept_subagent_terminal(
                Some(child_authored_notice),
                other_report.clone(),
                other_event.clone()
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        // A non-terminal event payload is rejected just as strictly.
        let wrong_event = envelope(
            &conversation_id,
            "not-a-terminal",
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                admitted_authority: None,
                subagent_id: other.clone(),
                child_agent_id: other_child,
                child_conversation_id:
                    crate::scripted_suites::common::identity::child_conversation_id(other.as_str()),
                origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: ToolCallId::new("call-other"),
                },
                agent: "explore".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                profile_digest: "sha256:profile".to_owned(),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                workspace: std::path::PathBuf::from("/shared-workspace"),
            },
        );
        assert!(matches!(
            store.accept_subagent_terminal(None, other_report, wrong_event),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(
            store.load_pending().unwrap().len(),
            2,
            "rejections accept nothing"
        );
    }

    /// A durable failure between the pair's acceptances and the event commit
    /// commits nothing: the bounded retry then publishes the pair and the
    /// fact cleanly and exactly once.
    #[test]
    fn subagent_terminal_pair_publication_retries_atomically_after_a_fault() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
        own_normal_subagent(&store, &subagent_id, &child_agent_id);
        let publication =
            || success_publication(&store, &subagent_id, &child_agent_id, "the final report");

        store.arm_fail_event_times(1);
        let (notice, report, event) = publication();
        assert!(
            store
                .accept_subagent_terminal(Some(notice), report, event)
                .is_err(),
            "the injected journal fault fails the publication"
        );
        assert_eq!(
            store.load_pending().unwrap().len(),
            0,
            "the failed transaction committed neither the notice nor the report"
        );

        let (notice, report, event) = publication();
        let (notice, report, persisted) = store
            .accept_subagent_terminal(Some(notice), report, event)
            .expect("the bounded retry commits the pair");
        assert!(!notice.expect("notice").retried);
        assert!(!report.retried);
        assert_eq!(persisted.sequence, 2);
        assert_eq!(store.load_pending().unwrap().len(), 2);
    }

    /// The subagent terminal transition is semantically closed (Issue #192):
    /// every combination of event, report, and notice that no legal runtime
    /// decision sequence could produce is rejected deterministically, and a
    /// rejection accepts nothing.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn subagent_terminal_transition_rejects_semantically_unreachable_states() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let owned = |ordinal: u64| {
            let subagent_id =
                crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
            let child_agent_id =
                crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
            own_normal_subagent(&store, &subagent_id, &child_agent_id);
            (subagent_id, child_agent_id)
        };
        let reject = |result: Result<_, ConversationStoreError>, pending: usize| {
            assert!(
                matches!(result, Err(ConversationStoreError::InvalidReference(_))),
                "the unreachable state is rejected"
            );
            assert_eq!(
                store.load_pending().unwrap().len(),
                pending,
                "a rejection accepts nothing"
            );
        };

        // A success without its runtime-authored terminal notice.
        let (subagent_id, child) = owned(1);
        let (_, report, event) = success_publication(&store, &subagent_id, &child, "report");
        reject(store.accept_subagent_terminal(None, report, event), 0);

        // A success whose notice names a different subagent.
        let (subagent_id, child) = owned(2);
        let (other_id, other_child) = owned(22);
        let (wrong_notice, _, _) = success_publication(&store, &other_id, &other_child, "other");
        let (_, report, event) = success_publication(&store, &subagent_id, &child, "report");
        reject(
            store.accept_subagent_terminal(Some(wrong_notice), report, event),
            0,
        );
        let _ = other_child;

        // A success with a non-canonical report message identity.
        let (subagent_id, child) = owned(3);
        let (notice, mut report, event) =
            success_publication(&store, &subagent_id, &child, "report");
        report.message_id = Some(MessageId::new("subagent-3-not-the-terminal"));
        reject(
            store.accept_subagent_terminal(Some(notice), report, event),
            0,
        );

        // A success with the wrong report producer correlation.
        let (subagent_id, child) = owned(4);
        let (notice, mut report, event) =
            success_publication(&store, &subagent_id, &child, "report");
        report.correlation = Some("subagent-terminal:someone-else".to_owned());
        reject(
            store.accept_subagent_terminal(Some(notice), report, event),
            0,
        );

        // A success whose notice carries the wrong producer correlation.
        let (subagent_id, child) = owned(5);
        let (mut notice, report, event) =
            success_publication(&store, &subagent_id, &child, "report");
        notice.correlation = Some("subagent-terminal-notice:someone-else".to_owned());
        reject(
            store.accept_subagent_terminal(Some(notice), report, event),
            0,
        );

        // A success whose notice has an invalid inbound kind.
        let (subagent_id, child) = owned(6);
        let (mut notice, report, event) =
            success_publication(&store, &subagent_id, &child, "report");
        notice.kind =
            InboundKind::Context(crate::message::types::ContextKind::RuntimeToolObservation);
        reject(
            store.accept_subagent_terminal(Some(notice), report, event),
            0,
        );

        // A terminal event referencing the wrong report message.
        let (subagent_id, child) = owned(7);
        let (notice, report, mut event) =
            success_publication(&store, &subagent_id, &child, "report");
        let RuntimeEvent::SubagentTerminalPublished { message_id, .. } = &mut event.event else {
            panic!("terminal event");
        };
        *message_id = MessageId::new("some-other-message");
        reject(
            store.accept_subagent_terminal(Some(notice), report, event),
            0,
        );

        // A non-success terminal must not carry the successful-terminal
        // notice.
        for (ordinal, state) in [
            (9, SubagentTerminalState::Failed),
            (10, SubagentTerminalState::Cancelled),
            (11, SubagentTerminalState::Interrupted),
        ] {
            let (subagent_id, child) = owned(ordinal);
            let (report, event) =
                non_success_publication(&store, &subagent_id, &child, state, "runtime terminal");
            let notice = crate::runtime::subagent::terminal_notice(
                &child,
                &subagent_id,
                &crate::runtime::subagent::SubagentName::parse("explore").expect("canonical name"),
                Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap(),
            );
            reject(
                store.accept_subagent_terminal(Some(notice), report, event),
                0,
            );
        }

        // A non-canonical event identity.
        let (subagent_id, child) = owned(12);
        let (notice, report, mut event) =
            success_publication(&store, &subagent_id, &child, "report");
        event.event_id = EventId::new("not-the-canonical-terminal-event");
        reject(
            store.accept_subagent_terminal(Some(notice), report, event),
            0,
        );

        // An attempt/turn envelope is not a publication fact's shape.
        let (subagent_id, child) = owned(13);
        let (notice, report, mut event) =
            success_publication(&store, &subagent_id, &child, "report");
        event.attempt_id = Some(AttemptId::new("attempt-13"));
        reject(
            store.accept_subagent_terminal(Some(notice), report, event),
            0,
        );
    }

    /// The notice/report pair is one atomic publication: a retry that finds
    /// exactly one side already durable — a state only a bypass could reach —
    /// is rejected, and the ambiguous conflicting retry of the same
    /// correlation is a deterministic conflict.
    #[test]
    fn subagent_terminal_pair_retry_must_be_unambiguous() {
        let store = store();
        let conversation_id = store.conversation_id().clone();

        // Only the notice side is durable (committed through the plain
        // acceptance seam): the pair retry must not complete.
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
        own_normal_subagent(&store, &subagent_id, &child_agent_id);
        let (notice, report, event) =
            success_publication(&store, &subagent_id, &child_agent_id, "report");
        store.accept_inbound(notice.clone()).expect("notice alone");
        assert!(matches!(
            store.accept_subagent_terminal(Some(notice), report, event),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(
            store.load_pending().unwrap().len(),
            1,
            "the mismatched pair retry accepted nothing further"
        );

        // Only the report side is durable: symmetric rejection.
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 2);
        let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
        own_normal_subagent(&store, &subagent_id, &child_agent_id);
        let (notice, report, event) =
            success_publication(&store, &subagent_id, &child_agent_id, "report");
        store.accept_inbound(report.clone()).expect("report alone");
        assert!(matches!(
            store.accept_subagent_terminal(Some(notice), report, event),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(store.load_pending().unwrap().len(), 2);

        // A committed pair retried with the same correlations but changed
        // content is a deterministic correlation conflict.
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 3);
        let child_agent_id = crate::runtime::identity::AgentId::new(format!("agent-{subagent_id}"));
        own_normal_subagent(&store, &subagent_id, &child_agent_id);
        let (notice, report, event) =
            success_publication(&store, &subagent_id, &child_agent_id, "report");
        store
            .accept_subagent_terminal(Some(notice), report, event)
            .expect("pair commits");
        let (notice, mut report, event) =
            success_publication(&store, &subagent_id, &child_agent_id, "report");
        report.content = vec![UserContentBlock::Text(TextBlock {
            text: "a conflicting report body".to_owned(),
        })];
        assert!(matches!(
            store.accept_subagent_terminal(Some(notice), report, event),
            Err(ConversationStoreError::CorrelationConflict { .. })
        ));
        assert_eq!(store.load_pending().unwrap().len(), 4, "exactly once");
    }

    fn workflow_terminal_fixture(
        node_instance: &crate::runtime::workflow::WorkflowNodeInstance,
        normal: bool,
    ) -> (
        SqliteConversationStore,
        RuntimeEventEnvelope,
        RuntimeEventEnvelope,
    ) {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let mut node_instance = node_instance.clone();
        node_instance.block.run.conversation_id = conversation_id.clone();
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let child_agent_id = AgentId::new(format!("agent-{subagent_id}"));
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap();
        if normal {
            own_normal_subagent(&store, &subagent_id, &child_agent_id);
        } else {
            store
                .append_event(envelope(
                    &conversation_id,
                    crate::runtime::subagent::subagent_ownership_event_id(&subagent_id).as_ref(),
                    None,
                    RuntimeEvent::SubagentOwnershipCommitted {
                        parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                        admitted_authority: None,
                        subagent_id: subagent_id.clone(),
                        child_agent_id: child_agent_id.clone(),
                        child_conversation_id:
                            crate::scripted_suites::common::identity::child_conversation_id(
                                subagent_id.as_str(),
                            ),
                        origin: crate::runtime::subagent::AgentActivationOrigin::Workflow {
                            node_id: Box::new(node_instance.clone()),
                        },
                        agent: "reviewer".to_owned(),
                        definition_digest: "sha256:definition".to_owned(),
                        profile_digest: "sha256:profile".to_owned(),
                        ownership: crate::events::types::SubagentOwnershipKind::Workflow,
                        workspace: std::path::PathBuf::from("/shared-workspace"),
                    },
                ))
                .expect("ownership fact");
        }

        let workflow_id = node_instance.block.definition.workflow_id.clone();
        let run_id = node_instance.block.run.clone();
        let terminal = crate::runtime::subagent::terminal_settlement(
            &conversation_id,
            &subagent_id,
            &child_agent_id,
            crate::events::types::SubagentTerminalState::Succeeded,
            true,
            timestamp,
        );
        let output = crate::runtime::subagent::workflow_output_event(
            &conversation_id,
            &subagent_id,
            &workflow_id,
            &run_id,
            &node_instance,
            serde_json::json!({"passed": true}),
            timestamp,
        );

        (store, terminal, output)
    }

    #[test]
    fn workflow_ownership_requires_matching_kind_origin_and_parent_conversation() {
        use crate::events::types::SubagentOwnershipKind;
        use crate::runtime::subagent::AgentActivationOrigin;
        for case in [
            "foreign_run",
            "foreign_envelope",
            "ordinary_origin",
            "normal_kind",
        ] {
            let (source, terminal, output) = workflow_terminal_fixture(
                &crate::runtime::workflow::test_instance("review_pr", "review"),
                false,
            );
            let target = store();
            let mut ownership = source.read_events(None, 10).unwrap().events[0].clone();
            ownership.sequence = 0;
            assert_eq!(&ownership.conversation_id, target.conversation_id());
            let mut invalid = ownership.clone();
            let RuntimeEvent::SubagentOwnershipCommitted {
                origin,
                ownership: kind,
                child_agent_id,
                ..
            } = &mut invalid.event
            else {
                unreachable!()
            };
            let child = child_agent_id.clone();
            match case {
                "foreign_run" | "foreign_envelope" => {
                    let AgentActivationOrigin::Workflow { node_id } = origin else {
                        unreachable!()
                    };
                    node_id.block.run.conversation_id = ConversationId::generate();
                    if case == "foreign_envelope" {
                        invalid.conversation_id = node_id.block.run.conversation_id.clone();
                    }
                }
                "ordinary_origin" => *origin = AgentActivationOrigin::ClientControl,
                "normal_kind" => *kind = SubagentOwnershipKind::Normal,
                _ => unreachable!(),
            }
            let result = if case == "normal_kind" {
                // Supply legitimate private authority so this tests the kind/origin
                // boundary, not an unrelated missing-authority rejection.
                let (event, authority) =
                    crate::local_runtime::session::tests::deletion_tests::admit_agent(invalid);
                target.append_agent_admission(event, &authority)
            } else {
                target.append_event(invalid)
            };
            if case == "foreign_envelope" {
                assert!(
                    matches!(
                        result,
                        Err(ConversationStoreError::ConversationIdMismatch { .. })
                    ),
                    "{case}: {result:?}"
                );
            } else {
                assert!(
                    matches!(result, Err(ConversationStoreError::InvalidReference(_))),
                    "{case}: {result:?}"
                );
            }
            assert!(
                target.read_events(None, 10).unwrap().events.is_empty(),
                "{case}"
            );
            assert!(
                target.load_agent_authority(&child).is_err(),
                "no partial private authority: {case}"
            );
            let lifecycle_count: i64 = target
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM lifecycle_state", [], |row| row.get(0))
                .unwrap();
            assert_eq!(lifecycle_count, 0, "no partial lifecycle: {case}");
            // The same exact identity is still admissible after rejection.
            target.append_event(ownership).unwrap();
            let committed = target
                .commit_workflow_agent_terminal(terminal.clone(), output.clone())
                .unwrap();
            assert_eq!(
                target
                    .commit_workflow_agent_terminal(terminal, output)
                    .unwrap(),
                committed
            );
            assert_eq!(target.read_events(None, 10).unwrap().events.len(), 3);
        }
    }

    #[test]
    fn normal_agent_creation_and_resume_ownership_keep_their_native_admission_paths() {
        let store = store();
        let first =
            crate::runtime::identity::SubagentId::for_conversation(store.conversation_id(), 1);
        let second =
            crate::runtime::identity::SubagentId::for_conversation(store.conversation_id(), 2);
        let child = AgentId::new("durable-ordinary-agent");
        own_normal_subagent(&store, &first, &child);
        let (notice, report, event) = success_publication(&store, &first, &child, "first result");
        store
            .accept_subagent_terminal(Some(notice), report, event)
            .unwrap();
        own_normal_subagent(&store, &second, &child);
        let (notice, report, event) =
            success_publication(&store, &second, &child, "resumed result");
        store
            .accept_subagent_terminal(Some(notice), report, event)
            .unwrap();
        assert!(store.load_agent_authority(&child).is_ok());
        let events = store.read_events(None, 10).unwrap().events;
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeEvent::SubagentOwnershipCommitted { .. }
                ))
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    RuntimeEvent::AgentActivationAdmission {
                        phase: crate::events::types::AgentActivationAdmissionPhase::Reserved,
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn workflow_agent_value_and_child_terminal_are_one_atomic_transition() {
        let (store, terminal, output) = workflow_terminal_fixture(
            &crate::runtime::workflow::test_instance("review_pr", "review"),
            false,
        );
        let mut unproven = terminal.clone();
        if let RuntimeEvent::SubagentTerminalSettled {
            physical_settlement_proven,
            ..
        } = &mut unproven.event
        {
            *physical_settlement_proven = false;
        }
        assert!(
            matches!(
                store.commit_workflow_agent_terminal(unproven, output.clone()),
                Err(ConversationStoreError::InvalidReference(_))
            ),
            "a valid value cannot commit Workflow success without physical proof"
        );
        assert_eq!(store.read_events(None, 10).unwrap().events.len(), 1);

        // A fault after both event rows are staged must roll the pair back;
        // the ownership fact is the only durable state left.
        store.arm_fail_event_times(1);
        assert!(
            store
                .commit_workflow_agent_terminal(terminal.clone(), output.clone())
                .is_err()
        );
        assert_eq!(store.read_events(None, 10).unwrap().events.len(), 1);

        let committed = store
            .commit_workflow_agent_terminal(terminal.clone(), output.clone())
            .expect("compound Workflow terminal");
        assert_eq!(committed.0.sequence, 2);
        assert_eq!(committed.1.sequence, 3);
        assert_eq!(store.read_events(None, 10).unwrap().events.len(), 3);

        // An ambiguous/idempotent retry returns the same two durable facts and
        // cannot allocate another lifecycle terminal or value event.
        let retried = store
            .commit_workflow_agent_terminal(terminal, output)
            .expect("idempotent compound retry");
        assert_eq!(retried, committed);
        assert_eq!(store.read_events(None, 10).unwrap().events.len(), 3);
    }

    /// Every rejection leaves the exact committed prefix untouched; the valid
    /// pair can then commit and retry, including repeated/nested instances.
    #[test]
    fn workflow_terminal_rejects_inconsistent_and_substituted_typed_identities() {
        use crate::runtime::workflow::{WorkflowId, test_instance};
        for case in [
            "run_field",
            "workflow_field",
            "owned_run",
            "owned_node",
            "visit",
            "loop",
            "parallel",
        ] {
            let mut owned = test_instance("review_pr", "review");
            if case == "loop" {
                owned.block.definition.blocks = vec!["retry".into(), "body".into()];
                owned.block.invocations = vec![0, 1];
            } else if case == "parallel" {
                owned.block.definition.blocks = vec!["checks".into(), "left".into()];
                owned.block.invocations = vec![0, 0];
            }
            let (store, terminal, output) = workflow_terminal_fixture(&owned, false);
            let before = store.read_events(None, 100).unwrap().events;
            let mut invalid = output.clone();
            let RuntimeEvent::WorkflowAgentOutputCommitted {
                workflow_id,
                run_id,
                node_id,
                ..
            } = &mut invalid.event
            else {
                unreachable!()
            };
            match case {
                "run_field" => run_id.invocation += 1,
                "workflow_field" => *workflow_id = WorkflowId::parse("other_workflow").unwrap(),
                "owned_run" => {
                    run_id.invocation += 1;
                    node_id.block.run = run_id.clone();
                }
                "owned_node" => node_id.node = "another_node".into(),
                "visit" => node_id.visit += 1,
                "loop" => node_id.block.invocations[1] += 1,
                "parallel" => node_id.block.definition.blocks[1] = "right".into(),
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    store.commit_workflow_agent_terminal(terminal.clone(), invalid.clone()),
                    Err(ConversationStoreError::InvalidReference(_))
                ),
                "{case}"
            );
            assert_eq!(
                store.read_events(None, 100).unwrap().events,
                before,
                "{case}"
            );
            let committed = store
                .commit_workflow_agent_terminal(terminal.clone(), output.clone())
                .unwrap();
            assert_eq!(
                store
                    .commit_workflow_agent_terminal(terminal.clone(), output)
                    .unwrap(),
                committed
            );
            let after = store.read_events(None, 100).unwrap().events;
            assert_eq!(after.len(), before.len() + 2);
            // The retry fast path must not bypass identity validation either.
            assert!(
                matches!(
                    store.commit_workflow_agent_terminal(terminal, invalid),
                    Err(ConversationStoreError::InvalidReference(_))
                ),
                "{case}"
            );
            assert_eq!(store.read_events(None, 100).unwrap().events, after);
        }
    }

    #[test]
    fn workflow_terminal_rejects_normal_agent_ownership_without_partial_facts() {
        let (store, terminal, output) = workflow_terminal_fixture(
            &crate::runtime::workflow::test_instance("review_pr", "review"),
            true,
        );
        let before = store.read_events(None, 100).unwrap().events;
        assert!(matches!(
            store.commit_workflow_agent_terminal(terminal.clone(), output),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(store.read_events(None, 100).unwrap().events, before);
        let RuntimeEvent::SubagentTerminalSettled {
            subagent_id,
            child_agent_id,
            ..
        } = terminal.event
        else {
            unreachable!()
        };
        let (notice, report, event) =
            success_publication(&store, &subagent_id, &child_agent_id, "ordinary success");
        store
            .accept_subagent_terminal(Some(notice), report, event)
            .expect("ordinary Agent remains publishable");
        assert_eq!(
            store.read_events(None, 100).unwrap().events.len(),
            before.len() + 1
        );
    }

    /// The frozen Session execution binding in an ownership fact is one
    /// absolute directory; a relative binding is refused before any write.
    #[test]
    fn subagent_ownership_rejects_a_relative_session_workspace_binding() {
        let store = store();
        let conversation_id = store.conversation_id().clone();
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 1);
        let workspace = std::path::PathBuf::from("relative/workspace");
        let error = store
            .append_event(envelope(
                &conversation_id,
                crate::runtime::subagent::subagent_ownership_event_id(&subagent_id).as_ref(),
                None,
                RuntimeEvent::SubagentOwnershipCommitted {
                    parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                    admitted_authority: None,
                    subagent_id: subagent_id.clone(),
                    child_agent_id: AgentId::new(format!("agent-{subagent_id}")),
                    child_conversation_id:
                        crate::scripted_suites::common::identity::child_conversation_id(
                            subagent_id.as_str(),
                        ),
                    origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                        tool_call_id: crate::runtime::identity::ToolCallId::new("call-sub"),
                    },
                    agent: "worker".to_owned(),
                    definition_digest: "sha256:definition".to_owned(),
                    profile_digest: "sha256:profile".to_owned(),
                    ownership: crate::events::types::SubagentOwnershipKind::Normal,
                    workspace,
                },
            ))
            .expect_err("durable ownership must reject a relative workspace binding");
        assert!(matches!(error, ConversationStoreError::InvalidReference(_)));
        assert!(store.read_events(None, 8).unwrap().events.is_empty());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn subagent_terminal_provenance_is_authorized_by_ownership_fact() {
        let store = store();
        let conversation_id = store.conversation_id().clone();

        let own = |ordinal: u64, child_agent_id: &AgentId| {
            let subagent_id =
                crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
            own_normal_subagent(&store, &subagent_id, child_agent_id);
        };
        let publish = |ordinal: u64,
                       claimed_child: &AgentId,
                       authored_by: &AgentId,
                       suffix: &str|
         -> Result<(), ConversationStoreError> {
            let subagent_id =
                crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
            let (notice, report, event) = success_publication(
                &store,
                &subagent_id,
                claimed_child,
                &format!("terminal-{suffix}"),
            );
            let mut report = report;
            report.source = UserSource::Agent {
                agent_id: authored_by.clone(),
            };
            store
                .accept_subagent_terminal(Some(notice), report, event)
                .map(|_| ())
        };

        let child_a = AgentId::new("agent-a");
        let child_b = AgentId::new("agent-b");

        // A terminal for a child with no durable ownership fact is an
        // invalid reference: the deterministic ownership-identity lookup
        // finds no opening fact.
        assert!(matches!(
            publish(50, &child_a, &child_a, "missing-ownership"),
            Err(ConversationStoreError::InvalidReference(_))
        ));

        // The event's repeated child identity is not authority: S owns A,
        // so a success claiming B and authored by B is rejected.
        own(10, &child_a);
        assert!(matches!(
            publish(10, &child_b, &child_b, "wrong-child"),
            Err(ConversationStoreError::InvalidReference(_))
        ));

        // Correct child provenance succeeds.
        own(11, &child_a);
        publish(11, &child_a, &child_a, "success").expect("correct Agent(A) success");

        // Success must not be Runtime-authored.
        own(12, &child_a);
        {
            let subagent_id =
                crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 12);
            let (notice, mut report, event) =
                success_publication(&store, &subagent_id, &child_a, "terminal-runtime-success");
            report.source = UserSource::Runtime;
            assert!(matches!(
                store.accept_subagent_terminal(Some(notice), report, event),
                Err(ConversationStoreError::InvalidReference(_))
            ));
        }

        // A failed/cancelled/interrupted terminal must not be Agent-authored.
        own(13, &child_a);
        {
            let subagent_id =
                crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 13);
            let (mut report, event) = non_success_publication(
                &store,
                &subagent_id,
                &child_a,
                SubagentTerminalState::Failed,
                "subagent failed",
            );
            report.source = UserSource::Agent {
                agent_id: child_a.clone(),
            };
            assert!(matches!(
                store.accept_subagent_terminal(None, report, event),
                Err(ConversationStoreError::InvalidReference(_))
            ));
        }

        for (ordinal, state) in [
            (14, SubagentTerminalState::Failed),
            (15, SubagentTerminalState::Cancelled),
            (16, SubagentTerminalState::Interrupted),
        ] {
            own(ordinal, &child_a);
            let subagent_id =
                crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
            let (report, event) =
                non_success_publication(&store, &subagent_id, &child_a, state, "runtime terminal");
            store
                .accept_subagent_terminal(None, report, event)
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
                parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                admitted_authority: None,
                subagent_id: s1.clone(),
                child_agent_id: AgentId::new("agent-a"),
                child_conversation_id: crate::runtime::identity::ConversationId::new(
                    "conv_4761f81f-3cff-7c88-84c5-21410a0b63f3",
                ),
                origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: crate::runtime::identity::ToolCallId::new("call-a"),
                },
                agent: "explore".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                profile_digest: "sha256:profile".to_owned(),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                workspace: std::path::PathBuf::from("/shared-workspace"),
            },
        );
        let (malformed, authority) =
            crate::local_runtime::session::tests::deletion_tests::admit_agent(malformed);
        assert!(matches!(
            store.append_agent_admission(malformed, &authority),
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
                parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                admitted_authority: None,
                subagent_id: s1.clone(),
                child_agent_id: AgentId::new("agent-a"),
                child_conversation_id: crate::runtime::identity::ConversationId::new(
                    "conv_4761f81f-3cff-7c88-84c5-21410a0b63f3",
                ),
                origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: crate::runtime::identity::ToolCallId::new("call-a"),
                },
                agent: "explore".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                profile_digest: "sha256:profile".to_owned(),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                workspace: std::path::PathBuf::from("/shared-workspace"),
            },
        );
        let (canonical, authority) =
            crate::local_runtime::session::tests::deletion_tests::admit_agent(canonical);
        store
            .append_agent_admission(canonical, &authority)
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
                parent_agent_id: crate::runtime::identity::AgentId::new("agent-parent"),
                admitted_authority: None,
                subagent_id: s2.clone(),
                child_agent_id: AgentId::new("agent-b"),
                child_conversation_id: crate::runtime::identity::ConversationId::new(
                    "conv_027fd54f-460d-7e47-84f4-ffff3ee8af4a",
                ),
                origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: crate::runtime::identity::ToolCallId::new("call-b"),
                },
                agent: "explore".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                profile_digest: "sha256:profile".to_owned(),
                ownership: crate::events::types::SubagentOwnershipKind::Normal,
                workspace: std::path::PathBuf::from("/shared-workspace"),
            },
        );
        {
            let mut connection = store.lock().expect("store lock");
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .expect("transaction");
            transaction
                .execute(
                    "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,NULL,NULL,?5,json_extract(?5,'$.timestamp'))",
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
        let (notice, report, event) =
            success_publication(&store, &s1, &AgentId::new("agent-b"), "terminal");
        assert!(
            matches!(
                store.accept_subagent_terminal(Some(notice), report, event),
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
                        failed_request_id: RequestId::new("request:attempt-events:turn-events:0"),
                        retry_number: 1,
                        retry_delay_ms: None,
                    },
                );
                late.turn_id = Some(TurnId::new("turn-events"));
                late
            }),
            // The event is rejected at the new request-correlation boundary
            // before lifecycle absorption can be checked: this fixture has
            // no durable failed request with that identity.
            Err(ConversationStoreError::InvalidReference(_))
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
                    RuntimeEvent::CompactionStarted {
                        operation_id: crate::runtime::identity::CompactionOperationId::new(
                            "fixture-compaction"
                        )
                    },
                ))
                .is_err()
        );
        let standalone = store
            .append_event(envelope(
                &conversation_id,
                "standalone-event",
                None,
                RuntimeEvent::CompactionStarted {
                    operation_id: crate::runtime::identity::CompactionOperationId::new(
                        "fixture-compaction",
                    ),
                },
            ))
            .unwrap();
        assert_eq!(standalone.sequence, 6);
        assert!(
            store
                .append_event(envelope(
                    &conversation_id,
                    "standalone-event",
                    None,
                    RuntimeEvent::CompactionStarted {
                        operation_id: crate::runtime::identity::CompactionOperationId::new(
                            "fixture-compaction"
                        )
                    },
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
        let receipt = store
            .commit_projection(
                crate::scripted_suites::support::context::checkpoint_commit_fixture(
                    &store,
                    crate::scripted_suites::support::context::checkpoint_fixture(
                        &store,
                        "bootstrap-checkpoint",
                        99,
                        "bounded",
                    ),
                    None,
                ),
            )
            .unwrap();
        let history = store.projected_history().unwrap();
        let active = history
            .messages
            .into_iter()
            .filter_map(|m| m.as_canonical().cloned())
            .collect::<Vec<_>>();
        let active_ids = active.iter().map(|m| m.id().clone()).collect();
        let state = ConversationState::from_durable_head(
            active,
            active_ids,
            revision,
            receipt.checkpoint.activation.activation.0,
        )
        .unwrap();
        assert_eq!(state.ledger().len(), 1);
        assert_eq!(state.surface().len(), 1);
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
        let conversation_id = ConversationId::new("conv_30d36aa9-aeae-79e9-8322-f17c7d13842a");
        drop(SqliteConversationStore::open(conversation_id, &path).unwrap());

        assert!(matches!(
            SqliteConversationStore::open(ConversationId::new("conv_ddb81244-a13c-7d38-842c-03c613793b90"), &path),
            Err(ConversationStoreError::ConversationIdMismatch { stored, requested })
                if stored == ConversationId::new("conv_30d36aa9-aeae-79e9-8322-f17c7d13842a")
                    && requested == ConversationId::new("conv_ddb81244-a13c-7d38-842c-03c613793b90")
        ));
    }

    #[test]
    fn corrupt_frozen_request_revision_metadata_fails_closed() {
        let request_store = store();
        let message = user_message("request-message", "request body");
        request_store
            .initialize(std::slice::from_ref(&message))
            .unwrap();
        let mut snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("request-corruption-attempt"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            crate::context::resources::LedgerFrontier(
                (request_store.load_head().unwrap().revision).get(),
            ),
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
        freeze_test_input(&request_store, &mut snapshot, &[]);
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
            value["canonical_frontier"] = serde_json::json!(999_u64);
            connection
                .execute(
                    "UPDATE request_snapshots SET canonical_frontier=999,snapshot_json=?1 WHERE request_id=?2",
                    params![value.to_string(), snapshot.request_id.as_str()],
                )
                .unwrap();
        }
        assert!(matches!(
            request_store.reconstruct_model_request(&snapshot.request_id),
            Err(ConversationStoreError::InvalidReference(detail))
                if detail.contains("manifest integrity mismatch")
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One retained-input loss, independent current execution and real SQLite restart proof.
    fn incomplete_historical_snapshot_does_not_block_independently_complete_current_checkpoint_or_restart()
     {
        use crate::context::resources::LedgerFrontier;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot-closure.sqlite");
        let id = ConversationId::generate();
        let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
        let reasoning = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("old-native"),
            content: vec![AssistantContentBlock::Reasoning(
                crate::message::types::ReasoningBlock {
                    text: None,
                    provider_state: Some(
                        crate::runtime::continuation::ProviderContinuationState::OpenAiResponses(
                            crate::runtime::continuation::OpenAiResponsesContinuation::Stateless {
                                items: vec![
                                    serde_json::json!({"type":"reasoning","id":"rs-old","encrypted_content":"retained-native-bytes"}),
                                ],
                            },
                        ),
                    ),
                },
            )],
        });
        store
            .initialize(&[user_message("old-user", "historical input"), reasoning])
            .unwrap();
        let mut first = test_request_snapshot(store.load_head().unwrap().revision, invocation());
        first.invocation.protocol = ModelProtocol::OpenAiResponses;
        first.invocation.compat.responses_storage =
            crate::model::catalog::ResponsesStorageMode::Stateless;
        freeze_test_input(&store, &mut first, &[]);
        store
            .commit_model_turn_start(&[], &first, Utc::now())
            .unwrap();
        let mut completed = envelope(
            store.conversation_id(),
            "old-request-completed",
            Some(first.identity.attempt_id.clone()),
            RuntimeEvent::ModelRequestCompleted {
                request_id: first.request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: None,
                generation: None,
            },
        );
        completed.turn_id = Some(first.identity.turn.clone());
        store.append_event(completed).unwrap();
        let exact_first = store.reconstruct_model_request(&first.request_id).unwrap();
        assert_eq!(
            exact_first.semantic_input_fingerprint(),
            first.semantic_input_fingerprint
        );
        store
            .commit_projection(
                crate::scripted_suites::support::context::checkpoint_commit_fixture(
                    &store,
                    crate::scripted_suites::support::context::checkpoint_fixture(
                        &store,
                        "current-checkpoint",
                        2,
                        "independently complete current summary",
                    ),
                    None,
                ),
            )
            .unwrap();
        {
            let connection = store.lock().unwrap();
            // Deliberately corrupt retained original authority, bypassing the
            // derived catalog FK that ordinarily prevents this deletion.
            connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            connection
                .execute(
                    "DELETE FROM request_snapshots WHERE request_id=?1",
                    [first.request_id.as_str()],
                )
                .unwrap();
            connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        }
        assert!(
            matches!(
                store.reconstruct_model_request(&first.request_id),
                Err(ConversationStoreError::RequestNotFound(_))
            ),
            "missing retained input never claims exact reconstruction"
        );
        store
            .append_canonical(&user_message("new-user", "fresh current input"))
            .unwrap();
        let mut next = test_request_snapshot(store.load_head().unwrap().revision, invocation());
        next.identity.turn = TurnId::new("next");
        next.request_id = next.identity.request_id();
        next.provisional_message_id = next.identity.provisional_message_id();
        freeze_test_input(&store, &mut next, &[]);
        assert_eq!(next.history_cut.base_frontier, LedgerFrontier(2));
        store
            .commit_model_turn_start(&[], &next, Utc::now())
            .unwrap();
        let before_restart = store.reconstruct_model_request(&next.request_id).unwrap();
        drop(store);
        let reopened = SqliteConversationStore::open_existing(id, &path).unwrap();
        assert_eq!(
            reopened
                .reconstruct_model_request(&next.request_id)
                .unwrap(),
            before_restart
        );
        assert!(reopened.projected_history().is_ok());
    }

    #[test]
    fn missing_frozen_original_ledger_dependency_fails_closed() {
        let missing_message_store = store();
        let missing_message = user_message("missing-request-message", "request body");
        missing_message_store
            .initialize(std::slice::from_ref(&missing_message))
            .unwrap();
        let mut missing_snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new("missing-message-attempt"),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            crate::context::resources::LedgerFrontier(
                (missing_message_store.load_head().unwrap().revision).get(),
            ),
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
        missing_snapshot
            .freeze_input(
                &missing_message_store.conversation_id,
                &ModelRequest {
                    continuation_source: None,
                    images: crate::model::images::ResolvedImages::new(),
                    invocation: missing_snapshot.invocation.clone(),
                    messages: crate::model::input::canonical_input(std::slice::from_ref(
                        &missing_message,
                    ))
                    .into(),
                    tools: vec![],
                    effective_system_prompt: missing_snapshot.effective_system_prompt.clone(),
                    continuation: None,
                },
            )
            .unwrap();
        missing_message_store
            .commit_model_turn_start(&[], &missing_snapshot, Utc::now())
            .unwrap();
        {
            let connection = missing_message_store.conn.lock().unwrap();
            let missing_message_id = crate::conversation::message_id_of(&missing_message);
            connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
            connection
                .execute(
                    "DELETE FROM message_ledger WHERE message_id=?1",
                    [missing_message_id.as_str()],
                )
                .unwrap();
            connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        }
        assert!(matches!(
            missing_message_store.reconstruct_model_request(&missing_snapshot.request_id),
            Err(ConversationStoreError::InvalidReference(detail))
                if detail.contains("message missing-request-message is unavailable")
        ));
    }

    #[test]
    fn new_schema_birth_rolls_back_late_ddl_and_commits_once_with_wal_normal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("schema-birth.sqlite");
        let mut connection = Connection::open(&path).unwrap();
        configure_connection(&mut connection, false).unwrap();
        let (conflict, _) = PRESENTATION_INDEXES[0];
        connection
            .execute_batch(&format!(
                "CREATE TABLE schema_birth_sentinel(value TEXT); INSERT INTO schema_birth_sentinel VALUES('original'); CREATE INDEX {conflict} ON schema_birth_sentinel(value);"
            ))
            .unwrap();
        assert!(create_schema(&mut connection).is_err());
        assert!(connection.is_autocommit());
        let objects = connection
            .prepare("SELECT name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let mut original = vec![conflict.to_owned(), "schema_birth_sentinel".to_owned()];
        original.sort();
        assert_eq!(
            objects, original,
            "no partially born schema survives late DDL failure"
        );
        let sentinel: String = connection
            .query_row("SELECT value FROM schema_birth_sentinel", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(sentinel, "original");
        connection
            .execute_batch(&format!(
                "DROP INDEX {conflict}; DROP TABLE schema_birth_sentinel;"
            ))
            .unwrap();
        let commits = Arc::new(AtomicUsize::new(0));
        let observed = commits.clone();
        connection
            .commit_hook(Some(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                false
            }))
            .unwrap();
        create_schema(&mut connection).unwrap();
        assert_eq!(
            commits.load(Ordering::SeqCst),
            1,
            "all schema DDL and validation share one commit"
        );
        verify_schema_shape(&connection).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        let conversation = ConversationId::new("conv_00000000-0000-7000-8000-000000000467");
        bind_identity(&mut connection, &conversation).unwrap();
        assert_eq!(
            commits.load(Ordering::SeqCst),
            2,
            "identity retains its separate native binding transaction"
        );
        drop(connection);
        let reopened = SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap();
        assert_eq!(reopened.conversation_id, conversation);
    }

    #[test]
    fn new_schema_birth_rolls_back_when_shape_validation_fails() {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

        let mut connection = Connection::open_in_memory().unwrap();
        configure_connection(&mut connection, true).unwrap();
        let verification = Arc::new(AtomicBool::new(false));
        let observed = verification.clone();
        connection
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Pragma {
                        pragma_name: "table_info",
                        pragma_value: Some("inherited_turn_provenance")
                    }
                ) {
                    observed.store(true, Ordering::SeqCst);
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }))
            .unwrap();
        assert!(create_schema(&mut connection).is_err());
        assert!(
            verification.load(Ordering::SeqCst),
            "the failure occurred during final shape validation"
        );
        connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        assert!(connection.is_autocommit());
        let objects: u64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            objects, 0,
            "validation failure rolls back every new table, index and root row"
        );
        create_schema(&mut connection).unwrap();
        verify_schema_shape(&connection).unwrap();
    }

    #[test]
    fn exact_frozen_request_needs_no_surface_history_replay() {
        let store = store();
        let seed = user_message("seed", "retained exact input");
        store.initialize(std::slice::from_ref(&seed)).unwrap();
        let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
        let request = ModelRequest {
            continuation_source: None,
            images: crate::model::images::ResolvedImages::new(),
            invocation: snapshot.invocation.clone(),
            messages: crate::model::input::canonical_input(&[seed]).into(),
            tools: vec![],
            effective_system_prompt: snapshot.effective_system_prompt.clone(),
            continuation: None,
        };
        snapshot
            .freeze_input(&store.conversation_id, &request)
            .unwrap();
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute("DELETE FROM surface_ops", [])
            .unwrap();
        assert!(
            store
                .reconstruct_surface(crate::conversation::SurfaceRevision::new(
                    snapshot.canonical_frontier.0
                ))
                .is_err()
        );
        assert_eq!(
            store
                .reconstruct_model_request(&snapshot.request_id)
                .unwrap(),
            request
        );
    }

    #[test]
    fn oversized_frozen_record_is_rejected_before_body_decode() {
        let store = store();
        store.initialize(&[]).unwrap();
        let snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_snapshots SET snapshot_json=CAST(zeroblob(67108865) AS TEXT)",
                [],
            )
            .unwrap();
        assert!(
            matches!(store.load_request_snapshot(&snapshot.request_id), Err(ConversationStoreError::InvalidReference(detail)) if detail.contains("record exceeds the byte budget"))
        );
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
            SqliteConversationStore::open(
                ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
                &path
            ),
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
        let conversation_id = ConversationId::new("conv_22848931-dd71-7669-86f8-0840ad55e434");
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

    /// The exact `events.event_json` a schema-30 runtime writes for a
    /// Questionnaire request, produced by `origin/main`'s own serializer.
    ///
    /// It is the obsolete choice-only shape: each question carries `options`
    /// and `multi_select` instead of a typed `answer`, and the subject records
    /// no requester identity at all.
    const V30_QUESTIONNAIRE_REQUESTED: &str = r#"{"schema_version":1,"event_id":"interaction-requested-v31","sequence":1,"conversation_id":"conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73","timestamp":"2026-01-01T00:00:00Z","event":{"type":"interaction_requested","interaction_id":"interaction-v31","subject":{"type":"questionnaire","invocation_id":{"caller":"agent","call_id":"legacy-call"},"questionnaire":{"questions":[{"question":"Which direction?","header":"Direction","options":[{"label":"First","description":"The first authored option."},{"label":"Second","description":"The second authored option."}],"multi_select":false}]}}}}"#;

    /// The matching schema-30 settlement: the answer names the authored
    /// **label**, which the typed vocabulary replaced with an option index.
    const V30_QUESTIONNAIRE_SETTLED: &str = r#"{"schema_version":1,"event_id":"interaction-settled-v31","sequence":2,"conversation_id":"conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73","timestamp":"2026-01-01T00:00:01Z","event":{"type":"interaction_settled","interaction_id":"interaction-v31","settlement":{"type":"questionnaire_submitted","submission":{"answers":[{"question_index":0,"answer":{"type":"single_option","value":{"label":"First"}}}]}}}}"#;

    /// Issue #242 changed the durable *semantic vocabulary* of the
    /// Questionnaire interaction audit, so the store version must gate open.
    ///
    /// A version-30 journal can hold the obsolete choice-only payload above.
    /// Under the typed vocabulary an authored label is not an option index and
    /// a request has no requester identity to recover, so there is nothing a
    /// v31 reader could honestly make of those rows. The contract is
    ///
    /// ```text
    /// incompatible durable semantic format -> store-open rejection
    /// ```
    ///
    /// and never "open, then fail later while decoding an old record". This
    /// proves both halves: `open` refuses the file, and the rows it refused
    /// really are undecodable under the current vocabulary — so the gate is
    /// what keeps the typed decoder from ever seeing them.
    #[test]
    fn a_version_30_questionnaire_journal_is_refused_at_open_never_decoded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("questionnaire-v31.sqlite");
        let conversation_id = ConversationId::new("conv_7ee23a93-1fb2-7d39-84d8-def51c2d5f73");
        {
            // A fully-shaped store at the current schema, carrying the exact
            // rows a schema-30 runtime would have written, then downgraded to
            // the version that actually wrote them. The table shape is
            // unchanged — only the meaning of `event_json` moved, which is
            // precisely the class of change the version exists to gate.
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            let connection = store.conn.lock().unwrap();
            for (sequence, event_id, payload) in [
                (
                    1i64,
                    "interaction-requested-v31",
                    V30_QUESTIONNAIRE_REQUESTED,
                ),
                (2i64, "interaction-settled-v31", V30_QUESTIONNAIRE_SETTLED),
            ] {
                connection
                    .execute(
                        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,NULL,NULL,?5,json_extract(?5,'$.timestamp'))",
                        params![
                            sequence,
                            event_id,
                            i64::from(EVENT_SCHEMA_VERSION),
                            conversation_id.as_str(),
                            payload
                        ],
                    )
                    .unwrap();
            }
            connection
                .execute(
                    "UPDATE rustx_store SET schema_version = 30, next_event_sequence = 2 WHERE id = 1",
                    [],
                )
                .unwrap();
        }

        // The gate refuses the file, by version, at `open`. No store handle
        // exists afterwards, so no Event Journal read of those rows is
        // reachable at all.
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 30,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
        assert_eq!(SQLITE_SCHEMA_VERSION, 74);

        // And the refusal is not ceremony: had the gate admitted the file,
        // these are the rows the typed decoder would have had to interpret,
        // and it cannot. The envelope framing is unchanged (still
        // `EVENT_SCHEMA_VERSION`), which is exactly why the *store* version —
        // not the envelope version — is the one that had to move.
        for payload in [V30_QUESTIONNAIRE_REQUESTED, V30_QUESTIONNAIRE_SETTLED] {
            let framing: serde_json::Value = serde_json::from_str(payload).unwrap();
            assert_eq!(framing["schema_version"], i64::from(EVENT_SCHEMA_VERSION));
            assert!(
                serde_json::from_str::<RuntimeEventEnvelope>(payload).is_err(),
                "the typed vocabulary cannot interpret a schema-30 Questionnaire payload: {payload}"
            );
        }
    }

    /// A schema-31 subagent ownership fact: `(agent, definition_digest)` and
    /// no effective execution-profile identity at all.
    const V31_SUBAGENT_OWNERSHIP: &str = r#"{"schema_version":1,"event_id":"subagent-committed-event:conv-v31-subagent-1","sequence":1,"conversation_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd","timestamp":"2026-01-01T00:00:00Z","event":{"type":"subagent_ownership_committed","subagent_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd-subagent-1","child_agent_id":"agent-child","child_conversation_id":"conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd-subagent-1","tool_call_id":"call-sub","agent":"explore","definition_digest":"sha256:d1","ownership":"normal","workspace":{"logical_workspace":"<shared-workspace>","isolation":{"mode":"shared"}}}}"#;

    /// Issue #258 made the effective execution-profile identity a durable
    /// execution fact, so the store version must gate open.
    ///
    /// A version-31 journal holds ownership facts with no `profile_digest`.
    /// The one thing a v33 reader must never do is reconstruct it — the
    /// current role definition and the current resource generation are both
    /// mutable, and deriving identity from them is exactly what committing
    /// the value durably exists to prevent. There is therefore nothing honest
    /// to make of those rows, and the contract is the usual one:
    ///
    /// ```text
    /// incompatible durable semantic format -> store-open rejection
    /// ```
    #[test]
    fn a_version_31_subagent_ownership_journal_is_refused_at_open_never_decoded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ownership-v31.sqlite");
        let conversation_id = ConversationId::new("conv_5b135cc1-effd-7c32-88a8-1e18c1b056bd");
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            let connection = store.conn.lock().unwrap();
            connection
                .execute(
                    "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,NULL,NULL,?5,json_extract(?5,'$.timestamp'))",
                    params![
                        1i64,
                        "subagent-committed-event:conv-v31-subagent-1",
                        i64::from(EVENT_SCHEMA_VERSION),
                        conversation_id.as_str(),
                        V31_SUBAGENT_OWNERSHIP
                    ],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE rustx_store SET schema_version = 31, next_event_sequence = 1 WHERE id = 1",
                    [],
                )
                .unwrap();
        }

        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 31,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
        assert_eq!(SQLITE_SCHEMA_VERSION, 74);

        // And the refusal is not ceremony: the envelope framing is unchanged,
        // and the row the gate refused really is undecodable under the current
        // vocabulary — which is exactly why the *store* version had to move.
        let framing: serde_json::Value = serde_json::from_str(V31_SUBAGENT_OWNERSHIP).unwrap();
        assert_eq!(framing["schema_version"], i64::from(EVENT_SCHEMA_VERSION));
        assert!(
            framing["event"].get("profile_digest").is_none(),
            "the schema-31 fact really does carry no effective profile identity"
        );
        assert!(
            serde_json::from_str::<RuntimeEventEnvelope>(V31_SUBAGENT_OWNERSHIP).is_err(),
            "the typed vocabulary cannot interpret a schema-31 ownership fact"
        );
    }

    #[test]
    fn journal_cut_observer_reports_commits_but_never_reads_or_rollbacks() {
        #[derive(Default)]
        struct Observer(Mutex<Vec<Option<u64>>>);
        impl super::super::presentation::JournalObserver for Observer {
            fn committed(&self, through: Option<u64>) {
                self.0.lock().unwrap().push(through);
            }
        }
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936178");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        let observer = Arc::new(Observer::default());
        assert_eq!(store.observe_journal(observer.clone()).unwrap(), 0);
        let mut event = envelope(
            &id,
            "committed",
            Some(AttemptId::new("attempt")),
            RuntimeEvent::TurnStarted,
        );
        event.turn_id = Some(TurnId::new("1"));
        let committed = store.append_event(event.clone()).unwrap();
        assert_eq!(*observer.0.lock().unwrap(), vec![Some(committed.sequence)]);
        assert!(store.append_event(event).is_err()); // Rejected duplicate is not a new cut.
        store.presentation_frontier().unwrap();
        store.load_head().unwrap();
        {
            let mut connection = store.lock().unwrap();
            let transaction = connection.transaction().unwrap();
            transaction.execute("UPDATE rustx_store SET next_event_sequence = next_event_sequence + 1 WHERE id = 1", []).unwrap();
            transaction.rollback().unwrap();
        }
        assert_eq!(*observer.0.lock().unwrap(), vec![Some(committed.sequence)]);
        assert_eq!(
            journal_prefix(&store.lock().unwrap()).unwrap(),
            committed.sequence
        );
    }

    #[test]
    fn t19_t20_journal_readiness_is_exact_committed_and_recovered_on_reinstall() {
        struct Observer {
            store: std::sync::Weak<SqliteConversationStore>,
            ready: Mutex<Vec<(u64, u64)>>,
        }
        impl super::super::presentation::JournalObserver for Observer {
            fn committed(&self, _through: Option<u64>) {}
            fn ready(&self, through: u64) {
                let frontier = self
                    .store
                    .upgrade()
                    .unwrap()
                    .observation_ready_frontier(0, through)
                    .unwrap();
                self.ready.lock().unwrap().push((through, frontier));
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("readiness.sqlite");
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936178");
        let store = Arc::new(SqliteConversationStore::open(id.clone(), &path).unwrap());
        let observer = Arc::new(Observer {
            store: Arc::downgrade(&store),
            ready: Mutex::new(Vec::new()),
        });
        assert_eq!(store.observe_journal(observer.clone()).unwrap(), 0);
        for index in 1..=3 {
            let mut event = envelope(
                &id,
                &format!("ready-{index}"),
                Some(AttemptId::new("attempt")),
                RuntimeEvent::TurnStarted,
            );
            event.turn_id = Some(TurnId::new(index.to_string()));
            assert_eq!(store.append_event(event).unwrap().sequence, index);
        }
        assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 0);
        // Owner two publishes first. This neither releases owner one nor skips it.
        store.mark_observation_direct(2, 2).unwrap();
        assert!(
            store
                .observation_event_witness(2, 3)
                .unwrap()
                .unwrap()
                .native_direct
        );
        assert!(
            !store
                .observation_event_witness(1, 3)
                .unwrap()
                .unwrap()
                .native_direct
        );
        assert_eq!(*observer.ready.lock().unwrap(), vec![(2, 0)]);
        assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 0);
        store.mark_observation_ready(1, 1).unwrap();
        assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 2);
        store.mark_observation_ready(1, 1).unwrap();
        assert!(store.mark_observation_ready(3, 4).is_err());
        assert!(store.mark_observation_ready(0, 1).is_err());
        assert_eq!(store.observation_ready_frontier(2, 3).unwrap(), 2);
        // A rolled-back tiny layout update is invisible and creates no wake.
        let wakes = observer.ready.lock().unwrap().len();
        {
            let mut connection = store.lock().unwrap();
            let transaction = connection.transaction().unwrap();
            observation_readiness::mark(&transaction, 3, 3).unwrap();
            transaction.rollback().unwrap();
        }
        assert_eq!(observer.ready.lock().unwrap().len(), wakes);
        assert_eq!(store.observation_ready_frontier(2, 3).unwrap(), 2);
        let connection = store.lock().unwrap();
        let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT MIN(sequence) FROM event_observation_readiness WHERE ready=0 AND sequence>0 AND sequence<=3", [], |row| row.get(3)).unwrap();
        assert!(
            plan.contains("event_observation_readiness_holes_idx"),
            "{plan}"
        );
        drop(connection);
        drop(store);
        let reopened = Arc::new(SqliteConversationStore::open(id, &path).unwrap());
        assert_eq!(reopened.observation_ready_frontier(0, 3).unwrap(), 2);
        let next = Arc::new(Observer {
            store: Arc::downgrade(&reopened),
            ready: Mutex::new(Vec::new()),
        });
        assert_eq!(reopened.observe_journal(next).unwrap(), 3);
        assert_eq!(reopened.observation_ready_frontier(0, 3).unwrap(), 3);
        assert!(
            !reopened
                .observation_event_witness(2, 3)
                .unwrap()
                .unwrap()
                .native_direct
        );
    }

    #[test]
    fn t10_t19_t20_readiness_layout_failure_never_refuses_native_journal_commit() {
        struct Observer;
        impl super::super::presentation::JournalObserver for Observer {
            fn committed(&self, _through: Option<u64>) {}
        }
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936174");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        let make = |label: &str| {
            let mut event = envelope(
                &id,
                label,
                Some(AttemptId::new("readiness-failure")),
                RuntimeEvent::TurnStarted,
            );
            event.turn_id = Some(TurnId::new("1"));
            event
        };
        store.append_event(make("first")).unwrap();
        store.lock().unwrap().execute_batch("CREATE TRIGGER readiness_layout_failure BEFORE INSERT ON event_observation_readiness WHEN NEW.sequence=2 BEGIN SELECT RAISE(FAIL,'derived readiness failure'); END;").unwrap();
        let second = store.append_event(make("second")).unwrap();
        assert_eq!(second.sequence, 2, "original native Journal committed");
        assert!(second.observation.as_ref().unwrap().source_unavailable);
        let flags: (bool, bool) = store.lock().unwrap().query_row(
            "SELECT observation_readiness_missing,observation_unavailable FROM events WHERE sequence=2",
            [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(
            flags,
            (true, false),
            "only the readiness owner needs repair"
        );
        assert!(
            store
                .observation_event_witness(2, 2)
                .unwrap()
                .unwrap()
                .source_unavailable
        );
        store.mark_observation_direct(1, 1).unwrap();
        assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 1);
        assert!(store.mark_observation_ready(2, 2).is_err());
        assert!(store.mark_observation_direct(2, 2).is_err());
        assert!(
            !store
                .observation_event_witness(2, 2)
                .unwrap()
                .unwrap()
                .native_direct
        );
        assert!(
            store.observe_journal(Arc::new(Observer)).is_err(),
            "failed repair remains unavailable"
        );
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER readiness_layout_failure")
            .unwrap();
        assert_eq!(store.observe_journal(Arc::new(Observer)).unwrap(), 2);
        assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 2);
        let recovered = store.observation_event_witness(2, 2).unwrap().unwrap();
        assert!(!recovered.source_unavailable);
        assert!(
            !observation_receipt::single(&store.lock().unwrap(), &id, &second)
                .unwrap()
                .source_unavailable
        );
        assert_eq!(store.read_events(Some(1), 1).unwrap().events, vec![second]);
        assert!(
            !recovered.native_direct,
            "restart reconstructs committed authority"
        );
    }

    #[test]
    fn t10_t19_t20_readiness_repair_preserves_independent_auxiliary_failure() {
        struct Observer;
        impl super::super::presentation::JournalObserver for Observer {
            fn committed(&self, _through: Option<u64>) {}
        }
        let id = ConversationId::new("conv_09276f12-ed32-741a-8cd5-91c38800ef71");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        store.lock().unwrap().execute_batch(
            "CREATE TRIGGER readiness_failure BEFORE INSERT ON event_observation_readiness BEGIN SELECT RAISE(FAIL,'readiness failure'); END;
             CREATE TRIGGER fallback_failure BEFORE UPDATE OF observation_json ON events BEGIN SELECT RAISE(FAIL,'independent fallback failure'); END;",
        ).unwrap();
        let mut event = envelope(
            &id,
            "both-layout-failures",
            Some(AttemptId::new("readiness-owner")),
            RuntimeEvent::TurnStarted,
        );
        event.turn_id = Some(TurnId::new("1"));
        let committed = store.append_event(event).unwrap();
        assert_eq!(committed.sequence, 1);
        assert!(committed.observation.as_ref().unwrap().source_unavailable);
        let flags = |store: &SqliteConversationStore| -> (bool, bool) {
            store.lock().unwrap().query_row(
                "SELECT observation_readiness_missing,observation_unavailable FROM events WHERE sequence=1",
                [], |row| Ok((row.get(0)?,row.get(1)?)),
            ).unwrap()
        };
        assert_eq!(flags(&store), (true, true));
        assert!(store.observe_journal(Arc::new(Observer)).is_err());
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER readiness_failure; DROP TRIGGER fallback_failure;")
            .unwrap();
        assert_eq!(store.observe_journal(Arc::new(Observer)).unwrap(), 1);
        assert_eq!(store.observation_ready_frontier(0, 1).unwrap(), 1);
        assert_eq!(flags(&store), (false, true));
        assert!(
            store
                .observation_event_witness(1, 1)
                .unwrap()
                .unwrap()
                .source_unavailable
        );
        assert!(
            observation_receipt::single(&store.lock().unwrap(), &id, &committed)
                .unwrap()
                .source_unavailable
        );
        assert_eq!(store.read_events(None, 1).unwrap().events, vec![committed]);
        let mut healthy = envelope(
            &id,
            "healthy-after-repair",
            Some(AttemptId::new("readiness-owner")),
            RuntimeEvent::TurnStarted,
        );
        healthy.turn_id = Some(TurnId::new("2"));
        let next = store.append_event(healthy).unwrap();
        assert_eq!(next.sequence, 2);
        assert!(!next.observation.as_ref().unwrap().source_unavailable);
        assert!(
            !store
                .observation_event_witness(2, 2)
                .unwrap()
                .unwrap()
                .source_unavailable
        );
    }

    #[test]
    fn t14_canonical_after_boundary_uses_original_ledger_and_selected_revision() {
        use crate::runtime_client::content::CanonicalContentLocator;
        let store = store();
        store.initialize(&[]).unwrap();
        let opening = store
            .append_canonical(&user_message("opening", "ordinary boundary"))
            .unwrap();
        let original_id = "assistant-".repeat(128);
        let closing = store
            .append_canonical(&assistant_message(&original_id, "canonical closing"))
            .unwrap();
        let locator = CanonicalContentLocator::ByLedgerPosition {
            position: closing.ledger_position,
        };
        let revision = closing.observation.read_cut.surface_revision;
        {
            let connection = store.lock().unwrap();
            connection
                .execute_batch("DELETE FROM observation_headers")
                .unwrap();
            connection
                .execute(
                    "UPDATE message_ledger SET message_json='unreadable original body' WHERE position=?1",
                    [closing.ledger_position],
                )
                .unwrap();
        }
        assert_eq!(
            store
                .canonical_boundary_message_id(revision, &locator)
                .unwrap(),
            MessageId::new(original_id),
            "After resolves the original identity without a body or display header"
        );
        assert!(matches!(
            store.canonical_boundary_message_id(
                opening.observation.read_cut.surface_revision,
                &locator,
            ),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert!(matches!(
            store.surface_boundary_message_id(revision, &locator),
            Err(ConversationStoreError::InvalidReference(_))
        ));
        assert_eq!(
            store
                .surface_boundary_message_id(
                    revision,
                    &CanonicalContentLocator::ByLedgerPosition {
                        position: opening.ledger_position,
                    },
                )
                .unwrap(),
            MessageId::new("opening"),
            "Before retains its ordinary inbound boundary semantics"
        );
    }

    #[test]
    fn t10_t19_t20_transcript_readiness_failure_preserves_native_canonical_commit() {
        struct Observer;
        impl super::super::presentation::JournalObserver for Observer {
            fn committed(&self, _through: Option<u64>) {}
        }
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936182");
        let store = SqliteConversationStore::in_memory(id).unwrap();
        store.initialize(&[]).unwrap();
        store.lock().unwrap().execute_batch("CREATE TRIGGER transcript_readiness_failure BEFORE INSERT ON transcript_observation_readiness WHEN NEW.position=1 BEGIN SELECT RAISE(FAIL,'derived transcript readiness failure'); END;").unwrap();
        let first = store
            .append_canonical(&user_message("first", "canonical authority"))
            .unwrap();
        assert_eq!(first.ledger_position, 1);
        assert_eq!(first.transcript_cursor.unwrap().get(), 1);
        assert_eq!(
            store
                .message_ledger_position(&MessageId::new("first"))
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            store.observation_transcript_ready_frontier(0, 1).unwrap(),
            0
        );
        assert!(store.mark_observation_transcript_ready(1).is_err());
        assert!(store.mark_canonical_transcript_ready(1, 1).is_err());
        let second = store
            .append_canonical(&user_message("second", "unrelated native commit"))
            .unwrap();
        assert_eq!(second.transcript_cursor.unwrap().get(), 2);
        store.mark_canonical_transcript_ready(2, 2).unwrap();
        assert_eq!(
            store.observation_transcript_ready_frontier(0, 2).unwrap(),
            0
        );
        assert!(store.observe_journal(Arc::new(Observer)).is_err());
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER transcript_readiness_failure")
            .unwrap();
        assert_eq!(store.observe_journal(Arc::new(Observer)).unwrap(), 0);
        assert_eq!(
            store.observation_transcript_ready_frontier(0, 2).unwrap(),
            2
        );
        let connection = store.lock().unwrap();
        let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT MIN(position) FROM transcript_order INDEXED BY transcript_observation_readiness_missing_idx WHERE observation_readiness_missing=1 AND position>0 AND position<=2", [], |row| row.get(3)).unwrap();
        assert!(
            plan.contains("transcript_observation_readiness_missing_idx"),
            "{plan}"
        );
    }

    #[test]
    fn t10_t15_t20_trace_count_hole_is_unavailable_without_refusing_native_commit() {
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936179");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        let make = |label: &str| {
            let mut event = envelope(
                &id,
                label,
                Some(AttemptId::new("trace-count")),
                RuntimeEvent::TurnStarted,
            );
            event.turn_id = Some(TurnId::new("1"));
            event
        };
        let first = store.append_event(make("first")).unwrap();
        assert_eq!(store.observation_trace_total(first.sequence).unwrap(), 1);
        store.lock().unwrap().execute_batch("CREATE TRIGGER trace_layout_failure BEFORE UPDATE OF observation_trace_total ON events WHEN NEW.sequence=2 BEGIN SELECT RAISE(FAIL,'derived count failure'); END;").unwrap();
        let second = store.append_event(make("second")).unwrap();
        assert_eq!(
            second.sequence, 2,
            "native fact committed despite derived fault"
        );
        assert!(second.observation.unwrap().trace_total.is_none());
        assert!(store.observation_trace_total(2).is_err());
        assert_eq!(store.observation_trace_total(1).unwrap(), 1);
        let connection = store.lock().unwrap();
        let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT sequence FROM events INDEXED BY events_observation_trace_holes_idx WHERE observation_trace_required=1 AND observation_trace_total IS NULL AND sequence<=2",[],|row|row.get(3)).unwrap();
        assert!(
            plan.contains("events_observation_trace_holes_idx"),
            "{plan}"
        );
    }

    #[test]
    fn t10_t19_derived_journal_body_failure_preserves_native_terminal_witness() {
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936179");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        let attempt = AttemptId::new("terminal-layout-failure");
        store
            .append_event(envelope(
                &id,
                "start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_display_body BEFORE INSERT ON content_segments WHEN NEW.source_kind='observation_journal' BEGIN SELECT RAISE(ABORT,'derived source unavailable'); END").unwrap();
        let event = store
            .append_event(envelope(
                &id,
                "settled",
                Some(attempt.clone()),
                RuntimeEvent::AttemptFailed {
                    attempt_id: attempt.clone(),
                    error: crate::events::types::AttemptFailure::Runtime {
                        error: crate::runtime::types::RuntimeError::Internal {
                            message: "native diagnostic".repeat(10000),
                        },
                    },
                },
            ))
            .unwrap();
        assert_eq!(event.sequence, 2);
        store.mark_observation_ready(1, 2).unwrap();
        assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 2);
        let witness = store.observation_event_witness(2, 2).unwrap().unwrap();
        assert_eq!(witness.event_kind, "attempt_failed");
        assert_eq!(witness.value["error"]["type"], "runtime");
        assert_eq!(witness.value["error"]["error"]["type"], "internal");
        assert!(witness.deferred);
        assert!(
            store
                .event_value_read(2, 2, &["event".into(), "error".into()], 0, 65536)
                .is_err()
        );
        let terminal = store.observation_attempt_summary(&attempt, 2).unwrap();
        assert_eq!(terminal.terminal_kind.as_deref(), Some("attempt_failed"));
        assert!(terminal.failure.is_some());
        assert_eq!(store.read_events(None, 32).unwrap().events.len(), 2);
    }

    #[test]
    fn t14_t19_t20_native_receipts_capture_only_released_predecessor_prefix() {
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936187");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        store.initialize(&[]).unwrap();
        let make = |name: &str| {
            let mut event = envelope(
                &id,
                name,
                Some(AttemptId::new("prefix-proof")),
                RuntimeEvent::TurnStarted,
            );
            event.turn_id = Some(TurnId::new("1"));
            event
        };
        let first = store.append_event(make("first")).unwrap();
        assert_eq!(first.observation.as_ref().unwrap().ready_before, Some(0));
        store.mark_observation_ready(1, 1).unwrap();
        let held = store.append_event(make("held")).unwrap();
        assert_eq!(held.observation.as_ref().unwrap().ready_before, Some(1));
        let later = store.append_event(make("later")).unwrap();
        assert_eq!(later.observation.as_ref().unwrap().ready_before, Some(1));
        let canonical = store
            .append_canonical(&user_message("prefix-canonical", "canonical"))
            .unwrap();
        assert_eq!(canonical.observation.ready_through, Some(1));
        assert_eq!(canonical.observation.read_cut.read.journal, 3);
        store.mark_observation_ready(3, 3).unwrap();
        assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 1);
        store.mark_observation_ready(2, 2).unwrap();
        assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 3);
        assert_eq!(
            later.observation.as_ref().unwrap().ready_before,
            Some(1),
            "later publication cannot retag frozen append evidence"
        );
        let follow = store
            .append_canonical(&user_message("prefix-follow", "follow"))
            .unwrap();
        assert_eq!(follow.observation.ready_through, Some(3));
    }

    #[test]
    fn t10_t14_t19_native_single_receipt_preserves_large_other_reason_and_exact_cut() {
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936185");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        let attempt = AttemptId::new("native-receipt-reason");
        let started = store
            .append_event(envelope(
                &id,
                "start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        let source = started
            .observation
            .as_ref()
            .expect("source-owned Single receipt");
        assert_eq!(source.read_cut.read.journal, 1);
        assert_eq!(source.journal_range, Some((1, 1)));
        let reason = "雪😀".repeat(300_000);
        let settled = store
            .append_event(envelope(
                &id,
                "settled",
                Some(attempt.clone()),
                RuntimeEvent::AttemptCompleted {
                    attempt_id: attempt.clone(),
                    finish_reason: ModelFinishReason::Other {
                        reason: reason.clone(),
                    },
                },
            ))
            .unwrap();
        let receipt = settled
            .observation
            .as_ref()
            .expect("native terminal receipt");
        let crate::durable::observation::NativeObservationTransition::Single(witness) =
            &receipt.transition
        else {
            panic!("scalar append family")
        };
        assert_eq!(witness.value["finish_reason"]["type"], "other");
        assert!(
            witness.value["finish_reason"]["reason"]
                .as_str()
                .unwrap()
                .len()
                <= 256
        );
        assert!(witness.deferred);
        assert_eq!(receipt.read_cut.read.journal, 2);
        assert_eq!(
            source.read_cut.read.journal, 1,
            "later native writes cannot retag prior source evidence"
        );
        let summary = store.observation_attempt_summary(&attempt, 2).unwrap();
        assert!(summary.finish_reason_deferred);
        assert_eq!(summary.finish_reason_sequence, Some(2));
        assert_eq!(summary.terminal_sequence, Some(2));
        let fields = ["event".into(), "finish_reason".into(), "reason".into()];
        let mut offset = 0;
        let mut bytes = Vec::new();
        loop {
            let chunk = store
                .event_value_read(2, 2, &fields, offset, 65_536)
                .unwrap();
            let (rows, materialized) = SqliteConversationStore::content_read_work();
            assert!(rows <= 2 && materialized <= 131_072);
            bytes.extend(chunk.bytes);
            match chunk.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), reason);
        let durable = serde_json::to_value(&settled).unwrap();
        assert!(durable.get("observation").is_none());
        assert_eq!(
            durable["event"]["finish_reason"]["reason"]
                .as_str()
                .unwrap(),
            reason
        );
    }

    #[test]
    fn t12_t19_native_failure_receipt_and_range_preserve_public_normalization() {
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936187");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        let attempt = AttemptId::new("public-failure-source");
        store
            .append_event(envelope(
                &id,
                "start",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        let error = crate::model::error::ModelError {
            kind: crate::model::error::ModelErrorKind::ProviderError,
            message: "雪".repeat(1000),
            retry_disposition: crate::model::error::ModelRetryDisposition::Never,
            retry_after_ms: Some(73),
            provider_code: Some("private-provider-code".into()),
            context_overflow: None,
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
        };
        let event = store
            .append_event(envelope(
                &id,
                &"private-native-id".repeat(100_000),
                Some(attempt.clone()),
                RuntimeEvent::AttemptFailed {
                    attempt_id: attempt.clone(),
                    error: crate::events::types::AttemptFailure::Model { error },
                },
            ))
            .unwrap();
        let headers = store
            .latest_observation_events(2, &["attempt_failed".into()], None, 32)
            .unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].sequence, 2);
        assert!(
            headers[0].event_id.is_none(),
            "metadata seek never hydrates an enormous original Event ID"
        );
        let receipt = event.observation.as_ref().unwrap();
        assert!(receipt.failure_deferred);
        assert_eq!(receipt.failure_sequence, Some(2));
        assert!(receipt.failure_readable_end > 3000);
        let failure = serde_json::to_string(receipt.failure.as_ref().unwrap()).unwrap();
        assert!(!failure.contains("provider_code") && !failure.contains("private-provider-code"));
        let chunk = store
            .observation_failure_value_read(&attempt, 2, 0, 65_536)
            .unwrap();
        assert!(chunk.next_offset.is_none());
        let failure: serde_json::Value = serde_json::from_slice(&chunk.bytes).unwrap();
        assert_eq!(failure["type"], "model");
        assert_eq!(failure["message"].as_str().unwrap().len(), 3000);
        assert_eq!(failure["retry_after_ms"], "73");
        assert!(failure.get("provider_code").is_none());
        assert!(
            serde_json::to_string(&event)
                .unwrap()
                .contains("private-provider-code"),
            "original native Journal authority is retained, not rewritten by display privacy"
        );
    }

    #[test]
    fn t14_t15_t20_pending_native_receipt_refills_head_without_a_body_mirror() {
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936186");
        let store = SqliteConversationStore::in_memory(id).unwrap();
        let time = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut accepted = Vec::new();
        for index in 0..73 {
            let item = store
                .accept_inbound(crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: crate::message::types::UserSource::Human,
                    kind: crate::message::types::InboundKind::Message,
                    content: vec![crate::message::types::UserContentBlock::Text(
                        crate::message::content::TextBlock {
                            text: format!("pending{index}"),
                        },
                    )],
                    timestamp: time,
                    correlation: None,
                })
                .unwrap();
            let receipt = item
                .pending_observation
                .as_ref()
                .expect("native indexed pending receipt");
            assert_eq!(receipt.total, index + 1);
            assert!(receipt.head.len() <= 32);
            assert_eq!(
                receipt.selected.as_ref().unwrap().sequence,
                item.sequence.get()
            );
            assert!(receipt.head.iter().all(
                |header| header.message.locator.is_none() && header.message.content.is_empty()
            ));
            accepted.push(item);
        }
        let first = &accepted[0];
        let removed = store
            .remove_pending(&crate::durable::inbox::PendingInboundRef {
                sequence: first.sequence,
                revision: 0,
            })
            .unwrap();
        let crate::durable::inbox::PendingMutationOutcome::Applied {
            pending_observation: Some(receipt),
            ..
        } = removed
        else {
            panic!("native pending removal receipt")
        };
        assert_eq!(receipt.total, 72);
        assert_eq!(receipt.removed_sequence, Some(1));
        assert_eq!(receipt.removed_transcript, first.transcript_cursor);
        assert_eq!(
            (
                receipt.head.first().unwrap().sequence,
                receipt.head.last().unwrap().sequence
            ),
            (2, 33)
        );
        assert_eq!(
            accepted
                .last()
                .unwrap()
                .pending_observation
                .as_ref()
                .unwrap()
                .total,
            73,
            "capture evidence remains immutable"
        );
    }

    #[test]
    fn t14_t20_atomic_canonical_batch_owns_one_finite_shared_observation_proof() {
        let store = SqliteConversationStore::in_memory(ConversationId::new(
            "conv_2be184c1-ab11-725b-82c3-81daea936180",
        ))
        .unwrap();
        store.initialize(&[]).unwrap();
        let messages = (0..73)
            .map(|index| user_message(&format!("batch-{index}"), &format!("body-{index}")))
            .collect::<Vec<_>>();
        let receipts = store.append_canonical_batch(&messages).unwrap();
        assert_eq!(receipts.len(), 73);
        let batch = &receipts[0].observation;
        assert_eq!(
            (batch.first_position, batch.last_position, batch.count),
            (1, 73, 73)
        );
        assert_eq!(batch.read_cut.ledger_through, 73);
        assert_eq!(batch.transcript_count, 73);
        assert_eq!(batch.head.len(), 8);
        assert_eq!(batch.head.first().unwrap().ledger_position, Some(66));
        assert_eq!(batch.head.last().unwrap().ledger_position, Some(73));
        assert!(batch.has_opening_user);
        assert_eq!(batch.opening_preview.as_deref(), Some("body-0"));
        assert_eq!(batch.journal_range, None);
        for (ordinal, receipt) in receipts.iter().enumerate() {
            assert!(Arc::ptr_eq(batch, &receipt.observation));
            assert_eq!(receipt.ordinal, ordinal as u64);
            assert_eq!(receipt.ledger_position, ordinal as u64 + 1);
        }
        let follow = store
            .append_canonical(&user_message("follow", "after-batch"))
            .unwrap();
        assert_eq!(follow.observation.count, 1);
        assert_eq!(follow.ordinal, 0);
        assert_eq!(follow.ledger_position, 74);
        assert_eq!(
            batch.read_cut.ledger_through, 73,
            "immutable batch does not jump to the new source head"
        );
    }

    #[test]
    fn t14_t19_t20_transcript_ready_prefix_fences_prior_terminal_without_position_mirror() {
        let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936181");
        let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
        store.initialize(&[]).unwrap();
        let attempt = AttemptId::new("gated-terminal");
        store
            .append_event(envelope(
                &id,
                "started",
                Some(attempt.clone()),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .unwrap();
        store
            .append_event(envelope(
                &id,
                "terminal",
                Some(attempt.clone()),
                RuntimeEvent::AttemptTimedOut {
                    attempt_id: attempt,
                },
            ))
            .unwrap();
        let messages = (0..73)
            .map(|index| user_message(&format!("after-terminal-{index}"), "canonical"))
            .collect::<Vec<_>>();
        let batch = store.append_canonical_batch(&messages).unwrap();
        assert_eq!(batch.last().unwrap().transcript_cursor.unwrap().get(), 74);
        store.mark_canonical_transcript_ready(1, 73).unwrap();
        assert_eq!(
            store.observation_transcript_ready_frontier(0, 74).unwrap(),
            0,
            "C release cannot release unrelated prior terminalT"
        );
        store.mark_observation_ready(1, 2).unwrap();
        assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 2);
        assert_eq!(
            store.observation_transcript_ready_frontier(0, 74).unwrap(),
            0,
            "J andT publication evidence remain distinct"
        );
        store.mark_observation_transcript_ready(1).unwrap();
        assert_eq!(
            store.observation_transcript_ready_frontier(0, 74).unwrap(),
            74
        );
        assert!(store.mark_observation_transcript_ready(75).is_err());
        assert!(store.mark_canonical_transcript_ready(1, 74).is_err());
        let connection = store.lock().unwrap();
        let plan:String=connection.query_row("EXPLAIN QUERY PLAN SELECT MIN(position) FROM transcript_observation_readiness WHERE ready=0 AND position>0 AND position<=74",[],|row|row.get(3)).unwrap();
        assert!(
            plan.contains("transcript_observation_readiness_holes_idx"),
            "{plan}"
        );
    }

    /// Capacity suite: this test's value comes from its scale, so FAST skips
    /// `::scale::` and FULL runs it (tests/README.md, "Capacity tests").
    mod scale {
        use super::*;

        #[test]
        fn t14_t19_t20_transcript_source_prefix_recovers_after_4097_later_owner_releases() {
            struct Observer {
                store: std::sync::Weak<SqliteConversationStore>,
                latest: Mutex<Option<crate::durable::observation::TranscriptSourceReady>>,
                wakes: std::sync::atomic::AtomicUsize,
            }
            impl super::super::super::presentation::JournalObserver for Observer {
                fn committed(&self, _through: Option<u64>) {}
                fn ready(&self, through: u64) {
                    assert_eq!(
                        through, 0,
                        "T publication wakes even when J does not change"
                    );
                    let proof = self
                        .store
                        .upgrade()
                        .unwrap()
                        .transcript_source_ready()
                        .unwrap();
                    *self.latest.lock().unwrap() = Some(proof);
                    self.wakes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936182");
            let store = Arc::new(SqliteConversationStore::in_memory(id.clone()).unwrap());
            store.initialize(&[]).unwrap();
            let observer = Arc::new(Observer {
                store: Arc::downgrade(&store),
                latest: Mutex::new(None),
                wakes: std::sync::atomic::AtomicUsize::new(0),
            });
            store.observe_journal(observer.clone()).unwrap();
            let attempt = AttemptId::new("held-terminal-t");
            store
                .append_event(envelope(
                    &id,
                    "held-start",
                    Some(attempt.clone()),
                    RuntimeEvent::AttemptStarted {
                        attempt_id: attempt.clone(),
                    },
                ))
                .unwrap();
            store
                .append_event(envelope(
                    &id,
                    "held-terminal",
                    Some(attempt.clone()),
                    RuntimeEvent::AttemptTimedOut {
                        attempt_id: attempt,
                    },
                ))
                .unwrap();
            let messages = (0..4097)
                .map(|index| user_message(&format!("later-t-{index}"), "canonical"))
                .collect::<Vec<_>>();
            let receipts = store.append_canonical_batch(&messages).unwrap();
            let batch = &receipts.last().unwrap().observation;
            assert_eq!(batch.ready_t_through, Some(0));
            assert_eq!(batch.last_transcript.unwrap().get(), 4098);
            let frozen = store.transcript_source_ready().unwrap();
            assert_eq!(frozen.prefix, 0);
            assert_eq!(frozen.read_cut.ledger_through, 4097);
            for position in 2..=4098 {
                store.mark_observation_transcript_ready(position).unwrap();
            }
            let held = observer.latest.lock().unwrap().clone().unwrap();
            assert_eq!(
                held, frozen,
                "later owner releases cannot skip held native T1"
            );
            store.mark_observation_transcript_ready(1).unwrap();
            let released = observer.latest.lock().unwrap().clone().unwrap();
            assert_eq!(released.prefix, 4098);
            assert_eq!(released.read_cut, frozen.read_cut);
            assert_eq!(
                observer.wakes.load(std::sync::atomic::Ordering::SeqCst),
                4098
            );
            assert_eq!(
                batch.ready_t_through,
                Some(0),
                "receipt proof remains immutable"
            );
            let follow = store
                .append_canonical(&user_message("follow-t", "next"))
                .unwrap();
            assert_eq!(follow.observation.ready_t_through, Some(4098));
            let current = store.transcript_source_ready().unwrap();
            assert_eq!(current.prefix, 4098);
            assert_eq!(current.read_cut.read.transcript, 4099);
            assert_eq!(frozen.read_cut.read.transcript, 4098);
        }
    }

    #[test]
    fn trace_schema_indexes_are_required_and_queries_use_them() {
        use super::super::presentation::{FactQuery, FactScope};
        let id = ConversationId::new("conv_ab18a567-5190-73a5-8156-2e8693be6392");
        let store = SqliteConversationStore::in_memory(id).unwrap();
        let connection = store.conn.lock().unwrap();
        verify_schema_shape(&connection).unwrap();
        let scopes = [
            (FactScope::All, "events_kind_idx"),
            (
                FactScope::Attempt(AttemptId::new("a")),
                "events_attempt_kind_idx",
            ),
            (
                FactScope::Step(AttemptId::new("a"), TurnId::new("1")),
                "events_step_kind_idx",
            ),
            (FactScope::Request("r".into()), "events_request_id_kind_idx"),
            (
                FactScope::Compaction(crate::runtime::identity::CompactionOperationId::new("o")),
                "events_compaction_operation_kind_idx",
            ),
            (FactScope::Message("m".into()), "events_message_id_kind_idx"),
            (FactScope::ContextReading, "events_context_reading_idx"),
            (
                FactScope::ToolCall {
                    call_id: "c".into(),
                    attempt: Some(AttemptId::new("a")),
                    turn: Some(TurnId::new("1")),
                },
                "events_tool_call_scope_kind_idx",
            ),
            (
                FactScope::Execution("e".into()),
                "events_execution_id_kind_idx",
            ),
            (
                FactScope::Subagent("s".into()),
                "events_subagent_id_kind_idx",
            ),
            (FactScope::Workflow("w".into()), "events_run_id_kind_idx"),
            (
                FactScope::Interaction("i".into()),
                "events_interaction_id_kind_idx",
            ),
        ];
        for (scope, index) in scopes {
            for ascending in [false, true] {
                let query = FactQuery {
                    scope: scope.clone(),
                    kinds: vec!["attempt_started", "turn_started"],
                    before: Some(99),
                    after: 2,
                    ascending,
                    through: 100,
                    limit: 32,
                };
                for (sql, params) in presentation_statements(&query).unwrap() {
                    let plan = connection
                        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                        .unwrap()
                        .query_map(rusqlite::params_from_iter(params), |row| {
                            row.get::<_, String>(3)
                        })
                        .unwrap()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap()
                        .join("\n");
                    assert!(
                        plan.contains(&format!("SEARCH events USING INDEX {index}")),
                        "{plan}"
                    );
                    assert!(
                        !plan.contains("SCAN ") && !plan.contains("TEMP B-TREE"),
                        "{plan}"
                    );
                }
            }
        }
        for (name, definition) in PRESENTATION_INDEXES {
            connection
                .execute_batch(&format!("DROP INDEX {name}"))
                .unwrap();
            assert!(matches!(
                verify_schema_shape(&connection),
                Err(ConversationStoreError::IncompatibleSchema { .. })
            ));
            connection
                .execute_batch(&format!("CREATE INDEX {name} ON events(sequence)"))
                .unwrap();
            assert!(
                verify_schema_shape(&connection).is_err(),
                "a matching name is insufficient"
            );
            connection
                .execute_batch(&format!(
                    "DROP INDEX {name}; CREATE INDEX {name} ON events{definition}"
                ))
                .unwrap();
            verify_schema_shape(&connection).unwrap();
        }
    }

    #[test]
    fn schema_34_without_fixed_trace_index_contract_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old.sqlite");
        let id = ConversationId::new("conv_a2b8feb8-c105-7980-8a72-d8482750c3fc");
        {
            let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE rustx_store SET schema_version = 34 WHERE id = 1",
                    [],
                )
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 34,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn preceding_schema_is_rejected_before_event_deserialization() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preceding.sqlite");
        let id = ConversationId::new("conv_497e1eed-f051-7f55-8d8c-4775b598e15c");
        {
            let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
            let connection = store.conn.lock().unwrap();
            // Valid JSON but deliberately not a decodable RuntimeEventEnvelope.
            // The version gate must win before any payload reader can run.
            connection.execute(
                "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(1,'incompatible',?1,?2,NULL,NULL,'{}','2026-01-01T00:00:00Z')",
                params![i64::from(EVENT_SCHEMA_VERSION), id.as_str()],
            ).unwrap();
            connection
                .execute(
                    "UPDATE rustx_store SET schema_version=?1, next_event_sequence=1 WHERE id=1",
                    params![SQLITE_SCHEMA_VERSION - 1],
                )
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch { stored, expected })
                if stored == SQLITE_SCHEMA_VERSION - 1 && expected == SQLITE_SCHEMA_VERSION
        ));
    }

    /// Pre-integration stores can hold retired workspace facts or lack the
    /// combined canonical child-history contract. The gate refuses them at open,
    /// before any row is decoded, and never rewrites, migrates or deletes it.
    #[test]
    fn pre_integration_stores_are_refused_and_left_untouched() {
        for previous_version in [50, 51, 55] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("pre-integration.sqlite");
            let id = ConversationId::new("conv_497e1eed-f051-7f55-8d8c-4775b598e15c");
            {
                let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
                let connection = store.conn.lock().unwrap();
                for (sequence, event) in [
                r#"{"type":"workflow_workspace_owned","run_id":{"conversation_id":"conv_497e1eed-f051-7f55-8d8c-4775b598e15c","attempt_id":"attempt","invocation":1},"workspace":{"logical_workspace":"/w","isolation":{"type":"shared"}}}"#,
                r#"{"type":"subagent_workspace_disposal_started","subagent_id":"conv_497e1eed-f051-7f55-8d8c-4775b598e15c-subagent-1"}"#,
            ]
            .into_iter()
            .enumerate()
            {
                connection
                    .execute(
                        "INSERT INTO events(sequence,event_id,schema_version,conversation_id,attempt_id,turn_id,event_json,event_timestamp) VALUES(?1,?2,?3,?4,NULL,NULL,?5,json_extract(?5,'$.timestamp'))",
                        params![
                            i64::try_from(sequence + 1).unwrap(),
                            format!("retired-{sequence}"),
                            i64::from(EVENT_SCHEMA_VERSION),
                            id.as_str(),
                            format!(
                                r#"{{"schema_version":{EVENT_SCHEMA_VERSION},"event_id":"retired-{sequence}","sequence":{},"conversation_id":"{}","timestamp":"2026-01-01T00:00:00Z","event":{event}}}"#,
                                sequence + 1,
                                id.as_str()
                            )
                        ],
                    )
                    .unwrap();
            }
                connection
                .execute(
                    "UPDATE rustx_store SET schema_version = ?1, next_event_sequence = 3 WHERE id = 1",
                    [previous_version],
                )
                .unwrap();
            }
            let before = std::fs::read(&path).unwrap();
            assert!(matches!(
                SqliteConversationStore::open(id, &path),
                Err(ConversationStoreError::SchemaVersionMismatch {
                    stored,
                    expected: SQLITE_SCHEMA_VERSION
                }) if stored == previous_version
            ));
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "refusal never migrates, rewrites or deletes old data"
            );
            let mut sidecars = std::fs::read_dir(directory.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect::<Vec<_>>();
            sidecars.sort();
            assert_eq!(sidecars, ["pre-integration.sqlite"]);
        }
    }

    /// The other half of the gate: a store this runtime creates records the
    /// version this runtime speaks, and reopens.
    #[test]
    fn a_current_store_records_the_typed_interaction_audit_schema_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("current.sqlite");
        let conversation_id = ConversationId::new("conv_497e1eed-f051-7f55-8d8c-4775b598e15c");
        let stored: i64 = {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            let connection = store.conn.lock().unwrap();
            connection
                .query_row(
                    "SELECT schema_version FROM rustx_store WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(stored, SQLITE_SCHEMA_VERSION);
        SqliteConversationStore::open(conversation_id, &path).expect("a current store reopens");
    }

    #[test]
    fn loop_lifecycle_facts_round_trip_without_executable_state() {
        use crate::runtime::workflow::{WorkflowExecutionOutcome, WorkflowLoopExit, test_instance};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("loop-facts.sqlite");
        let conversation = ConversationId::new("conv_701197a0-a688-77da-8a0f-ce405d769eb8");
        let mut node = test_instance("feedback", "loop");
        node.block.run.conversation_id = conversation.clone();
        node.block.invocations = vec![0, 2];
        let mut body = node.block.clone();
        body.definition
            .blocks
            .extend(["loop".into(), "body".into()]);
        body.invocations.push(3);
        let facts = vec![
            RuntimeEvent::WorkflowLoopIterationAdmitted {
                node: node.clone(),
                body: body.clone(),
                iteration: 3,
            },
            RuntimeEvent::WorkflowLoopIterationSettled {
                node: node.clone(),
                body,
                iteration: 3,
                outcome: WorkflowExecutionOutcome::Completed,
            },
            RuntimeEvent::WorkflowLoopExited {
                node: node.clone(),
                iterations: 3,
                status: WorkflowLoopExit::Satisfied,
            },
            RuntimeEvent::WorkflowLoopExited {
                node,
                iterations: 3,
                status: WorkflowLoopExit::Exhausted,
            },
        ];
        {
            let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
            for (index, fact) in facts.iter().enumerate() {
                store
                    .append_event(envelope(
                        &conversation,
                        &format!("loop-fact-{index}"),
                        None,
                        fact.clone(),
                    ))
                    .unwrap();
            }
        }
        let store = SqliteConversationStore::open(conversation, &path).unwrap();
        let read = store
            .read_events(None, 16)
            .unwrap()
            .events
            .into_iter()
            .map(|e| e.event)
            .collect::<Vec<_>>();
        assert_eq!(read, facts);
        assert!(store.load_canonical().unwrap().is_empty());
    }

    #[test]
    fn schema_29_without_loop_lifecycle_facts_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-loop.sqlite");
        let conversation = ConversationId::new("conv_4406c108-b4ff-727c-8a70-ab32b1f0efd5");
        {
            let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE rustx_store SET schema_version = 29 WHERE id = 1",
                    [],
                )
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 29,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn schema_27_without_workflow_recovery_guards_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("without-recovery-guard.sqlite");
        let conversation = ConversationId::new("conv_d85e9707-bae4-7aba-8e7e-3bf98a5f0a4b");
        {
            let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE rustx_store SET schema_version = 27 WHERE id = 1",
                    [],
                )
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 27,
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
        let conversation_id = ConversationId::new("conv_f9a4212a-8c18-76b4-8b37-69acee1c9a60");
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

    /// Issue #111 adds the durable answer obligation
    /// (`InboundTurnAdopted`) that startup recovery reads to decide whether an
    /// adopted turn is still owed an answer.
    ///
    /// A version-8 database predates that vocabulary: its adoption
    /// transactions committed no obligation fact, so a current reader would
    /// interpret "the fact is absent" as "no answer is owed" and silently
    /// strand exactly the crash states the obligation rescues — a turn adopted
    /// into canonical history and killed before its attempt reached a request
    /// start. The physical tables are unchanged, which is precisely why the
    /// version must gate open rather than the schema shape.
    #[test]
    fn pre_answer_obligation_schema_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-answer-obligation.sqlite");
        let conversation_id = ConversationId::new("conv_09e32769-fc6b-71ac-81e2-c993e7ff5861");
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute("UPDATE rustx_store SET schema_version = 8 WHERE id = 1", [])
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 8,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    /// Issue #126 changes the durable interaction payload vocabulary from the
    /// old Question/Answered contract to the structured Questionnaire
    /// contract. A version-9 database therefore cannot be opened by the
    /// current store, even though its physical tables are unchanged.
    #[test]
    fn pre_structured_questionnaire_schema_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-structured-questionnaire.sqlite");
        let conversation_id = ConversationId::new("conv_9be9531e-63d0-70aa-8359-c06d857d1479");
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute("UPDATE rustx_store SET schema_version = 9 WHERE id = 1", [])
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 9,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    /// Issue #134 is one clean breaking development-schema transition from
    /// the merged main schema (v11) to the retry-aware event format (v13).
    /// There is no migration path for an earlier development schema.
    #[test]
    fn pre_issue_130_schema_version_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-issue-130.sqlite");
        let conversation_id = ConversationId::new("conv_e0814883-a446-7275-827d-b31701d9c6cc");
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE rustx_store SET schema_version = 11 WHERE id = 1",
                    [],
                )
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 11,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    /// Issue #136 changes the durable cancellation status from an untyped
    /// reason-only value to a reason-plus-phase value. The previous v13
    /// development schema is rejected explicitly; no default phase is
    /// supplied for old rows.
    #[test]
    fn issue136_schema_version_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-issue-136.sqlite");
        let conversation_id = ConversationId::new("conv_fa864790-c311-717f-8884-b4f0363af078");
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE rustx_store SET schema_version = 13 WHERE id = 1",
                    [],
                )
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 13,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    /// Issue #137 adds the durable pending source pointer and frozen
    /// request-only snapshot fields. The immediately previous development
    /// schema is rejected explicitly; there is no legacy decoder or
    /// migration for those fields.
    #[test]
    fn issue137_schema_version_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-issue-137.sqlite");
        let conversation_id = ConversationId::new("conv_a824ca2a-191a-74db-864b-400f12825324");
        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE rustx_store SET schema_version = 14 WHERE id = 1",
                    [],
                )
                .unwrap();
        }
        assert!(matches!(
            SqliteConversationStore::open(conversation_id, &path),
            Err(ConversationStoreError::SchemaVersionMismatch {
                stored: 14,
                expected: SQLITE_SCHEMA_VERSION
            })
        ));
    }

    /// Both typed cancellation phases are durable canonical facts. They must
    /// survive a close/reopen round trip without a compatibility default or
    /// prose-based reconstruction.
    #[test]
    fn issue136_cancellation_phases_survive_durable_reload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cancellation-phases.sqlite");
        let conversation_id = ConversationId::new("conv_d865e314-7198-7c52-8c86-68581b290333");
        let base = user_message("user-issue-136", "Run the tools");
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("assistant-issue-136"),
            content: vec![
                AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new("call-before-start"),
                    tool_id: ToolId::new("tool-before-start"),
                    name: "before_start".to_owned(),
                    arguments: serde_json::json!({}),
                }),
                AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new("call-during-execution"),
                    tool_id: ToolId::new("tool-during-execution"),
                    name: "during_execution".to_owned(),
                    arguments: serde_json::json!({}),
                }),
            ],
        });
        let cancelled = |index: u32,
                         message_id: &str,
                         call_id: &str,
                         tool_id: &str,
                         reason: CancellationReason,
                         phase: ToolCancellationPhase| {
            MessageBlock::Tool(ToolMessageBlock {
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    crate::runtime::identity::MessageId::new("assistant-issue-136"),
                    crate::message::types::ContentBlockIndex::new(index),
                ),
                id: MessageId::new(message_id),
                tool_call_id: ToolCallId::new(call_id),
                tool_id: ToolId::new(tool_id),
                result: ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Cancelled { reason, phase },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            })
        };
        let before_start = cancelled(
            0,
            "result-before-start",
            "call-before-start",
            "tool-before-start",
            CancellationReason::UserRequested,
            ToolCancellationPhase::BeforeStart,
        );
        let during_execution = cancelled(
            1,
            "result-during-execution",
            "call-during-execution",
            "tool-during-execution",
            CancellationReason::RuntimeShutdown,
            ToolCancellationPhase::DuringExecution,
        );
        let expected = vec![
            base.clone(),
            assistant.clone(),
            before_start.clone(),
            during_execution.clone(),
        ];

        {
            let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
            store.initialize(std::slice::from_ref(&base)).unwrap();
            store.append_canonical(&assistant).unwrap();
            store
                .append_canonical_batch(&[before_start.clone(), during_execution.clone()])
                .unwrap();
            assert_eq!(store.load_canonical().unwrap(), expected);
        }

        let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
        assert_eq!(reopened.load_canonical().unwrap(), expected);
    }

    /// Issue #108 proposal-state rows are a development-only physical
    /// contract. A version-5 database is rejected explicitly instead of
    /// attempting to interpret its older proposal table or migrate it.
    #[test]
    fn pre_proposal_state_machine_schema_is_rejected_explicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pre-proposal-state-machine.sqlite");
        let conversation_id = ConversationId::new("conv_dfd0b924-9046-7f41-80a4-91c6b251f14c");
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
    fn assert_status_original_ranges(
        store: &SqliteConversationStore,
        view: &crate::runtime_client::snapshot::AgentStatusView,
        rendered: &str,
        target: &MessageId,
    ) {
        use base64::Engine as _;
        let bytes = store
            .content_read(view.rendered_ref.as_ref().unwrap(), 0, 65_536)
            .unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(bytes.data_base64)
                .unwrap(),
            rendered.as_bytes()
        );
        let reference = view
            .opportunities
            .fresh_inbound
            .as_ref()
            .unwrap()
            .target_metadata_ref
            .as_ref()
            .unwrap();
        let chunk = store.content_read(reference, 0, 65_536).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .unwrap();
        let expected_end = reference.readable_end.min(65_536);
        assert_eq!(bytes.len() as u64, expected_end);
        assert_eq!(chunk.offset, 0);
        assert_eq!(chunk.end_offset, expected_end);
        assert_eq!(chunk.readable_end, reference.readable_end);
        assert_eq!(chunk.complete, expected_end == reference.readable_end);
        assert_eq!(
            chunk.next_offset,
            (expected_end < reference.readable_end).then_some(expected_end)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["id"],
            target.as_str()
        );
        let (rows, materialized) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && materialized <= 131_072);
    }

    /// T14/T19/T20: hidden Context uses its actual immutable C source, and a
    /// giant `FreshInbound` identity keeps its native placement without body cloning.
    #[test]
    fn t14_t19_t20_status_receipt_preserves_giant_source_identity_and_placement() {
        let store = store();
        // Native model-head identity admission is <=4096 UTF-8 bytes.
        // Escaped original JSON still exceeds16 KiB and must use Source ranges.
        let native_id = format!("native-input{}", "\u{1}".repeat(3900));
        let base = user_message(&native_id, "ordinary user");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let time = Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap();
        let status = crate::context::AgentStatus {
            generated_at: time,
            sections: vec![crate::context::AgentStatusSection {
                id: crate::context::AgentStatusSectionId::new("temporal"),
                data: crate::context::AgentStatusSectionData::Temporal {
                    current_time: time,
                    timezone: None,
                },
            }],
        };
        let status_id = MessageId::new(format!("native-status{}", "\u{1}".repeat(3900)));
        let rendered = crate::context::render_agent_status(&status);
        let mut canonical =
            agent_status_message(status_id.as_str(), time, &[AgentStatusModuleId::Time]);
        let MessageBlock::User(user) = &mut canonical else {
            panic!("status User context")
        };
        user.content = vec![crate::message::types::UserContentBlock::Text(
            crate::message::content::TextBlock {
                text: rendered.clone(),
            },
        )];
        let mut snapshot = status_start_snapshot_for(
            "status-source-attempt",
            "1",
            store.load_head().unwrap().revision.next(),
            status_id.clone(),
            vec![],
            vec![status_id],
        );
        let contribution = snapshot.contributions.last_mut().unwrap();
        contribution.metadata = ContextKind::AgentStatus(status.generation_metadata());
        contribution.presentation =
            Some(crate::context::contribution::ContributionPresentation::AgentStatus(status));
        contribution.opportunities.fresh_inbound = Some(crate::context::FreshInboundOpportunity {
            target_message_id: base.id().clone(),
        });
        freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&canonical));
        let committed = store
            .commit_model_turn_start(&[canonical], &snapshot, time)
            .unwrap();
        assert_eq!(committed.status_sources.len(), 1);
        let native = committed.started.observation.as_ref().unwrap();
        let crate::durable::observation::NativeObservationTransition::RequestStart {
            statuses, ..
        } = &native.transition
        else {
            panic!("closed RequestStart family")
        };
        assert_eq!(statuses.len(), 1);
        let view = &statuses[0];
        assert!(view.status_message_id.is_none());
        assert_eq!(
            view.source_locator,
            Some(
                crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
                    position: 2
                }
            )
        );
        let fresh = view.opportunities.fresh_inbound.as_ref().unwrap();
        assert!(fresh.target_message_id.is_none());
        assert_eq!(
            fresh.target_locator,
            Some(
                crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
                    position: 1
                }
            )
        );
        assert_eq!(
            fresh.transcript_anchor,
            Some(crate::runtime_client::snapshot::RuntimeClientTranscriptCursor::new(1))
        );
        assert_eq!(native.read_cut.ledger_through, 2);
        assert_eq!(native.read_cut.read.journal, committed.started.sequence);
        assert!(serde_json::to_vec(view).unwrap().len() < 16_384);
        assert_status_original_ranges(&store, view, &rendered, base.id());
        assert_eq!(
            store.observation_read_cut().unwrap().transcript_total,
            1,
            "hidden status remains outside transcript"
        );
    }

    /// T14/T20: `GoalRound` accounting, pending head and J frontier are one source receipt.
    #[test]
    fn t14_t20_goal_receipt_owns_pending_and_journal_atomically() {
        let store = store();
        let (result, receipt) = store
            .write_goal(crate::goal::GoalWrite::Create {
                objective: "goal objective".into(),
                budget: 3,
                origin: crate::goal::GoalOrigin::RuntimeControl,
            })
            .unwrap();
        let goal = result.unwrap();
        let receipt = receipt.unwrap();
        assert_eq!(
            receipt.view.current.as_ref().unwrap().reference,
            goal.reference
        );
        assert!(receipt.pending.is_none());
        assert_eq!(receipt.native.as_ref().unwrap().journal_range, Some((1, 1)));
        let accepted = store
            .accept_goal_round(
                &goal.reference,
                crate::durable::inbox::InboundDraft {
                    message_id: None,
                    source: crate::message::types::UserSource::Runtime,
                    kind: crate::message::types::InboundKind::GoalContinuation(
                        goal.reference.clone(),
                    ),
                    content: vec![crate::message::types::UserContentBlock::Text(
                        crate::message::content::TextBlock {
                            text: "continue".into(),
                        },
                    )],
                    timestamp: Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                    correlation: None,
                },
            )
            .unwrap()
            .unwrap();
        let receipt = accepted.goal_observation.as_ref().unwrap();
        let current = receipt.view.current.as_ref().unwrap();
        assert_eq!(current.reference.revision, goal.reference.revision + 1);
        assert_eq!(current.autonomous_rounds_consumed, 1);
        let pending = receipt.pending.as_ref().unwrap();
        assert_eq!(pending.read_cut, accepted.read_cut);
        assert_eq!(pending.total, 1);
        assert_eq!(
            pending.selected.as_ref().unwrap().sequence,
            accepted.sequence.get()
        );
        let native = receipt.native.as_ref().unwrap();
        assert_eq!(native.read_cut.read, accepted.read_cut);
        assert_eq!(native.journal_range, Some((2, 2)));
    }
}

// Finite archive readers share the concrete durable schema, never transport DTOs.
pub(crate) mod archive;
mod compaction_resources;
mod lineage;
mod provenance;
mod reading;
#[cfg(test)]
mod reading_tests;

#[cfg(test)]
mod executor_wait_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const LIVENESS: Duration = Duration::from_secs(60);

    /// One worker: anything that parks it stops every other task.
    fn one_worker() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap()
    }

    /// Another task on the same runtime runs to completion.
    fn executor_progresses(runtime: &tokio::runtime::Runtime) {
        let (sender, receiver) = mpsc::channel();
        runtime.spawn(async move { sender.send(()).unwrap() });
        receiver
            .recv_timeout(LIVENESS)
            .expect("the runtime must keep running other tasks");
    }

    #[test]
    fn writer_busy_schedule_is_sqlites_own_with_the_store_budget() {
        let delays: Vec<_> = (0..)
            .map_while(writer_busy_delay)
            .map(|delay| delay.as_millis())
            .collect();
        assert_eq!(delays[..12], [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100]);
        assert!(delays[12..].iter().all(|delay| *delay <= 100));
        assert_eq!(delays.iter().sum::<u128>(), 5000);
        assert_eq!(writer_busy_delay(i32::MAX), None);
    }

    /// A store write waiting for another connection's lock leaves the
    /// executor running; releasing the lock then lets the same write commit.
    #[test]
    fn a_busy_store_writer_does_not_park_its_tokio_worker() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("store.sqlite");
        let mut writer = Connection::open(&path).unwrap();
        configure_connection(&mut writer, false).unwrap();
        writer
            .execute_batch("CREATE TABLE contended (value INTEGER)")
            .unwrap();
        let competitor = Connection::open(&path).unwrap();
        competitor.execute_batch("BEGIN IMMEDIATE").unwrap();
        let runtime = one_worker();
        let (busy, waiting) = mpsc::channel();
        let write = runtime.spawn(async move {
            let _probe = busy_probe::observe(move |count| {
                if count == 0 {
                    busy.send(()).unwrap();
                }
            });
            // No await: the whole write is one synchronous poll, as in the store.
            writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO contended VALUES (1); COMMIT")
        });
        // SQLite itself reports the contention from the writer's handler.
        waiting.recv_timeout(LIVENESS).unwrap();
        executor_progresses(&runtime);
        competitor.execute_batch("ROLLBACK").unwrap();
        runtime.block_on(write).unwrap().unwrap();
        let committed: i64 = competitor
            .query_row("SELECT count(*) FROM contended", [], |row| row.get(0))
            .unwrap();
        assert_eq!(committed, 1);
    }

    /// Waiting for the connection another store operation holds likewise
    /// leaves the executor running.
    #[test]
    fn a_connection_wait_does_not_park_its_tokio_worker() {
        let owner = Arc::new(ConnectionOwner {
            manifest: manifest::Owner::uninstalled(),
            append_validation: append_validation::Owner::uninstalled(),
            available: Mutex::new(Some(Connection::open_in_memory().unwrap())),
            wake: Condvar::new(),
            poisoned: AtomicBool::new(false),
        });
        // Declared first so a failing assertion still releases the lease
        // before the runtime waits for its worker on unwind.
        let runtime = one_worker();
        let held = owner.lock().unwrap();
        let waiter = owner.clone();
        let (started, running) = mpsc::channel();
        let wait = runtime.spawn(async move {
            started.send(()).unwrap();
            waiter.lock().map(|_| ())
        });
        running.recv_timeout(LIVENESS).unwrap();
        // The waiter's poll continues synchronously into the held connection,
        // so another task can run only if that wait hands the worker off.
        executor_progresses(&runtime);
        drop(held);
        runtime.block_on(wait).unwrap().unwrap();
    }
}

#[cfg(test)]
mod presentation_cancellation_tests {
    use super::*;

    #[test]
    fn presentation_cancellation_interrupts_running_sql_and_restores_writer_policy() {
        let manifest = manifest::Owner::uninstalled();
        let owner = Arc::new(ConnectionOwner {
            manifest,
            append_validation: append_validation::Owner::uninstalled(),
            available: Mutex::new(Some(Connection::open_in_memory().unwrap())),
            wake: Condvar::new(),
            poisoned: AtomicBool::new(false),
        });
        let mut reader = SqliteConversationStore::from_connection_owner(
            ConversationId::new("conv_8307904d-d256-77de-8e8a-63789f0ed1a5"),
            owner.clone(),
        );
        reader.read_cancel = Some(Arc::new(AtomicBool::new(false)));
        let (entered, receiving) = std::sync::mpsc::channel();
        *reader.read_progress_entered.lock().unwrap() = Some(entered);
        let reader = Arc::new(reader);
        let running = reader.clone();
        let task = std::thread::spawn(move || {
            running.lock().unwrap().query_row(
                "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n) SELECT sum(x) FROM n",
                [], |row| row.get::<_, i64>(0),
            )
        });
        receiving.recv().unwrap(); // The actual SQLite VM has executed 1,000 operations.
        reader.close_presentation_reader();
        assert!(
            matches!(task.join().unwrap(), Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::OperationInterrupted)
        );
        let conn = owner.lock().unwrap();
        // The zero-wait presentation policy is gone: the writer handler is
        // back, and SQLite's own timeout no longer governs this connection.
        assert_eq!(
            conn.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        conn.execute_batch(
            "CREATE TABLE normal_write (value INTEGER); INSERT INTO normal_write VALUES (1)",
        )
        .unwrap();
    }

    #[test]
    fn presentation_cancellation_ends_connection_wait_without_releasing_writer() {
        let manifest = manifest::Owner::uninstalled();
        let owner = Arc::new(ConnectionOwner {
            manifest,
            append_validation: append_validation::Owner::uninstalled(),
            available: Mutex::new(Some(Connection::open_in_memory().unwrap())),
            wake: Condvar::new(),
            poisoned: AtomicBool::new(false),
        });
        let writer = owner.lock().unwrap();
        let mut reader = SqliteConversationStore::from_connection_owner(
            ConversationId::new("conv_8307904d-d256-77de-8e8a-63789f0ed1a5"),
            owner.clone(),
        );
        reader.read_cancel = Some(Arc::new(AtomicBool::new(false)));
        let reader = Arc::new(reader);
        let running = reader.clone();
        let task = std::thread::spawn(move || running.lock().map(|_| ()));
        reader.close_presentation_reader();
        assert!(task.join().unwrap().is_err());
        drop(writer); // The writer never needed to release for reader termination.
    }
}

mod content;
mod content_encoding;
mod content_metadata;
mod observation;

mod observation_identity;
mod observation_members;
mod observation_statistics;
mod observation_turn;

mod observation_delivery;
mod observation_event;
mod observation_execution;
mod observation_membership;
mod observation_occupancy;
mod observation_readiness;
mod observation_receipt;
mod observation_request;
mod observation_request_summary;
mod observation_state;
mod observation_tools;
mod observation_trace;

/// Optional immutable read layout belongs to the native source transaction.
/// Failure retires this layout without refusing the authoritative write.
fn optional_display_layout<T>(
    connection: &Connection,
    build: impl FnOnce() -> Result<T, ConversationStoreError>,
) -> Option<T> {
    connection
        .execute_batch("SAVEPOINT rustx_display_layout")
        .ok()?;
    match build() {
        Ok(value)
            if connection
                .execute_batch("RELEASE rustx_display_layout")
                .is_ok() =>
        {
            Some(value)
        }
        _ => {
            let _ = connection
                .execute_batch("ROLLBACK TO rustx_display_layout; RELEASE rustx_display_layout");
            None
        }
    }
}
mod append_validation;
mod manifest;
mod projection;

mod observation_projection;

mod projection_publication;
