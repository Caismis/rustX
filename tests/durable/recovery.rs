//! Issue #12 (M9a) — durable startup recovery and recovery classification.
//!
//! Every regression here builds an **exact committed prefix** in a file-backed
//! `SQLite` conversation, drops the store (the "process died here" boundary),
//! reopens the same database, and runs the real recovery pipeline over it.
//! There is no sleep, no timer, and no timing assumption anywhere: the crash
//! boundary is a `drop` and the reopen is a `SqliteConversationStore::open`.
//!
//! ```text
//! open store -> commit the exact prefix -> drop -> reopen -> recover()
//!            -> assert classification / reconciliation / idempotence
//! ```
//!
//! What is asserted is always the same shape:
//!
//! ```text
//! durable evidence -> deterministic classification -> bounded reconciliation
//! ```
//!
//! and, above all, that **outcome unknown is never converted into retry,
//! success, or ordinary failure**.
//!
//! Runtime-level restart regressions that need a driven model turn (headless
//! auto-admission after restart, the Class B continuation, the attempt-id
//! allocator across a restart) live beside the coordinator in
//! `src/runtime/conversation_runtime.rs`, and the real-provider proof that a
//! recovered runtime resends nothing lives in `tests/conformance/agent_loop.rs`.

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use rustx::context::ContextGeneration;
use rustx::conversation::message_id_of;
use rustx::durable::{ConversationStore, InboundDraft, SqliteConversationStore};
use rustx::events::types::{
    AttemptFailure, BackgroundTerminalState, EVENT_SCHEMA_VERSION, RuntimeEvent,
    RuntimeEventEnvelope, SubagentTerminalState,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, ToolMessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::catalog::{ModelCapabilities, ModelCompat};
use rustx::model::{
    ModelFinishReason, ModelInvocationConfig, ModelProtocol, RequestIdentity, RequestParams,
    RequestSnapshot,
};
use rustx::runtime::identity::{
    AgentId, AttemptId, CapabilityRevision, ConversationId, EventId, MessageId, SubagentId,
    ToolCallId, ToolExecutionId, ToolId, TurnId,
};
use rustx::runtime::recovery::{
    AttemptRecoveryClass, KnownModelOutcome, RecoveryError, RecoveryEvidence, RecoveryPlan,
    RecoveryReport, RequestOutcome, ResumeDisposition, recover,
};
use rustx::runtime::types::{CancellationReason, RuntimeClock, RuntimeError, SystemClock};
use rustx::tools::types::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const CONVERSATION: &str = "conv-m9a";

fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
        .single()
        .expect("valid fixed time")
}

/// A fixed clock: every recovery-generated timestamp is deterministic, so a
/// repeated restart produces byte-identical facts.
#[derive(Debug, Clone, Copy)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        fixed_time()
    }
}

fn conversation_id() -> ConversationId {
    ConversationId::new(CONVERSATION)
}

/// A retained temp directory plus the durable database path inside it.
struct Durable {
    _dir: TempDir,
    path: std::path::PathBuf,
}

impl Durable {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("conversation.sqlite");
        Self { _dir: dir, path }
    }

    /// Opens the same durable conversation again. Between two `open` calls the
    /// previous handle has been dropped, which is the crash boundary.
    fn open(&self) -> SqliteConversationStore {
        SqliteConversationStore::open(conversation_id(), &self.path).expect("open durable store")
    }
}

fn text(text: &str) -> Vec<UserContentBlock> {
    vec![UserContentBlock::Text(TextBlock {
        text: text.to_owned(),
    })]
}

fn human(body: &str) -> InboundDraft {
    InboundDraft {
        message_id: None,
        source: UserSource::Human,
        kind: InboundKind::Message,
        content: text(body),
        timestamp: fixed_time(),
        correlation: None,
    }
}

fn user_block(id: &str, body: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: text(body),
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(fixed_time()),
    })
}

fn assistant_with_calls(id: &str, calls: &[&str]) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new(id),
        content: calls
            .iter()
            .map(|call| {
                AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new(*call),
                    tool_id: ToolId::new("tool-a"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                })
            })
            .collect(),
    })
}

fn success_result(body: &str) -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: vec![rustx::tools::types::ToolResultContent::Text(TextBlock {
            text: body.to_owned(),
        })],
        duration_ms: 7,
        exit_code: Some(0),
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

/// Commits exactly the durable transition `repair_tool_turns` commits: the
/// canonical `ToolResult` sibling plus its `ToolMessageCommitted` fact, with
/// the recovery envelope carrying **no** attempt identity.
///
/// This is the narrow, deterministic hook the second-crash regressions use:
/// it builds the "recovery repair committed, attempt terminal absent" durable
/// prefix without sleeps or timing — the exact prefix that must reclassify
/// truthfully on the next startup.
fn commit_recovery_tool_repair(
    store: &SqliteConversationStore,
    assistant_message_id: &str,
    call_id: &str,
    result: ToolExecutionResult,
) {
    let message_id = MessageId::new(format!("{assistant_message_id}-recovered-tool-{call_id}"));
    let block = MessageBlock::Tool(ToolMessageBlock {
        id: message_id.clone(),
        tool_call_id: ToolCallId::new(call_id),
        tool_id: ToolId::new("tool-a"),
        result,
    });
    let event = envelope(
        &format!("recovery-tool-committed:{message_id}"),
        None,
        None,
        RuntimeEvent::ToolMessageCommitted {
            message_id,
            tool_call_id: ToolCallId::new(call_id),
        },
    );
    store
        .append_canonical_batch_with_events(&[block], &[event])
        .expect("recovery tool repair commits atomically");
}

fn envelope(
    event_id: &str,
    attempt_id: Option<AttemptId>,
    turn_id: Option<TurnId>,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 0,
        conversation_id: conversation_id(),
        attempt_id,
        turn_id,
        timestamp: fixed_time(),
        event,
    }
}

fn invocation(model: &str) -> ModelInvocationConfig {
    ModelInvocationConfig {
        model: model.to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        max_output_tokens: 128,
        request_params: RequestParams::new(),
        capabilities: ModelCapabilities::text_only(true, true),
        compat: ModelCompat::default(),
    }
}

/// Freezes one Request Snapshot exactly as the live request-start path does.
fn snapshot_at(
    store: &SqliteConversationStore,
    attempt_id: &AttemptId,
    turn: &str,
    model: &str,
) -> RequestSnapshot {
    let head = store.load_head().expect("head");
    RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt_id.clone(),
            turn: TurnId::new(turn),
            retry_number: 0,
        },
        head.revision,
        "the frozen effective system prompt".to_owned(),
        Vec::new(),
        rustx::runtime::RuntimeResourceRevision::new(1),
        invocation(model),
        64_000,
        None,
        false,
        Vec::new(),
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    )
}

/// Runs the real recovery pipeline over a freshly reopened durable store.
fn recover_reopened(durable: &Durable) -> RecoveryReport {
    let store = durable.open();
    recover(&store, &FixedClock).expect("recovery succeeds")
}

fn all_events(store: &SqliteConversationStore) -> Vec<RuntimeEvent> {
    let mut events = Vec::new();
    let mut cursor = None;
    loop {
        let page = store.read_events(cursor, 64).expect("events");
        if page.events.is_empty() {
            break;
        }
        events.extend(page.events.into_iter().map(|envelope| envelope.event));
        cursor = page.next_sequence;
        if cursor.is_none() {
            break;
        }
    }
    events
}

fn terminal_count(events: &[RuntimeEvent], attempt_id: &AttemptId) -> usize {
    events
        .iter()
        .filter(|event| match event {
            RuntimeEvent::AttemptCompleted { attempt_id: id, .. }
            | RuntimeEvent::AttemptCancelled { attempt_id: id, .. }
            | RuntimeEvent::AttemptTimedOut { attempt_id: id }
            | RuntimeEvent::AttemptLimitExceeded { attempt_id: id, .. }
            | RuntimeEvent::AttemptFailed { attempt_id: id, .. } => id == attempt_id,
            _ => false,
        })
        .count()
}

// ---------------------------------------------------------------------------
// Test A — accepted Pending Inbound before the crash
// ---------------------------------------------------------------------------

/// Accepted-but-not-yet-adopted inbound survives a crash with its exact
/// durable identity, stays pending, and is classified as ordinary admissible
/// work — never as something recovery has to recreate from producer state.
#[test]
fn accepted_pending_inbound_survives_a_crash_unchanged() {
    let durable = Durable::new();
    let accepted = {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        store
            .accept_inbound(InboundDraft {
                correlation: Some("producer-correlation-1".to_owned()),
                ..human("still pending")
            })
            .expect("accept")
    };
    assert_eq!(accepted.sequence.get(), 1);

    let report = recover_reopened(&durable);
    assert_eq!(report.attempt_class(), &AttemptRecoveryClass::NotStarted);
    assert_eq!(report.resume(), ResumeDisposition::PendingInboundOnly);
    assert_eq!(report.pending_inbound(), 1);
    assert!(
        report.reconciliation().is_empty(),
        "an untouched pending item needs no recovery fact"
    );

    let store = durable.open();
    let pending = store.load_pending().expect("pending");
    assert_eq!(pending.len(), 1, "the item is still pending");
    let item = &pending[0];
    assert_eq!(
        item.sequence, accepted.sequence,
        "InboundSequence preserved"
    );
    assert_eq!(item.message_id, accepted.message_id, "MessageId preserved");
    assert_eq!(
        item.message.source,
        UserSource::Human,
        "provenance preserved"
    );
    assert_eq!(
        item.message.content,
        text("still pending"),
        "content preserved"
    );
    assert_eq!(
        item.message.timestamp,
        Some(fixed_time()),
        "the producer timestamp is preserved, never restamped"
    );
    assert_eq!(
        item.correlation.as_deref(),
        Some("producer-correlation-1"),
        "producer correlation preserved"
    );
    assert!(
        store.load_canonical().expect("canonical").is_empty(),
        "recovery never adopts on its own"
    );
}

