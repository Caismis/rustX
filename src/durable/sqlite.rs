//! `SQLite` implementation of the semantic conversation durability contract.
//!
//! One database contains the deliberately separate authority domains:
//! Pending Inbound, the append-only Message Ledger, immutable Surface
//! operations, immutable Request Snapshots, the append-only Event Journal, and
//! the durable publication plane (Issue #108). A narrow transcript ordering
//! spine stores references only; it is not a body store or a second history.
//! The tables share transactions where rustX needs one semantic linearization
//! point, but no table is a serialized `ConversationRecord` or transcript.

use std::sync::{Arc, Condvar, Mutex};

#[cfg(test)]
use std::collections::VecDeque;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

use sha2::{Digest, Sha256};

use crate::conversation::SurfaceRevision;
use crate::message::types::{AssistantContentBlock, MessageBlock};
use crate::publication::{PublicationAudit, PublicationFrame, PublicationPayload};
use crate::runtime::identity::{AttemptId, ConversationId, MessageId, PublicationStreamId};

use super::{ConversationStore, ConversationStoreError, LineageSeed, TranscriptCursor};

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
pub const SQLITE_SCHEMA_VERSION: i64 = 71;

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
    pub context: crate::durable::TranscriptCommitReceipt,
    pub audit: crate::durable::PublicationAuditReceipt,
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
) -> Result<crate::observation::turn::AttemptIdentitySource, ConversationStoreError> {
    observation_identity::journal(sequence, attempt)
}

impl SqliteConversationStore {
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
    /// Runs `read` inside one native read snapshot: the leased connection opens
    /// one deferred read transaction, and dropping it rolls the snapshot back.
    fn read_transaction<T>(
        &self,
        read: impl FnOnce(&Transaction<'_>) -> Result<T, ConversationStoreError>,
    ) -> Result<T, ConversationStoreError> {
        let connection = self.lock()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| storage(error.to_string()))?;
        read(&transaction)
    }
    /// The bounded observation read-cut lifecycle: validate the authentic
    /// native `cut` and run `read` in the same read snapshot, so no query can
    /// observe state the cut does not own.
    fn read_at_cut<T>(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        read: impl FnOnce(&Transaction<'_>) -> Result<T, ConversationStoreError>,
    ) -> Result<T, ConversationStoreError> {
        self.read_transaction(|transaction| {
            observation::validate(transaction, &self.conversation_id, cut)?;
            read(transaction)
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
mod pending_transaction_tests;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, Utc};
    use rusqlite::{Connection, Transaction, TransactionBehavior, params};

    use std::sync::atomic::{AtomicBool, Ordering};

    use std::sync::atomic::AtomicUsize;

    use sha2::{Digest, Sha256};

    use crate::conversation::{SurfaceOp, SurfaceRevision};
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, ContentBlockIndex, ContributionEmission, InboundKind, MessageBlock,
        UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::snapshot::RequestSnapshot;
    use crate::model::types::ModelRequest;
    use crate::publication::{PublicationFrame, PublicationPayload, PublicationStreamStart};
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, EventId, MessageId, PublicationStreamId, RequestId,
        ToolCallId, ToolId, TurnId,
    };

    use super::canonical::ledger::*;
    use super::canonical::transcript::*;
    use super::canonical::*;
    use super::journal::reference::*;
    use super::journal::*;
    use super::request::snapshot::*;
    use super::request::*;
    use super::schema::shape::*;
    use super::schema::*;
    use crate::durable::{
        CanonicalStore, InboxStore, JournalStore, ObservationStore, ProjectionStore,
        PublicationStore, RequestStore,
    };
    use crate::durable::{
        ConversationStore, ConversationStoreError, InboundDraft, LineageSeed,
        ModelTurnStartCommitDisposition, TranscriptCursor, TranscriptItem,
    };
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
    use crate::events::types::SubagentTerminalState;
    use crate::message::types::{
        AgentStatusGenerationMetadata, AgentStatusModuleId, AssistantMessageBlock, ContextKind,
        ToolMessageBlock,
    };
    use crate::model::catalog::ModelCapabilities;
    use crate::model::catalog::ModelCompat;
    use crate::model::finish::ModelFinishReason;
    use crate::model::invocation::{ModelInvocationConfig, RequestParams};
    use crate::model::snapshot::{ContributionStart, RequestIdentity};
    use crate::model::types::ModelProtocol;
    use crate::runtime::types::CancellationReason;
    use crate::tools::types::{
        ToolCall, ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus,
    };
    use chrono::TimeZone;

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
        let cut = super::super::TranscriptSnapshotCut {
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
        let unresolved = crate::observation::transcript::transcript_page_view(
            store.load_transcript_page(None, 1).unwrap(),
        )
        .unwrap();
        let current = &unresolved.entries[0].tool_calls[0];
        assert_eq!(current.message_id, Some(MessageId::new("b")));
        assert!(matches!(
            current.state,
            crate::observation::tool::ForegroundToolState::Assembled { .. }
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
            crate::observation::transcript::transcript_page_view(before.clone()).unwrap();
        for (index, tool) in unresolved.entries[0].tool_calls.iter().enumerate() {
            assert_eq!(tool.message_id, Some(MessageId::new("assistant")));
            assert_eq!(tool.block_index.get() as usize, index);
            assert!(matches!(
                tool.state,
                crate::observation::tool::ForegroundToolState::Assembled { .. }
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
        let projected = crate::observation::transcript::transcript_page_view(older).unwrap();
        // Canonical result acceptance is positive C evidence. No execution J
        // fact occurred in this fixture, so it cannot settle the lifecycle.
        for (tool, expected) in projected.entries[0].tool_calls.iter().zip(original_results) {
            assert!(matches!(
                tool.state,
                crate::observation::tool::ForegroundToolState::Assembled { .. }
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
        assert_eq!(SQLITE_SCHEMA_VERSION, 71);

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
        assert_eq!(SQLITE_SCHEMA_VERSION, 71);

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
        use crate::observation::content::CanonicalContentLocator;
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
        let store = SqliteConversationStore::in_memory(ConversationId::new(
            "conv_ab18a567-5190-73a5-8156-2e8693be6392",
        ))
        .unwrap();
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
        for (name, expression) in PRESENTATION_INDEXES {
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
                    "DROP INDEX {name}; CREATE INDEX {name} ON events({expression})"
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
        view: &crate::observation::status::AgentStatusView,
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
                crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                    position: 2
                }
            )
        );
        let fresh = view.opportunities.fresh_inbound.as_ref().unwrap();
        assert!(fresh.target_message_id.is_none());
        assert_eq!(
            fresh.target_locator,
            Some(
                crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                    position: 1
                }
            )
        );
        assert_eq!(
            fresh.transcript_anchor,
            Some(crate::observation::transcript::RuntimeClientTranscriptCursor::new(1))
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
pub(crate) mod canonical;
mod compaction_resources;
mod inbox;
mod journal;
mod lineage;
mod provenance;
mod publication;
mod reading;
#[cfg(test)]
mod reading_tests;
pub(crate) mod request;
mod schema;

#[cfg(test)]
mod executor_wait_tests {
    use std::sync::{Arc, Condvar, Mutex};

    use rusqlite::Connection;

    use std::sync::atomic::AtomicBool;

    use super::schema::configure_connection;
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
    use std::sync::{Arc, Condvar, Mutex};

    use rusqlite::Connection;

    use std::sync::atomic::AtomicBool;

    use crate::runtime::identity::ConversationId;

    use super::*;
    use crate::durable::ConversationStore;

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
