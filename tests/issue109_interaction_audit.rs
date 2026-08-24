//! Issue #109 (FND-04) — the durable interaction audit's store contract and
//! its crash/restart behaviour.
//!
//! Each regression builds an **exact committed prefix** of the two interaction
//! linearization points in a file-backed `SQLite` conversation, drops the store
//! (the "process died here" boundary), reopens the same database, and asserts
//! what the durable authority permits and what recovery does — and, just as
//! importantly, what it refuses to do:
//!
//! ```text
//! InteractionRequested          opens  interaction:{id}
//! InteractionSettled            closes interaction:{id}   (exactly once)
//! ```
//!
//! The hard invariant this suite exists for is:
//!
//! > A historical `Approved` interaction is audit evidence only. It never
//! > grants execution authority after recovery/restart.
//!
//! The crash boundary is a `drop` and the reopen is a
//! `SqliteConversationStore::open`. There is no sleep and no timing assumption
//! anywhere.
//!
//! The Agent-Loop-facing half of the same contract — durable-before-prompt,
//! `InteractionSettled(Approved)` before `ToolExecutionStarted`, denial
//! semantics, and client detach/reattach — lives in the in-crate scripted
//! suite `tests/scripted/issue109_interaction_audit.rs`, because it needs the
//! scripted model adapter.

#![allow(clippy::too_many_lines)] // deterministic store scenarios stay linear

use chrono::{DateTime, TimeZone, Utc};
use rustx::context::ContextGeneration;
use rustx::durable::{
    ConversationStore, ConversationStoreError, SqliteConversationStore,
    interaction_audit_capability,
};
use rustx::events::types::{
    EVENT_SCHEMA_VERSION, InteractionSettlement, InteractionSubject, RuntimeEvent,
    RuntimeEventEnvelope,
};
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
};
use rustx::model::catalog::{ModelCapabilities, ModelCompat};
use rustx::model::{
    ModelFinishReason, ModelInvocationConfig, ModelProtocol, RequestIdentity, RequestParams,
    RequestSnapshot,
};
use rustx::publication::{PublicationFrame, PublicationPayload, PublicationStreamStart};
use rustx::runtime::identity::{
    AttemptId, CapabilityRevision, ConversationId, EventId, InteractionId, MessageId,
    PublicationStreamId, RequestId, ToolCallId, ToolId, TurnId,
};
use rustx::runtime::recovery::{AttemptRecoveryClass, RecoveryReport, recover};
use rustx::runtime::types::{CancellationReason, RuntimeClock};
use rustx::runtime::{ApprovalDecision, QuestionAnswer};
use rustx::tools::types::{ToolCall, ToolCallStart, ToolExecutionStatus};
use std::sync::Arc;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const CONVERSATION: &str = "conv-fnd04";

fn fixed_time() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0)
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

fn attempt() -> AttemptId {
    AttemptId::new("attempt-1")
}

fn call_id() -> ToolCallId {
    ToolCallId::new("call-approved")
}

fn interaction_id() -> InteractionId {
    InteractionId::for_attempt(&attempt(), 1)
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

fn invocation() -> ModelInvocationConfig {
    ModelInvocationConfig {
        model: "model-x".to_owned(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        max_output_tokens: 128,
        request_params: RequestParams::new(),
        capabilities: ModelCapabilities::text_only(true, true),
        compat: ModelCompat::default(),
    }
}

fn envelope(event_id: &str, event: RuntimeEvent) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 0,
        conversation_id: conversation_id(),
        attempt_id: Some(attempt()),
        turn_id: Some(TurnId::new("1")),
        timestamp: fixed_time(),
        event,
    }
}

/// The canonical durable identity of an interaction's requested fact. It is
/// restated here rather than imported so the suite pins the wire contract
/// instead of the implementation's private helper.
fn requested_event_id(interaction_id: &InteractionId) -> String {
    format!("interaction-requested-event:{interaction_id}")
}

fn settled_event_id(interaction_id: &InteractionId) -> String {
    format!("interaction-settled-event:{interaction_id}")
}