/// A lineage seeded from supplied history — a fork, a clone, a tree node, a
/// persona seed — owes no answer for its own bootstrap prefix.
///
/// The prefix ends in an ordinary human message, exactly like an adopted turn
/// this conversation accepted, so no canonical *shape* can tell the two apart.
/// The difference is durable and structural: supplied history enters through
/// `initialize`, which is not an adoption and commits no answer obligation, so
/// a reopened forked lineage starts nothing until its own first inbound
/// arrives.
#[test]
fn a_seeded_lineage_owes_no_answer_for_its_bootstrap_prefix() {
    let durable = Durable::new();
    {
        let store = durable.open();
        // The exact `/fork` seed shape: the canonical prefix up to and
        // including the selected user message.
        store
            .initialize(&[
                user_block("seed-user-1", "the forked question"),
                assistant_with_calls("seed-assistant-1", &[]),
                user_block("seed-user-2", "the trailing forked question"),
            ])
            .expect("bootstrap the forked lineage");
    }

    let report = recover_reopened(&durable);
    assert_eq!(
        report.resume(),
        ResumeDisposition::PendingInboundOnly,
        "supplied history is context, never work this conversation accepted"
    );
    assert!(report.reconciliation().is_empty());

    // The same lineage after it accepts and adopts one message of its own does
    // owe an answer, so the rule is about adoption rather than about seeds.
    {
        let store = durable.open();
        let batch = {
            store
                .accept_inbound(human("my own first turn"))
                .expect("accept");
            store
                .select_pending_batch()
                .expect("select")
                .expect("batch")
        };
        adopt_through(&store, batch.watermark);
    }
    assert_eq!(
        recover_reopened(&durable).resume(),
        ResumeDisposition::ContinueAdoptedTurn,
        "the lineage's own adopted turn is owed an answer"
    );
}

// ---------------------------------------------------------------------------
// Test B — crash after the canonical adoption commit
// ---------------------------------------------------------------------------

/// The exact adoption boundary: the Ledger append, the Surface advance, the
/// checkpoint, and the pending deletion commit together. After a crash the
/// `UserMessage` is canonical exactly once, under the same `MessageId`, and it
/// is never re-adopted. Identity — not content equality — is the idempotency
/// key.
#[test]
fn crash_after_adoption_keeps_the_user_message_canonical_exactly_once() {
    let durable = Durable::new();
    let accepted = {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store.accept_inbound(human("adopt me")).expect("accept");
        let batch = store
            .select_pending_batch()
            .expect("select")
            .expect("batch");
        adopt_through(&store, batch.watermark);
        accepted
    };

    let report = recover_reopened(&durable);
    assert_eq!(
        report.pending_inbound(),
        0,
        "the adopted item is not pending"
    );
    assert!(report.reconciliation().is_empty());

    let store = durable.open();
    assert!(store.load_pending().expect("pending").is_empty());
    let canonical = store.load_canonical().expect("canonical");
    let occurrences = canonical
        .iter()
        .filter(|block| message_id_of(block) == accepted.message_id)
        .count();
    assert_eq!(occurrences, 1, "canonical exactly once, by identity");
    assert_eq!(
        canonical.iter().map(message_id_of).collect::<Vec<_>>(),
        vec![accepted.message_id.clone()],
    );
    // Re-adopting the same watermark finds nothing: the durable transition
    // consumed the pending record in the same transaction.
    assert!(
        adopt_through(&store, accepted.sequence).is_empty(),
        "an adopted item can never re-enter adoption"
    );
}

// ---------------------------------------------------------------------------
// Test C — crash before the request-start commit
// ---------------------------------------------------------------------------

/// An attempt that was admitted and whose inbound was already canonicalized,
/// but which crashed before any external start commit, is Class B.
///
/// Nothing claims a request started, no `ModelRequestStarted` is fabricated,
/// the canonical turn is not lost, and continuation is explicitly permitted
/// because no external side effect is outstanding.
#[test]
fn crash_before_request_start_is_class_b_and_permits_continuation() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store.accept_inbound(human("answer me")).expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        // CRASH: no ModelRequestStarted, no ToolExecutionStarted.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt.clone(),
        },
        "no external side effect crossed a start commit"
    );
    assert_eq!(plan.resume(), ResumeDisposition::ContinueAdoptedTurn);
    // Classification is pure: the same durable facts classify identically.
    assert_eq!(RecoveryPlan::classify(&evidence), plan);

    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt),
        "the dead attempt receives an explicit interrupted recovery terminal"
    );
    let events = all_events(&store);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. })),
        "restart never claims a request started"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. })),
        "no fake provider failure is invented for lifecycle symmetry"
    );
    assert_eq!(terminal_count(&events, &attempt), 1);
    // The already-canonical user turn is intact and was not re-adopted.
    assert_eq!(store.load_canonical().expect("canonical").len(), 1);
    assert!(store.load_pending().expect("pending").is_empty());
}

/// The answer obligation survives the recovery terminal that recovery itself
/// writes, and keeps surviving an unbounded chain of deaths in the
/// adoption/request-start window.
///
/// This is the durability half of the Class-B permission. Reconciliation
/// terminalizes the interrupted attempt with `RestartInterrupted` **before**
/// the continuation it just permitted can reach a `ModelRequestStarted`. If
/// that terminal consumed the obligation like a decided one, the very next
/// process death would reopen a conversation whose journal holds an adopted
/// canonical turn, a terminal attempt, and no obligation — and the accepted
/// message would be stranded forever with nothing pending to re-admit it.
///
/// A `RestartInterrupted` terminal is a statement about a dead *attempt*, not
/// a decision about the *turn*, so the obligation transfers to whichever
/// attempt continues the turn next.
#[test]
fn the_answer_obligation_survives_a_chain_of_recovery_terminals() {
    let durable = Durable::new();
    let attempt_one = AttemptId::for_conversation(&conversation_id(), 0);
    let attempt_two = AttemptId::for_conversation(&conversation_id(), 1);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store.accept_inbound(human("answer me")).expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-started-1",
                Some(attempt_one.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_one.clone(),
                },
            ))
            .expect("attempt started");
        // CRASH #1, inside the attempt-start window.
    }

    // Recovery #1 permits the continuation *and* durably terminalizes the
    // dead attempt in the same pass.
    let first = recover_reopened(&durable);
    assert_eq!(first.resume(), ResumeDisposition::ContinueAdoptedTurn);
    assert_eq!(
        first.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt_one)
    );

    // Reopening that durable authority with no further work must reach the
    // same verdict: recovery never destroys its own permission.
    let repeated = recover_reopened(&durable);
    assert_eq!(
        repeated.attempt_class(),
        &AttemptRecoveryClass::AlreadyTerminal,
        "the interrupted attempt is absorbing"
    );
    assert_eq!(
        repeated.resume(),
        ResumeDisposition::ContinueAdoptedTurn,
        "the recovery terminal transferred the obligation instead of consuming it"
    );
    assert!(
        repeated.reconciliation().is_empty(),
        "a repeated recovery commits no second terminal"
    );

    {
        // The permitted continuation starts its new attempt and dies again in
        // exactly the same window.
        let store = durable.open();
        store
            .append_event(envelope(
                "attempt-started-2",
                Some(attempt_two.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_two.clone(),
                },
            ))
            .expect("attempt started");
        // CRASH #2, before any request start.
    }

    let second = recover_reopened(&durable);
    assert_eq!(
        second.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt_two.clone(),
        }
    );
    assert_eq!(
        second.resume(),
        ResumeDisposition::ContinueAdoptedTurn,
        "the turn is still owed an answer after a second death"
    );
    assert_eq!(
        second.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt_two)
    );

    // The turn was never re-adopted, never duplicated, and never answered.
    let store = durable.open();
    assert_eq!(store.load_canonical().expect("canonical").len(), 1);
    assert!(store.load_pending().expect("pending").is_empty());
    let events = all_events(&store);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. })),
        "no chain of restarts ever sends anything"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::InboundTurnAdopted { .. }))
            .count(),
        1,
        "one adoption, one obligation, however many restarts"
    );
}

/// A *decided* terminal still consumes the obligation, so the survival rule
/// above is about `RestartInterrupted` alone and not about attempt terminals
/// in general.
///
/// A cancelled turn is the sharpest case: it is adopted, canonical, and
/// permanently unanswered, and a reopened runtime must still start nothing
/// for it.
#[test]
fn a_decided_terminal_still_consumes_the_answer_obligation() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store.accept_inbound(human("answer me")).expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        store
            .append_event(envelope(
                "attempt-cancelled",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptCancelled {
                    attempt_id: attempt.clone(),
                    reason: CancellationReason::UserRequested,
                },
            ))
            .expect("attempt cancelled");
    }

    let report = recover_reopened(&durable);
    assert_eq!(
        report.attempt_class(),
        &AttemptRecoveryClass::AlreadyTerminal
    );
    assert_eq!(
        report.resume(),
        ResumeDisposition::PendingInboundOnly,
        "the runtime decided this turn was over; recovery does not reopen it"
    );
    assert!(report.reconciliation().is_empty());
}

/// The same survival rule under Class E: an attempt whose *previous* request
/// outcome is durably known, carrying a turn adopted after it at a safe
/// boundary.
///
/// Recovery terminalizes that attempt honestly — nothing is resent — and the
/// newly adopted turn, which never reached a request start, is still owed an
/// answer after the terminal commits.
#[test]
fn a_known_outcome_terminal_transfers_the_obligation_of_a_later_adopted_turn() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let first = store.accept_inbound(human("first")).expect("accept");
        adopt_through(&store, first.sequence);
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        let snapshot = snapshot_at(&store, &attempt, "0", "model-a");
        let request_id = snapshot.request_id.clone();
        store
            .commit_model_turn_start(&[], &snapshot, fixed_time())
            .expect("request start");
        store
            .append_event(envelope(
                "request-completed",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ModelRequestCompleted {
                    request_id,
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                },
            ))
            .expect("request completed");
        // The live attempt drains a second message at a safe boundary and
        // dies before the request that would carry it.
        let second = store.accept_inbound(human("second")).expect("accept");
        adopt_through(&store, second.sequence);
    }

    let first = recover_reopened(&durable);
    assert!(
        matches!(
            first.attempt_class(),
            AttemptRecoveryClass::ExternalOutcomeKnown { .. }
        ),
        "the interrupted attempt carries a known external outcome: {:?}",
        first.attempt_class()
    );
    assert_eq!(first.resume(), ResumeDisposition::ContinueAdoptedTurn);
    assert_eq!(
        first.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt)
    );

    let repeated = recover_reopened(&durable);
    assert_eq!(
        repeated.resume(),
        ResumeDisposition::ContinueAdoptedTurn,
        "the drained turn is still owed an answer after its attempt's recovery terminal"
    );
    assert!(repeated.reconciliation().is_empty());
}

// ---------------------------------------------------------------------------
// Test D — request start committed, provider outcome unknown
// ---------------------------------------------------------------------------

