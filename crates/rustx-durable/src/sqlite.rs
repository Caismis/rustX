//! `SQLite` implementation of the semantic conversation durability contract.
//!
//! One database contains the deliberately separate authority domains:
//! Pending Inbound, the append-only Message Ledger, immutable Surface
//! operations, immutable Request Snapshots, the append-only Event Journal, and
//! the durable publication plane (Issue #108). A narrow transcript ordering
//! spine stores references only; it is not a body store or a second history.
//! The tables share transactions where rustX needs one semantic linearization
//! point, but no table is a serialized `ConversationRecord` or transcript.

#[cfg(any(test, feature = "test-hooks"))]
thread_local! { static OPTIONAL_LAYOUT_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
use crate::TranscriptCommitReceipt;
use crate::inbound::InboundSequence;
use crate::model::snapshot::RequestSnapshot;
use crate::process_death;
use idle_cache::ConnectionSlot;
use rusqlite::TransactionBehavior;
use rustx_publication::{PublicationSettlement, PublicationStreamStart};
use rustx_types::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use statements::FixedStatements;
use std::sync::{Arc, Condvar, Mutex};

#[cfg(any(test, feature = "test-hooks"))]
use std::collections::VecDeque;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(any(test, feature = "test-hooks"))]
use std::sync::atomic::AtomicUsize;

use sha2::{Digest, Sha256};

use rustx_conversation::SurfaceRevision;
use rustx_publication::{PublicationAudit, PublicationFrame, PublicationPayload};
use rustx_types::message::types::{AssistantContentBlock, MessageBlock};
use rustx_types::runtime::identity::{AttemptId, ConversationId, MessageId, PublicationStreamId};

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
#[cfg(any(test, feature = "test-hooks"))]
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
/// thread. See `CONVERSATION_STORE_OPENS_ON_THREAD` for why the per-thread
/// delta — not the global count — is the contamination-free measurement.
#[cfg(any(test, feature = "test-hooks"))]
#[must_use]
pub fn conversation_store_opens_on_this_thread() -> u64 {
    CONVERSATION_STORE_OPENS_ON_THREAD.with(std::cell::Cell::get)
}