fn approval_subject() -> InteractionSubject {
    InteractionSubject::Approval {
        call_id: call_id(),
        tool_id: ToolId::new("tool-alpha"),
        tool_name: "alpha".to_owned(),
        arguments_digest: "0".repeat(64),
        reason: "the policy asked".to_owned(),
    }
}

fn requested(interaction_id: &InteractionId, subject: InteractionSubject) -> RuntimeEventEnvelope {
    envelope(
        &requested_event_id(interaction_id),
        RuntimeEvent::InteractionRequested {
            interaction_id: interaction_id.clone(),
            subject,
        },
    )
}

fn settled(
    interaction_id: &InteractionId,
    settlement: InteractionSettlement,
) -> RuntimeEventEnvelope {
    envelope(
        &settled_event_id(interaction_id),
        RuntimeEvent::InteractionSettled {
            interaction_id: interaction_id.clone(),
            settlement,
        },
    )
}

/// Commits the one durable request-start transaction of a turn and returns
/// the started request identity.
fn start_request(store: &SqliteConversationStore) -> RequestId {
    let head = store.load_head().expect("head");
    let snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt(),
            turn: TurnId::new("1"),
            retry_number: 0,
        },
        head.revision,
        "the frozen effective system prompt".to_owned(),
        Vec::new(),
        rustx::runtime::RuntimeResourceRevision::new(1),
        invocation(),
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
    );
    let request_id = snapshot.request_id.clone();
    store
        .commit_model_turn_start(&[], &snapshot, fixed_time())
        .expect("request start");
    request_id
}

fn frame(
    start: &PublicationStreamStart,
    sequence: u64,
    payload: PublicationPayload,
) -> PublicationFrame {
    PublicationFrame {
        stream_id: start.stream_id.clone(),
        message_id: start.message_id.clone(),
        sequence,
        payload,
    }
}

/// Commits the exact durable prefix a real attempt reaches immediately before
/// the pre-tool policy boundary: an attempt start, one complete provider
/// generation, and one canonical Assistant turn proposing `call-approved`.
///
/// Nothing here is an execution authorization: no `ToolExecutionStarted`
/// exists, which is precisely the state the interaction regressions build on.
fn commit_turn_up_to_the_policy_boundary(store: &SqliteConversationStore) {
    store.initialize(&[]).expect("initialize");
    store
        .append_event(RuntimeEventEnvelope {
            turn_id: None,
            ..envelope(
                "attempt-start",
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt(),
                },
            )
        })
        .expect("attempt start");
    let request_id = start_request(store);
    let message_id = MessageId::new(format!("{}-agent-1", attempt()));
    let start = PublicationStreamStart {
        stream_id: PublicationStreamId::for_request(&attempt(), &message_id),
        attempt_id: attempt(),
        turn_id: TurnId::new("1"),
        request_id: request_id.clone(),
        message_id: message_id.clone(),
    };
    store.open_publication_stream(&start).expect("open");
    store
        .stage_publication_frames(&[frame(
            &start,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call_id(),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                },
            },
        )])
        .expect("proposal start");
    store
        .stage_publication_frames(&[frame(
            &start,
            1,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCall {
                    id: call_id(),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                },
            },
        )])
        .expect("proposal complete");
    store
        .append_event(envelope(
            "request-completed-1",
            RuntimeEvent::ModelRequestCompleted {
                request_id,
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            },
        ))
        .expect("provider outcome");
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[frame(&start, 2, PublicationPayload::TerminalOnly)],
        )
        .expect("publication terminal");
    store
        .commit_canonical_publication(
            &start.stream_id,
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: message_id.clone(),
                content: vec![AssistantContentBlock::ToolCall(ToolCall {
                    id: call_id(),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                })],
            }),
            envelope(
                "assistant-committed-1",
                RuntimeEvent::AssistantMessageCommitted { message_id },
            ),
        )
        .expect("canonical Assistant");
}

fn journal(store: &SqliteConversationStore) -> Vec<RuntimeEvent> {
    const PAGE: usize = 32;
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let page = store.read_events(cursor, PAGE).expect("Event Journal page");
        if page.events.is_empty() {
            break;
        }
        events.extend(page.events.iter().map(|envelope| envelope.event.clone()));
        cursor = page.next_sequence;
    }
    events
}