/// The critical class. `RequestSnapshot` + `ModelRequestStarted` exist, no
/// request outcome and no attempt terminal do.
///
/// Recovery reconstructs the exact historical provider-neutral request (for
/// diagnosis and audit), classifies the outcome as indeterminate, settles the
/// attempt, and resends nothing. Reconstructability is explicitly **not**
/// replay permission.
#[test]
fn started_request_with_unknown_outcome_is_indeterminate_and_never_resent() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    let request_id;
    let historical;
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store
            .accept_inbound(human("ask the model"))
            .expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        let snapshot = snapshot_at(&store, &attempt, "0", "model-before-restart");
        request_id = snapshot.request_id.clone();
        // The one request-start transaction: snapshot + ModelRequestStarted.
        store
            .commit_model_turn_start(&[], &snapshot, fixed_time())
            .expect("request start");
        historical = store
            .reconstruct_model_request(&request_id)
            .expect("historical request");
        // CRASH: no ModelRequestCompleted, no ModelRequestFailed, no terminal.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id: attempt.clone(),
            model_request: Some(request_id.clone()),
            tool_calls: Vec::new(),
        },
    );
    assert_eq!(
        plan.resume(),
        ResumeDisposition::BlockedIndeterminate,
        "an unknown provider outcome never authorizes a continuation"
    );

    // Exact historical reconstruction, from frozen durable facts alone.
    let reconstructed = store
        .reconstruct_model_request(&request_id)
        .expect("reconstruct after restart");
    assert_eq!(
        reconstructed, historical,
        "the historical provider-neutral request reconstructs exactly"
    );
    assert_eq!(reconstructed.invocation.model, "model-before-restart");

    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt)
    );

    let events = all_events(&store);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1,
        "recovery starts no second request"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestCompleted { .. })),
        "recovery never invents a provider success"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. })),
        "recovery never invents a provider failure"
    );
    // The attempt settles; the request outcome stays unknown. Those are
    // different facts and recovery keeps them apart.
    let terminal = events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::AttemptFailed { attempt_id, error } if attempt_id == &attempt => {
                Some(error.clone())
            }
            _ => None,
        })
        .expect("one interrupted recovery terminal");
    let AttemptFailure::Runtime {
        error: RuntimeError::RestartInterrupted { message },
    } = terminal
    else {
        panic!("the recovery terminal is an explicit restart interruption");
    };
    assert!(
        message.contains("outcome is unknown"),
        "the diagnostic states the ambiguity: {message}"
    );
}

// ---------------------------------------------------------------------------
// Tests E1 / E2 — external outcome durably known, canonical settlement missing
// ---------------------------------------------------------------------------

/// A model request whose provider outcome completed **durably** before the
/// crash — but whose canonical Assistant message never committed — must not
/// become the Class-B "no external start" continuation.
///
/// The provider already executed the request; resending would re-execute a
/// turn whose outcome rustX durably observed. The Assistant message content
/// is never recoverable from `ModelRequestCompleted`, so recovery neither
/// fabricates a body nor resends: it classifies the external work as
/// durably started with a known outcome, settles the attempt honestly, and
/// permits no automatic continuation.
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn completed_model_request_before_assistant_commit_is_not_class_b() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    let request_id;
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store
            .accept_inbound(human("the model answered"))
            .expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        let snapshot = snapshot_at(&store, &attempt, "0", "model-before-restart");
        request_id = snapshot.request_id.clone();
        // The one request-start transaction: snapshot + ModelRequestStarted.
        store
            .commit_model_turn_start(&[], &snapshot, fixed_time())
            .expect("request start");
        // The provider completed the request; rustX observed and committed
        // the outcome. The canonical Assistant message never committed.
        store
            .append_event(envelope(
                "request-completed",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ModelRequestCompleted {
                    request_id: snapshot.request_id.clone(),
                    finish_reason: rustx::model::ModelFinishReason::Stop,
                    usage: None,
                },
            ))
            .expect("request completed");
        // CRASH: no AssistantMessageCommitted, no attempt terminal.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::ExternalOutcomeKnown {
            attempt_id: attempt.clone(),
            model_request: Some(KnownModelOutcome {
                request_id: Some(request_id.clone()),
                outcome: RequestOutcome::Completed,
            }),
            tool_calls: Vec::new(),
        },
        "a durably completed model request is never 'never started'"
    );
    assert_ne!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt.clone(),
        },
        "the completed request must never classify as Class B"
    );
    assert_eq!(
        plan.resume(),
        ResumeDisposition::PendingInboundOnly,
        "no automatic continuation of the answered model turn"
    );

    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt),
        "the dead attempt settles exactly once"
    );
    assert!(
        report.reconciliation().repaired_tool_results.is_empty(),
        "no tool repair applies"
    );

    // Recovery starts no replacement request and fabricates no Assistant
    // body.
    let events = all_events(&store);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1,
        "recovery performs zero automatic resend"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::AssistantMessageCommitted { .. })),
        "a known request outcome never fabricates an Assistant body"
    );
    let canonical = store.load_canonical().expect("canonical");
    assert_eq!(canonical.len(), 1, "only the User turn is canonical");
    let terminal = events.iter().find_map(|event| match event {
        RuntimeEvent::AttemptFailed { attempt_id, error } if attempt_id == &attempt => {
            Some(error.clone())
        }
        _ => None,
    });
    let AttemptFailure::Runtime {
        error: RuntimeError::RestartInterrupted { message },
    } = terminal.expect("one interrupted recovery terminal")
    else {
        panic!("the recovery terminal is an explicit restart interruption");
    };
    assert!(
        message.contains("completed durably") && message.contains("no response body is fabricated"),
        "the diagnostic records the durable completion and the non-fabrication: {message}"
    );
    assert_eq!(terminal_count(&events, &attempt), 1, "terminal is unique");
}

/// A model request whose provider outcome **failed** durably before the
/// crash must not become a safe Class-B continuation either: converting it
/// into "no external start" would create an implicit generic retry policy,
/// which M9a explicitly does not have.
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn failed_model_request_before_terminal_is_not_retried() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    let request_id;
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store.accept_inbound(human("ask")).expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        let snapshot = snapshot_at(&store, &attempt, "0", "model-before-restart");
        request_id = snapshot.request_id.clone();
        store
            .commit_model_turn_start(&[], &snapshot, fixed_time())
            .expect("request start");
        // The provider failed durably; the crash happened before the attempt
        // could settle.
        store
            .append_event(envelope(
                "request-failed",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ModelRequestFailed {
                    request_id: snapshot.request_id.clone(),
                    error: rustx::model::error::ModelError {
                        kind: rustx::model::error::ModelErrorKind::ProviderError,
                        message: "durable provider failure".to_owned(),
                        retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
                        retry_after_ms: None,
                        provider_code: None,
                        context_overflow: None,
                        malformed_tool_proposal: None,
                    },
                    usage: None,
                },
            ))
            .expect("request failed");
        // CRASH: no attempt terminal.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::ExternalOutcomeKnown {
            attempt_id: attempt.clone(),
            model_request: Some(KnownModelOutcome {
                request_id: Some(request_id),
                outcome: RequestOutcome::Failed,
            }),
            tool_calls: Vec::new(),
        },
        "a durably failed model request is never 'never started'"
    );
    assert_ne!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt.clone(),
        },
        "the durable failure must never authorize a Class-B continuation"
    );
    assert_eq!(
        plan.resume(),
        ResumeDisposition::PendingInboundOnly,
        "no implicit generic retry"
    );

    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt)
    );

    // No automatic provider retry: recovery appends no request start, no
    // retry schedule, and no invented success.
    let events = all_events(&store);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1,
        "the failed request is never re-issued"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRetryScheduled { .. })),
        "M9a has no generic retry engine"
    );
    // The historical failure remains durable.
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ModelRequestFailed { .. })),
        "the historical request failure stays a durable fact"
    );
    let terminal = events.iter().find_map(|event| match event {
        RuntimeEvent::AttemptFailed { attempt_id, error } if attempt_id == &attempt => {
            Some(error.clone())
        }
        _ => None,
    });
    let AttemptFailure::Runtime {
        error: RuntimeError::RestartInterrupted { message },
    } = terminal.expect("one interrupted recovery terminal")
    else {
        panic!("the recovery terminal is an explicit restart interruption");
    };
    assert!(
        message.contains("failed durably") && message.contains("was not retried"),
        "the diagnostic preserves the historical failure: {message}"
    );
    assert_eq!(terminal_count(&events, &attempt), 1, "terminal is unique");
}

// ---------------------------------------------------------------------------
// Test F — a durable terminal survives repeated restarts
// ---------------------------------------------------------------------------

/// A durably settled attempt is absorbing: no restart creates a second,
/// conflicting terminal, and the property is owned by the durable lifecycle,
/// not by an in-memory flag.
#[test]
fn a_durable_attempt_terminal_is_never_created_twice() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        store
            .append_event(envelope(
                "attempt-completed",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptCompleted {
                    attempt_id: attempt.clone(),
                    finish_reason: rustx::model::ModelFinishReason::Stop,
                },
            ))
            .expect("attempt completed");
    }

    for restart in 0..3 {
        let report = recover_reopened(&durable);
        assert_eq!(
            report.attempt_class(),
            &AttemptRecoveryClass::AlreadyTerminal,
            "restart #{restart} still observes the absorbing terminal"
        );
        assert!(
            report.reconciliation().is_empty(),
            "restart #{restart} committed no new fact"
        );
        let store = durable.open();
        assert_eq!(
            terminal_count(&all_events(&store), &attempt),
            1,
            "still exactly one terminal after restart #{restart}"
        );
    }

    // The durable lifecycle — not a process-local flag — is what refuses a
    // second terminal, so even a direct append is rejected.
    let store = durable.open();
    let refused = store.append_event(envelope(
        "second-terminal",
        Some(attempt.clone()),
        None,
        RuntimeEvent::AttemptTimedOut {
            attempt_id: attempt.clone(),
        },
    ));
    assert!(
        matches!(
            refused,
            Err(rustx::durable::ConversationStoreError::TerminalViolation(_))
        ),
        "a conflicting second terminal is a typed durable violation: {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Test G — foreground tool started, outcome unknown
// ---------------------------------------------------------------------------

/// A foreground tool whose external start committed and whose outcome is
/// unknown becomes a typed `OutcomeUnknown` canonical result — never a silent
/// re-execution, never an invented success, never an ordinary `Failed`.
///
/// The structurally incomplete tool turn is completed, so the conversation is
/// not left permanently unable to form a valid later model request.
#[test]
fn started_tool_with_unknown_outcome_becomes_outcome_unknown_and_is_never_replayed() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "run the tool")])
            .expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        let assistant = assistant_with_calls("assistant-1", &["call-1"]);
        store
            .append_canonical_with_event(
                &assistant,
                envelope(
                    "assistant-committed",
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .expect("assistant tool-call turn");
        store
            .append_event(envelope(
                "tool-started",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-a"),
                },
            ))
            .expect("tool started");
        // CRASH: the external tool ran (or did not) and rustX cannot know.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id: attempt.clone(),
            model_request: None,
            tool_calls: vec![ToolCallId::new("call-1")],
        },
    );
    assert_eq!(plan.resume(), ResumeDisposition::BlockedIndeterminate);
    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert_eq!(
        report.reconciliation().repaired_tool_results,
        vec![ToolCallId::new("call-1")]
    );

    let canonical = store.load_canonical().expect("canonical");
    let repaired = canonical
        .iter()
        .find_map(|block| match block {
            MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-1") => {
                Some(tool.clone())
            }
            _ => None,
        })
        .expect("the missing sibling is canonical after recovery");
    assert_eq!(
        repaired.result.status,
        ToolExecutionStatus::OutcomeUnknown {
            detail: "execution started, then the runtime restarted before a durable outcome was committed".to_owned(),
        },
        "unknown is preserved as unknown"
    );
    assert!(
        repaired.result.content.is_empty(),
        "recovery never invents output that was never durably known"
    );
    // Issue #202: the typed status carries the semantics and the content
    // stays empty, but the crash-recovered unknown outcome must still reach
    // the next model-facing turn as text — never as an empty tool result.
    let projection = repaired.result.model_facing_projection();
    assert!(
        !projection.is_empty(),
        "the repaired unknown outcome is visible to the model"
    );
    assert!(
        projection.byte_len() <= rustx::tools::limits::MAX_MODEL_TOOL_RESULT_BYTES,
        "the model-facing repair stays within the tool-result byte budget"
    );
    let projection_text = projection.as_text();
    assert!(
        projection_text.contains("could not establish its final external outcome"),
        "the projection communicates that the final external outcome is unknown"
    );
    assert!(
        projection_text.contains("may have partially or fully completed"),
        "the projection warns that side effects may have occurred"
    );
    // The turn is structurally complete again: every issued call owns exactly
    // one committed result.
    let active_ids = store.load_head().expect("head").active_message_ids;
    let active = store.load_messages(&active_ids).expect("active");
    rustx::conversation::recovery_safety(&active)
        .expect("the reconciled Surface is at a safe boundary");
    // Exactly one `ToolExecutionStarted` fact exists: nothing was re-executed.
    assert_eq!(
        all_events(&store)
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        1,
        "no tool is silently replayed"
    );
}