/// Records one store open on both the process-wide and the per-thread
/// diagnostics counters.
fn count_conversation_store_open() {
    CONVERSATION_STORE_OPENS.fetch_add(1, Ordering::Relaxed);
    #[cfg(any(test, feature = "test-hooks"))]
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
/// The mutation-lock seam a `SqliteConversationStore` needs from its owning
/// session allocation. The journal serializes ownership-affecting durable
/// transitions under the allocation's mutation guard; the holder's concrete
/// lifecycle machinery stays outside this crate.
pub trait OwnershipLock: std::fmt::Debug + Send + Sync {
    /// Acquire the mutation guard; the returned box is held for the duration of
    /// the journal transaction and dropped when it commits or rolls back.
    ///
    /// # Errors
    /// Failure to acquire the guard fails the journal transition.
    fn ownership_mutation(&self) -> std::io::Result<Box<dyn std::any::Any + Send>>;
}

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
/// [`EVENT_SCHEMA_VERSION`](rustx_types::events::types::EVENT_SCHEMA_VERSION) envelope
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
/// Version 59 replaces canonical summary/Surface replacement ownership with
/// independently durable typed checkpoints, historical activations, operation
/// receipts and exact frozen request manifests. Earlier stores are refused.
/// Version 60 normalizes immutable activation eligibility and installed prefix authority.
/// Immutable context projection plus finite bounded observation layouts.
/// Version 62 gives request fields explicit inline-header or original-span storage.
/// Version 67 stores child lookup paths rather than duplicate Source Fact payloads.
/// Version 68 binds canonical binary `ContentRefs` to frozen fixed-chunk digests.
/// Version 69 requires explicit native artifact settlement evidence.
/// Main versions 70/71 removed redundant active history and added the bounded
/// authenticated Source page codec. The AUX branch independently used 70..=73
/// for request ownership, audit/lineage and consumer-defined attribution.
/// Version 74 combines both layouts; neither prior branch layout is accepted.
/// Version 75 adds bounded Auxiliary retries and first-Human Title admission.
/// Version 76 retains external Auxiliary delivery and bounded result evidence.
/// Version 77 includes the native JSON Schema capability and prepared response
/// schema in frozen request fingerprints, including ordinary text requests.
/// Version 78 integrated Scheduler definitions, generations and Human-request budgets.
/// Version 79 combined Scheduler contracts with structured request fingerprints.
/// Version 80 also includes Workflow node-wait Journal and Auxiliary Call identities.
/// Version 82 combines Scheduler and physical lifecycle contracts with reasoning-free
/// model invocations, workspace changes and MCP draft preparation. Earlier stores are rejected.
/// Version 83 adds original Workflow node input/output Journal facts and data-node kinds.
/// Version 84 requires exact Workflow run/instance indexes for bounded historical reads.
/// Version 85 records Session configuration commits as Journal facts with
/// their own transcript positions.
/// Version 70 removes the redundant serialized active history from the Surface head.
/// Version 71 stores authenticated Source pages with a bounded binary codec.
/// Version 72 excludes unrelated NULL-key rows from scoped event indexes.
/// Version 73 derives state field previews lazily from retained geometry and bytes.
/// Version 74 removes independent original-capture Source witness seats.
/// Version 75 makes ordinary accepted model output and its no-tool boundary atomic.
/// Version 77 shares eagerly indexed immutable request configuration suffixes.
/// Version 79 uses compact private request-inspection records and removes obsolete event display copies.
/// Version 80 keeps publication progress in connection-local memory, consolidates sparse event indexes,
/// and derives request field previews from authenticated original bytes.
/// Version 81 stores event quote overlays over native bytes and compact request width runs.
/// Version 82 shares two sparse event head indexes across four independent read domains.
/// Version 83 stores compact independent event evidence and prefix-compressed request pages.
/// Version 84 merges chronological attempt reads through the shared attempt-kind index.
/// Version 85 stores immutable first content chunks alongside their exact frontier.
/// Version 86 stores bounded binary Statistics and Execution tuples with unchanged JSON admission.
/// Version 87 removes redundant trailing sequence keys from rowid event indexes.
/// Version 90 stores independent statistical contributions with sparse checkpoints.
/// Version 91 binds shared Request Source preparation to frozen native configuration.
/// Version 95 stores one fact-bound complete Statistics record per relevant event.
/// Version 96 stores bounded full-key binary Request Source navigation pages.
/// Version 98 indexes Workflow coordinates and nodes only for matching facts.
/// Version 99 scopes the Agent request index to Agent-owned facts.
/// Version 100 combines the scoped native indexes and observation layout with shared-control inbox semantics.
/// Version 107 stores timestamp prefix maxima and pins bounded ledger/owner seeks.
/// It also shares immutable Trace catalog versions with witnessed request bindings.
/// Version 108 combines these with shared Tool association facts and immutable request payloads.
/// Version 109 unifies sparse event identities, integer request ownership, selected-input
/// and Source trees, co-located transcript headers, numeric Content coordinates, and Surface identity.
/// Version 113 shares typed native event keys across sparse indexes and gives
/// canonical Tool lookup an independent native Ledger position.
/// Version 114 adds admission-bound Workflow run snapshots and the generated
/// workflow-agent node index. Earlier stores are refused; there is no migration.
pub const SQLITE_SCHEMA_VERSION: i64 = 114;

/// One operation in a deterministic admission fault script.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionFaultOperation {
    /// Fail the next scripted pending-batch selection.
    SelectPendingBatch,
    /// Fail the next scripted pending-batch adoption.
    AdoptPendingBatch,
}

/// One scripted compaction-stage fault used by atomicity regressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionFaultOperation {
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
    #[cfg(any(test, feature = "test-hooks"))]
    AfterCommit,
    #[cfg(any(test, feature = "test-hooks"))]
    AfterCommitOneHotReadFailure,
    #[cfg(any(test, feature = "test-hooks"))]
    AfterCommitTwoHotReadFailures,
}

/// One scripted model-turn-start-stage fault used by atomicity regressions
/// (Issue #12, M9b).
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStartFaultOperation {
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

pub struct HiddenContextAuditFixture {
    pub store: Arc<SqliteConversationStore>,
    pub before: crate::observation::ObservationReadCut,
    pub context: crate::TranscriptCommitReceipt,
    pub audit: crate::PublicationAuditReceipt,
}

/// Shared Journal observation binding retained by native scoped writers.
type JournalObserverBinding = Option<(Arc<dyn super::presentation::JournalObserver>, u64)>;

/// The native durable conversation authority for one conversation.
pub struct SqliteConversationStore {
    pub conversation_id: ConversationId,
    pub conn: Arc<ConnectionOwner>,
    read_cancel: Option<Arc<AtomicBool>>,
    write_cancel: Option<Arc<WriteCancellation>>,
    #[cfg(any(test, feature = "test-hooks"))]
    read_progress_entered: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    journal_observer: Arc<Mutex<JournalObserverBinding>>,
    lifecycle: Option<Arc<dyn crate::sqlite::OwnershipLock>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_accept_remaining: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    head_read_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    // Park only after SQLite has granted the writer, before reading pending rows.
    #[cfg(any(test, feature = "test-hooks"))]
    pending_mutation_transaction_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pending_adoption_transaction_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_adopt_remaining: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_select_remaining: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub checkpoint_read_counts: Arc<[AtomicUsize; 3]>,
    #[cfg(any(test, feature = "test-hooks"))]
    fail_projection_history_remaining: AtomicUsize,
    #[cfg(any(test, feature = "test-hooks"))]
    projection_commit_return_probe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_event_remaining: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_terminal_event_remaining: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub terminal_event_attempts: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub request_snapshot_page_reads: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub request_snapshot_body_reads: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub contribution_head_lookup_reads: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub admission_fault_script: Arc<Mutex<VecDeque<AdmissionFaultOperation>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub compaction_fault_script: Arc<Mutex<VecDeque<CompactionFaultOperation>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub request_start_fault_script: Arc<Mutex<VecDeque<RequestStartFaultOperation>>>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_publication_frames_remaining: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_publication_terminal_remaining: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_publication_audit_remaining: Arc<AtomicUsize>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub fail_workflow_snapshot_remaining: Arc<AtomicUsize>,
}

impl std::fmt::Debug for SqliteConversationStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteConversationStore")
            .field("conversation_id", &self.conversation_id)
            .finish_non_exhaustive()
    }
}