fn interaction_facts(store: &SqliteConversationStore) -> Vec<RuntimeEvent> {
    journal(store)
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionSettled { .. }
            )
        })
        .collect()
}

fn has_tool_start(store: &SqliteConversationStore) -> bool {
    journal(store)
        .iter()
        .any(|event| matches!(event, RuntimeEvent::ToolExecutionStarted { .. }))
}

/// Runs the real recovery pipeline over a freshly reopened durable store.
fn recover_reopened(durable: &Durable) -> (SqliteConversationStore, RecoveryReport) {
    let store = durable.open();
    let report = recover(&store, &FixedClock).expect("recovery succeeds");
    (store, report)
}

// ---------------------------------------------------------------------------
// The durable state machine of one interaction identity
// ---------------------------------------------------------------------------

/// **Regression 8 (durable half).** One interaction identity settles exactly
/// once, and a settlement without its requested fact is refused outright.
#[test]
fn the_durable_authority_owns_the_interaction_state_machine() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let id = interaction_id();

    // A settlement for an interaction that never durably existed asserts a
    // decision about a prompt no user was ever shown.
    assert!(matches!(
        store.append_event(settled(&id, InteractionSettlement::Approved)),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    store
        .append_event(requested(&id, approval_subject()))
        .expect("requested");
    // The identity is spent: a second requested fact would fork the audit.
    assert!(matches!(
        store.append_event(requested(&id, approval_subject())),
        Err(ConversationStoreError::TerminalViolation(_))
    ));

    store
        .append_event(settled(&id, InteractionSettlement::Approved))
        .expect("settled");
    assert!(matches!(
        store.append_event(settled(&id, InteractionSettlement::Approved)),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert!(
        matches!(
            store.append_event(settled(
                &id,
                InteractionSettlement::Denied {
                    reason: "contradicting the first settlement".to_owned(),
                },
            )),
            Err(ConversationStoreError::TerminalViolation(_))
        ),
        "a contradictory second terminal is the same violation, not a correction"
    );
}

/// A settlement must be a terminal its requested subject can actually
/// produce: an Approval cannot be answered with a Question answer, and a
/// Question cannot be approved.
#[test]
fn a_settlement_must_match_the_subject_it_settles() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");

    let approval = InteractionId::for_attempt(&attempt(), 1);
    store
        .append_event(requested(&approval, approval_subject()))
        .expect("approval requested");
    assert!(matches!(
        store.append_event(settled(
            &approval,
            InteractionSettlement::Answered {
                answer: QuestionAnswer::FreeText {
                    value: "not an approval decision".to_owned(),
                },
            },
        )),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    let question = InteractionId::for_attempt(&attempt(), 2);
    store
        .append_event(requested(
            &question,
            InteractionSubject::Question {
                prompt: "Which target?".to_owned(),
                choices: None,
                allow_free_text: true,
            },
        ))
        .expect("question requested");
    assert!(matches!(
        store.append_event(settled(&question, InteractionSettlement::Approved)),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    // Cancellation is the one terminal both subjects share.
    store
        .append_event(settled(
            &question,
            InteractionSettlement::Cancelled {
                reason: CancellationReason::RuntimeShutdown,
            },
        ))
        .expect("cancellation settles either subject");
}

/// The audit facts carry a canonical durable identity derived from the
/// interaction identity, so the requested/settled pair resolves through the
/// unique event index instead of a Journal scan. A mismatched pair is
/// malformed and is rejected rather than silently rewritten.
#[test]
fn interaction_audit_facts_carry_their_canonical_identity() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let id = interaction_id();

    let mut forged = requested(&id, approval_subject());
    forged.event_id = EventId::new("interaction-requested-event:some-other-interaction");
    assert!(matches!(
        store.append_event(forged),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    store
        .append_event(requested(&id, approval_subject()))
        .expect("requested");
    let mut forged_settlement = settled(&id, InteractionSettlement::Approved);
    forged_settlement.event_id = EventId::new("interaction-settled-event:some-other-interaction");
    assert!(matches!(
        store.append_event(forged_settlement),
        Err(ConversationStoreError::InvalidReference(_))
    ));
}

/// The narrow interaction audit capability is not a general Event Journal
/// seam: it commits the two interaction facts and refuses everything else,
/// including the very facts that would authorize a side effect.
#[test]
fn the_narrow_audit_capability_commits_interaction_facts_only() {
    let store: Arc<dyn ConversationStore> =
        Arc::new(SqliteConversationStore::in_memory(conversation_id()).expect("store"));
    store.initialize(&[]).expect("initialize");
    let audit = interaction_audit_capability(Arc::clone(&store));
    assert_eq!(audit.conversation_id(), &conversation_id());

    let tool_start = envelope(
        "smuggled-tool-start",
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: call_id(),
            tool_id: ToolId::new("tool-alpha"),
        },
    );
    assert!(matches!(
        audit.commit_interaction_requested(tool_start.clone()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert!(matches!(
        audit.commit_interaction_settled(tool_start),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    let id = interaction_id();
    // The two operations are not interchangeable either.
    assert!(matches!(
        audit.commit_interaction_settled(requested(&id, approval_subject())),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    audit
        .commit_interaction_requested(requested(&id, approval_subject()))
        .expect("the requested fact commits through its own operation");
    assert!(matches!(
        audit.commit_interaction_requested(settled(&id, InteractionSettlement::Approved)),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    audit
        .commit_interaction_settled(settled(&id, InteractionSettlement::Approved))
        .expect("the settled fact commits through its own operation");
}

// ---------------------------------------------------------------------------
// Regression 3 — durable approval, crash before the tool started
// ---------------------------------------------------------------------------

/// **Regression 3.** `InteractionSettled(Approved)` is durable and the process
/// dies before `ToolExecutionStarted`. After restart the tool is **not**
/// executed: recovery classifies the attempt as one that never crossed an
/// external start commit, commits no start fact, and re-executes nothing.
#[test]
fn a_durable_approval_and_a_crash_before_tool_start_executes_nothing() {
    let durable = Durable::new();
    {
        let store = durable.open();
        commit_turn_up_to_the_policy_boundary(&store);
        let id = interaction_id();
        store
            .append_event(requested(&id, approval_subject()))
            .expect("requested");
        store
            .append_event(settled(&id, InteractionSettlement::Approved))
            .expect("approved");
        assert!(
            !has_tool_start(&store),
            "the prefix stops exactly at the approval"
        );
    }

    let (store, report) = recover_reopened(&durable);
    assert!(
        !has_tool_start(&store),
        "recovery never manufactures the start fact the old approval referred to"
    );
    assert!(
        matches!(
            report.attempt_class(),
            AttemptRecoveryClass::ExternalOutcomeKnown { attempt_id, tool_calls, .. }
                if *attempt_id == attempt() && tool_calls.is_empty()
        ),
        "a durable approval leaves no tool call with an unknown external outcome; got {:?}",
        report.attempt_class()
    );
    // The approved call is settled honestly rather than executed: because it
    // never crossed a start commit, recovery commits a canonical *cancelled*
    // result slot for it. That is the opposite of acting on the old approval.
    assert_eq!(
        report.reconciliation().repaired_tool_results,
        vec![call_id()]
    );
    let repaired = store
        .load_canonical()
        .expect("canonical")
        .into_iter()
        .find_map(|message| match message {
            MessageBlock::Tool(tool) if tool.tool_call_id == call_id() => Some(tool.result.status),
            _ => None,
        })
        .expect("the approved call received its canonical result slot");
    assert!(
        matches!(repaired, ToolExecutionStatus::Cancelled { .. }),
        "the tool was never executed on the authority of the old approval, got {repaired:?}"
    );

    // A second restart changes nothing: the historical approval is inert.
    let (store, second) = recover_reopened(&durable);
    assert!(
        second.reconciliation().is_empty(),
        "a settled recovery is idempotent"
    );
    assert!(!has_tool_start(&store));
}

// ---------------------------------------------------------------------------
// Regression 4 — a historical approval is never restart authorization
// ---------------------------------------------------------------------------

/// **Regression 4.** The historical approval cannot be reused by current
/// policy or runtime reconciliation.
///
/// After restart the interaction identity is durably spent in both
/// directions: it can neither be re-requested nor re-settled. A current
/// runtime that wants to run the same tool must reach a **new** live approval,
/// which necessarily allocates a new interaction identity.
#[test]
fn a_historical_approval_cannot_be_replayed_as_current_authority() {
    let durable = Durable::new();
    let id = interaction_id();
    {
        let store = durable.open();
        commit_turn_up_to_the_policy_boundary(&store);
        store
            .append_event(requested(&id, approval_subject()))
            .expect("requested");
        store
            .append_event(settled(&id, InteractionSettlement::Approved))
            .expect("approved");
    }

    let (store, report) = recover_reopened(&durable);
    // Recovery's reconciliation vocabulary has no interaction dimension at
    // all: an interaction is never something recovery acts on. The only
    // repair is the canonical `Interrupted` slot the dead attempt owed, and
    // the tool still never ran.
    assert!(!has_tool_start(&store));
    assert!(report.reconciliation().background_terminals.is_empty());
    assert!(report.reconciliation().subagent_terminals.is_empty());

    // Re-asserting the historical decision is a typed durable violation.
    assert!(matches!(
        store.append_event(settled(&id, InteractionSettlement::Approved)),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert!(matches!(
        store.append_event(requested(&id, approval_subject())),
        Err(ConversationStoreError::TerminalViolation(_))
    ));

    // A new live approval is an ordinary new identity in the current
    // (post-restart) attempt domain, entirely disjoint from the historical
    // one.
    let fresh_attempt = AttemptId::new("attempt-2");
    let fresh = InteractionId::for_attempt(&fresh_attempt, 1);
    assert_ne!(fresh, id);
    store
        .append_event(RuntimeEventEnvelope {
            attempt_id: Some(fresh_attempt),
            ..requested(&fresh, approval_subject())
        })
        .expect("a new attempt asks its own new question");

    assert_eq!(
        interaction_facts(&store).len(),
        3,
        "the historical pair is untouched and the new request is additive"
    );
}

// ---------------------------------------------------------------------------
// Regression 7 — process death with a pending unanswered interaction
// ---------------------------------------------------------------------------

/// **Regression 7.** A process death with a pending, unanswered interaction
/// leaves durable evidence that the prompt existed and nothing more. Recovery
/// synthesizes no settlement, no prompt, and no waiter.
#[test]
fn an_unanswered_interaction_is_evidence_and_is_never_resurrected() {
    let durable = Durable::new();
    let id = interaction_id();
    {
        let store = durable.open();
        commit_turn_up_to_the_policy_boundary(&store);
        store
            .append_event(requested(
                &id,
                InteractionSubject::Question {
                    prompt: "Which target?".to_owned(),
                    choices: Some(vec!["staging".to_owned(), "production".to_owned()]),
                    allow_free_text: false,
                },
            ))
            .expect("requested");
    }

    let (store, report) = recover_reopened(&durable);
    let facts = interaction_facts(&store);
    assert!(
        matches!(
            facts.as_slice(),
            [RuntimeEvent::InteractionRequested {
                subject: InteractionSubject::Question { .. },
                ..
            }]
        ),
        "recovery invented no settlement for the unanswered prompt, got {facts:?}"
    );
    assert!(
        matches!(
            report.attempt_class(),
            AttemptRecoveryClass::ExternalOutcomeKnown { tool_calls, .. } if tool_calls.is_empty()
        ),
        "an unanswered prompt starts nothing external; got {:?}",
        report.attempt_class()
    );
    assert!(!has_tool_start(&store));

    // Cold reopen may still record a settlement — but only as an explicit new
    // fact committed by a live decision, never by recovery. Nothing above did
    // so, and the lifecycle is still open, which is exactly the honest record.
    store
        .append_event(settled(
            &id,
            InteractionSettlement::Cancelled {
                reason: CancellationReason::RuntimeShutdown,
            },
        ))
        .expect("an explicit live settlement is still possible");
    let (_, second) = recover_reopened(&durable);
    assert!(second.reconciliation().is_empty());
}

// ---------------------------------------------------------------------------
// Bounded payloads
// ---------------------------------------------------------------------------

/// The audit payload is bounded and self-describing. The approval subject
/// pins the exact argument value by digest instead of copying it into the
/// low-frequency Journal, and the argument value itself remains durable
/// by-value in the canonical `ToolCall` the Message Ledger owns.
#[test]
fn the_approval_subject_is_bounded_and_still_pins_the_exact_arguments() {
    let durable = Durable::new();
    let store = durable.open();
    commit_turn_up_to_the_policy_boundary(&store);
    let id = interaction_id();
    store
        .append_event(requested(&id, approval_subject()))
        .expect("requested");

    let RuntimeEvent::InteractionRequested {
        subject:
            InteractionSubject::Approval {
                arguments_digest,
                call_id: subject_call,
                ..
            },
        ..
    } = interaction_facts(&store)
        .into_iter()
        .next()
        .expect("one requested fact")
    else {
        panic!("the approval subject is an approval");
    };
    assert_eq!(arguments_digest.len(), 64, "a fixed-size argument pin");
    assert_eq!(subject_call, call_id());

    // The canonical Ledger still owns the exact argument value the digest
    // pins, so the audit is complete without duplicating it.
    let arguments = store
        .load_canonical()
        .expect("canonical")
        .into_iter()
        .find_map(|message| match message {
            MessageBlock::Assistant(assistant) => {
                assistant.content.into_iter().find_map(|block| match block {
                    AssistantContentBlock::ToolCall(call) if call.id == call_id() => {
                        Some(call.arguments)
                    }
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("the canonical Assistant proposed the approved call");
    assert_eq!(arguments, serde_json::json!({}));
}

/// A denial settlement retains the exact client-facing reason, and a Question
/// settlement retains the exact user answer, so the audit answers "what did
/// the human actually decide" without consulting current policy.
#[test]
fn settlements_retain_the_exact_decision_by_value() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");

    let denied = InteractionId::for_attempt(&attempt(), 1);
    store
        .append_event(requested(&denied, approval_subject()))
        .expect("requested");
    store
        .append_event(settled(
            &denied,
            InteractionSettlement::Denied {
                reason: "the operator refused".to_owned(),
            },
        ))
        .expect("denied");

    let answered = InteractionId::for_attempt(&attempt(), 2);
    store
        .append_event(requested(
            &answered,
            InteractionSubject::Question {
                prompt: "Which target?".to_owned(),
                choices: Some(vec!["staging".to_owned()]),
                allow_free_text: false,
            },
        ))
        .expect("requested");
    store
        .append_event(settled(
            &answered,
            InteractionSettlement::Answered {
                answer: QuestionAnswer::Choice {
                    value: "staging".to_owned(),
                },
            },
        ))
        .expect("answered");

    let facts = interaction_facts(&store);
    assert!(matches!(
        &facts[1],
        RuntimeEvent::InteractionSettled {
            settlement: InteractionSettlement::Denied { reason },
            ..
        } if reason == "the operator refused"
    ));
    assert!(matches!(
        &facts[3],
        RuntimeEvent::InteractionSettled {
            settlement: InteractionSettlement::Answered {
                answer: QuestionAnswer::Choice { value }
            },
            ..
        } if value == "staging"
    ));

    // The finite approval decision has no argument channel, which is why the
    // settlement vocabulary carries a decision and never a replacement input.
    assert_eq!(
        serde_json::to_value(ApprovalDecision::Allow).expect("serialize"),
        serde_json::json!({"type": "allow"})
    );
}

// ---------------------------------------------------------------------------
// Schema policy
// ---------------------------------------------------------------------------

/// The interaction audit changed the durable event vocabulary incompatibly, so
/// the store schema version was bumped and an older development database is
/// rejected outright. There is no migration and no compatibility layer.
#[test]
fn an_older_development_database_is_rejected() {
    let durable = Durable::new();
    {
        let store = durable.open();
        store.initialize(&[]).expect("initialize");
    }
    let connection = rusqlite_open(&durable.path);
    connection
        .execute("UPDATE rustx_store SET schema_version=6 WHERE id=1", [])
        .expect("downgrade the stored schema version");
    drop(connection);

    assert!(matches!(
        SqliteConversationStore::open(conversation_id(), &durable.path),
        Err(ConversationStoreError::SchemaVersionMismatch { stored: 6, .. })
    ));
}

fn rusqlite_open(path: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).expect("open the raw database")
}