/// The mixed plane: the request outcome is durably **known**, but a started
/// tool execution's outcome is unknown. The unknown tool is the blocking
/// fact: the attempt must stay indeterminate (never "no external start",
/// never an automatic continuation) even though the request itself is not
/// ambiguous.
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn known_request_with_unknown_tool_outcome_stays_indeterminate() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "run the tool")])
            .expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        let snapshot = snapshot_at(&store, &attempt, "0", "model-before-restart");
        store
            .commit_model_turn_start(&[], &snapshot, fixed_time())
            .expect("request start");
        store
            .append_event(envelope(
                "request-completed",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ModelRequestCompleted {
                    request_id: snapshot.request_id.clone(),
                    finish_reason: rustx::model::ModelFinishReason::ToolCalls,
                    usage: None,
                },
            ))
            .expect("request completed");
        store
            .append_canonical_with_event(
                &assistant_with_calls("assistant-1", &["call-1"]),
                envelope(
                    "assistant-committed",
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .expect("assistant tool-call turn");
        store
            .append_event(envelope(
                "tool-started",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-a"),
                },
            ))
            .expect("tool started");
        // CRASH: the tool's external outcome is unknown.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id: attempt.clone(),
            model_request: None,
            tool_calls: vec![ToolCallId::new("call-1")],
        },
        "the unknown tool outcome dominates the known request outcome"
    );
    assert_eq!(
        plan.resume(),
        ResumeDisposition::BlockedIndeterminate,
        "no automatic continuation while a tool outcome is unknown"
    );
    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert_eq!(
        report.reconciliation().repaired_tool_results,
        vec![ToolCallId::new("call-1")],
        "the unknown call is repaired as outcome-unknown"
    );
    let canonical = store.load_canonical().expect("canonical");
    let repaired = canonical
        .iter()
        .find_map(|block| match block {
            MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-1") => {
                Some(tool.result.clone())
            }
            _ => None,
        })
        .expect("the repair result");
    assert!(matches!(
        repaired.status,
        ToolExecutionStatus::OutcomeUnknown { .. }
    ));
}

// ---------------------------------------------------------------------------
// Test H — mixed sibling tool recovery
// ---------------------------------------------------------------------------

/// Some siblings have a durably known outcome, one is indeterminate, and one
/// provably never started. The recovery batch is built from that evidence
/// alone, in canonical model-call order, in one atomic transaction — no
/// completion-race ordering and no invented result body.
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn mixed_sibling_batch_is_recovered_only_from_durable_evidence() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "run three")])
            .expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        store
            .append_canonical_with_event(
                &assistant_with_calls("assistant-1", &["call-a", "call-b", "call-c"]),
                envelope(
                    "assistant-committed",
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .expect("assistant tool-call turn");
        for call in ["call-a", "call-b"] {
            store
                .append_event(envelope(
                    &format!("tool-started-{call}"),
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::ToolExecutionStarted {
                        tool_call_id: ToolCallId::new(call),
                        tool_id: ToolId::new("tool-a"),
                    },
                ))
                .expect("tool started");
        }
        // `call-b` finished durably *before* the crash — physically second,
        // canonically third — while `call-a` stayed unknown and `call-c`
        // never started at all.
        store
            .append_event(envelope(
                "tool-completed-b",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: ToolCallId::new("call-b"),
                    tool_id: ToolId::new("tool-a"),
                    result: success_result("durably known output"),
                },
            ))
            .expect("tool completed");
        // CRASH before the canonical sibling batch was committed.
    }

    let store = durable.open();
    let report = recover(&store, &FixedClock).expect("recovery");
    assert_eq!(
        report.reconciliation().repaired_tool_results,
        vec![
            ToolCallId::new("call-a"),
            ToolCallId::new("call-b"),
            ToolCallId::new("call-c"),
        ],
        "the batch is built in canonical model-call order, not completion order"
    );

    let canonical = store.load_canonical().expect("canonical");
    let results: Vec<(ToolCallId, ToolExecutionStatus)> = canonical
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Tool(tool) => {
                Some((tool.tool_call_id.clone(), tool.result.status.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        vec![
            (
                ToolCallId::new("call-a"),
                ToolExecutionStatus::OutcomeUnknown {
                    detail: "execution started, then the runtime restarted before a durable outcome was committed".to_owned(),
                }
            ),
            (ToolCallId::new("call-b"), ToolExecutionStatus::Success),
            (
                ToolCallId::new("call-c"),
                ToolExecutionStatus::Cancelled {
                    reason: CancellationReason::ParentCancelled,
                    phase: rustx::tools::types::ToolCancellationPhase::BeforeStart,
                }
            ),
        ],
        "started/unknown, durably known, and provably never started stay distinct"
    );
    let known = canonical
        .iter()
        .find_map(|block| match block {
            MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-b") => {
                Some(tool.result.clone())
            }
            _ => None,
        })
        .expect("call-b");
    assert_eq!(
        known,
        success_result("durably known output"),
        "the durable result is used verbatim, never re-derived"
    );
    let active_ids = store.load_head().expect("head").active_message_ids;
    let active = store.load_messages(&active_ids).expect("active");
    rustx::conversation::recovery_safety(&active).expect("safe boundary");
}

/// The sibling batch is atomic: a durable failure of the repair commits no
/// member at all, so a crash can never expose a durable prefix of the batch.
#[test]
fn a_failed_sibling_repair_commits_no_prefix() {
    let durable = Durable::new();
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "run two")])
            .expect("bootstrap");
        store
            .append_canonical(&assistant_with_calls("assistant-1", &["call-a", "call-b"]))
            .expect("assistant tool-call turn");
    }
    let store = durable.open();
    // A conflicting Ledger identity for the *second* member alone. The first
    // member is perfectly committable, so a non-atomic implementation would
    // leave it behind. The colliding message answers no tool call, so both
    // siblings genuinely remain missing.
    store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("assistant-1-recovered-tool-call-b"),
            content: vec![AssistantContentBlock::Text(TextBlock {
                text: "collides with the second recovery identity".to_owned(),
            })],
        }))
        .expect("seed the colliding identity");

    let failed = recover(&store, &FixedClock);
    assert!(failed.is_err(), "the repair fails closed: {failed:?}");
    let canonical = store.load_canonical().expect("canonical");
    assert!(
        !canonical
            .iter()
            .any(|block| message_id_of(block).as_str() == "assistant-1-recovered-tool-call-a"),
        "no member of the failed batch became canonical"
    );
}

// ---------------------------------------------------------------------------
// Tests E3 — known foreground tool result before the canonical ToolMessage
// ---------------------------------------------------------------------------

/// A foreground tool whose **exact** durable result committed before the
/// crash — but whose canonical `ToolMessage` batch did not — is recovered
/// from that exact result, never re-executed, and never described as "no
/// external start".
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn known_foreground_tool_result_is_recovered_verbatim_and_never_replayed() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    let exact = success_result("the exact durable output");
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "run the tool")])
            .expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        store
            .append_canonical_with_event(
                &assistant_with_calls("assistant-1", &["call-1"]),
                envelope(
                    "assistant-committed",
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .expect("assistant tool-call turn");
        store
            .append_event(envelope(
                "tool-started",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-a"),
                },
            ))
            .expect("tool started");
        store
            .append_event(envelope(
                "tool-completed",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-a"),
                    result: exact.clone(),
                },
            ))
            .expect("tool completed");
        // CRASH: the canonical ToolMessage batch never committed.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::ExternalOutcomeKnown {
            attempt_id: attempt.clone(),
            model_request: None,
            tool_calls: vec![ToolCallId::new("call-1")],
        },
        "a started tool with a known outcome is external-outcome-known, never 'no external start'"
    );
    assert_ne!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt.clone(),
        },
        "the known tool outcome must never claim no external work started"
    );
    assert_eq!(plan.resume(), ResumeDisposition::PendingInboundOnly);

    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert_eq!(
        report.reconciliation().repaired_tool_results,
        vec![ToolCallId::new("call-1")]
    );
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt)
    );

    // The exact durable result — body, status, duration, artifacts — becomes
    // the canonical ToolResult, verbatim.
    let canonical = store.load_canonical().expect("canonical");
    let repaired = canonical
        .iter()
        .find_map(|block| match block {
            MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-1") => {
                Some(tool.result.clone())
            }
            _ => None,
        })
        .expect("the missing sibling is canonical after recovery");
    assert_eq!(repaired, exact, "the durable result is used verbatim");
    assert_eq!(repaired.duration_ms, 7, "the durable duration is preserved");
    assert!(
        !repaired.content.is_empty(),
        "the durable result body is preserved"
    );
    // The tool is never re-executed.
    assert_eq!(
        all_events(&store)
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        1,
        "no tool is silently replayed"
    );
    let active_ids = store.load_head().expect("head").active_message_ids;
    let active = store.load_messages(&active_ids).expect("active");
    rustx::conversation::recovery_safety(&active).expect("safe boundary");
}