pub struct ConnectionOwner {
    available: Mutex<ConnectionSlot>,
    wake: Condvar,
    poisoned: AtomicBool,
    lease_generation: AtomicU64,
    active_interrupt: Mutex<Option<(u64, rusqlite::InterruptHandle)>>,
    pub manifest: Arc<manifest::Owner>,
    append_validation: Arc<append_validation::Owner>,
    response: Arc<observation_response::Owner>,
    pub event_guard: event_insert::Guard,
    /// Main-database writes since the current transaction/lease boundary.
    /// Read-only ROLLBACK must not invalidate committed immutable evidence.
    transaction_writes: Arc<AtomicBool>,
}

#[derive(Default)]
struct WriteCancellation {
    cancelled: Arc<AtomicBool>,
    generation: AtomicU64,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static AUXILIARY_CONNECTION_WAIT: std::cell::RefCell<Option<std::sync::mpsc::SyncSender<()>>> = const { std::cell::RefCell::new(None) };
}

impl ConnectionOwner {
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn lock(&self) -> Result<ConnectionLease<'_>, ConversationStoreError> {
        self.acquire(None)
    }
    #[cfg(any(test, feature = "test-hooks"))]
    fn acquire(
        &self,
        cancelled: Option<&AtomicBool>,
    ) -> Result<ConnectionLease<'_>, ConversationStoreError> {
        self.acquire_scoped(cancelled, None)
    }

    fn acquire_scoped(
        &self,
        cancelled: Option<&AtomicBool>,
        write_scope: Option<&WriteCancellation>,
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
                return Err(if write_scope.is_some() {
                    ConversationStoreError::OperationCancelled
                } else {
                    storage("presentation read cancelled")
                });
            }
            if let Some(connection) = slot.connection.take() {
                let Ok(previous) = self.lease_generation.try_update(
                    Ordering::AcqRel,
                    Ordering::Acquire,
                    |current| current.checked_add(1),
                ) else {
                    slot.returned(Some(connection));
                    return Err(storage("connection lease generation exhausted"));
                };
                let generation = previous + 1;
                *self
                    .active_interrupt
                    .lock()
                    .expect("connection interrupt owner") =
                    Some((generation, connection.get_interrupt_handle()));
                if let Some(scope) = write_scope {
                    scope.generation.store(generation, Ordering::Release);
                }
                return Ok(ConnectionLease {
                    connection: Some(connection),
                    owner: self,
                });
            }
            #[cfg(any(test, feature = "test-hooks"))]
            AUXILIARY_CONNECTION_WAIT.with(|probe| {
                if let Some(entered) = probe.borrow_mut().take() {
                    let _ = entered.send(());
                }
            });
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
    #[cfg(any(test, feature = "test-hooks"))]
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

/// Thread-local observation of `writer_busy` for deterministic contention
/// tests: SQLite invokes the handler on the waiting writer's own thread.
#[cfg(any(test, feature = "test-hooks"))]
pub mod busy_probe {
    use std::cell::RefCell;
    type Probe = Box<dyn FnMut(i32)>;
    thread_local! {
        static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
    }
    /// Observe this thread's writer busy waits until the returned guard drops.
    pub fn observe(probe: impl FnMut(i32) + 'static) -> Guard {
        PROBE.with(|slot| *slot.borrow_mut() = Some(Box::new(probe)));
        Guard
    }
    pub struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            PROBE.with(|slot| slot.borrow_mut().take());
        }
    }
    pub fn invoked(count: i32) {
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
pub struct ConnectionLease<'a> {
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
        if self
            .connection
            .as_ref()
            .is_some_and(Connection::is_autocommit)
        {
            self.owner
                .transaction_writes
                .store(false, Ordering::Release);
        }
        let mut slot = self.owner.available.lock().expect("connection slot");
        *self
            .owner
            .active_interrupt
            .lock()
            .expect("connection interrupt owner") = None;
        slot.returned(self.connection.take());
        self.owner.wake.notify_all();
    }
}

