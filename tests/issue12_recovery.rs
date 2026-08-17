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
//! recovered runtime resends nothing lives in `tests/issue47_conformance.rs`.

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use rustx::context::ContextGeneration;
use rustx::conversation::message_id_of;
use rustx::durable::{ConversationStore, InboundDraft, SqliteConversationStore};
use rustx::events::types::{
    AttemptFailure, BackgroundTerminalState, EVENT_SCHEMA_VERSION, RuntimeEvent,
    RuntimeEventEnvelope,
};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, UserContentBlock,
    UserMessageBlock, UserSource,
};
use rustx::model::catalog::{ModelCapabilities, ModelCompat};
use rustx::model::{
    ModelInvocationConfig, ModelProtocol, RequestIdentity, RequestParams, RequestSnapshot,
};
use rustx::runtime::identity::{
    AttemptId, CapabilityRevision, ConversationId, EventId, MessageId, ToolCallId, ToolExecutionId,
    ToolId, TurnId,
};
use rustx::runtime::recovery::{
    AttemptRecoveryClass, RecoveryEvidence, RecoveryPlan, RecoveryReport, ResumeDisposition,
    recover,
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
    }
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
        store.adopt_pending_batch(batch.watermark).expect("adopt");
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
        store
            .adopt_pending_batch(accepted.sequence)
            .expect("adopt again")
            .is_empty(),
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
        store.adopt_pending_batch(accepted.sequence).expect("adopt");
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
        store.adopt_pending_batch(accepted.sequence).expect("adopt");
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
            .persist_request_start(&snapshot, fixed_time())
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
/// unknown becomes a typed `Interrupted` canonical result — never a silent
/// re-execution, never an invented success, never an ordinary `Failed`.
///
/// The structurally incomplete tool turn is completed, so the conversation is
/// not left permanently unable to form a valid later model request.
#[test]
fn started_tool_with_unknown_outcome_becomes_interrupted_and_is_never_replayed() {
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
        ToolExecutionStatus::Interrupted,
        "unknown is preserved as unknown"
    );
    assert!(
        repaired.result.content.is_empty(),
        "recovery never invents output that was never durably known"
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
            (ToolCallId::new("call-a"), ToolExecutionStatus::Interrupted),
            (ToolCallId::new("call-b"), ToolExecutionStatus::Success),
            (
                ToolCallId::new("call-c"),
                ToolExecutionStatus::Cancelled {
                    reason: CancellationReason::ParentCancelled
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
                tool_name: "background_task".to_owned(),
            },
        ))
        .expect("background ownership");
}

/// A background execution that was durably owned and never settled does not
/// survive the process. Recovery neither assumes it is alive nor relaunches
/// it: it commits the strongest honest terminal — `Interrupted`, not `Failed`
/// — together with exactly one model-visible terminal notification.
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
        vec![BackgroundTerminalState::Interrupted],
        "unknown is published as interrupted, never as a known failure"
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
        store.adopt_pending_batch(accepted.sequence).expect("adopt");
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
            .persist_request_start(&snapshot, fixed_time())
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
                .persist_request_start(&snapshot, fixed_time())
                .expect("request start");
            store
                .append_event(envelope(
                    &format!("request-completed-{ordinal}"),
                    Some(attempt.clone()),
                    Some(TurnId::new("0")),
                    RuntimeEvent::ModelRequestCompleted {
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
        store.adopt_pending_batch(accepted.sequence).expect("adopt");
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