// ---------------------------------------------------------------------------
// Tests E4 / E5 — the second crash after a recovery repair
// ---------------------------------------------------------------------------

/// The current PR review finding, deterministically: a crash **after** the
/// recovery tool-turn repair committed but **before** the attempt terminal
/// commits must not erase the historical `ToolExecutionStarted`.
///
/// ```text
/// prefix 1: AttemptStarted, Assistant ToolCall canonical, ToolExecutionStarted, CRASH #1
/// repair committed: ToolMessage(OutcomeUnknown) + ToolMessageCommitted   (no attempt terminal)
/// CRASH #2
/// ```
///
/// On restart the attempt is still non-terminal, but it must still be known
/// to have crossed a tool external-start commit — never reclassified as the
/// safe Class-B continuation.
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn second_crash_after_tool_repair_preserves_external_start_evidence() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "run the tool")])
            .expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        store
            .append_canonical_with_event(
                &assistant_with_calls("assistant-1", &["call-1"]),
                envelope(
                    "assistant-committed",
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .expect("assistant tool-call turn");
        store
            .append_event(envelope(
                "tool-started",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-a"),
                },
            ))
            .expect("tool started");
        // CRASH #1: the external tool ran (or did not) and rustX cannot know.
    }

    // Recovery #1 runs only the tool-turn repair transition — the exact
    // durable prefix "repair committed, attempt terminal absent".
    let first = {
        let store = durable.open();
        let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
        let plan = RecoveryPlan::classify(&evidence);
        assert_eq!(
            plan.attempt_class(),
            &AttemptRecoveryClass::IndeterminateExternalOutcome {
                attempt_id: attempt.clone(),
                model_request: None,
                tool_calls: vec![ToolCallId::new("call-1")],
            },
            "the first recovery still sees the unknown outcome"
        );
        // Narrow, deterministic hook: commit exactly the repair transaction.
        commit_recovery_tool_repair(
            &store,
            "assistant-1",
            "call-1",
            ToolExecutionResult {
                status: ToolExecutionStatus::OutcomeUnknown {
                    detail: "execution started, then the runtime restarted before a durable outcome was committed".to_owned(),
                },
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        );
        plan
    };
    // CRASH #2: the process dies before the recovery attempt terminal.
    drop(first);

    // The next startup reconstructs and classifies again.
    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id: attempt.clone(),
            model_request: None,
            // The call identity was released with its repair evidence: the
            // committed canonical `OutcomeUnknown` result means the per-call
            // repair entry is gone. The attempt's external summary still
            // proves the execution started and its outcome stayed unknown,
            // which is what keeps the class indeterminate.
            tool_calls: Vec::new(),
        },
        "the committed canonical repair does not erase the historical start"
    );
    assert_ne!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt.clone(),
        },
        "the second crash must never turn the attempt into Class B"
    );
    assert_eq!(
        plan.resume(),
        ResumeDisposition::BlockedIndeterminate,
        "the unknown external outcome still blocks automatic continuation"
    );

    // The second recovery writes at most the missing attempt terminal; it
    // does not create a second ToolResult.
    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert!(
        report.reconciliation().repaired_tool_results.is_empty(),
        "the repair already committed; no second ToolResult is created"
    );
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt),
        "the second recovery writes the missing attempt terminal"
    );
    let canonical = store.load_canonical().expect("canonical");
    assert_eq!(
        canonical
            .iter()
            .filter(|block| matches!(block, MessageBlock::Tool(_)))
            .count(),
        1,
        "exactly one canonical ToolResult exists"
    );
    let repaired = canonical
        .iter()
        .find_map(|block| match block {
            MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-1") => {
                Some(tool.result.clone())
            }
            _ => None,
        })
        .expect("the repair result");
    assert_eq!(
        repaired.status,
        ToolExecutionStatus::OutcomeUnknown {
            detail: "execution started, then the runtime restarted before a durable outcome was committed".to_owned(),
        },
        "the unknown external outcome stays represented as outcome-unknown"
    );

    // Repeated restart after the terminal is fully idempotent.
    for restart in 0..2 {
        let report = recover_reopened(&durable);
        assert!(
            report.reconciliation().is_empty(),
            "restart #{restart} committed a new fact"
        );
        assert_eq!(
            report.attempt_class(),
            &AttemptRecoveryClass::AlreadyTerminal
        );
        let store = durable.open();
        assert_eq!(terminal_count(&all_events(&store), &attempt), 1);
    }
}

/// The known-outcome twin of the second-crash regression: the recovery
/// repair committed the **exact** durable tool result, crashed before the
/// attempt terminal, and the next startup must still know that external
/// work crossed a start commit.
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn second_crash_after_known_outcome_repair_preserves_external_start() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    let exact = success_result("the exact durable output");
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "run the tool")])
            .expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        store
            .append_canonical_with_event(
                &assistant_with_calls("assistant-1", &["call-1"]),
                envelope(
                    "assistant-committed",
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .expect("assistant tool-call turn");
        for (index, event) in [
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-a"),
            },
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-a"),
                result: exact.clone(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            store
                .append_event(envelope(
                    &format!("tool-fact-{index}"),
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    event,
                ))
                .expect("tool fact");
        }
        // Recovery #1 repair transition: the exact result becomes canonical.
        commit_recovery_tool_repair(&store, "assistant-1", "call-1", exact.clone());
        // CRASH #2: before the recovery attempt terminal.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::ExternalOutcomeKnown {
            attempt_id: attempt.clone(),
            model_request: None,
            // The exact result was already repaired into the canonical
            // Surface; its per-call repair evidence was released, so no call
            // identity remains namable. The attempt summary still proves
            // external tool work happened with a known outcome — never "no
            // external start".
            tool_calls: Vec::new(),
        },
        "the attempt still had external execution; the outcome is durably known"
    );
    assert_ne!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt.clone(),
        },
        "a committed repair result never means 'no external side effect crossed a start commit'"
    );

    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert!(
        report.reconciliation().repaired_tool_results.is_empty(),
        "no replay and no second ToolResult"
    );
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt)
    );
    let canonical = store.load_canonical().expect("canonical");
    let repaired = canonical
        .iter()
        .find_map(|block| match block {
            MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-1") => {
                Some(tool.result.clone())
            }
            _ => None,
        })
        .expect("the repair result");
    assert_eq!(repaired, exact, "the exact durable result is preserved");
    assert_eq!(
        all_events(&store)
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
            .count(),
        1,
        "no tool is re-executed across the second crash"
    );
}

// ---------------------------------------------------------------------------
// Finding-15 regression — historical attempt evidence never aliases the
// current unresolved call
// ---------------------------------------------------------------------------

/// The recovery fold keys tool evidence by owning attempt **and** call id
/// because the durable authority does not guarantee `ToolCallId` uniqueness
/// across the conversation lifetime (providers mint call ids; only the
/// active Surface rejects duplicates).
///
/// Here a **terminal** historical attempt still carries a started-unknown
/// tool call whose owning Assistant turn is active (the Class-D repair
/// shape). A new attempt then crashes with zero external starts. The
/// historical ambiguity belongs to the settled attempt's own terminal, so
/// the new attempt must classify as Class B — the old bare-call-id fold
/// would have made it indeterminate from the historical entry.
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn historical_attempt_tool_evidence_never_aliases_the_unsettled_attempt() {
    let durable = Durable::new();
    let conversation = conversation_id();
    let first = AttemptId::for_conversation(&conversation, 0);
    let current = AttemptId::for_conversation(&conversation, 1);
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "turn one")])
            .expect("bootstrap");
        // Attempt 1: a tool call started, outcome unknown, and the attempt
        // settled anyway — its owning Assistant turn stays active, so its
        // missing result still needs the Class-D repair.
        store
            .append_event(envelope(
                "attempt-1-started",
                Some(first.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: first.clone(),
                },
            ))
            .expect("attempt 1 started");
        store
            .append_canonical_with_event(
                &assistant_with_calls("assistant-1", &["call-1"]),
                envelope(
                    "assistant-1-committed",
                    Some(first.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .expect("assistant 1 tool-call turn");
        store
            .append_event(envelope(
                "tool-started-1",
                Some(first.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-a"),
                },
            ))
            .expect("tool 1 started");
        store
            .append_event(envelope(
                "attempt-1-failed",
                Some(first.clone()),
                None,
                RuntimeEvent::AttemptFailed {
                    attempt_id: first.clone(),
                    error: AttemptFailure::Runtime {
                        error: RuntimeError::Internal {
                            message: "the live batch commit failed".to_owned(),
                        },
                    },
                },
            ))
            .expect("attempt 1 terminal");
        // Attempt 2: the provider reuses the same call id in its next
        // response, but the process crashes before anything starts.
        let accepted = store.accept_inbound(human("turn two")).expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-2-started",
                Some(current.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: current.clone(),
                },
            ))
            .expect("attempt 2 started");
        // CRASH: no ModelRequestStarted, no ToolExecutionStarted.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: current.clone(),
        },
        "the historical attempt's unresolved tool never aliases the current attempt"
    );
    assert_eq!(
        plan.resume(),
        ResumeDisposition::ContinueAdoptedTurn,
        "the current attempt has zero external-start evidence of its own"
    );

    // The Class-D repair still uses the **historical** attempt's own durable
    // evidence — attributed exactly, not by bare call id.
    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert_eq!(
        report.reconciliation().repaired_tool_results,
        vec![ToolCallId::new("call-1")],
        "the settled attempt's incomplete turn is still repaired"
    );
    let canonical = store.load_canonical().expect("canonical");
    let repaired = canonical
        .iter()
        .find_map(|block| match block {
            MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-1") => {
                Some(tool.result.clone())
            }
            _ => None,
        })
        .expect("the Class-D repair result");
    assert_eq!(
        repaired.status,
        ToolExecutionStatus::OutcomeUnknown {
            detail: "execution started, then the runtime restarted before a durable outcome was committed".to_owned(),
        },
        "the historical unknown outcome stays outcome-unknown, never invented"
    );
    assert_eq!(
        terminal_count(&all_events(&store), &current),
        1,
        "the current attempt settles exactly once"
    );
}

// ---------------------------------------------------------------------------
// Tests I / J — background recovery and the publication boundary
// ---------------------------------------------------------------------------