/// Stages only successfully committed Journal receipts before releasing the
/// store lock. Staging cannot advance a Runtime Client cursor. The native owner
/// subsequently publishes the exact receipt after installing its semantic state.
/// Rollbacks and read-only guards produce no receipt.
pub struct StoreGuard<'a> {
    connection: ConnectionLease<'a>,
    cancellable: bool,
    changes: u64,
    observer: &'a Mutex<JournalObserverBinding>,
    event_guard: &'a event_insert::Guard,
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
pub fn journal_prefix(connection: &Connection) -> Result<u64, ConversationStoreError> {
    let through: i64 = connection
        .prepare_cached("SELECT next_event_sequence FROM rustx_store WHERE id=1")
        .and_then(|mut statement| statement.query_row([], |row| row.get(0)))
        .map_err(|error| sqlite_error(&error, error.to_string()))?;
    sequence_from_i64(through)
}
impl Drop for StoreGuard<'_> {
    fn drop(&mut self) {
        if self.cancellable {
            self.connection
                .progress_handler(0, None::<fn() -> bool>)
                .expect("restore SQLite progress policy");
            writer_busy_policy(&self.connection).expect("restore SQLite busy policy");
        }
        self.event_guard.publish_statistics(&self.connection);
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

pub fn journal_attempt_source(
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
            write_cancel: None,
            #[cfg(any(test, feature = "test-hooks"))]
            read_progress_entered: Mutex::new(None),
            journal_observer: Arc::new(Mutex::new(None)),
            lifecycle: None,
            #[cfg(any(test, feature = "test-hooks"))]
            head_read_probe: Mutex::new(None),
            #[cfg(any(test, feature = "test-hooks"))]
            pending_mutation_transaction_probe: Mutex::new(None),
            #[cfg(any(test, feature = "test-hooks"))]
            pending_adoption_transaction_probe: Mutex::new(None),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_accept_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_adopt_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_select_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            checkpoint_read_counts: Arc::new(std::array::from_fn(|_| AtomicUsize::new(0))),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_projection_history_remaining: AtomicUsize::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            projection_commit_return_probe: Mutex::new(None),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_event_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_terminal_event_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            terminal_event_attempts: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            request_snapshot_page_reads: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            request_snapshot_body_reads: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            contribution_head_lookup_reads: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            admission_fault_script: Arc::new(Mutex::new(VecDeque::new())),
            #[cfg(any(test, feature = "test-hooks"))]
            compaction_fault_script: Arc::new(Mutex::new(VecDeque::new())),
            #[cfg(any(test, feature = "test-hooks"))]
            request_start_fault_script: Arc::new(Mutex::new(VecDeque::new())),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_publication_frames_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_publication_terminal_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_publication_audit_remaining: Arc::new(AtomicUsize::new(0)),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_workflow_snapshot_remaining: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn auxiliary_writer_scope(&self) -> Self {
        let mut writer =
            Self::from_connection_owner(self.conversation_id.clone(), self.conn.clone());
        writer.write_cancel = Some(Arc::new(WriteCancellation::default()));
        writer.lifecycle.clone_from(&self.lifecycle);
        writer.journal_observer = self.journal_observer.clone();
        writer
    }

    pub fn lock(&self) -> Result<StoreGuard<'_>, ConversationStoreError> {
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
        let cancellation = self.read_cancel.clone().or_else(|| {
            self.write_cancel
                .as_ref()
                .map(|scope| scope.cancelled.clone())
        });
        let connection = self
            .conn
            .acquire_scoped(cancellation.as_deref(), self.write_cancel.as_deref())?;
        let cancellable = cancellation.is_some();
        if let Some(cancel) = cancellation {
            if self.read_cancel.is_some() {
                connection
                    .busy_timeout(std::time::Duration::ZERO)
                    .map_err(|e| storage(e.to_string()))?;
            }
            #[cfg(any(test, feature = "test-hooks"))]
            let mut entered = self.read_progress_entered.lock().unwrap().take();
            #[cfg(any(test, feature = "test-hooks"))]
            let progress_interval = if start_probe::pending() { 1 } else { 1000 };
            #[cfg(not(any(test, feature = "test-hooks")))]
            let progress_interval = 1000;
            #[cfg(any(test, feature = "test-hooks"))]
            let mut probe_ticks = 0;
            connection
                .progress_handler(
                    progress_interval,
                    Some(move || {
                        #[cfg(any(test, feature = "test-hooks"))]
                        if progress_interval == 1 {
                            if start_probe::progress() {
                                // Reset the ordinary cadence after the consumed
                                // probe so rollback retains its production policy.
                                probe_ticks = 0;
                            } else {
                                probe_ticks += 1;
                                if probe_ticks < 1000 {
                                    return false;
                                }
                                probe_ticks = 0;
                            }
                        }
                        #[cfg(any(test, feature = "test-hooks"))]
                        if let Some(entered) = entered.take() {
                            let _ = entered.send(());
                        }
                        cancel.load(Ordering::Acquire)
                    }),
                )
                .map_err(|error| {
                    writer_busy_policy(&connection).expect("restore SQLite busy policy");
                    sqlite_error(&error, format!("install SQLite cancellation: {error}"))
                })?;
        }
        let changes = connection.total_changes();
        Ok(StoreGuard {
            connection,
            cancellable,
            changes,
            observer: &self.journal_observer,
            event_guard: &self.conn.event_guard,
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
            .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
        read(&transaction)
    }
    /// The bounded observation read-cut lifecycle: validate the authentic
    /// native `cut` and run `read` in the same read snapshot, so no query can
    /// observe state the cut does not own.
    fn read_at_cut<T>(
        &self,
        cut: &crate::observation::ObservationReadCut,
        read: impl FnOnce(&Transaction<'_>) -> Result<T, ConversationStoreError>,
    ) -> Result<T, ConversationStoreError> {
        self.read_transaction(|transaction| {
            observation::validate(transaction, &self.conversation_id, cut)?;
            read(transaction)
        })
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn trace_read_work() -> (usize, usize) {
        observation_trace::read_work()
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn content_read_work() -> (usize, usize) {
        content::last_read_work()
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn request_source_read_work() -> observation_request::composed::Work {
        observation_request::composed::take_work()
    }
    /// Fails the next `count` publication staging commits.
    #[cfg(target_os = "linux")]
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_publication_frames_times(&self, count: usize) {
        self.fail_publication_frames_remaining
            .store(count, Ordering::SeqCst);
    }
    /// Fails the next `count` publication terminal (U) commits.
    #[cfg(target_os = "linux")]
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_publication_terminal_times(&self, count: usize) {
        self.fail_publication_terminal_remaining
            .store(count, Ordering::SeqCst);
    }
    /// Fails the next `count` publication audit terminalizations.
    #[cfg(target_os = "linux")]
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_publication_audit_times(&self, count: usize) {
        self.fail_publication_audit_remaining
            .store(count, Ordering::SeqCst);
    }
    /// Fails the next `count` atomic Workflow snapshot start commits.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_workflow_snapshot_times(&self, count: usize) {
        self.fail_workflow_snapshot_remaining
            .store(count, Ordering::SeqCst);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_next_accept_commit(&self) {
        self.arm_fail_accept_times(1);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_accept_times(&self, count: usize) {
        self.fail_accept_remaining
            .fetch_add(count, Ordering::SeqCst);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn hidden_context_audit_fixture() -> HiddenContextAuditFixture {
        test_support::hidden_context_audit_fixture()
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_next_adopt_commit(&self) {
        self.arm_fail_adopt_times(1);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_adopt_times(&self, count: usize) {
        self.fail_adopt_remaining.fetch_add(count, Ordering::SeqCst);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_select_times(&self, count: usize) {
        self.fail_select_remaining
            .fetch_add(count, Ordering::SeqCst);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_event_times(&self, count: usize) {
        self.fail_event_remaining.fetch_add(count, Ordering::SeqCst);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_fail_next_terminal_event(&self) {
        self.fail_terminal_event_remaining
            .fetch_add(1, Ordering::SeqCst);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn terminal_event_attempts(&self) -> usize {
        self.terminal_event_attempts.load(Ordering::SeqCst)
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn request_snapshot_page_reads(&self) -> usize {
        self.request_snapshot_page_reads.load(Ordering::SeqCst)
    }
    /// Returns how many bounded latest-emission head reads the test exercised.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn contribution_head_lookup_reads(&self) -> usize {
        self.contribution_head_lookup_reads.load(Ordering::SeqCst)
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_admission_fault_script(
        &self,
        operations: impl IntoIterator<Item = AdmissionFaultOperation>,
    ) {
        self.admission_fault_script
            .lock()
            .expect("admission fault script lock")
            .extend(operations);
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_compaction_fault_script(
        &self,
        operations: impl IntoIterator<Item = CompactionFaultOperation>,
    ) {
        self.compaction_fault_script
            .lock()
            .expect("compaction fault script lock")
            .extend(operations);
    }
    /// Park after durable commit and Journal staging, before returning the receipt.
    #[cfg(target_os = "linux")]
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_projection_commit_return_probe(&self, probe: impl FnOnce() + Send + 'static) {
        *self
            .projection_commit_return_probe
            .lock()
            .expect("projection return probe") = Some(Box::new(probe));
    }
    #[cfg(any(test, feature = "test-hooks"))]
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
    #[cfg(any(test, feature = "test-hooks"))]
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
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn arm_request_start_fault_script(
        &self,
        operations: impl IntoIterator<Item = RequestStartFaultOperation>,
    ) {
        self.request_start_fault_script
            .lock()
            .expect("request-start fault script lock")
            .extend(operations);
    }
    #[cfg(any(test, feature = "test-hooks"))]
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
    #[cfg(any(test, feature = "test-hooks"))]
    fn consume(counter: &AtomicUsize) -> bool {
        counter
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }
    /// Test-only sequence exhaustion hook retained for deterministic boundary
    /// tests; it changes no process-local allocator because the value is in
    /// the durable store.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn force_next_sequence_for_test(&self, value: i64) {
        self.conn
            .lock()
            .expect("connection")
            .execute(
                "UPDATE rustx_store SET next_inbound_sequence = ?1 WHERE id = 1",
                [value],
            )
            .expect("force sequence");
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn install_after_request_row_read_hook(
        request_id: rustx_types::runtime::identity::RequestId,
        hook: impl FnOnce() + 'static,
    ) {
        auxiliary::install_after_request_row_read_hook(request_id, hook);
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
    #[cfg(any(test, feature = "test-hooks"))]
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

    fn auxiliary_writer(&self) -> Arc<dyn ConversationStore> {
        Arc::new(self.auxiliary_writer_scope())
    }

    fn cancel_auxiliary_writer(&self) {
        if let Some(scope) = &self.write_cancel {
            let _slot = self.conn.available.lock().expect("connection slot");
            scope.cancelled.store(true, Ordering::Release);
            if let Some((generation, interrupt)) = self
                .conn
                .active_interrupt
                .lock()
                .expect("connection interrupt owner")
                .as_ref()
                && *generation == scope.generation.load(Ordering::Acquire)
            {
                interrupt.interrupt();
            }
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

pub fn decode<T: for<'de> Deserialize<'de>>(
    json: &str,
    name: &str,
) -> Result<T, ConversationStoreError> {
    serde_json::from_str(json).map_err(|error| storage(format!("decode {name}: {error}")))
}

fn sequence_from_i64(value: i64) -> Result<u64, ConversationStoreError> {
    u64::try_from(value).map_err(|_| ConversationStoreError::SequenceExhausted)
}

pub fn seq_to_i64(value: u64) -> Result<i64, ConversationStoreError> {
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

/// Preserve SQLite's typed interrupt cause before diagnostic formatting.
pub fn sqlite_error(
    error: &rusqlite::Error,
    diagnostic: impl Into<String>,
) -> ConversationStoreError {
    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted) {
        ConversationStoreError::OperationCancelled
    } else {
        storage(diagnostic)
    }
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

// Finite archive readers share the concrete durable schema, never transport DTOs.
pub mod archive;
#[cfg(any(test, feature = "test-hooks"))]
pub mod auxiliary;
#[cfg(not(any(test, feature = "test-hooks")))]
mod auxiliary;
pub mod canonical;
#[cfg(any(test, feature = "test-hooks"))]
pub mod compaction_resources;
#[cfg(not(any(test, feature = "test-hooks")))]
mod compaction_resources;
#[cfg(any(test, feature = "test-hooks"))]
pub mod inbox;
#[cfg(not(any(test, feature = "test-hooks")))]
mod inbox;
#[cfg(any(test, feature = "test-hooks"))]
pub mod journal;
#[cfg(not(any(test, feature = "test-hooks")))]
mod journal;
#[cfg(any(test, feature = "test-hooks"))]
pub mod lineage;
#[cfg(not(any(test, feature = "test-hooks")))]
mod lineage;
#[cfg(any(test, feature = "test-hooks"))]
pub mod provenance;
#[cfg(not(any(test, feature = "test-hooks")))]
mod provenance;
#[cfg(any(test, feature = "test-hooks"))]
pub mod publication;
#[cfg(not(any(test, feature = "test-hooks")))]
mod publication;
#[cfg(any(test, feature = "test-hooks"))]
pub mod reading;
#[cfg(not(any(test, feature = "test-hooks")))]
mod reading;
#[cfg(any(test, feature = "test-hooks"))]
pub mod recovery_frontier;
#[cfg(not(any(test, feature = "test-hooks")))]
mod recovery_frontier;
pub mod request;
#[cfg(any(test, feature = "test-hooks"))]
pub mod request_owner_shape;
#[cfg(not(any(test, feature = "test-hooks")))]
mod request_owner_shape;
#[cfg(any(test, feature = "test-hooks"))]
pub mod schedule;
#[cfg(not(any(test, feature = "test-hooks")))]
mod schedule;
pub mod schedule_read;
#[cfg(any(test, feature = "test-hooks"))]
pub mod scheduled_workflow;
#[cfg(not(any(test, feature = "test-hooks")))]
mod scheduled_workflow;
#[cfg(any(test, feature = "test-hooks"))]
pub mod schema;
#[cfg(not(any(test, feature = "test-hooks")))]
mod schema;
#[cfg(any(test, feature = "test-hooks"))]
pub mod start_probe;
#[cfg(any(test, feature = "test-hooks"))]
pub use start_probe::AuxiliaryStartProbe;
#[cfg(any(test, feature = "test-hooks"))]
pub mod test_support;
#[cfg(any(test, feature = "test-hooks"))]
pub use query_work::MeasuredStatements;
#[cfg(any(test, feature = "test-hooks"))]
pub use test_support::append_completed_turns;
#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-hooks"))]
pub mod content;
#[cfg(not(any(test, feature = "test-hooks")))]
mod content;
#[cfg(any(test, feature = "test-hooks"))]
pub mod content_encoding;
#[cfg(not(any(test, feature = "test-hooks")))]
mod content_encoding;
pub use rustx_types::durable::sqlite::content_metadata;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation;

#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_identity;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_identity;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_members;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_members;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_statistics;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_statistics;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_turn;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_turn;

#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_delivery;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_delivery;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_event;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_event;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_execution;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_execution;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_membership;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_membership;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_occupancy;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_occupancy;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_readiness;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_readiness;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_receipt;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_receipt;
pub mod observation_request;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_request_summary;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_request_summary;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_state;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_state;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_tools;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_tools;
#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_trace;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_trace;
#[cfg(any(test, feature = "test-hooks"))]
pub mod workflow_snapshot;
#[cfg(not(any(test, feature = "test-hooks")))]
mod workflow_snapshot;
#[cfg(any(test, feature = "test-hooks"))]
pub mod workflow_trace;
#[cfg(not(any(test, feature = "test-hooks")))]
mod workflow_trace;

/// A missing derived value may be contained; cancellation and transaction
/// loss must reach the native owner even in read-only preparation helpers.
fn optional_display_value<T>(
    result: Result<T, ConversationStoreError>,
) -> Result<Option<T>, ConversationStoreError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(
            error @ (ConversationStoreError::OperationCancelled
            | ConversationStoreError::TransactionInvalid(_)),
        ) => Err(error),
        Err(_) => Ok(None),
    }
}

/// Optional immutable read layout belongs to the native source transaction.
/// Only a failure contained by its savepoint may retire the optional layout.
/// Cancellation and loss of transaction integrity must reach the write owner
/// before any subsequent authoritative SQL is issued.
fn optional_display_layout<T>(
    connection: &Connection,
    build: impl FnOnce() -> Result<T, ConversationStoreError>,
) -> Result<Option<T>, ConversationStoreError> {
    #[cfg(any(test, feature = "test-hooks"))]
    OPTIONAL_LAYOUT_WORK.with(|work| work.set(work.get() + 1));
    if connection.is_autocommit() {
        return Err(ConversationStoreError::TransactionInvalid(
            "optional layout requires its original owning transaction".into(),
        ));
    }
    connection
        .execute_cached("SAVEPOINT rustx_display_layout", [])
        .map_err(|error| match sqlite_error(&error, error.to_string()) {
            ConversationStoreError::OperationCancelled => {
                ConversationStoreError::OperationCancelled
            }
            error => ConversationStoreError::TransactionInvalid(format!(
                "optional layout savepoint: {error}"
            )),
        })?;
    match build() {
        Ok(value) => {
            if connection.is_autocommit() {
                return Err(ConversationStoreError::TransactionInvalid(
                    "optional layout lost its owning transaction".into(),
                ));
            }
            connection
                .execute_cached("RELEASE rustx_display_layout", [])
                .map_err(|error| match sqlite_error(&error, error.to_string()) {
                    ConversationStoreError::OperationCancelled => {
                        ConversationStoreError::OperationCancelled
                    }
                    error => ConversationStoreError::TransactionInvalid(format!(
                        "optional layout release: {error}"
                    )),
                })?;
            if connection.is_autocommit() {
                return Err(ConversationStoreError::TransactionInvalid(
                    "optional layout release lost its owning transaction".into(),
                ));
            }
            Ok(Some(value))
        }
        Err(error)
            if connection.is_autocommit()
                || matches!(
                    error,
                    ConversationStoreError::OperationCancelled
                        | ConversationStoreError::TransactionInvalid(_)
                ) =>
        {
            Err(error)
        }
        Err(error) => {
            connection
                .execute_cached("ROLLBACK TO rustx_display_layout", [])
                .and_then(|_| connection.execute_cached("RELEASE rustx_display_layout", []))
                .map_err(
                    |cleanup| match sqlite_error(&cleanup, cleanup.to_string()) {
                        ConversationStoreError::OperationCancelled => {
                            ConversationStoreError::OperationCancelled
                        }
                        cleanup => ConversationStoreError::TransactionInvalid(format!(
                            "optional layout failed ({error}); savepoint cleanup failed ({cleanup})"
                        )),
                    },
                )?;
            if connection.is_autocommit() {
                return Err(ConversationStoreError::TransactionInvalid(
                    "optional layout cleanup lost its owning transaction".into(),
                ));
            }
            Ok(None)
        }
    }
}
#[cfg(any(test, feature = "test-hooks"))]
pub mod append_validation;
#[cfg(not(any(test, feature = "test-hooks")))]
mod append_validation;
#[cfg(any(test, feature = "test-hooks"))]
pub mod manifest;
#[cfg(not(any(test, feature = "test-hooks")))]
mod manifest;
#[cfg(any(test, feature = "test-hooks"))]
pub mod projection;
#[cfg(not(any(test, feature = "test-hooks")))]
mod projection;

#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_projection;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_projection;

#[cfg(any(test, feature = "test-hooks"))]
pub mod projection_publication;
#[cfg(not(any(test, feature = "test-hooks")))]
mod projection_publication;

#[cfg(any(test, feature = "test-hooks"))]
pub use auxiliary::install_lookup_hook as install_auxiliary_lookup_hook;

/// A connection lease owns SQL, not the condition mutex. Cancellation can
/// therefore wake a waiting presentation reader without waiting for SQL.
#[cfg(any(test, feature = "test-hooks"))]
pub mod idle_cache;
#[cfg(not(any(test, feature = "test-hooks")))]
mod idle_cache;

#[cfg(test)]
mod scoped_event_index_tests;

#[cfg(any(test, feature = "test-hooks"))]
pub mod event_heads;
#[cfg(not(any(test, feature = "test-hooks")))]
mod event_heads;

#[cfg(any(test, feature = "test-hooks"))]
pub mod event_holes;
#[cfg(not(any(test, feature = "test-hooks")))]
mod event_holes;

pub use rustx_types::durable::sqlite::statements;

#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_checkpoint;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_checkpoint;

#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_codec;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_codec;

#[cfg(any(test, feature = "test-hooks"))]
pub mod observation_response;
#[cfg(not(any(test, feature = "test-hooks")))]
mod observation_response;

#[cfg(any(test, feature = "test-hooks"))]
pub mod append_facts;
#[cfg(not(any(test, feature = "test-hooks")))]
mod append_facts;

#[cfg(any(test, feature = "test-hooks"))]
pub mod attempt_start;
#[cfg(not(any(test, feature = "test-hooks")))]
mod attempt_start;

#[cfg(any(test, feature = "test-hooks"))]
pub mod event_insert;
#[cfg(not(any(test, feature = "test-hooks")))]
mod event_insert;
pub use rustx_types::durable::sqlite::event_kind_key;
#[cfg(any(test, feature = "test-hooks"))]
pub mod event_owner_key;
#[cfg(not(any(test, feature = "test-hooks")))]
mod event_owner_key;

#[cfg(any(test, feature = "test-hooks"))]
pub mod associations;
#[cfg(any(test, feature = "test-hooks"))]
pub mod auxiliary_lineage;
#[cfg(not(any(test, feature = "test-hooks")))]
pub(crate) mod auxiliary_lineage;

#[cfg(not(any(test, feature = "test-hooks")))]
mod associations;
#[cfg(any(test, feature = "test-hooks"))]
pub mod native_request;
#[cfg(not(any(test, feature = "test-hooks")))]
mod native_request;

#[cfg(test)]
mod optional_layout_tests;

#[cfg(any(test, feature = "test-hooks"))]
pub mod final_settlement;
#[cfg(not(any(test, feature = "test-hooks")))]
mod final_settlement;

#[cfg(test)]
mod bounded_work_tests;
#[cfg(test)]
mod compact_lookup_tests;
#[cfg(any(test, feature = "test-hooks"))]
pub mod query_work;
#[cfg(test)]
mod turn_write_work_tests;

#[cfg(test)]
fn take_optional_layout_work() -> usize {
    OPTIONAL_LAYOUT_WORK.with(|work| work.replace(0))
}