fn commit_background_ownership(store: &SqliteConversationStore, execution: &ToolExecutionId) {
    store
        .append_event(envelope(
            &format!("background-committed-event:{execution}"),
            None,
            None,
            RuntimeEvent::BackgroundExecutionCommitted {
                execution_id: execution.clone(),
                tool_call_id: ToolCallId::new("call-bg"),
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bash".to_owned(),
            },
        ))
        .expect("background ownership");
}

/// A background execution that was durably owned and never settled does not
/// survive the process. Recovery neither assumes it is alive nor relaunches
/// it: it commits the strongest honest terminal — `OutcomeUnknown`, not
/// `Failed` — together with exactly one model-visible terminal notification.
#[test]
fn nonterminal_background_work_is_terminalized_exactly_once_and_never_relaunched() {
    let durable = Durable::new();
    let execution = ToolExecutionId::background(1);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        commit_background_ownership(&store, &execution);
        // CRASH while the detached task was Starting/Running/Cancelling.
    }

    let report = recover_reopened(&durable);
    assert_eq!(
        report.background_classes().len(),
        1,
        "the durably owned execution is classified"
    );
    assert_eq!(
        report.background_classes()[0].evidence.execution_id,
        execution
    );
    assert_eq!(
        report.reconciliation().background_terminals,
        vec![execution.clone()]
    );
    assert_eq!(
        report.highest_background_ordinal(),
        1,
        "the allocator watermark is recovered from durable evidence"
    );

    let store = durable.open();
    let published: Vec<BackgroundTerminalState> = all_events(&store)
        .into_iter()
        .filter_map(|event| match event {
            RuntimeEvent::BackgroundTerminalPublished { state, .. } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        published,
        vec![BackgroundTerminalState::OutcomeUnknown],
        "unknown is published as outcome-unknown, never as a known failure"
    );
    let pending = store.load_pending().expect("pending");
    assert_eq!(pending.len(), 1, "exactly one terminal notification");
    assert_eq!(
        pending[0].message_id,
        MessageId::new(format!("background-{execution}-terminal")),
        "the recovery notification reuses the live identity contract"
    );
    assert_eq!(
        pending[0].correlation.as_deref(),
        Some(format!("background-terminal:{execution}").as_str()),
    );

    // Repeated restarts change nothing.
    for restart in 0..2 {
        let report = recover_reopened(&durable);
        assert!(
            report.background_classes().is_empty(),
            "restart #{restart} sees an absorbing background terminal"
        );
        assert!(report.reconciliation().is_empty());
        let store = durable.open();
        assert_eq!(
            store.load_pending().expect("pending").len(),
            1,
            "no duplicate terminal inbound after restart #{restart}"
        );
    }
}

/// The `PublishingTerminal` boundary, both sides of the commit.
///
/// Before the publication transaction commits, nothing model-visible exists
/// and recovery owns the publication. After it commits, the publication is
/// absorbing and recovery publishes nothing. The two states recover
/// differently and the notification is published exactly once either way.
#[test]
fn the_terminal_publication_boundary_recovers_differently_on_each_side() {
    // --- pre-commit: the executor settled, the transaction did not commit ---
    let before = Durable::new();
    let execution = ToolExecutionId::background(1);
    {
        let store = before.open();
        store.initialize(&[]).expect("bootstrap");
        commit_background_ownership(&store, &execution);
    }
    let report = recover_reopened(&before);
    assert_eq!(
        report.reconciliation().background_terminals,
        vec![execution.clone()],
        "recovery owns the unpublished terminal"
    );
    assert_eq!(before.open().load_pending().expect("pending").len(), 1);

    // --- post-commit: the same transaction committed before the crash ---
    let after = Durable::new();
    {
        let store = after.open();
        store.initialize(&[]).expect("bootstrap");
        commit_background_ownership(&store, &execution);
        let notification = UserMessageBlock {
            id: MessageId::new(format!("background-{execution}-terminal")),
            content: text("Background execution settled: succeeded"),
            source: UserSource::Runtime,
            kind: InboundKind::Message,
            timestamp: Some(fixed_time()),
        };
        store
            .accept_inbound_with_event(
                InboundDraft {
                    message_id: Some(notification.id.clone()),
                    source: notification.source,
                    kind: notification.kind,
                    content: notification.content.clone(),
                    timestamp: fixed_time(),
                    correlation: Some(format!("background-terminal:{execution}")),
                },
                envelope(
                    &format!("background-terminal-event:{execution}"),
                    None,
                    None,
                    RuntimeEvent::BackgroundTerminalPublished {
                        execution_id: execution.clone(),
                        message_id: notification.id.clone(),
                        state: BackgroundTerminalState::Succeeded,
                    },
                ),
            )
            .expect("live terminal publication");
    }
    let report = recover_reopened(&after);
    assert!(
        report.background_classes().is_empty(),
        "an already-published terminal is absorbing"
    );
    assert!(report.reconciliation().is_empty());
    let store = after.open();
    assert_eq!(
        store.load_pending().expect("pending").len(),
        1,
        "publication remains exactly once"
    );
    let published: Vec<BackgroundTerminalState> = all_events(&store)
        .into_iter()
        .filter_map(|event| match event {
            RuntimeEvent::BackgroundTerminalPublished { state, .. } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        published,
        vec![BackgroundTerminalState::Succeeded],
        "the durably known outcome is never overwritten by a recovery guess"
    );
}

// ---------------------------------------------------------------------------
// Test K — repeated restart idempotence
// ---------------------------------------------------------------------------

/// After the first successful recovery, durable state stops changing — for the
/// ambiguous-attempt case and the background case alike.
#[test]
fn repeated_restarts_settle_once_and_then_change_nothing() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    let execution = ToolExecutionId::background(1);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store.accept_inbound(human("ask")).expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        let snapshot = snapshot_at(&store, &attempt, "0", "model-x");
        store
            .commit_model_turn_start(&[], &snapshot, fixed_time())
            .expect("request start");
        commit_background_ownership(&store, &execution);
    }

    let first = recover_reopened(&durable);
    assert_eq!(
        first.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt)
    );
    assert_eq!(
        first.reconciliation().background_terminals,
        vec![execution.clone()]
    );
    let settled = {
        let store = durable.open();
        (
            all_events(&store),
            store.load_pending().expect("pending"),
            store.load_canonical().expect("canonical"),
        )
    };

    for restart in 0..3 {
        let report = recover_reopened(&durable);
        assert!(
            report.reconciliation().is_empty(),
            "restart #{restart} committed a new fact"
        );
        assert_eq!(
            report.attempt_class(),
            &AttemptRecoveryClass::AlreadyTerminal
        );
        assert!(report.background_classes().is_empty());
        let store = durable.open();
        assert_eq!(all_events(&store), settled.0, "events unchanged");
        assert_eq!(store.load_pending().expect("pending"), settled.1);
        assert_eq!(store.load_canonical().expect("canonical"), settled.2);
    }
}

// ---------------------------------------------------------------------------
// Test L — identity allocator recovery
// ---------------------------------------------------------------------------

/// A restart never reuses an identity that already entered durable authority.
///
/// The attempt allocator is reseeded above every durable ordinal, and the
/// durable Event Journal independently refuses a second `AttemptStarted` for
/// one identity — so a reset ordinal cannot silently produce two logical
/// attempts under one durable identity.
#[test]
fn recovered_identity_allocators_never_collide_with_durable_history() {
    let durable = Durable::new();
    let conversation = conversation_id();
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        for ordinal in 0..3 {
            let attempt = AttemptId::for_conversation(&conversation, ordinal);
            store
                .append_event(envelope(
                    &format!("attempt-started-{ordinal}"),
                    Some(attempt.clone()),
                    None,
                    RuntimeEvent::AttemptStarted {
                        attempt_id: attempt.clone(),
                    },
                ))
                .expect("attempt started");
            store
                .append_event(envelope(
                    &format!("attempt-completed-{ordinal}"),
                    Some(attempt.clone()),
                    None,
                    RuntimeEvent::AttemptCompleted {
                        attempt_id: attempt,
                        finish_reason: rustx::model::ModelFinishReason::Stop,
                    },
                ))
                .expect("attempt completed");
        }
        for ordinal in 1..=4 {
            commit_background_ownership(&store, &ToolExecutionId::background(ordinal));
        }
    }

    let report = recover_reopened(&durable);
    assert_eq!(
        report.next_attempt_ordinal(),
        3,
        "the next attempt ordinal is past every durable one"
    );
    assert_eq!(report.highest_background_ordinal(), 4);
    let next = AttemptId::for_conversation(&conversation, report.next_attempt_ordinal());
    for ordinal in 0..3 {
        assert_ne!(next, AttemptId::for_conversation(&conversation, ordinal));
    }
    // The identity bijection round-trips, so the fold is a defined mapping and
    // not string guessing.
    assert_eq!(next.conversation_ordinal(&conversation), Some(3));
    assert_eq!(
        AttemptId::new("some-other-attempt-7").conversation_ordinal(&conversation),
        None,
        "an identity outside this conversation's domain never moves the watermark"
    );

    // The durable authority is the second, independent guard.
    let store = durable.open();
    let already_durable = AttemptId::for_conversation(&conversation, 1);
    let refused = store.append_event(envelope(
        "attempt-started-reused",
        Some(already_durable.clone()),
        None,
        RuntimeEvent::AttemptStarted {
            attempt_id: already_durable,
        },
    ));
    assert!(
        matches!(
            refused,
            Err(rustx::durable::ConversationStoreError::TerminalViolation(_))
        ),
        "an attempt identity starts exactly once in durable authority: {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Bounded working set / no second authority
// ---------------------------------------------------------------------------

/// The recovery fold retains only the unresolved working set: a long history
/// of fully settled attempts leaves nothing behind, so complete Event Journal,
/// Request Snapshot, and Ledger history are never materialized as recovery
/// state.
#[test]
fn the_recovery_fold_retains_only_unresolved_work() {
    let durable = Durable::new();
    let conversation = conversation_id();
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        for ordinal in 0..40 {
            let attempt = AttemptId::for_conversation(&conversation, ordinal);
            store
                .append_event(envelope(
                    &format!("started-{ordinal}"),
                    Some(attempt.clone()),
                    None,
                    RuntimeEvent::AttemptStarted {
                        attempt_id: attempt.clone(),
                    },
                ))
                .expect("started");
            let snapshot = snapshot_at(&store, &attempt, "0", "model-x");
            store
                .commit_model_turn_start(&[], &snapshot, fixed_time())
                .expect("request start");
            store
                .append_event(envelope(
                    &format!("request-completed-{ordinal}"),
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::ModelRequestCompleted {
                        request_id: snapshot.request_id.clone(),
                        finish_reason: rustx::model::ModelFinishReason::Stop,
                        usage: None,
                    },
                ))
                .expect("request completed");
            store
                .append_event(envelope(
                    &format!("completed-{ordinal}"),
                    Some(attempt.clone()),
                    None,
                    RuntimeEvent::AttemptCompleted {
                        attempt_id: attempt,
                        finish_reason: rustx::model::ModelFinishReason::Stop,
                    },
                ))
                .expect("completed");
        }
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(plan.attempt_class(), &AttemptRecoveryClass::AlreadyTerminal);
    assert!(plan.background_classes().is_empty());
    assert!(evidence.active_messages().is_empty());
    assert!(evidence.pending_inbound().is_empty());
    assert_eq!(evidence.next_attempt_ordinal(), 40);
    // 160 durable events and 40 request snapshots are behind this fold; the
    // classification carries none of them.
    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert!(report.reconciliation().is_empty());
}

// ---------------------------------------------------------------------------
// Bounded tool-evidence regression — one long non-terminal attempt
// ---------------------------------------------------------------------------

/// The bounded-working-set correction: one non-terminal attempt that fully
/// canonicalized **many** tool calls across multiple logical turns retains
/// exactly one bounded attempt summary and **zero** detailed per-call repair
/// evidence.
///
/// ```text
/// AttemptStarted
/// for call 0..N:
///     AssistantMessageCommitted(call_N)   canonical
///     ToolExecutionStarted(call_N)
///     ToolExecutionCompleted(call_N)      durable exact result
///     ToolMessageCommitted(call_N)        canonical ToolResult
/// // no attempt terminal
/// ```
///
/// After reconstruction the attempt must still know external tool work
/// occurred (never Class B), while the recovery hot working set must not
/// scale with the number of settled calls: every canonicalized call yields
/// zero retained tool repairs. The durable *setup* cost is quadratic here
/// because each canonical append rewrites the growing active Surface head
/// (store design, not recovery); the fold property itself is structural — a
/// `ToolMessageCommitted` releases the entry unconditionally — and the
/// exact "1000 calls => 1 summary => 0 repairs" cardinality is asserted
/// directly on the fold in `src/runtime/recovery.rs`
/// (`a_long_settled_fold_retains_bounded_repair_evidence`), where no store
/// makes the 1000-call prefix free.
#[test]
#[allow(clippy::too_many_lines)] // One loop, one durable prefix, one place.
fn a_long_settled_attempt_retains_bounded_repair_evidence() {
    const CALLS: usize = 200;
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        for call in 0..CALLS {
            let turn = TurnId::new(call.to_string());
            let call_id = ToolCallId::new(format!("call-{call}"));
            let assistant_id = MessageId::new(format!("assistant-{call}"));
            store
                .append_canonical_with_event(
                    &assistant_with_calls(&format!("assistant-{call}"), &[&format!("call-{call}")]),
                    envelope(
                        &format!("assistant-committed-{call}"),
                        Some(attempt.clone()),
                        Some(turn.clone()),
                        RuntimeEvent::AssistantMessageCommitted {
                            message_id: assistant_id,
                        },
                    ),
                )
                .expect("assistant tool-call turn");
            store
                .append_event(envelope(
                    &format!("tool-started-{call}"),
                    Some(attempt.clone()),
                    Some(turn.clone()),
                    RuntimeEvent::ToolExecutionStarted {
                        tool_call_id: call_id.clone(),
                        tool_id: ToolId::new("tool-a"),
                    },
                ))
                .expect("tool started");
            store
                .append_event(envelope(
                    &format!("tool-completed-{call}"),
                    Some(attempt.clone()),
                    Some(turn.clone()),
                    RuntimeEvent::ToolExecutionCompleted {
                        tool_call_id: call_id.clone(),
                        tool_id: ToolId::new("tool-a"),
                        result: success_result(&format!("exact-{call}")),
                    },
                ))
                .expect("tool completed");
            let tool_message_id = MessageId::new(format!("assistant-{call}-tool-{call}"));
            store
                .append_canonical_batch_with_events(
                    &[MessageBlock::Tool(ToolMessageBlock {
                        id: tool_message_id.clone(),
                        tool_call_id: call_id.clone(),
                        tool_id: ToolId::new("tool-a"),
                        result: success_result(&format!("exact-{call}")),
                    })],
                    &[envelope(
                        &format!("tool-committed-{call}"),
                        Some(attempt.clone()),
                        Some(turn.clone()),
                        RuntimeEvent::ToolMessageCommitted {
                            message_id: tool_message_id,
                            tool_call_id: call_id,
                        },
                    )],
                )
                .expect("canonical tool message");
        }
        // CRASH: the attempt never reached its terminal.
    }

    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::ExternalOutcomeKnown {
            attempt_id: attempt.clone(),
            model_request: None,
            // Every call's repair evidence was released with its canonical
            // ToolResult; no call identity remains namable.
            tool_calls: Vec::new(),
        },
        "the non-terminal attempt still knows external tool work happened"
    );
    assert_ne!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt.clone(),
        },
        "many canonicalized tool calls never regress the attempt to 'never externally started'"
    );
    // The end-to-end observable contract of the bounded working set: every
    // call already owns its canonical ToolResult, so reconciliation performs
    // **zero** tool repairs — the recovered runtime never recreates or
    // replays a settled result — and writes only the missing attempt
    // terminal. The exact hot-state cardinality (N settled calls => one
    // bounded summary, zero retained repair details) is asserted directly on
    // the private fold in `src/runtime/recovery.rs`.
    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert!(
        report.reconciliation().repaired_tool_results.is_empty(),
        "no tool repair applies to a fully canonicalized turn"
    );
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt),
        "recovery writes only the missing attempt terminal"
    );
    for restart in 0..2 {
        let report = recover_reopened(&durable);
        assert!(
            report.reconciliation().is_empty(),
            "restart #{restart} committed a new fact"
        );
        assert_eq!(
            report.attempt_class(),
            &AttemptRecoveryClass::AlreadyTerminal
        );
    }
}

/// The mixed unresolved batch keeps **unknown dominance** in the attempt
/// summary.
///
/// ```text
/// AttemptStarted
/// call A: ToolExecutionStarted, ToolExecutionCompleted, ToolMessageCommitted
/// call B: ToolExecutionStarted                         // no outcome
/// ```
///
/// First reconstruction: the class is indeterminate (never "known results
/// elsewhere hide one unknown started side effect"), and the repair map holds
/// exactly the unresolved structurally relevant call B. After B's recovery
/// `OutcomeUnknown` repair and a second crash before the attempt terminal, the
/// repair map is empty — yet the attempt summary still classifies as
/// external-start + unknown until the attempt terminal commits.
#[test]
#[allow(clippy::too_many_lines)] // One crash prefix, one recovery, one place.
fn a_mixed_unresolved_batch_keeps_unknown_dominance_after_repair() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store
            .initialize(&[user_block("msg-u0", "run two")])
            .expect("bootstrap");
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
        store
            .append_canonical_with_event(
                &assistant_with_calls("assistant-1", &["call-a", "call-b"]),
                envelope(
                    "assistant-committed",
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: MessageId::new("assistant-1"),
                    },
                ),
            )
            .expect("assistant tool-call turn");
        // call A: fully settled, canonical ToolResult committed with the
        // live attempt identity.
        let exact_a = success_result("durably known output of A");
        for (index, event) in [
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: ToolCallId::new("call-a"),
                tool_id: ToolId::new("tool-a"),
            },
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: ToolCallId::new("call-a"),
                tool_id: ToolId::new("tool-a"),
                result: exact_a.clone(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            store
                .append_event(envelope(
                    &format!("call-a-fact-{index}"),
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    event,
                ))
                .expect("call A fact");
        }
        let tool_message_a = MessageId::new("assistant-1-tool-call-a");
        store
            .append_canonical_batch_with_events(
                &[MessageBlock::Tool(ToolMessageBlock {
                    id: tool_message_a.clone(),
                    tool_call_id: ToolCallId::new("call-a"),
                    tool_id: ToolId::new("tool-a"),
                    result: exact_a,
                })],
                &[envelope(
                    "call-a-committed",
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::ToolMessageCommitted {
                        message_id: tool_message_a,
                        tool_call_id: ToolCallId::new("call-a"),
                    },
                )],
            )
            .expect("canonical ToolResult of A");
        // call B: started, outcome unknown, no canonical result.
        store
            .append_event(envelope(
                "call-b-started",
                Some(attempt.clone()),
                Some(TurnId::new("0")),
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: ToolCallId::new("call-b"),
                    tool_id: ToolId::new("tool-a"),
                },
            ))
            .expect("call B started");
        // CRASH #1: B's outcome is unknowable.
    }

    // First reconstruction: unknown dominates the mixed batch.
    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id: attempt.clone(),
            model_request: None,
            // The still-repairable unknown call is named: call-a's repair was
            // released with its canonical ToolResult, so it is not named.
            tool_calls: vec![ToolCallId::new("call-b")],
        },
        "one unknown started side effect dominates the known settled sibling"
    );
    assert_eq!(
        plan.resume(),
        ResumeDisposition::BlockedIndeterminate,
        "the unknown outcome blocks automatic continuation"
    );

    // Recovery #1: commit B's repair (OutcomeUnknown) but crash before the
    // attempt terminal — the exact "repair committed, terminal absent"
    // prefix, this time with A already settled.
    commit_recovery_tool_repair(
        &store,
        "assistant-1",
        "call-b",
        ToolExecutionResult {
            status: ToolExecutionStatus::OutcomeUnknown {
                detail: "execution started, then the runtime restarted before a durable outcome was committed".to_owned(),
            },
            content: Vec::new(),
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        },
    );
    drop(plan);

    // CRASH #2, then the next startup: the repair map is empty, yet the
    // attempt summary still proves external start + unknown.
    let store = durable.open();
    let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
    let plan = RecoveryPlan::classify(&evidence);
    assert_eq!(
        plan.attempt_class(),
        &AttemptRecoveryClass::IndeterminateExternalOutcome {
            attempt_id: attempt.clone(),
            model_request: None,
            // B's call identity was released with its repair evidence; the
            // summary still knows the external outcome stayed unknown.
            tool_calls: Vec::new(),
        },
        "the committed OutcomeUnknown repair never resolves the old unknown outcome"
    );
    assert_ne!(
        plan.attempt_class(),
        &AttemptRecoveryClass::AdmittedWithoutExternalStart {
            attempt_id: attempt.clone(),
        },
        "the second crash must never turn the attempt into Class B"
    );
    assert_eq!(
        plan.resume(),
        ResumeDisposition::BlockedIndeterminate,
        "the unknown external outcome still blocks continuation"
    );
    let report = plan.reconcile(&store, &FixedClock).expect("reconcile");
    assert!(
        report.reconciliation().repaired_tool_results.is_empty(),
        "no second repair is created"
    );
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt),
        "recovery writes only the missing attempt terminal"
    );
}

/// Recovery reads only rustX-owned durable authority.
///
/// A fresh store handle constructed with nothing but the database path — no
/// client, no mailbox, no registry, no model catalog, no filesystem state —
/// produces the identical classification, which is what "every recovery
/// decision is explainable from durable facts alone" means operationally.
#[test]
fn recovery_is_a_pure_function_of_durable_authority() {
    let durable = Durable::new();
    let attempt = AttemptId::for_conversation(&conversation_id(), 0);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        let accepted = store.accept_inbound(human("hello")).expect("accept");
        adopt_through(&store, accepted.sequence);
        store
            .append_event(envelope(
                "attempt-started",
                Some(attempt.clone()),
                None,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt.clone(),
                },
            ))
            .expect("attempt started");
    }

    let first = {
        let store = durable.open();
        let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
        RecoveryPlan::classify(&evidence)
    };
    let second = {
        let store = durable.open();
        let evidence = RecoveryEvidence::reconstruct(&store).expect("evidence");
        RecoveryPlan::classify(&evidence)
    };
    assert_eq!(first, second, "classification is deterministic");
    // A different wall clock produces the same classification; only the
    // recovery facts' timestamps differ.
    let system: Arc<dyn RuntimeClock> = Arc::new(SystemClock);
    let store = durable.open();
    let report = second
        .reconcile(&store, system.as_ref())
        .expect("reconcile");
    assert_eq!(
        report.reconciliation().attempt_terminal.as_ref(),
        Some(&attempt)
    );
}

/// A durable authority that contradicts the one-active-attempt invariant is
/// reported, not silently truncated.
///
/// Settling only whichever attempt sorted first would leave the other durably
/// non-terminal forever while the runtime reported a clean recovery. Recovery
/// fails closed instead, so no runtime exists that could admit work over an
/// incoherent attempt plane.
#[test]
fn two_concurrently_unsettled_attempts_fail_recovery_closed() {
    let durable = Durable::new();
    let conversation = conversation_id();
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        for ordinal in 0..2 {
            let attempt = AttemptId::for_conversation(&conversation, ordinal);
            store
                .append_event(envelope(
                    &format!("started-{ordinal}"),
                    Some(attempt.clone()),
                    None,
                    RuntimeEvent::AttemptStarted {
                        attempt_id: attempt,
                    },
                ))
                .expect("attempt started");
        }
    }

    let store = durable.open();
    let failure = recover(&store, &FixedClock).expect_err("recovery must fail closed");
    let RecoveryError::Unrecoverable(detail) = failure else {
        panic!("an incoherent attempt plane is unrecoverable, not a storage failure");
    };
    assert!(
        detail.contains("non-terminal attempts"),
        "the report names the violation: {detail}"
    );
    // Nothing was settled: neither attempt received a partial recovery
    // terminal.
    let events = all_events(&store);
    for ordinal in 0..2 {
        assert_eq!(
            terminal_count(
                &events,
                &AttemptId::for_conversation(&conversation, ordinal)
            ),
            0,
            "a failed recovery terminalizes nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #60 — subagent recovery: interrupted-only classification
// ---------------------------------------------------------------------------

fn commit_subagent_ownership(store: &SqliteConversationStore, subagent: &SubagentId) {
    store
        .append_event(envelope(
            &format!("subagent-committed-event:{subagent}"),
            None,
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id: subagent.clone(),
                child_agent_id: AgentId::new(format!("agent-{subagent}")),
                child_conversation_id: ConversationId::new(subagent.as_str()),
                tool_call_id: ToolCallId::new("call-sub"),
                agent: "explore".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                ownership: rustx::events::types::SubagentOwnershipKind::Normal,
                workspace: rustx::runtime::subagent::WorkspaceSnapshot::shared(
                    std::path::PathBuf::from("<shared-workspace>"),
                ),
            },
        ))
        .expect("subagent ownership");
}

fn commit_workflow_ownership(store: &SqliteConversationStore, subagent: &SubagentId) {
    store
        .append_event(envelope(
            &format!("subagent-committed-event:{subagent}"),
            None,
            None,
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id: subagent.clone(),
                child_agent_id: AgentId::new(format!("agent-{subagent}")),
                child_conversation_id: ConversationId::new(subagent.as_str()),
                tool_call_id: ToolCallId::new("workflow-call"),
                agent: "reviewer".to_owned(),
                definition_digest: "sha256:workflow-definition".to_owned(),
                ownership: rustx::events::types::SubagentOwnershipKind::Workflow,
                workspace: rustx::runtime::subagent::WorkspaceSnapshot::shared(
                    std::path::PathBuf::from("<shared-workspace>"),
                ),
            },
        ))
        .expect("Workflow ownership");
}

/// A subagent child that was durably owned and never settled does not
/// survive its owning process. Recovery publishes the honest interrupted
/// terminal notice exactly once — never a failure, never a relaunch, never
/// a reattach.
#[test]
fn nonterminal_subagent_work_is_terminalized_exactly_once_and_never_relaunched() {
    let durable = Durable::new();
    let subagent = SubagentId::for_conversation(&conversation_id(), 1);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        commit_subagent_ownership(&store, &subagent);
        // CRASH while the child runtime was running.
    }

    let report = recover_reopened(&durable);
    assert_eq!(
        report.subagent_classes().len(),
        1,
        "the durably owned child is classified"
    );
    assert_eq!(report.subagent_classes()[0].evidence.subagent_id, subagent);
    // Issue #144: recovery reports the identity the child actually started
    // with. Name alone would let a later catalog reinterpret an old child.
    assert_eq!(report.subagent_classes()[0].evidence.agent, "explore");
    assert_eq!(
        report.subagent_classes()[0].evidence.definition_digest,
        "sha256:definition"
    );
    assert_eq!(
        report.reconciliation().subagent_terminals,
        vec![subagent.clone()]
    );
    assert_eq!(
        report.highest_subagent_ordinal(),
        1,
        "the ordinal watermark is recovered from durable evidence"
    );

    let store = durable.open();
    let published: Vec<SubagentTerminalState> = all_events(&store)
        .into_iter()
        .filter_map(|event| match event {
            RuntimeEvent::SubagentTerminalPublished { state, .. } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        published,
        vec![SubagentTerminalState::Interrupted],
        "unknown is published as interrupted, never as a known failure"
    );
    let pending = store.load_pending().expect("pending");
    assert_eq!(pending.len(), 1, "exactly one terminal notification");
    assert_eq!(
        pending[0].message_id,
        MessageId::new(format!("subagent-{subagent}-terminal")),
        "the recovery notification reuses the live identity contract"
    );
    assert_eq!(
        pending[0].correlation.as_deref(),
        Some(format!("subagent-terminal:{subagent}").as_str()),
    );

    // Repeated restarts change nothing.
    for restart in 0..2 {
        let report = recover_reopened(&durable);
        assert!(
            report.subagent_classes().is_empty(),
            "restart #{restart} sees an absorbing subagent terminal"
        );
        assert!(report.reconciliation().is_empty());
        let store = durable.open();
        assert_eq!(
            store.load_pending().expect("pending").len(),
            1,
            "no duplicate terminal inbound after restart #{restart}"
        );
    }
}

/// A nonterminal Workflow child is recovered through the direct native
/// terminal-settlement path: no parent Pending Inbound notice is created and
/// no Workflow node is replayed.
#[test]
fn nonterminal_workflow_child_is_settled_without_parent_notification() {
    let durable = Durable::new();
    let subagent = SubagentId::for_conversation(&conversation_id(), 1);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        commit_workflow_ownership(&store, &subagent);
    }

    let report = recover_reopened(&durable);
    assert_eq!(report.subagent_classes().len(), 1);
    assert_eq!(
        report.subagent_classes()[0].evidence.ownership,
        rustx::events::types::SubagentOwnershipKind::Workflow
    );
    assert_eq!(
        report.reconciliation().subagent_terminals,
        vec![subagent.clone()]
    );

    let store = durable.open();
    let events = all_events(&store);
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::SubagentTerminalSettled {
            subagent_id,
            state: SubagentTerminalState::Interrupted,
            ..
        } if *subagent_id == subagent
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::SubagentTerminalPublished { .. }))
    );
    assert!(
        store.load_pending().expect("pending").is_empty(),
        "Workflow recovery never creates parent notification inbound"
    );

    let report = recover_reopened(&durable);
    assert!(report.subagent_classes().is_empty());
    assert!(report.reconciliation().is_empty());
}

/// A durably settled subagent (ownership + terminal) is not recovery work:
/// the fold closes over the absorbing terminal and restart is a no-op.
#[test]
fn a_durably_settled_subagent_needs_no_recovery() {
    let durable = Durable::new();
    let subagent = SubagentId::for_conversation(&conversation_id(), 1);
    {
        let store = durable.open();
        store.initialize(&[]).expect("bootstrap");
        commit_subagent_ownership(&store, &subagent);
        let (draft, event) = rustx::runtime::subagent::recovery_terminal_publication(
            &conversation_id(),
            &subagent,
            &AgentId::new(format!("agent-{subagent}")),
            "explore",
            "sha256:definition",
            &rustx::events::types::SubagentWorkspaceTerminalResource::None,
            fixed_time(),
        );
        store
            .accept_inbound_with_event(draft, event)
            .expect("terminal publication");
    }

    let report = recover_reopened(&durable);
    assert!(report.subagent_classes().is_empty());
    assert!(report.reconciliation().is_empty());
    assert_eq!(
        report.highest_subagent_ordinal(),
        1,
        "the watermark still reseeds from settled evidence"
    );
}

/// The durable answer obligation of one adoption, built from exactly the
/// pending items the adoption transaction will consume.
fn adoption_of(
    store: &SqliteConversationStore,
    watermark: rustx::runtime::inbound::InboundSequence,
) -> rustx::events::types::RuntimeEventEnvelope {
    rustx::durable::inbox::inbound_adoption_event(
        store.conversation_id(),
        None,
        store
            .load_pending()
            .expect("pending")
            .into_iter()
            .filter(|item| item.sequence <= watermark)
            .map(|item| item.message_id)
            .collect(),
    )
}

/// Adopts everything through `watermark`, together with the durable answer
/// obligation the adoption transaction requires.
fn adopt_through(
    store: &SqliteConversationStore,
    watermark: rustx::runtime::inbound::InboundSequence,
) -> Vec<MessageBlock> {
    store
        .adopt_pending_batch(watermark, adoption_of(store, watermark))
        .expect("adopt")
}
